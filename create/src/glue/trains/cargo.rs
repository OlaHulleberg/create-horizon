//! What carriages carry (`CarriageContraption`'s `MountedStorageManager`): the chests,
//! barrels, vaults and toolboxes on a carriage keep their items while it is a train, as the
//! contraptions' mounted storage does (`contraptions::actors`, keyed per carriage), saved with
//! the train and put back when it is taken apart. A portable storage interface on a carriage
//! moving slower than a quarter block a tick connects to a stationary one facing it
//! (`PortableStorageInterfaceMovement` on a carriage: it never stalls the train), so funnels,
//! chutes and hoppers at a station reach the carriage's storage; every transfer counts as cargo
//! moving (`IdleCargoCondition`).

use super::super::server::ServerLevel;
use super::super::world::with_world;
use crate::glue::contraptions::actors;
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{Compound, ItemStack, Tag};

/// The mounted storage key of carriage `car` of train `train` (no piston, bearing or gantry
/// keys its contraption at x = `i32::MIN`).
pub fn key(train: u32, car: usize) -> [i32; 3] {
    [i32::MIN, train as i32, car as i32]
}

/// The train a mounted storage key belongs to.
pub fn train_of(key: [i32; 3]) -> Option<u32> {
    (key[0] == i32::MIN).then_some(key[1] as u32)
}

/// The storage blocks of a carriage assembling (offsets from `origin`) give their items to it.
pub fn carry(
    lv: &mut ServerLevel<'_>,
    train: u32,
    car: usize,
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    actors::carry_storage(lv, key(train, car), origin, blocks);
}

/// A carriage about to be taken apart: what its other block entities and the engine kept for
/// its blocks waits for them, each going back at `place(offset)` (`addBlocksToWorld`).
pub fn expect(
    lv: &mut ServerLevel<'_>,
    train: u32,
    car: usize,
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    actors::expect_back(lv, key(train, car), place);
}

/// A carriage taken apart: its items go back into its storage blocks, each now at
/// `place(offset)`.
pub fn release(
    lv: &mut ServerLevel<'_>,
    train: u32,
    car: usize,
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    actors::release_storage(lv, key(train, car), place);
}

/// Every stack the train's carriages hold.
pub fn stacks(dim: usize, train: u32, cars: usize) -> Vec<ItemStack> {
    (0..cars)
        .flat_map(|c| actors::storage_stacks(dim, key(train, c)))
        .collect()
}

/// The train's cargo as its save keeps it: each carriage's (`Car`) storage (`Storage`).
pub fn to_nbt(lv: &ServerLevel<'_>, train: u32, cars: usize) -> Option<Tag> {
    let list: Vec<Tag> = (0..cars)
        .filter_map(|c| {
            let s = actors::storage_to_nbt(lv, key(train, c))?;
            let mut k = Compound::new();
            k.insert("Car".into(), Tag::Int(c as i32));
            k.insert("Storage".into(), s);
            Some(Tag::Compound(k))
        })
        .collect();
    (!list.is_empty()).then_some(Tag::List(list))
}

/// What [`to_nbt`] saved, carried again by the train now numbered `train`.
pub fn from_nbt(lv: &ServerLevel<'_>, train: u32, tag: Option<&Tag>) {
    let Some(Tag::List(list)) = tag else {
        return;
    };
    for k in list {
        let Tag::Compound(k) = k else { continue };
        let Some(Tag::Int(c)) = k.get(&"Car".into()) else {
            continue;
        };
        actors::storage_from_nbt(
            lv,
            key(train, (*c).max(0) as usize),
            k.get(&"Storage".into()),
        );
    }
}

/// Cargo of trains this level no longer has (unloaded with their column) is forgotten.
pub fn prune(lv: &ServerLevel<'_>) {
    let dim = lv.dim;
    let alive: Vec<u32> = with_world(|w| w.dims[dim].railway.trains.iter().map(|t| t.id).collect())
        .unwrap_or_default();
    actors::retain_storage(dim, |k| train_of(k).is_none_or(|t| alive.contains(&t)));
}

/// Cargo moved in or out of train `train` (through an interface): its idle time starts again.
pub fn moved(dim: usize, train: u32) {
    with_world(|w| {
        if let Some(t) = w.dims[dim]
            .railway
            .trains
            .iter_mut()
            .find(|t| t.id == train)
        {
            t.idle = 0;
        }
    });
}

/// `Direction.getNearest` of a unit-ish vector along an axis.
fn dir_of(v: [i32; 3]) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| d.offset() == v)
}

/// The item interfaces on slow carriages connect to the stationary ones facing them
/// (`PortableStorageInterfaceMovement.visitNewPosition`, `findInterface`, `tick`).
pub fn tick_interfaces(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let host = &*lv.host;
    // Each slow carriage's interfaces: carriage key, world position of the block's centre, and
    // facing.
    let found: Vec<([i32; 3], [f64; 3], Dir)> = with_world(|w| {
        let r = &w.dims[dim].railway;
        let mut out = Vec::new();
        for t in r.trains.iter().filter(|t| t.train.bound) {
            if t.train.speed.abs() > 0.25 {
                continue;
            }
            for (c, car) in t.cars.iter().enumerate() {
                let interfaces: Vec<&([i32; 3], u32)> = car
                    .iter()
                    .filter(|(_, s)| {
                        create_engine::states::index(*s).is_some()
                            && w.table
                                .get(*s)
                                .is_some_and(|(b, _)| b == Block::PortableStorageInterface)
                    })
                    .collect();
                if interfaces.is_empty() {
                    continue;
                }
                let (pivot, front) = t.train.carriage_pose(&r.net.graph, c);
                let (yaw, _) = create_engine::trains::yaw_pitch(front, t.heading);
                let quarter = ((yaw / 90.0).round() as i32 * 90).rem_euclid(360);
                for (o, s) in interfaces {
                    let Some(facing) = w.table.get(*s).and_then(|(_, i)| i.facing) else {
                        continue;
                    };
                    let turned =
                        create_engine::contraption::rotate(facing.offset(), quarter, Axis::Y);
                    let Some(facing) = dir_of(turned) else {
                        continue;
                    };
                    let ro = create_engine::contraption::rotate([o[0], 0, o[2]], quarter, Axis::Y);
                    // The block's centre where taking the carriage apart would put it.
                    let centre = [
                        (pivot[0] + f64::from(ro[0])).floor() + 0.5,
                        (pivot[1] + f64::from(o[1]) + 1.0).round() + 0.5,
                        (pivot[2] + f64::from(ro[2])).floor() + 0.5,
                    ];
                    out.push((key(t.id, c), centre, facing));
                }
            }
        }
        out
    })
    .unwrap_or_default();
    let _ = host;
    for (k, centre, facing) in found {
        let f = facing.offset();
        let ahead = [
            (centre[0] + f64::from(f[0]) * create_engine::psi::REACH).floor() as i32,
            (centre[1] + f64::from(f[1]) * create_engine::psi::REACH).floor() as i32,
            (centre[2] + f64::from(f[2]) * create_engine::psi::REACH).floor() as i32,
        ];
        for i in 0..2 {
            let p = [
                ahead[0] + f[0] * i,
                ahead[1] + f[1] * i,
                ahead[2] + f[2] * i,
            ];
            if lv.create_block(p) != Some(Block::PortableStorageInterface) {
                continue;
            }
            let state = lv.block(p);
            let faces_back = with_world(|w| {
                w.table.get(state).and_then(|(_, info)| info.facing) == Some(facing.opposite())
            })
            .unwrap_or(false);
            if !faces_back {
                continue;
            }
            // `findInterface`: the distance between their faces along the facing.
            let along = (f64::from(p[0]) + 0.5 - centre[0]) * f64::from(f[0])
                + (f64::from(p[1]) + 0.5 - centre[1]) * f64::from(f[1])
                + (f64::from(p[2]) + 0.5 - centre[2]) * f64::from(f[2]);
            let distance = (along.abs() + create_engine::psi::REACH - 1.0) as f32;
            with_world(|w| {
                let d = &mut w.dims[dim];
                let Some(s) = d.psis.get_mut(&p) else {
                    return;
                };
                if s.powered {
                    return;
                }
                let was = *s;
                if s.connected.is_none() {
                    s.start_transferring_to(k, distance);
                }
                s.keep_alive = 2;
                let now = *s;
                if was.is_connected() != now.is_connected() {
                    d.analog_checks.push(p);
                }
                if was.connected.is_none() && now.connected.is_some() {
                    d.chute_pulls.wake([p[0], p[1] - 1, p[2]]);
                }
                if !d.psi_running.contains(&p) {
                    d.psi_running.push(p);
                }
            });
            break;
        }
    }
}
