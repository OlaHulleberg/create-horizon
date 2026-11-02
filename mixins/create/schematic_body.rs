// What Create's schematics read and write of the game beyond the hook helpers: the game folder's
// `schematics/` files (gzip NBT through Horizon's reader and writer, as `NbtIo` reads and writes
// them), a schematic's data on its stack (`minecraft:custom_data`: Horizon adds no component
// types) and each block's item (`Block.asItem`). Compiled against Horizon by `horizon.py check`;
// the template test swaps it for `tools/fixture/schematic_body.rs`.
use super::Game;
use create_engine::schematic::{Nbt, SchematicData};
use horizon_client::items::ItemStack;
use horizon_world::compression::Compression;
use horizon_world::nbt::{self, Compound, JavaString, Limits, NamedTag, Tag};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

const CUSTOM_DATA: &str = "minecraft:custom_data";
/// `SchematicItem.loadSchematic`'s `NbtAccounter.create(0x20000000L)`.
const MAX_BYTES: usize = 0x2000_0000;

thread_local! {
    /// Each block's item, by block name, made once per world.
    static ITEMS: RefCell<Option<HashMap<String, String>>> = const { RefCell::new(None) };
}

/// The world closed.
pub(super) fn forget() {
    ITEMS.with_borrow_mut(|m| *m = None);
}

/// `CreatePaths.SCHEMATICS_DIR`: `schematics/` in the game folder (beside `config/`).
fn base() -> Option<PathBuf> {
    let config = Game::native_config_dir().ok()?;
    Some(config.parent()?.join("schematics"))
}

fn path(rel: &str) -> Option<PathBuf> {
    Some(base()?.join(rel))
}

fn to_engine(t: &Tag) -> Nbt {
    match t {
        Tag::End => Nbt::Compound(Default::default()),
        Tag::Byte(v) => Nbt::Byte(*v),
        Tag::Short(v) => Nbt::Short(*v),
        Tag::Int(v) => Nbt::Int(*v),
        Tag::Long(v) => Nbt::Long(*v),
        Tag::Float(v) => Nbt::Float(*v),
        Tag::Double(v) => Nbt::Double(*v),
        Tag::ByteArray(v) => Nbt::ByteArray(v.clone()),
        Tag::String(s) => Nbt::String(s.to_string_lossy()),
        Tag::List(l) => Nbt::List(l.iter().map(to_engine).collect()),
        Tag::Compound(c) => Nbt::Compound(
            c.iter()
                .map(|(k, v)| (k.to_string_lossy(), to_engine(v)))
                .collect(),
        ),
        Tag::IntArray(v) => Nbt::IntArray(v.clone()),
        Tag::LongArray(v) => Nbt::LongArray(v.clone()),
    }
}

fn to_game(t: &Nbt) -> Tag {
    match t {
        Nbt::Byte(v) => Tag::Byte(*v),
        Nbt::Short(v) => Tag::Short(*v),
        Nbt::Int(v) => Tag::Int(*v),
        Nbt::Long(v) => Tag::Long(*v),
        Nbt::Float(v) => Tag::Float(*v),
        Nbt::Double(v) => Tag::Double(*v),
        Nbt::ByteArray(v) => Tag::ByteArray(v.clone()),
        Nbt::String(s) => Tag::String(s.as_str().into()),
        Nbt::List(l) => Tag::List(l.iter().map(to_game).collect()),
        Nbt::Compound(c) => Tag::Compound(
            c.iter()
                .map(|(k, v)| (JavaString::from(k.as_str()), to_game(v)))
                .collect(),
        ),
        Nbt::IntArray(v) => Tag::IntArray(v.clone()),
        Nbt::LongArray(v) => Tag::LongArray(v.clone()),
    }
}

/// A structure file under `schematics/` (`NbtIo.read` of the gzip stream), once per load.
pub(super) fn read_structure(rel: &str) -> Option<Nbt> {
    let bytes = std::fs::read(path(rel)?).ok()?;
    let raw = Compression::Gzip.decode(&bytes, MAX_BYTES).ok()?;
    let limits = Limits {
        max_bytes: MAX_BYTES,
        ..Limits::templates()
    };
    match nbt::read_named(&raw, limits) {
        Ok((named, _)) => Some(to_engine(&named.tag)),
        Err(e) => {
            eprintln!("[create] Failed to read schematic {rel}: {e}");
            None
        }
    }
}

/// `NbtIo.writeCompressed` to a file under `schematics/` (its folders made, an old file
/// replaced); its size.
pub(super) fn write_structure(rel: &str, tag: &Nbt) -> Result<u64, String> {
    let file = path(rel).ok_or("no game folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let named = NamedTag {
        name: JavaString::from(""),
        tag: to_game(tag),
    };
    let raw = nbt::write_named(&named, Limits::saved()).map_err(|e| e.to_string())?;
    let gz = Compression::Gzip.encode(&raw).map_err(|e| e.to_string())?;
    std::fs::write(&file, &gz).map_err(|e| e.to_string())?;
    Ok(gz.len() as u64)
}

pub(super) fn exists(rel: &str) -> bool {
    path(rel).is_some_and(|p| p.exists())
}

pub(super) fn size(rel: &str) -> Option<u64> {
    Some(std::fs::metadata(path(rel)?).ok()?.len())
}

pub(super) fn copy(from: &str, to: &str) -> Result<(), String> {
    let (from, to) = (
        path(from).ok_or("no game folder")?,
        path(to).ok_or("no game folder")?,
    );
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub(super) fn remove(rel: &str) {
    if let Some(p) = path(rel) {
        let _ = std::fs::remove_file(p);
    }
}

/// The entries of a folder under `schematics/` (`Files.list`): each name, whether it is a file,
/// and when it last changed (milliseconds).
pub(super) fn entries(rel: &str) -> Vec<(String, bool, u128)> {
    let Some(dir) = path(rel) else {
        return Vec::new();
    };
    let Ok(list) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    list.flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            let modified = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis());
            (
                e.file_name().to_string_lossy().into_owned(),
                meta.is_some_and(|m| m.is_file()),
                modified,
            )
        })
        .collect()
}

const KEYS: [&str; 7] = [
    "File", "Owner", "Deployed", "Anchor", "Rotation", "Mirror", "Bounds",
];

/// A stack's schematic data (empty for a stack without).
pub(super) fn schematic_data(stack: &ItemStack) -> SchematicData {
    let Some(Tag::Compound(c)) = stack.data().and_then(|d| d.other(CUSTOM_DATA)) else {
        return SchematicData::default();
    };
    let found: HashMap<&str, Nbt> = KEYS
        .iter()
        .filter_map(|k| Some((*k, to_engine(c.get(&JavaString::from(*k))?))))
        .collect();
    SchematicData::from_entries(|k| found.get(k))
}

/// Writes `data` over the stack's schematic keys, keeping the rest of its custom data.
pub(super) fn set_schematic_data(stack: &mut ItemStack, data: &SchematicData) {
    let d = stack.data_mut();
    let mut c = match d.other(CUSTOM_DATA) {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    for k in KEYS {
        c.remove(&JavaString::from(k));
    }
    for (k, v) in data.to_entries() {
        c.insert(JavaString::from(k), to_game(&v));
    }
    d.set_other(CUSTOM_DATA, (!c.is_empty()).then_some(Tag::Compound(c)));
}

/// `Block.asItem` (`Item.BY_BLOCK`): the item placing the block, by the items' blocks; a wall
/// variant's (`wall_torch`, `oak_wall_sign`) is its standing block's item.
pub(super) fn item_for_block(game: &Game<'_>, block: &str) -> Option<String> {
    ITEMS.with_borrow_mut(|m| {
        let map = m.get_or_insert_with(|| {
            let registry = game.pack.assets.registry;
            let mut map: HashMap<String, String> = HashMap::new();
            for (id, info) in &game.items().items {
                let Some(state) = info.block else { continue };
                let Some(s) = registry.state(state) else {
                    continue;
                };
                // The item named as its block wins.
                if s.name == **id || !map.contains_key(&s.name) {
                    map.insert(s.name.clone(), id.to_string());
                }
            }
            map
        });
        if let Some(i) = map.get(block) {
            return Some(i.clone());
        }
        let standing = block.replacen("_wall_", "_", 1).replacen(":wall_", ":", 1);
        map.get(&standing).cloned()
    })
}
