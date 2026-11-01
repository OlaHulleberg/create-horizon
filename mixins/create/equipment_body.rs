// What Create's equipment reads and writes of a stack and a player's body beyond the hook helpers:
// the backtank's air on its stack (kept in `minecraft:custom_data`, since Horizon adds no component
// types), its Capacity level, the breathing state `LivingBreatheEvent` would hand over, the air
// supply, the reach modifiers, a hand's wear and the local player's motion. Compiled against
// Horizon by `horizon.py check`; the template test swaps it for `tools/fixture/equipment_body.rs`.
use super::Game;
use create_engine::equipment::{AIR_KEY, CAPACITY, Diver, MAX_AIR_SUPPLY};
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};

const CUSTOM_DATA: &str = "minecraft:custom_data";
const BLOCK_RANGE: &str = "minecraft:block_interaction_range";

/// The air stored on a backtank stack (0 when none).
pub(super) fn stack_air(stack: &ItemStack) -> i32 {
    match stack.data().and_then(|d| d.other(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => match c.get(&AIR_KEY.into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

pub(super) fn set_stack_air(stack: &mut ItemStack, air: i32) {
    let data = stack.data_mut();
    let mut c = match data.other(CUSTOM_DATA) {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    c.insert(AIR_KEY.into(), Tag::Int(air));
    data.set_other(CUSTOM_DATA, Some(Tag::Compound(c)));
}

/// The stack's Capacity enchantment level (`getEnchantmentLevel(CAPACITY)`).
pub(super) fn capacity(stack: &ItemStack) -> i32 {
    stack
        .data()
        .and_then(|d| d.enchantments.iter().find(|(e, _)| &**e == CAPACITY))
        .map_or(0, |(_, l)| *l as i32)
}

pub(super) fn set_capacity(stack: &mut ItemStack, level: i32) {
    if level > 0 {
        let data = stack.data_mut();
        data.enchantments.retain(|(e, _)| &**e != CAPACITY);
        data.enchantments
            .push((std::sync::Arc::from(CAPACITY), level as u32));
    }
}

/// The acting player's breathing: whether they breathe without help (eyes out of water, Water
/// Breathing, invulnerable), whether they are in lava, and in creative.
pub(super) struct Body {
    pub(super) can_breathe: bool,
    pub(super) in_lava: bool,
    pub(super) creative: bool,
}

pub(super) fn body(game: &Game<'_>) -> Option<Body> {
    let p = game.player.as_ref()?;
    Some(Body {
        can_breathe: !p.is_eye_in_fluid(horizon_client::motion::WATER)
            || p.status.water_breathing
            || p.invulnerable,
        in_lava: p.is_in_lava(),
        creative: game.creative,
    })
}

/// `setRefillAirAmount(getMaxAirSupply())`: the air supply full again.
pub(super) fn refill_air(game: &mut Game<'_>) {
    if let Some(p) = game.player.as_mut() {
        p.air = MAX_AIR_SUPPLY;
    }
}

/// Whether the acting player's block reach has the modifier `id`.
pub(super) fn has_reach(game: &mut Game<'_>, id: &str) -> bool {
    let Some(p) = game.player.as_mut() else {
        return false;
    };
    let Some(key) = p.attributes.key(BLOCK_RANGE) else {
        return false;
    };
    p.attributes.has_modifier(Some(key), id).unwrap_or(false)
}

/// Adds (`addTransientAttributeModifiers`) or removes a block reach modifier; the player's
/// client gets it with the attribute sync.
pub(super) fn set_reach(game: &mut Game<'_>, modifier: (&str, f64), on: bool) {
    if let Some(p) = game.player.as_mut() {
        let _ = p.set_modifier(BLOCK_RANGE, modifier.0, modifier.1, "add_value", on);
    }
}

/// `hurtAndBreak(1)` on the stack in the main or off hand (Unbreaking applies; none in creative).
pub(super) fn damage_hand(game: &mut Game<'_>, off: bool) {
    let hand = if off {
        horizon_client::first_person::Hand::Off
    } else {
        horizon_client::first_person::Hand::Main
    };
    game.with_hand(hand, |g| g.damage_held(1));
}

/// The local player as the diving boots read them, with whether they are in water, swimming and
/// flying. The window's player only (its client moves it).
pub(super) fn diver(game: &Game<'_>) -> Option<(Diver, bool, bool, bool)> {
    let p = game.player.as_ref()?;
    Some((
        Diver {
            motion: [p.velocity.x, p.velocity.y, p.velocity.z],
            jumping: p.jumping,
            on_ground: p.ground.on_ground,
            vertical_collision: p.collision.vertical,
            xxa: p.applied_input[0],
            zza: p.applied_input[1],
            shift: p.shift_key_down,
        },
        p.fluids.was_touching_water,
        p.pose == horizon_client::pose::Pose::Swimming,
        p.flying,
    ))
}

pub(super) fn set_dive(game: &mut Game<'_>, motion: [f64; 3], on_ground: bool) {
    if let Some(p) = game.player.as_mut() {
        (p.velocity.x, p.velocity.y, p.velocity.z) = (motion[0], motion[1], motion[2]);
        p.ground.on_ground = on_ground;
    }
}
