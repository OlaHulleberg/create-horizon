//! Two punching deployers facing each other bump fists (`DeployerBlockEntity.startFistBump`,
//! `triggerFistBump`): one at rest finds the other two to four blocks ahead, and both reach
//! out to meet halfway; the first to reach the end of its stroke, its partner there too, pulls
//! both back with a punch's sound, and earns its placer `FIST_BUMP`. A bump is not saved, as
//! Create's `fistBump` is not.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::deployer::{Mode, State};
use create_engine::kinetics::Dir;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// The deployers in a bump (`fistBump`), by level and position.
    static BUMPING: RefCell<HashSet<(usize, [i32; 3])>> = RefCell::new(HashSet::new());
}

/// The world closed.
pub(super) fn clear() {
    BUMPING.with_borrow_mut(HashSet::clear);
}

/// The first deployer two to four blocks ahead of the one at `pos` facing `facing`, and how
/// far; None when one of those blocks is not loaded or none is there.
fn partner(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> Option<([i32; 3], i32)> {
    let dim = lv.dim;
    for i in 2..5 {
        let p = super::step(pos, facing, i);
        if !lv.is_loaded(p) {
            return None;
        }
        if with_world(|w| w.dims[dim].deployers.contains_key(&p)) == Some(true) {
            return Some((p, i));
        }
    }
    None
}

/// `startFistBump` for the punching deployer at `pos` at rest: true when it and the one facing
/// it set off toward each other.
pub(super) fn start(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let dim = lv.dim;
    if BUMPING.with_borrow(|b| b.contains(&(dim, pos))) {
        return false;
    }
    let Some((other, i)) = partner(lv, pos, facing) else {
        return false;
    };
    let faces_back = lv.facing_of(other, Block::Deployer) == Some(facing.opposite());
    let punching = with_world(|w| w.dims[dim].deployers.get(&other).map(|d| d.mode)).flatten()
        == Some(Mode::Punch);
    if !faces_back || !punching || lv.speed(other) == 0.0 {
        return false;
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        for p in [pos, other] {
            if let Some(dp) = d.deployers.get_mut(&p) {
                dp.reach = (i - 2) as f32 * 0.5;
                dp.timer = 1000;
                dp.state = State::Expanding;
                d.deployer_changes.push(p);
            }
            if !d.deploying.contains(&p) {
                d.deploying.push(p);
            }
        }
    });
    BUMPING.with_borrow_mut(|b| {
        b.insert((dim, pos));
        b.insert((dim, other));
    });
    true
}

/// `triggerFistBump` for the deployer at `pos` at the end of its stroke: its partner there too
/// is drawn back, the bump sounds between them and its placer earns `FIST_BUMP`.
pub(super) fn trigger(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    if !BUMPING.with_borrow(|b| b.contains(&(dim, pos))) {
        return;
    }
    let Some(facing) = lv.facing_of(pos, Block::Deployer) else {
        return;
    };
    let Some((other, _)) = partner(lv, pos, facing) else {
        return;
    };
    let ready = BUMPING.with_borrow(|b| b.contains(&(dim, other)))
        && with_world(|w| {
            w.dims[dim]
                .deployers
                .get(&other)
                .is_some_and(|d| d.state == State::Expanding && d.timer <= 0)
        }) == Some(true);
    if !ready {
        return;
    }
    BUMPING.with_borrow_mut(|b| {
        b.remove(&(dim, pos));
        b.remove(&(dim, other));
    });
    with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(dp) = d.deployers.get_mut(&other) {
            dp.state = State::Retracting;
            dp.timer = 1000;
            d.deployer_changes.push(other);
        }
    });
    crate::glue::advancements::award_placer(lv, pos, "fist_bump");
    // Between the two, `PLAYER_ATTACK_NODAMAGE` at .75, .75.
    let at = std::array::from_fn(|i| (((pos[i] + other[i]) as f64 + 1.0) * 0.5).floor() + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:entity.player.attack.nodamage",
        at,
        mod_api::SoundSource::Blocks,
        0.75,
        0.75,
    );
}
