//! Moving structures to the entities around them (`ContraptionCollider.collideEntities`): each
//! structure out of the world gives an entity its blocks' collision boxes as a moving collider,
//! so entities stand on it, walk into its sides and ride along as it moves and turns. Bearings,
//! windmills, clocks, pistons, pulleys and elevators each have a stand-in of Create's
//! `ControlledContraptionEntity`, gantries of `GantryContraptionEntity` and train carriages of
//! `CarriageContraptionEntity`, saved with the structure it carries; a cart's structure is
//! carried by its rider, so the cart under it passes (`canCollideWith`).
//!
//! The game sweeps entities against the boxes and carries the bodies standing on them. It reads a
//! collider's move within the level's entity phase, so each carrier moves in its own tick to
//! where its structure is going. Left out (`API_GAPS.md`): a structure moving into an entity
//! does not push it, and its blocks have no contact effects.

use super::super::server::ServerLevel;
use super::super::subsystem::Subsystem;
use super::super::world::level_index;
use super::super::world::with_world;
use super::create_info;
use create_engine::collider::{self, Aabb, Pose};
use create_engine::kinetics::Dir;
use mod_api::{
    AttachmentCodec, AttachmentError, AttachmentType, BlockShapeKind, Collider, ColliderBox,
    Compound, EntityJoinLevelEvent, EntityPart, EntityRef, EntitySpawn, Flow,
    GeneralEntityCategory, GeneralEntityTickEvent, GeneralSavePolicy, Holder, Registrar,
    RegistrationContext, RemovalReason, ServerHost, SpawnBody, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// `ControlledContraptionEntity`: bearings', windmills', clocks', pistons', pulleys' and
/// elevators' structures.
pub const STATIONARY: &str = "create:stationary_contraption";
/// `GantryContraptionEntity`.
pub const GANTRY: &str = "create:gantry_contraption";
/// `CarriageContraptionEntity`.
pub const CARRIAGE: &str = "create:carriage_contraption";

/// The structure a collider stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    /// A structure its anchor block holds (a bearing, piston or pulley); a clock's hands by
    /// number.
    Anchor([i32; 3], u8),
    /// A gantry carriage's, by the shaft block it started on.
    Gantry([i32; 3]),
    /// A train's carriage.
    Carriage(u32, usize),
    /// A cart's, by the cart's entity.
    Cart(u32),
}

/// What carries the boxes.
#[derive(Clone, Copy, Debug)]
pub enum Carrier {
    /// A stand-in of this kind, put where the structure is each tick.
    Own(&'static str),
    /// An entity that moves with the structure (a cart's rider).
    Entity(EntityRef),
}

struct Placed {
    entity: EntityRef,
    own: bool,
    blocks: u64,
    shift: [f64; 3],
    /// Where its carrier stands and its boxes' turn, and where they go next (`apply_to`).
    at: [f64; 3],
    rotation: [f32; 4],
    next: ([f64; 3], [f32; 4]),
    /// Where its structure was at the end of the last tick.
    now: ([f64; 3], [f64; 4]),
    tick: i64,
}

/// The structure a stand-in carries, saved with it, so that the one that comes back with its
/// chunk carries its structure on, solid from the start.
static CARRYING: AttachmentType<Option<Owner>> = AttachmentType {
    namespace: "create",
    name: "carried_structure",
    default: || None,
    codec: Some(AttachmentCodec {
        encode: |owner| {
            let (kind, at, n) = match owner {
                Some(Owner::Anchor(p, n)) => (0, *p, i32::from(*n)),
                Some(Owner::Gantry(p)) => (1, *p, 0),
                Some(Owner::Carriage(train, car)) => (2, [*car as i32, 0, 0], *train as i32),
                Some(Owner::Cart(_)) | None => (-1, [0; 3], 0),
            };
            Ok(Compound::from([
                ("Kind".into(), Tag::Int(kind)),
                ("At".into(), Tag::IntArray(at.to_vec())),
                ("N".into(), Tag::Int(n)),
            ]))
        },
        decode: |saved| {
            let int = |key: &str| match saved.get(&key.into()) {
                Some(Tag::Int(v)) => Ok(*v),
                _ => Err(AttachmentError::Decode),
            };
            let Some(Tag::IntArray(at)) = saved.get(&"At".into()) else {
                return Err(AttachmentError::Decode);
            };
            let at: [i32; 3] = at
                .as_slice()
                .try_into()
                .map_err(|_| AttachmentError::Decode)?;
            let n = int("N")?;
            Ok(match int("Kind")? {
                0 => Some(Owner::Anchor(at, n.clamp(0, 255) as u8)),
                1 => Some(Owner::Gantry(at)),
                2 => Some(Owner::Carriage(n as u32, at[0].max(0) as usize)),
                _ => None,
            })
        },
    }),
    copy_on_death: false,
    sync: false,
};

thread_local! {
    static PLACED: RefCell<HashMap<(usize, Owner), Placed>> = RefCell::new(HashMap::new());
    /// Stand-ins that came back with their chunks this tick, by their structures.
    static LOADED: RefCell<HashMap<(usize, Owner), EntityRef>> = RefCell::new(HashMap::new());
    /// Stand-ins that came back carrying nothing.
    static STRAYS: RefCell<Vec<(usize, EntityRef)>> = const { RefCell::new(Vec::new()) };
    /// The structure each carrier carries, by the carrier's entity id.
    static CARRIERS: RefCell<HashMap<u32, (usize, Owner)>> = RefCell::new(HashMap::new());
}

/// A structure's blocks (offsets and states), for telling whether its boxes must be made again.
pub fn blocks_hash(blocks: &[([i32; 3], u32)]) -> u64 {
    let mut h = DefaultHasher::new();
    blocks.hash(&mut h);
    h.finish()
}

/// The structures out of the world give entities their boxes: after every structure moved this
/// tick (after `Contraptions`, `Carts` and `Trains`), and the boxes go there in their carriers'
/// next tick (`apply_to`).
pub struct Colliders;

impl Subsystem for Colliders {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        // Create's tracking ranges (`AllEntityTypes.contraption`); every tick, as these don't
        // move themselves on clients.
        for (path, chunks) in [
            ("stationary_contraption", 20),
            ("gantry_contraption", 10),
            ("carriage_contraption", 15),
        ] {
            if let Err(e) = r.declare_general_entity(
                path,
                GeneralEntityCategory::Misc,
                0.5,
                0.5,
                None,
                GeneralSavePolicy::Save,
                false,
                false,
                chunks,
                1,
                1,
                &[],
            ) {
                crate::create_log!("[create] {path} type was refused: {e:?}");
            }
        }
        for kind in [STATIONARY, GANTRY, CARRIAGE] {
            r.entity_part(kind, Carrying);
        }
        if let Err(e) = r.register_attachment_type("create:carried_structure", false) {
            crate::create_log!("[create] declaring carried_structure: {e:?}");
        }
    }

    fn world_closed(&self) {
        PLACED.with_borrow_mut(HashMap::clear);
        CARRIERS.with_borrow_mut(HashMap::clear);
        LOADED.with_borrow_mut(HashMap::clear);
        STRAYS.with_borrow_mut(Vec::clear);
    }

    /// A stand-in came back from its chunk's save: its structure takes it again as it places its
    /// collider in this tick, else it goes at the tick's end.
    fn entity_join_level(
        &self,
        host: &mut dyn ServerHost,
        event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        if event.loaded_from_disk
            && [STATIONARY, GANTRY, CARRIAGE].contains(&event.kind)
            && let Some(dim) = level_index(event.dimension)
            && let Some(entity) = host.entity(event.entity)
        {
            let owner = CARRYING
                .get_existing(host, Holder::Entity(event.entity))
                .ok()
                .flatten()
                .flatten();
            let stray = match owner {
                Some(owner) => LOADED.with_borrow_mut(|l| l.insert((dim, owner), entity)),
                None => Some(entity),
            };
            if let Some(stray) = stray {
                STRAYS.with_borrow_mut(|s| s.push((dim, stray)));
            }
        }
        Flow::Continue
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        place_anchored(lv);
        let (dim, now) = (lv.dim, lv.game_time());
        let gone: Vec<Placed> = PLACED.with_borrow_mut(|m| {
            let keys: Vec<(usize, Owner)> = m
                .iter()
                .filter(|(k, p)| k.0 == dim && p.tick != now)
                .map(|(k, _)| *k)
                .collect();
            keys.into_iter().filter_map(|k| m.remove(&k)).collect()
        });
        for p in gone {
            take_away(lv, &p);
        }
        let mut unclaimed: Vec<EntityRef> = LOADED.with_borrow_mut(|l| {
            let keys: Vec<(usize, Owner)> = l.keys().filter(|k| k.0 == dim).copied().collect();
            keys.into_iter().filter_map(|k| l.remove(&k)).collect()
        });
        STRAYS.with_borrow_mut(|s| {
            unclaimed.extend(s.iter().filter(|e| e.0 == dim).map(|e| e.1));
            s.retain(|e| e.0 != dim);
        });
        for e in unclaimed {
            e.remove(&mut *lv.host, RemovalReason::Discarded);
        }
    }
}

struct Carrying;

#[mod_api::sunrise_part]
impl EntityPart for Carrying {
    fn general_entity_tick(
        &self,
        host: &mut dyn ServerHost,
        event: &mut GeneralEntityTickEvent<'_>,
    ) {
        apply_to(host, event.entity);
    }
}

/// In the carrier's tick: its boxes go where its structure moved to in the last level tick. The
/// game carries the bodies standing on them by that move once the level's entities ticked (it
/// reads a collider's move from where it stood as the level's tick began, and the structures move
/// at the tick's end).
pub fn apply_to(host: &mut dyn ServerHost, entity: EntityRef) {
    let Some(key) = CARRIERS.with_borrow(|c| c.get(&entity.id()).copied()) else {
        return;
    };
    let Some((at, rotation)) = PLACED.with_borrow_mut(|m| {
        let p = m.get_mut(&key).filter(|p| p.entity == entity)?;
        let moved = p.own && p.at != p.next.0;
        let turned = p.rotation != p.next.1;
        (p.at, p.rotation) = p.next;
        Some((moved.then_some(p.at), turned.then_some(p.rotation)))
    }) else {
        return;
    };
    if let Some(rotation) = rotation {
        let _ = entity.set_collider_transform(host, rotation);
    }
    if let Some(at) = at {
        entity.teleport_to(host, at, None);
    }
}

fn take_away(lv: &mut ServerLevel<'_>, p: &Placed) {
    CARRIERS.with_borrow_mut(|c| c.remove(&p.entity.id()));
    if p.own {
        p.entity.remove(&mut *lv.host, RemovalReason::Discarded);
    } else {
        let _ = p.entity.set_collider(&mut *lv.host, None);
    }
}

/// The structures the engine keeps out of the world by their anchors, where they stand now.
fn place_anchored(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let facing = |lv: &ServerLevel<'_>, p: [i32; 3]| -> Option<Dir> {
        create_info(lv.block(p)).and_then(|(_, i)| i.facing)
    };
    let along = |base: [i32; 3], dir: [i32; 3], offset: f64| -> [f64; 3] {
        [0, 1, 2].map(|i| f64::from(base[i]) + f64::from(dir[i]) * offset)
    };
    let now = lv.game_time();
    let anchors: Vec<[i32; 3]> = with_world(|w| {
        let d = &w.dims[dim];
        d.windmills
            .keys()
            .chain(d.pistons.keys())
            .chain(d.lifts.clocks.keys())
            .copied()
            .collect()
    })
    .unwrap_or_default();
    let facings: HashMap<[i32; 3], Dir> = anchors
        .into_iter()
        .filter_map(|p| Some((p, facing(lv, p)?)))
        .collect();
    let standing: Vec<(Owner, &'static str, Pose, u64)> = with_world(|w| {
        let d = &w.dims[dim];
        let mut out = Vec::new();
        let mut put = |owner, kind, pose, blocks: &[([i32; 3], u32)]| {
            if !blocks.is_empty() {
                out.push((owner, kind, pose, blocks_hash(blocks)));
            }
        };
        for (p, m) in &d.windmills {
            if let Some(f) = facings.get(p) {
                let deg = f64::from(m.turn.angle_at(now));
                let pose = Pose::turned(*p, f.axis(), deg);
                put(Owner::Anchor(*p, 0), STATIONARY, pose, &m.blocks);
            }
        }
        for (p, m) in &d.pistons {
            if let Some(f) = facings.get(p) {
                let pose = Pose::translated(along(m.anchor, f.offset(), f64::from(m.offset)));
                put(Owner::Anchor(*p, 0), STATIONARY, pose, &m.blocks);
            }
        }
        for g in &d.gantries {
            let corner = along(g.start, g.shaft_facing.offset(), f64::from(g.offset));
            put(
                Owner::Gantry(g.start),
                GANTRY,
                Pose::translated(corner),
                &g.blocks,
            );
        }
        let l = &d.lifts;
        for (p, c) in &l.pulleys {
            let base = [c.anchor[0], c.anchor[1] + c.initial, c.anchor[2]];
            let pose = Pose::translated(along(base, [0, -1, 0], f64::from(c.offset)));
            put(Owner::Anchor(*p, 0), STATIONARY, pose, &c.blocks);
        }
        for (p, e) in &l.elevators {
            let corner = [f64::from(e.anchor[0]), e.y, f64::from(e.anchor[2])];
            let pose = Pose::translated(corner);
            put(Owner::Anchor(*p, 0), STATIONARY, pose, &e.blocks);
        }
        for (p, c) in &l.clocks {
            let axis = c.facing.axis();
            let hour = Pose::turned(*p, axis, f64::from(c.clock.hour));
            put(Owner::Anchor(*p, 0), STATIONARY, hour, &c.hour);
            if let Some((_, minute)) = &c.minute {
                let pose = Pose::turned(*p, axis, f64::from(c.clock.minute));
                put(Owner::Anchor(*p, 1), STATIONARY, pose, minute);
            }
        }
        out
    })
    .unwrap_or_default();
    for (owner, kind, pose, hash) in standing {
        place(lv, owner, Carrier::Own(kind), pose, hash, || {
            anchored_blocks(dim, owner)
        });
    }
}

fn anchored_blocks(dim: usize, owner: Owner) -> Vec<([i32; 3], u32)> {
    with_world(|w| {
        let d = &w.dims[dim];
        let l = &d.lifts;
        match owner {
            Owner::Anchor(p, 0) => d
                .windmills
                .get(&p)
                .map(|m| &m.blocks)
                .or_else(|| d.pistons.get(&p).map(|m| &m.blocks))
                .or_else(|| l.pulleys.get(&p).map(|m| &m.blocks))
                .or_else(|| l.elevators.get(&p).map(|m| &m.blocks))
                .or_else(|| l.clocks.get(&p).map(|m| &m.hour))
                .cloned(),
            Owner::Anchor(p, _) => l
                .clocks
                .get(&p)
                .and_then(|c| c.minute.as_ref().map(|m| m.1.clone())),
            Owner::Gantry(start) => d
                .gantries
                .iter()
                .find(|g| g.start == start)
                .map(|g| g.blocks.clone()),
            _ => None,
        }
    })
    .flatten()
    .unwrap_or_default()
}

/// The structure `owner` stands at `pose` now, its blocks hashing to `hash` (made by `blocks`
/// only when they changed): its carrier's boxes follow it (`apply_to`). A structure not placed in a
/// level's tick is gone from it by the end of the tick.
pub fn place(
    lv: &mut ServerLevel<'_>,
    owner: Owner,
    carrier: Carrier,
    pose: Pose,
    hash: u64,
    blocks: impl FnOnce() -> Vec<([i32; 3], u32)>,
) {
    let key = (lv.dim, owner);
    let now = lv.game_time();
    let mut last = PLACED.with_borrow_mut(|m| m.remove(&key));
    if let Some(p) = last.take_if(|p| match carrier {
        Carrier::Entity(e) => p.entity != e,
        Carrier::Own(_) => lv.host.entity(p.entity.id()).is_none(),
    }) {
        take_away(lv, &p);
    }
    let own = matches!(carrier, Carrier::Own(_));
    let entity = match (carrier, &last) {
        (_, Some(p)) => p.entity,
        (Carrier::Entity(e), None) => e,
        (Carrier::Own(kind), None) => match LOADED.with_borrow_mut(|l| l.remove(&key)) {
            Some(e) => e,
            None => {
                let spawn = EntitySpawn::new(SpawnBody::General);
                let Some(e) = lv
                    .level
                    .spawn_entity(&mut *lv.host, kind, pose.centre, &spawn)
                else {
                    return;
                };
                let _ = CARRYING.set(&mut *lv.host, Holder::Entity(e.id()), Some(owner));
                e
            }
        },
    };
    let at = if own {
        pose.centre
    } else {
        match entity.position(&*lv.host) {
            Some(at) => at,
            None => return,
        }
    };
    let shift = pose.shift_from(at);
    let rotation = unit(pose.rotation());
    let (was_at, was_turned) = match &last {
        Some(p) => (p.at, p.rotation),
        None => (entity.position(&*lv.host).unwrap_or(at), rotation),
    };
    let same_boxes = last
        .as_ref()
        .is_some_and(|p| p.blocks == hash && (0..3).all(|i| (p.shift[i] - shift[i]).abs() < 1e-6));
    if !same_boxes {
        let boxes = boxes(lv, &blocks(), pose, shift);
        let collider = Collider {
            boxes,
            rotation: was_turned,
        };
        if let Err(e) = entity.set_collider(&mut *lv.host, Some(collider)) {
            crate::create_log!("[create] a moving structure's collider was refused: {e:?}");
            if own {
                entity.remove(&mut *lv.host, RemovalReason::Discarded);
            }
            return;
        }
    }
    // The carrier moves in the next tick's entity phase, ahead of its structure's move at that
    // tick's end, so that what it carries stands where the structure's blocks will be.
    let reported = (at, pose.rotation());
    let (to, turn) = last
        .as_ref()
        .map_or(reported, |p| collider::ahead(p.now, reported));
    CARRIERS.with_borrow_mut(|c| c.insert(entity.id(), key));
    PLACED.with_borrow_mut(|m| {
        m.insert(
            key,
            Placed {
                entity,
                own,
                blocks: hash,
                shift,
                at: was_at,
                rotation: was_turned,
                next: (to, unit(turn)),
                now: reported,
                tick: now,
            },
        )
    });
}

/// `q` as a unit quaternion of `f32`s, its `w` never negative: the same turn from tick to tick
/// is the same four numbers.
fn unit(q: [f64; 4]) -> [f32; 4] {
    let len = q.iter().map(|v| v * v).sum::<f64>().sqrt() * q[3].signum();
    q.map(|v| (v / len) as f32)
}

/// The blocks' collision boxes (each block's shape where the structure has it now), joined, in
/// the carrier's frame.
fn boxes(
    lv: &ServerLevel<'_>,
    blocks: &[([i32; 3], u32)],
    pose: Pose,
    shift: [f64; 3],
) -> Vec<ColliderBox> {
    let shapes: Vec<([i32; 3], Vec<Aabb>)> = blocks
        .iter()
        .map(|(o, s)| {
            let cell = pose
                .to_world(o.map(|v| f64::from(v) + 0.5))
                .map(|v| v.floor() as i32);
            let shape = super::unnum(*s)
                .and_then(|state| {
                    lv.level
                        .block_shape(&*lv.host, state, cell, BlockShapeKind::Collision)
                })
                .unwrap_or_default();
            (*o, shape)
        })
        .collect();
    collider::simplified(shapes.iter().map(|(o, s)| (*o, s.as_slice())))
        .into_iter()
        .map(|b| ColliderBox {
            min: [0, 1, 2].map(|i| b[i] + shift[i]),
            max: [0, 1, 2].map(|i| b[i + 3] + shift[i]),
        })
        .collect()
}
