// The deployer's uses of an item on the block in front of `pos` (toward `facing`), by the
// dispenser's own behaviours (`DeployerHandler.activate` → `useOn` as a dispenser would): what is
// left of the stack, or None for an item without such a use.
pub(crate) fn use_item_at(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    facing: horizon_core::geometry::Direction,
    stack: horizon_client::items::ItemStack,
) -> Option<Option<horizon_client::items::ItemStack>> {
    let at = (pos[0], pos[1], pos[2]);
    let used = match &*stack.item {
        "minecraft:bucket" => game.dispense_empty_bucket(at, facing, stack),
        "minecraft:water_bucket" | "minecraft:lava_bucket" | "minecraft:powder_snow_bucket" => {
            game.dispense_filled_bucket(at, facing, stack)
        }
        "minecraft:flint_and_steel" => game.dispense_flint_and_steel(at, facing, stack),
        "minecraft:bone_meal" => game.dispense_bone_meal(at, facing, stack),
        "minecraft:glass_bottle" => game.dispense_glass_bottle(at, facing, stack),
        "minecraft:glowstone" => game.dispense_glowstone(at, facing, stack),
        "minecraft:shears" => game.dispense_shears(at, facing, stack),
        "minecraft:honeycomb" => game.dispense_honeycomb(at, facing, stack),
        "minecraft:brush" => Ok(game.dispense_brush(at, facing, stack)),
        _ => return None,
    };
    used.ok().map(|(left, _)| left)
}
