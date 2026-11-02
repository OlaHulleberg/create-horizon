// Stand-in for `mixins/create/schematic_body.rs` in the template test: the `schematics/` folder
// is a map of trees in memory (no gzip), the fixture's stacks have no components (a schematic's
// data is kept by index in `damage`), and a block's item is its own name.
use super::Game;
use create_engine::schematic::{Nbt, SchematicData};
use horizon_client::items::ItemStack;
use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    /// The `schematics/` folder: each file's tree, by path.
    pub(crate) static FILES: RefCell<BTreeMap<String, Nbt>> = const { RefCell::new(BTreeMap::new()) };
    /// Schematic data of stacks, by `damage - 1`.
    static DATA: RefCell<Vec<SchematicData>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn forget() {}

pub(super) fn read_structure(rel: &str) -> Option<Nbt> {
    FILES.with_borrow(|f| f.get(rel).cloned())
}

pub(super) fn write_structure(rel: &str, tag: &Nbt) -> Result<u64, String> {
    FILES.with_borrow_mut(|f| f.insert(rel.to_owned(), tag.clone()));
    Ok(100)
}

pub(super) fn exists(rel: &str) -> bool {
    FILES.with_borrow(|f| f.contains_key(rel))
}

pub(super) fn size(rel: &str) -> Option<u64> {
    exists(rel).then_some(100)
}

pub(super) fn copy(from: &str, to: &str) -> Result<(), String> {
    let tag = read_structure(from).ok_or("missing")?;
    write_structure(to, &tag).map(|_| ())
}

pub(super) fn remove(rel: &str) {
    FILES.with_borrow_mut(|f| f.remove(rel));
}

pub(super) fn entries(rel: &str) -> Vec<(String, bool, u128)> {
    let prefix = format!("{rel}/");
    FILES.with_borrow(|f| {
        f.keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter(|k| !k.contains('/'))
            .map(|k| (k.to_owned(), true, 0))
            .collect()
    })
}

pub(super) fn schematic_data(stack: &ItemStack) -> SchematicData {
    if stack.damage == 0 {
        return SchematicData::default();
    }
    DATA.with_borrow(|d| d.get(stack.damage as usize - 1).cloned().unwrap_or_default())
}

pub(super) fn set_schematic_data(stack: &mut ItemStack, data: &SchematicData) {
    DATA.with_borrow_mut(|d| {
        d.push(data.clone());
        stack.damage = d.len() as u32;
    });
}

pub(super) fn item_for_block(game: &Game<'_>, block: &str) -> Option<String> {
    let _ = game;
    (block != "minecraft:water").then(|| block.to_owned())
}
