//! Sitting on a carriage's seats (`AbstractContraptionEntity.addSittingPassenger`): a click on
//! a seat block of a carriage seats the player there, and the seat carries them with the
//! train. Create seats them on the carriage entity itself; here, as the world's seats do
//! (`contraptions::seats`), on an empty item display (no model, no hitbox, no gravity) tagged
//! `create.carriage_seat` that the server moves to the seat every tick, gliding between. Getting
//! off puts the rider where Create does (`ContraptionDismountLocation`); a stand-in with no
//! rider, or whose train went, goes. Stand-ins are not saved as Create's: one that comes back
//! with its chunk or a player's file is discarded (`visuals::sweep`), its rider let off.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::train_controls::{Frame, Target};
use mod_api::{Compound, EntityRef, ServerHost, Tag};
use std::cell::RefCell;

/// The tag on the stand-ins.
pub const TAG: &str = "create.carriage_seat";

/// A seat in use: its level, the carriage's block it is, and its stand-in.
#[derive(Clone, Copy, Debug)]
struct Seated {
    dim: usize,
    at: Target,
    stand: u32,
}

thread_local! {
    static SEATED: RefCell<Vec<Seated>> = const { RefCell::new(Vec::new()) };
}

pub fn forget() {
    SEATED.with_borrow_mut(Vec::clear);
}

/// The players seated on each train of level `dim` (`countPlayerPassengers`).
pub fn passengers(dim: usize) -> std::collections::HashMap<u32, i32> {
    let mut out = std::collections::HashMap::new();
    SEATED.with_borrow(|s| {
        for s in s.iter().filter(|s| s.dim == dim) {
            *out.entry(s.at.train).or_insert(0) += 1;
        }
    });
    out
}

/// Whether the carriage block `at` is a seat (`SeatBlock`), by its state.
pub fn is_seat(host: &dyn ServerHost, state: mod_api::BlockState) -> bool {
    super::state_parts(host, state)
        .is_some_and(|(name, _)| name.starts_with("create:") && name.ends_with("_seat"))
}

/// The carriage's frame now.
fn frame(dim: usize, at: Target) -> Option<Frame> {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        let t = r
            .trains
            .iter()
            .find(|t| t.id == at.train && t.train.bound)?;
        let c = usize::from(at.car);
        (c < t.train.carriages.len()).then(|| {
            let (pivot, front) = t.train.carriage_pose(&r.net.graph, c);
            Frame::new(pivot, front, t.heading)
        })
    })
    .flatten()
}

/// The stand-in's compound: tagged as ours, gliding between moves as a carriage's displays do.
fn stand_nbt() -> Compound {
    let mut nbt = Compound::new();
    nbt.insert("teleport_duration".into(), Tag::Int(2));
    nbt.insert(
        "Tags".into(),
        Tag::List(vec![
            Tag::String(TAG.into()),
            Tag::String(super::visuals::session_tag().as_str().into()),
        ]),
    );
    nbt
}

/// `addSittingPassenger`: `player` sits on the carriage's seat `at`, unless someone else does
/// (a mob sitting there is put off for a player in Create; riders here are only players).
/// A player on another seat moves to this one.
pub fn sit(host: &mut dyn ServerHost, dim: usize, player: u128, at: Target) {
    let taken = SEATED.with_borrow(|s| {
        s.iter().any(|s| {
            s.dim == dim && s.at.train == at.train && s.at.car == at.car && s.at.local == at.local
        })
    });
    if taken {
        return;
    }
    let Some(f) = frame(dim, at) else {
        return;
    };
    let Some(p) = host.player(player) else {
        return;
    };
    let rider = p.entity();
    let Some(dimension) = p.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
        return;
    };
    if rider.vehicle(&*host).is_some() {
        rider.stop_riding(host);
    }
    let got = host.summon(
        &dimension,
        "minecraft:item_display",
        f.seat(at.local),
        stand_nbt(),
    );
    let Some(id) = got else {
        return;
    };
    let Some(stand) = host.entity(id) else {
        return;
    };
    let rode = rider.start_riding(host, stand, true);
    if !rode {
        stand.discard(host);
        return;
    }
    SEATED.with_borrow_mut(|s| s.push(Seated { dim, at, stand: id }));
}

/// A rider got off `vehicle`: if it is one of ours, it moves first to where Create lets them off
/// (`ContraptionDismountLocation`), so the game puts them there.
pub fn dismounted(host: &mut dyn ServerHost, vehicle: u32) {
    let Some(s) = SEATED.with_borrow(|s| s.iter().find(|s| s.stand == vehicle).copied()) else {
        return;
    };
    if let (Some(f), Some(stand)) = (frame(s.dim, s.at), host.entity(vehicle)) {
        stand.teleport_to(host, f.dismount(s.at.local), None);
    }
}

/// The players riding train `train`'s seats in level `dim` are let off (a train taken apart).
pub fn train_gone(lv: &mut ServerLevel<'_>, train: u32) {
    let dim = lv.dim;
    let gone: Vec<Seated> = SEATED.with_borrow_mut(|s| {
        let (gone, kept) = std::mem::take(s)
            .into_iter()
            .partition(|s| s.dim == dim && s.at.train == train);
        *s = kept;
        gone
    });
    for s in gone {
        if let Some(e) = lv.host.entity(s.stand) {
            e.eject_passengers(lv.host);
            e.discard(lv.host);
        }
    }
}

/// The level's tick, after the trains moved: stand-ins that came back go, each seat in use
/// moves to its seat, and goes when it has no rider or its train is gone.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let seated: Vec<Seated> =
        SEATED.with_borrow(|s| s.iter().filter(|s| s.dim == dim).copied().collect());
    for s in seated {
        let stand: Option<EntityRef> = lv.host.entity(s.stand);
        let ridden = stand
            .and_then(|e| e.passengers(&*lv.host))
            .is_some_and(|p| !p.is_empty());
        let f = frame(dim, s.at);
        match (stand, f) {
            (Some(e), Some(f)) if ridden => {
                let to = f.seat(s.at.local);
                if e.position(&*lv.host) != Some(to) {
                    e.teleport_to(lv.host, to, None);
                }
            }
            _ => {
                if let Some(e) = stand {
                    e.eject_passengers(lv.host);
                    e.discard(lv.host);
                }
                SEATED.with_borrow_mut(|v| v.retain(|v| v.stand != s.stand));
            }
        }
    }
}
