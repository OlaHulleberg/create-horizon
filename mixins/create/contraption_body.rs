// What contraptions reach of the game beyond the hook helpers: a chest's or barrel's items carried
// as mounted storage (`MountedStorageManager`), a block broken by an actor with its drops handed
// back (`BlockHelper.destroyBlockAs`), items dropped from a moving contraption and a breaker's hit
// sound. Compiled against Horizon by `horizon.py check`; the template test swaps it for
// `tools/fixture/contraption_body.rs`.
use super::Game;
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;

/// A storage block's items while its contraption moves.
pub(super) type Stored = horizon_client::containers::BlockEntity;

/// The block entity of a chest, trapped chest or barrel at `pos`, out of the world
/// (`MountedStorage` for Create's simple item storages); None for any other block.
pub(super) fn take_storage(game: &mut Game<'_>, pos: [i32; 3]) -> Option<Stored> {
    let key = (pos[0], pos[1], pos[2]);
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let name = game.pack.assets.registry.state(state)?.name.as_str();
    if !matches!(
        name,
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel"
    ) {
        return None;
    }
    match game.world.block_entities.get(&key) {
        Some(horizon_client::containers::BlockEntity::Chest(_)) => {}
        _ => return None,
    }
    game.world.block_entities.remove(&key)
}

/// Whether `pos` holds a storage block a contraption carries.
pub(super) fn is_storage(game: &Game<'_>, pos: [i32; 3]) -> bool {
    matches!(
        game.world.block_entities.get(&(pos[0], pos[1], pos[2])),
        Some(horizon_client::containers::BlockEntity::Chest(_))
    ) && game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .is_some_and(|s| {
            matches!(
                s.name.as_str(),
                "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel"
            )
        })
}

/// The carried items back in the storage block now at `pos` (`addBlocksToWorld`).
pub(super) fn put_storage(game: &mut Game<'_>, pos: [i32; 3], stored: Stored) {
    game.world
        .block_entities
        .insert((pos[0], pos[1], pos[2]), stored);
}

/// Carried storage's items as its block entity saves them (`Items`, with their slots), for a
/// contraption kept in the save or an item.
pub(super) fn stored_tag(stored: &Stored) -> horizon_world::nbt::Tag {
    match stored {
        horizon_client::containers::BlockEntity::Chest(items) => {
            horizon_items::item_nbt::encode_items(items)
        }
        _ => horizon_world::nbt::Tag::List(Vec::new()),
    }
}

/// Carried storage back from [`stored_tag`]: a chest's or barrel's 27 slots.
pub(super) fn stored_of(tag: &horizon_world::nbt::Tag) -> Option<Stored> {
    Some(horizon_client::containers::BlockEntity::Chest(
        horizon_items::item_nbt::decode_items(Some(tag), 27),
    ))
}

/// The slots of carried storage.
pub(super) fn slots(stored: &mut Stored) -> Option<&mut [Option<ItemStack>]> {
    match stored {
        horizon_client::containers::BlockEntity::Chest(items) => Some(items.as_mut_slice()),
        _ => None,
    }
}

/// A chest slot's limit for a stack (`min(64, getMaxStackSize)`).
pub(super) fn slot_limit(game: &Game<'_>, stack: &ItemStack) -> u32 {
    game.items()
        .get(&stack.item)
        .map_or(64, |i| i.max_stack)
        .min(64)
}

pub(super) fn same_stack(a: &ItemStack, b: &ItemStack) -> bool {
    a.same_item_same_components(b)
}

/// `BlockHelper.destroyBlockAs` with no player: the block's loot drawn with `tool` (when blocks
/// drop), an infested block's silverfish, then the block gone with its break particles and
/// replaced by its fluid (mined ice by water over a solid block). The drops, for the actor to
/// collect or drop.
pub(super) fn break_block(
    game: &mut Game<'_>,
    dimension: &str,
    at: [i32; 3],
    tool: Option<&ItemStack>,
) -> Vec<ItemStack> {
    if (game.serving.is_none() && game.options.dedicated.is_none()) || dimension != game.dim.id() {
        return Vec::new();
    }
    let state = game.world.block(at[0], at[1], at[2]);
    let name = game.res.facts().get(state).name.clone();
    let mut drops = Vec::new();
    if game.ui.rules.bool("block_drops") {
        drops = game
            .block_loot_at(at, state, tool)
            .into_iter()
            .filter(|s| &*s.item != "minecraft:air" && s.count > 0)
            .collect();
        if name.starts_with("minecraft:infested_") {
            let feet = [
                f64::from(at[0]) + 0.5,
                f64::from(at[1]),
                f64::from(at[2]) + 0.5,
            ];
            if let Some(id) = game.spawn_mob("minecraft:silverfish", feet, false) {
                game.mob_poof(id);
            }
        }
    }
    let pos = horizon_core::position::BlockPos::new(at[0], at[1], at[2]);
    game.with_level(|b, level, ticks| {
        b.destroy_block(level, ticks, pos, false);
        if &*name == "minecraft:ice" {
            b.ice_destroyed_by_player(level, ticks, pos, state, false);
        }
    });
    let _ = game.process_level_events();
    drops
}

/// An item entity at `at` with `motion` (`new ItemEntity(..)` + `setDeltaMovement`).
pub(super) fn drop_stack(game: &mut Game<'_>, at: [f64; 3], motion: [f64; 3], stack: ItemStack) {
    game.spawn_item(stack, at, motion, 10);
}

/// `world.random.nextFloat()`.
pub(super) fn random_float(game: &mut Game<'_>) -> f32 {
    use horizon_core::random::RandomSource;
    game.world.random.next_float()
}

/// A block's hit sound at a quarter volume (`playSound(.., getHitSound(), NEUTRAL, .25, 1)`).
pub(super) fn hit_sound(game: &mut Game<'_>, at: [i32; 3], state: BlockStateId) {
    let hit = game.res.facts().get(state).sounds.hit.clone();
    game.audio
        .play(&hit, Some(at.map(|v| f64::from(v) + 0.5)), 0.25, 1.0);
}
