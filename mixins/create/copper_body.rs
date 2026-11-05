// What Create's copper shingles and tiles read of the level when they weather (block data, the
// level's random, setting the block); the template test swaps it for
// `tools/fixture/copper_body.rs`.
use super::create::with_world;
use create_engine::states;
use horizon_world::registry::BlockStateId;

/// A random tick of a Create copper shingle or tile block (`ChangeOverTimeBlock.changeOverTime`,
/// which their vanilla classes run): vanilla's chance and neighbour rule, as Horizon's copper
/// weathers, but into Create's next age (`CopperRegistries.addWeathering`), not vanilla's. Waxed
/// and oxidized blocks don't tick randomly (their templates don't).
pub(super) fn random_tick(
    level: &mut super::GameLevel<'_>,
    ticks: &mut horizon_world::level::Ticks,
    pos: horizon_core::position::BlockPos,
    state: BlockStateId,
    handled: &mut bool,
) {
    use horizon_core::random::RandomSource;
    use horizon_world::level::LevelAccess;
    use horizon_world::level::classes::Interface;
    if states::index(state.0).is_none() {
        return;
    }
    let Some(next) = with_world(|w| w.table.get(state.0).map(|(b, _)| b))
        .flatten()
        .and_then(create_engine::decor::copper_next)
    else {
        return;
    };
    // Vanilla's own random tick would turn it into vanilla's next copper.
    *handled = true;
    if level.random().next_float() >= 0.056_888_89 {
        return;
    }
    let b = level.behaviours;
    let own = b.data(state).weather;
    let (mut same, mut older) = (0u32, 0u32);
    for x in -4i32..=4 {
        for y in -4i32..=4 {
            for z in -4i32..=4 {
                if x.abs() + y.abs() + z.abs() > 4 || (x, y, z) == (0, 0, 0) {
                    continue;
                }
                let data = b.data(level.block_state(pos.offset(x, y, z)));
                if !data.implements(Interface::WeatheringCopper) {
                    continue;
                }
                match data.weather.cmp(&own) {
                    std::cmp::Ordering::Less => return,
                    std::cmp::Ordering::Greater => older += 1,
                    std::cmp::Ordering::Equal => same += 1,
                }
            }
        }
    }
    let chance = (older + 1) as f32 / (older + same + 1) as f32;
    let modifier = if own == 0 { 0.75 } else { 1.0 };
    if level.random().next_float() < chance * chance * modifier
        && let Some(block) = b.table.block_by_name(&format!("create:{}", next.name()))
    {
        let to = b.table.with_properties_of(block, state);
        b.set_block(
            level,
            ticks,
            pos,
            to,
            horizon_world::level::flags::UPDATE_ALL,
        );
    }
}
