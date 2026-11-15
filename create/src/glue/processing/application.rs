//! `ManualApplicationRecipe.manualApplicationRecipesApplyInWorld`: a player's click with an item
//! on a block a `create:item_application` recipe takes (andesite alloy on a stripped log, brass on
//! stripped wood, a sturdy sheet on a brass casing...) turns the block into the recipe's block.

use super::{recipes, stacks};
use crate::glue::server::ServerLevel;
use create_engine::processing::Rng;
use mod_api::{EquipmentSlot, InteractionResult, RightClickBlockEvent, ServerHost};

/// Applies the held item to the clicked block when a recipe takes both: true when it did (the
/// click is then the recipe's).
pub fn apply_in_world(host: &mut dyn ServerHost, event: &mut RightClickBlockEvent<'_>) -> bool {
    let click = event.click;
    // The main hand's item: `take_held` and `hurt_and_break` act on it.
    if click.hand != mod_api::Hand::Main {
        return false;
    }
    let Some(stack) = click.stack else {
        return false;
    };
    let dimension = click.dimension.key().id().to_owned();
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let pos = click.pos;
    if lv.is_air(pos) {
        return false;
    }
    let Some(player) = lv.host.player(click.player) else {
        return false;
    };
    let host = &*lv.host;
    let Some(block) = lv.level.block(host, pos) else {
        return false;
    };
    let Some(target) = host.block_item(block).map(|i| i.id(host)) else {
        return false;
    };
    let held = stacks::id(host, stack);
    let Some(recipe) = recipes::with(|r| {
        create_engine::deployer::find(&r.manual_application, &target, &held).cloned()
    })
    .flatten() else {
        return false;
    };
    *event.cancellation_result = InteractionResult::Success;

    sound(&mut lv, pos);
    let Some(old) = lv.block_state(pos) else {
        return true;
    };
    lv.destroy_block(pos, false);
    // `transformBlock`: the first result's block with the old block's properties, else air;
    // the rest (`getRollableResultsExceptBlock`) pop out.
    let host = &*lv.host;
    let first = recipe.results.first();
    let to_block = first
        .and_then(|o| stacks::key(host, &o.item))
        .and_then(|i| host.item_block(i));
    let new_state = to_block.and_then(|b| crate::glue::blocks::copied_state(host, old, b));
    if let Some(s) = new_state {
        lv.set_state(pos, s, crate::glue::server::update_all());
    }
    let skip = usize::from(new_state.is_some());
    let rest = create_engine::processing::Recipe {
        id: recipe.id.clone(),
        ingredients: Vec::new(),
        results: recipe.results.iter().skip(skip).cloned().collect(),
        time: 0,
    };
    let mut rng = Rng::new(lv.seed_at(pos));
    let rolled = create_engine::processing::roll(&rest, &mut rng);
    let at = pos.map(|v| f64::from(v) + 0.5);
    for (item, n) in rolled {
        lv.spawn_moving(at, &item, u32::from(n), [0.0; 3]);
    }

    let unbreakable = lv
        .host
        .item_default_component(stack.item(), "minecraft:unbreakable")
        .is_some()
        || stack
            .components()
            .is_some_and(|c| c.as_nbt().contains_key(&"minecraft:unbreakable".into()));
    let keep = recipe.keep_held || lv.creative(player);
    if !unbreakable && !keep {
        let damageable = lv
            .host
            .item_default_component(stack.item(), "minecraft:max_damage")
            .is_some();
        if damageable {
            if let Some(living) = lv.host.living(player.entity()) {
                living.hurt_and_break(lv.host, EquipmentSlot::MainHand, 1);
            }
        } else {
            // A container item leaves what it holds (`getCraftingRemainingItem`).
            let leftover = lv.host.item_crafting_remainder(stack);
            lv.take_held(player, 1);
            if let Some(l) = leftover {
                if lv.held(player).is_none() {
                    lv.set_held(player, Some(l));
                } else {
                    lv.give(player, l);
                }
            }
        }
    }
    let placed = first.filter(|_| new_state.is_some()).map(|o| &*o.item);
    crate::glue::advancements::award_casing(lv.host, player, placed);
    true
}

/// `SoundEvents.COPPER_BREAK` at 1.45 pitch, for players.
fn sound(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.break",
        at,
        mod_api::SoundSource::Players,
        1.0,
        1.45,
    );
}
