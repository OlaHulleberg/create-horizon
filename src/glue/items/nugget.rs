//! `ExperienceNuggetItem.use`: the stack (one when sneaking) thrown out from before the eyes as
//! experience orbs, 3 points a nugget, and used up.

use super::{held, set_held, sneaking};
use mod_api::{Compound, HostSide, InteractionResult, ItemPart, ServerHost, Tag, UseItemEvent};

pub(super) struct Nugget;

#[mod_api::horizon_part]
impl ItemPart for Nugget {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        if let HostSide::Server(host) = host {
            throw(host, event);
        }
        Some(InteractionResult::Success)
    }
}

fn throw(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> Option<()> {
    let player = host.player(event.player)?;
    let entity = player.entity();
    let eye = entity.eye_position(&*host)?;
    let look = entity.view_vector(&*host)?;
    let [yaw, _] = entity.rotation(&*host)?;
    let dimension = player.dimension(&*host)?.to_string();
    let count = event.stack.count();
    let used = if sneaking(&*host, player) { 1 } else { count };
    // `look × rotate((-.75, 0, 0), -yRot, Y)`: to the side of the view.
    let yaw = f64::from(yaw).to_radians();
    let side = [-0.75 * yaw.cos(), 0.0, -0.75 * yaw.sin()];
    let cross = [
        look[1] * side[2] - look[2] * side[1],
        look[2] * side[0] - look[0] * side[2],
        look[0] * side[1] - look[1] * side[0],
    ];
    let at = [
        eye[0] + look[0] * 0.5 + cross[0],
        eye[1] + look[1] * 0.5 + cross[1],
        eye[2] + look[2] * 0.5 + cross[2],
    ];
    // `offsetRandomly(ZERO, level.random, 1).normalize()`: a scatter; any draw will do.
    let time = player
        .level(&*host)
        .and_then(|l| l.game_time(&*host))
        .unwrap_or(0);
    let mut seed = time as u64 ^ 0x9e37_79b9_7f4a_7c15;
    let mut draw = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 2.0
    };
    for value in create_engine::nugget::orbs(used) {
        let o = [draw(), draw(), draw()];
        let n = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt().max(1e-9);
        let motion = [
            look[0] * 0.2 + o[0] / n * 0.1,
            look[1] * 0.2 + 0.2 + o[1] / n * 0.1,
            look[2] * 0.2 + o[2] / n * 0.1,
        ];
        let nbt = Compound::from([
            ("Value".into(), Tag::Short(value as i16)),
            (
                "Motion".into(),
                Tag::List(motion.iter().map(|v| Tag::Double(v.to_bits())).collect()),
            ),
        ]);
        host.summon(&dimension, "minecraft:experience_orb", at, nbt);
    }
    let stack = held(&*host, player, event.hand)?;
    set_held(host, player, event.hand, stack.shrink(used));
    Some(())
}
