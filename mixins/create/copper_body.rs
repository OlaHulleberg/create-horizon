// What Create's copper shingles and tiles read of the game when they weather (block data, the
// level's random, setting the block) and when an axe or honeycomb is used on them; the template test swaps it for
// `tools/fixture/copper_body.rs`.
use super::Game;
use super::create::{TAKEN_OFF_HAND, with_world};
use create_engine::decor;
use create_engine::states;
use horizon_client::items::ItemStack;
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

/// An axe or honeycomb used on a Create copper shingle or tile block, as the `minecraft:axe`
/// transformer's scrape and wax-off transforms (`BlockTransformer.transformBlock`) and
/// `HoneycombItem.useOn` do vanilla's copper: into Create's previous age, unwaxed or waxed twin.
/// True when the use was theirs.
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    claimed: &mut bool,
) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if states::index(state.0).is_none() {
        return false;
    }
    let Some(block) = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten() else {
        return false;
    };
    let axe = game
        .res
        .items()
        .get(&stack.item)
        .and_then(|i| i.block_transformer.as_deref())
        == Some("minecraft:axe");
    let (to, event, sound) = if axe {
        // playerHasBlockingItemUseIntent: a shield in the off hand is raised instead, unless
        // sneaking.
        let shield = game
            .offhand
            .as_ref()
            .and_then(|s| game.items().get(&s.item))
            .is_some_and(|i| i.blocks_attacks.is_some());
        if shield && !game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
            return false;
        }
        match (decor::copper_previous(block), decor::copper_unwaxed(block)) {
            (Some(b), _) => (b, 3005, "minecraft:item.axe.scrape"),
            (None, Some(b)) => (b, 3004, "minecraft:item.axe.wax_off"),
            _ => return false,
        }
    } else if &*stack.item == "minecraft:honeycomb" {
        match decor::copper_waxed(block) {
            Some(b) => (b, 3003, "minecraft:item.honeycomb.wax_on"),
            None => return false,
        }
    } else {
        return false;
    };
    let b = game.res.behaviours();
    let Some(id) = b.table.block_by_name(&format!("create:{}", to.name())) else {
        return false;
    };
    let new = b.table.with_properties_of(id, state);
    *claimed = true;
    // SUCCESS ends the click in vanilla; Horizon's client, not knowing it, also sends the off
    // hand's click, which is taken (`TAKEN_OFF_HAND`, as for clicks Create's blocks take).
    if game.hand == horizon_client::first_person::Hand::Main {
        let player = game.native_acting_player().map_or(0, |p| p.uuid);
        TAKEN_OFF_HAND.with(|t| t.set(Some((player, game.world.game_time))));
    }
    // ITEM_USED_ON_BLOCK before the swap.
    game.trigger_block("minecraft:item_used_on_block", pos, stack);
    if game.level_set(pos, new).is_err() {
        return true;
    }
    game.level_event_at(event, pos, 0);
    let at = pos.map(|v| f64::from(v) + 0.5);
    // Vanilla plays it for the others only (`Level.playSound(player, ...)`), the actor's client
    // from its own prediction; Horizon's client predicts only vanilla's copper, so everyone hears
    // it from here.
    game.audio.play(sound, Some(at), 1.0, 1.0);
    if axe {
        game.stats.add("used", &stack.item, 1);
        game.damage_held(1);
        game.post_block_game_event("minecraft:block_change", pos, game.sculk_actor(), Some(new));
    } else {
        game.post_block_game_event("minecraft:block_change", pos, game.sculk_actor(), Some(new));
        game.stats.add("used", &stack.item, 1);
        if !game.creative {
            game.consume_held();
        }
    }
    true
}
