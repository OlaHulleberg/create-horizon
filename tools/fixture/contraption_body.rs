// Stand-in for `mixins/create/contraption_body.rs` in the template test: storage is a plain list
// of slots kept by position in `STORAGE`, a broken block leaves air and drops what `DROPS` says,
// and dropped stacks are recorded in `DROPPED`.
use super::Game;
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;

pub(super) type Stored = Vec<Option<ItemStack>>;

thread_local! {
    /// Storage blocks in the world by position (a chest's 27 slots).
    pub(crate) static STORAGE: std::cell::RefCell<std::collections::HashMap<[i32; 3], Stored>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// What a broken block drops, by its state.
    pub(crate) static DROPS: std::cell::RefCell<std::collections::HashMap<u32, Vec<(String, u32)>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Stacks dropped in the world: where, and the item and count.
    pub(crate) static DROPPED: std::cell::RefCell<Vec<([f64; 3], String, u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn take_storage(game: &mut Game<'_>, pos: [i32; 3]) -> Option<Stored> {
    let _ = game;
    STORAGE.with_borrow_mut(|s| s.remove(&pos))
}

pub(super) fn is_storage(game: &Game<'_>, pos: [i32; 3]) -> bool {
    let _ = game;
    STORAGE.with_borrow(|s| s.contains_key(&pos))
}

pub(super) fn put_storage(game: &mut Game<'_>, pos: [i32; 3], stored: Stored) {
    let _ = game;
    STORAGE.with_borrow_mut(|s| s.insert(pos, stored));
}

/// Each filled slot as a compound of its slot, item and count.
pub(super) fn stored_tag(stored: &Stored) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::Tag;
    Tag::List(
        stored
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                Some(Tag::compound([
                    ("Slot", Tag::IntArray(vec![i as i32])),
                    ("id", Tag::String(s.item.as_ref().into())),
                    ("count", Tag::IntArray(vec![s.count as i32])),
                ]))
            })
            .collect(),
    )
}

pub(super) fn stored_of(tag: &horizon_world::nbt::Tag) -> Option<Stored> {
    use horizon_world::nbt::Tag;
    let Tag::List(list) = tag else { return None };
    let mut slots = vec![None; 27];
    for t in list {
        let Tag::Compound(c) = t else { continue };
        let int = |k: &str| match c.get(&k.into()) {
            Some(Tag::IntArray(v)) => v.first().copied(),
            _ => None,
        };
        let id = match c.get(&"id".into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => continue,
        };
        if let (Some(i), Some(n)) = (int("Slot"), int("count"))
            && let Some(slot) = slots.get_mut(i as usize)
        {
            *slot = Some(ItemStack::new(&id, n as u32));
        }
    }
    Some(slots)
}

pub(super) fn slots(stored: &mut Stored) -> Option<&mut [Option<ItemStack>]> {
    Some(stored.as_mut_slice())
}

pub(super) fn slot_limit(game: &Game<'_>, stack: &ItemStack) -> u32 {
    let _ = (game, stack);
    64
}

pub(super) fn same_stack(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item
}

pub(super) fn break_block(
    game: &mut Game<'_>,
    dimension: &str,
    at: [i32; 3],
    tool: Option<&ItemStack>,
) -> Vec<ItemStack> {
    let _ = tool;
    let state = game.world.block(at[0], at[1], at[2]).0;
    game.native_set_block(dimension, at, BlockStateId(0), 3);
    DROPS.with_borrow(|d| {
        d.get(&state)
            .map(|v| v.iter().map(|(i, n)| ItemStack::new(i, *n)).collect())
            .unwrap_or_default()
    })
}

pub(super) fn drop_stack(game: &mut Game<'_>, at: [f64; 3], motion: [f64; 3], stack: ItemStack) {
    let _ = (game, motion);
    DROPPED.with_borrow_mut(|d| d.push((at, stack.item.to_string(), stack.count)));
}

pub(super) fn drop_stack_now(
    game: &mut Game<'_>,
    at: [f64; 3],
    motion: [f64; 3],
    stack: ItemStack,
) {
    drop_stack(game, at, motion, stack);
}

pub(super) fn random_float(game: &mut Game<'_>) -> f32 {
    let _ = game;
    0.5
}

pub(super) fn hit_sound(game: &mut Game<'_>, at: [i32; 3], state: BlockStateId) {
    let _ = (game, at, state);
}
