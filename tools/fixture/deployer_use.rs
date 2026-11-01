// The test fixture's stand-in for `mixins/create/deployer_use.rs`: the same functions, doing
// nothing. The real module reaches deep into Horizon's dispenser, entity and loot code, which
// the fixture's small Horizon doesn't model; the real one is compiled against Horizon itself.
use create_engine::kinetics::Dir;
use horizon_client::items::ItemStack;

pub(crate) fn use_item_at(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    facing: Dir,
    stack: ItemStack,
) -> Option<Option<ItemStack>> {
    let _ = (game, pos, facing, stack);
    None
}

pub(crate) fn punch_entity_at(
    game: &mut crate::game::Game<'_>,
    at: [i32; 3],
    from: [f64; 3],
    damage: f32,
) -> bool {
    let _ = (game, at, from, damage);
    false
}

pub(crate) fn deployer_harvest(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    at: [i32; 3],
    stack: &ItemStack,
    harvest: bool,
) {
    let _ = (game, dimension, at, stack, harvest);
}

pub(crate) fn deployer_crack(
    game: &mut crate::game::Game<'_>,
    breaker: i32,
    at: [i32; 3],
    stage: i32,
) {
    let _ = (game, breaker, at, stage);
}

pub(crate) fn deployer_punch(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
) {
    let _ = (game, dimension, dim, pos, facing);
}
