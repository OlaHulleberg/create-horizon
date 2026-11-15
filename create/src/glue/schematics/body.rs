//! What schematics read and write of the game beyond the hooks: the `schematics/` files (gzip
//! NBT, as `NbtIo` reads and writes them, through the engine's `gzip` and the API's NBT
//! encoding), a schematic's data on its stack (`minecraft:custom_data`) and each block's
//! item (`Block.asItem`).
//!
//! The files live in the mod's game data folder (`horizon-mod-data/create/schematics/`): the
//! API reaches a mod's own folders only, not the game folder's `schematics/` where Create keeps
//! them.

use crate::glue::packages::body::{CustomData, set_custom};
use create_engine::gzip;
use create_engine::schematic::{Nbt, SchematicData};
use mod_api::{ConfigEntryKind, ConfigFiles, ConfigPath, Host, ItemStack, Tag};
use std::collections::HashMap;

/// `SchematicItem.loadSchematic`'s `NbtAccounter.create(0x20000000L)`.
const MAX_BYTES: usize = 0x2000_0000;

/// The folder under the mod's data root.
const ROOT: &str = "schematics";

fn path(rel: &str) -> Option<ConfigPath> {
    ConfigPath::new(&format!("{ROOT}/{rel}")).ok()
}

fn files(host: &dyn Host) -> &'static dyn ConfigFiles {
    host.data_files()
}

pub(crate) fn to_engine(t: &Tag) -> Nbt {
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

pub(crate) fn to_game(t: &Nbt) -> Tag {
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
                .map(|(k, v)| (k.as_str().into(), to_game(v)))
                .collect(),
        ),
        Nbt::IntArray(v) => Tag::IntArray(v.clone()),
        Nbt::LongArray(v) => Tag::LongArray(v.clone()),
    }
}

/// A file's root tag (`NbtIo.read`: a named root, its name dropped).
fn read_named(raw: &[u8]) -> Option<Tag> {
    // `[type][name length][name][payload]`; the API reads the nameless `[type][payload]`.
    let (&kind, rest) = raw.split_first()?;
    let (len, rest) = rest.split_first_chunk::<2>()?;
    let rest = rest.get(usize::from(u16::from_be_bytes(*len))..)?;
    let mut nameless = Vec::with_capacity(rest.len() + 1);
    nameless.push(kind);
    nameless.extend_from_slice(rest);
    Tag::from_network_bytes(&nameless, Some(MAX_BYTES as u64))
        .ok()
        .map(|(t, _)| t)
}

/// `tag` as a file's root, named "" (`NbtIo.write`).
fn write_named(tag: &Tag) -> Option<Vec<u8>> {
    let nameless = tag.to_network_bytes().ok()?;
    let (&kind, payload) = nameless.split_first()?;
    let mut named = Vec::with_capacity(nameless.len() + 2);
    named.extend_from_slice(&[kind, 0, 0]);
    named.extend_from_slice(payload);
    Some(named)
}

/// A structure file under `schematics/` (`NbtIo.read` of the gzip stream), once per load.
pub fn read_structure(host: &dyn Host, rel: &str) -> Option<Nbt> {
    let bytes = files(host).read(&path(rel)?).ok()?;
    let raw = match gzip::decode(&bytes, MAX_BYTES) {
        Ok(raw) => raw,
        Err(e) => {
            crate::create_log!("[create] Failed to read schematic {rel}: {e:?}");
            return None;
        }
    };
    match read_named(&raw) {
        Some(t) => Some(to_engine(&t)),
        None => {
            crate::create_log!("[create] Failed to read schematic {rel}: not NBT");
            None
        }
    }
}

/// `NbtIo.writeCompressed` to a file under `schematics/` (its folders made, an old file
/// replaced); its size.
pub fn write_structure(host: &dyn Host, rel: &str, tag: &Nbt) -> Result<u64, String> {
    let file = path(rel).ok_or("not a file name")?;
    let raw = write_named(&to_game(tag)).ok_or("not writable as NBT")?;
    let gz = gzip::encode(&raw);
    files(host)
        .write(&file, &gz)
        .map_err(|e| format!("{e:?}"))?;
    Ok(gz.len() as u64)
}

/// The schematics saved in `schematics/` (`ClientSchematicLoader.refresh`: its `.nbt` files, by
/// name).
pub fn local_files(host: &dyn Host) -> Vec<String> {
    let Ok(folder) = ConfigPath::new(ROOT) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    while let Ok(page) = files(host).list(&folder, after.as_deref(), 256) {
        let Some(last) = page.last().map(|e| e.name.clone()) else {
            break;
        };
        out.extend(
            page.into_iter()
                .map(|e| e.name)
                .filter(|n| n.ends_with(".nbt")),
        );
        after = Some(last);
    }
    out.sort();
    out
}

pub fn exists(host: &dyn Host, rel: &str) -> bool {
    path(rel).is_some_and(|p| matches!(files(host).stat(&p), Ok(Some(_))))
}

pub fn size(host: &dyn Host, rel: &str) -> Option<u64> {
    match files(host).stat(&path(rel)?) {
        Ok(Some(s)) if s.kind == ConfigEntryKind::File => Some(s.len),
        _ => None,
    }
}

pub fn copy(host: &dyn Host, from: &str, to: &str) -> Result<(), String> {
    let (from, to) = (
        path(from).ok_or("not a file name")?,
        path(to).ok_or("not a file name")?,
    );
    let bytes = files(host).read(&from).map_err(|e| format!("{e:?}"))?;
    files(host).write(&to, &bytes).map_err(|e| format!("{e:?}"))
}

const KEYS: [&str; 7] = [
    "File", "Owner", "Deployed", "Anchor", "Rotation", "Mirror", "Bounds",
];

/// A stack's schematic data (empty for a stack without).
pub fn schematic_data(stack: &ItemStack) -> SchematicData {
    let Some(c) = stack.get(&CustomData) else {
        return SchematicData::default();
    };
    let found: HashMap<&str, Nbt> = KEYS
        .iter()
        .filter_map(|k| Some((*k, to_engine(c.get(&(*k).into())?))))
        .collect();
    SchematicData::from_entries(|k| found.get(k))
}

/// Writes `data` over the stack's schematic keys, keeping the rest of its custom data.
pub fn set_schematic_data(stack: &mut ItemStack, data: &SchematicData) {
    for k in KEYS {
        set_custom(stack, k, None);
    }
    for (k, v) in data.to_entries() {
        set_custom(stack, k, Some(to_game(&v)));
    }
}

/// `Block.asItem` (`Item.BY_BLOCK`): the item placing the block; a wall variant's
/// (`wall_torch`, `oak_wall_sign`) is its standing block's item.
pub fn item_for_block(host: &dyn Host, block: &str) -> Option<String> {
    let item = |name: &str| {
        let key = host.block_key(name)?;
        let item = host.block_item(key)?;
        let id = item.id(host);
        (id != "minecraft:air").then_some(id)
    };
    item(block).or_else(|| {
        let standing = block.replacen("_wall_", "_", 1).replacen(":wall_", ":", 1);
        item(&standing)
    })
}

#[cfg(test)]
mod tests {
    use super::{read_named, to_engine, to_game, write_named};
    use create_engine::gzip;
    use create_engine::schematic::Nbt;
    use mod_api::Tag;

    #[test]
    fn files_keep_their_root_through_gzip() {
        let root = Tag::compound([
            ("DataVersion", Tag::Int(3955)),
            (
                "size",
                Tag::List(vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)]),
            ),
            ("name", Tag::String("schematic".into())),
        ]);
        let raw = write_named(&root).expect("writable");
        // A named root: its type, an empty name, then the payload.
        assert_eq!(&raw[..3], &[10, 0, 0]);
        let back = gzip::decode(&gzip::encode(&raw), usize::MAX).expect("gzip");
        assert_eq!(read_named(&back), Some(root.clone()));
        let engine = to_engine(&root);
        assert_eq!(engine.get("DataVersion"), Some(&Nbt::Int(3955)));
        assert_eq!(to_game(&engine), root);
    }

    #[test]
    fn a_named_root_keeps_its_payload_past_its_name() {
        let root = Tag::compound([("a", Tag::Byte(1))]);
        let raw = write_named(&root).expect("writable");
        let mut named = vec![10, 0, 3, b'a', b'b', b'c'];
        named.extend_from_slice(&raw[3..]);
        assert_eq!(read_named(&named), Some(root));
    }
}
