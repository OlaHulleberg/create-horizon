//! Bearings and windmills, pistons, chassis and gantries moving blocks as one, and their saved
//! data (`windmills`, `chassis` in the column's compound).

use super::super::server::{ServerLevel, update_all};
use super::super::world::with_world;
use super::{
    actors, add, axis_of, clicker, dir_name, dir_of, draw, flag, found, in_click_level, prop,
    put_back_blocks, state_of_text, state_text, state_with, take_blocks, turned_state,
};
use create_engine::Block;
use create_engine::actors::Carried;
use create_engine::contraption::{self, Turn};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockPart, Compound, InteractionResult, Registrar, ServerHost, Tag, UseItemOnEvent};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// A turning structure's actors, and a windmill's own turn.
type Turning = (Carried, Option<Turn>);

/// A structure's blocks: offsets and states.
type Blocks = Vec<([i32; 3], u32)>;

/// A piston's speed and the game time when it stopped of itself.
type Stopped = (f32, i64);

thread_local! {
    /// The actors of bearings' turning structures (mechanical and windmill bearings), by
    /// dimension and bearing; only structures that have any. A windmill's with the angle it has
    /// turned to (a mechanical bearing keeps its own, `Windmill.bearing`).
    static BEARING_CARRIED: RefCell<HashMap<(usize, [i32; 3]), Turning>> =
        RefCell::new(HashMap::new());
    /// The structures whose actors were looked for (once each: assembled or loaded).
    static BEARINGS_LOOKED: RefCell<HashSet<(usize, [i32; 3])>> = RefCell::new(HashSet::new());
    /// Pistons that stopped of themselves (at an end, against the world) at a speed: the
    /// block updates of their stop are no speed change (`assembleNextTick` is only set by
    /// `onSpeedChanged`), so they don't start again from them: the speed and game time of the
    /// stop, for the checks of the next few ticks.
    static PISTONS_STOPPED: RefCell<HashMap<(usize, [i32; 3]), Stopped>> =
        RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    BEARING_CARRIED.with_borrow_mut(HashMap::clear);
    BEARINGS_LOOKED.with_borrow_mut(HashSet::clear);
    PISTONS_STOPPED.with_borrow_mut(HashMap::clear);
}

pub(super) fn register(r: &mut dyn Registrar) {
    use super::super::states::key;
    for b in [
        Block::LinearChassis,
        Block::SecondaryLinearChassis,
        Block::RadialChassis,
    ] {
        r.block_part(key(b), ChassisPart);
    }
    r.block_part(key(Block::WindmillBearing), WindmillPart);
    r.block_part(key(Block::MechanicalBearing), MechanicalBearingPart);
}

/// `MechanicalBearingBlockEntity.onSpeedChanged`: looked at on the next tick.
pub(super) fn bearing_speed_changed(dim: usize, pos: [i32; 3]) {
    with_world(|w| w.dims[dim].bearing_checks.push(pos));
}

// ---------------------------------------------------------------------------------------------
// Chassis

/// `ChassisBlockEntity.collectChassisGroup`: the chassis joined to `pos`, of its kind and axis
/// (a radial chassis' group: the radial chassis on its axis either side). For the value box's
/// range (`ChassisScrollValueBehaviour`, `World::set_chassis_range`).
pub fn chassis_group(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<[i32; 3]> {
    let kind = |p: [i32; 3]| {
        let (name, props) = lv.name_props(p)?;
        matches!(
            name.as_str(),
            "create:linear_chassis" | "create:secondary_linear_chassis" | "create:radial_chassis"
        )
        .then(|| (name, prop(&props, "axis").map(str::to_owned)))
    };
    let Some(own) = kind(pos) else {
        return Vec::new();
    };
    let radial = own.0 == "create:radial_chassis";
    let axis = axis_of(own.1.as_deref());
    let mut group = vec![pos];
    let mut i = 0;
    while i < group.len() && group.len() < 256 {
        let p = group[i];
        i += 1;
        for d in Dir::ALL {
            if radial && d.axis() != axis {
                continue;
            }
            let n = add(p, d.offset());
            if !group.contains(&n) && kind(n).as_ref() == Some(&own) {
                group.push(n);
            }
        }
    }
    group
}

struct ChassisPart;

#[mod_api::horizon_part]
impl BlockPart for ChassisPart {
    /// `AbstractChassisBlock.useItemOn`: a slime ball on a glueable side glues it (on a glued
    /// side: every side that may be glued); sneaking with an empty hand unglues it. A linear
    /// chassis' end against a chassis of its kind on its axis can't be glued
    /// (`glueAllowedOnSide`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = clicker(&*host, event.click);
        let pos = event.click.pos;
        let face = event.click.hit.face;
        in_click_level(host, event.click, |lv| use_chassis(lv, pos, face, &c))?
    }
}

fn use_chassis(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    c: &super::Clicker,
) -> Option<InteractionResult> {
    let (name, mut props) = lv.name_props(pos)?;
    let slime = matches!(
        c.held.as_deref(),
        Some("minecraft:slime_ball" | "create:super_glue")
    );
    let axis = prop(&props, "axis").unwrap_or_default().to_owned();
    let radial = name == "create:radial_chassis";
    // getGlueableSide: a linear chassis' end on its axis, sticky_top toward the positive end;
    // a radial chassis' side across it (`RadialChassisBlock.getGlueableSide`).
    let side_of = |f: [i32; 3]| -> Option<&'static str> {
        let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
        if radial {
            let a = [Axis::X, Axis::Y, Axis::Z][i];
            return contraption::radial_side(a, Dir::from_offset(f)?);
        }
        match f[i] {
            1 => Some("sticky_top"),
            -1 => Some("sticky_bottom"),
            _ => None,
        }
    };
    let side = side_of(face)?;
    // `glueAllowedOnSide`: a radial chassis may be glued on any side.
    let allowed = |lv: &ServerLevel<'_>, f: [i32; 3]| {
        radial
            || lv
                .name_props(add(pos, f))
                .is_none_or(|(n, p)| n != name || prop(&p, "axis") != Some(axis.as_str()))
    };
    let glued = flag(&props, side);
    let set = |props: &mut Vec<(String, String)>, k: &str, v: &str| match props
        .iter_mut()
        .find(|(key, _)| key == k)
    {
        Some(e) => e.1 = v.to_owned(),
        None => props.push((k.to_owned(), v.to_owned())),
    };
    if slime && glued {
        for d in Dir::ALL {
            let f = d.offset();
            if let Some(other) = side_of(f)
                && allowed(lv, f)
            {
                set(&mut props, other, "true");
            }
        }
    } else if (slime || (c.sneaking && c.held.is_none())) && glued != slime && allowed(lv, face) {
        set(&mut props, side, if slime { "true" } else { "false" });
    } else {
        return None;
    }
    let p: Vec<(&str, &str)> = props
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Some(new) = state_with(&*lv.host, &name, &p)
        && Some(new) != lv.state_num(pos)
    {
        lv.set_num(pos, new, update_all());
    }
    Some(InteractionResult::Success)
}

// ---------------------------------------------------------------------------------------------
// Windmills and mechanical bearings

struct WindmillPart;

#[mod_api::horizon_part]
impl BlockPart for WindmillPart {
    /// `WindmillBearingBlock.useItemOn` with an empty hand, not sneaking: assembled sails come
    /// apart now; otherwise they assemble on the next tick (`assembleNextTick`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = clicker(&*host, event.click);
        if c.sneaking {
            return Some(InteractionResult::Fail);
        }
        if c.held.is_some() || c.after_empty_main {
            return None;
        }
        let pos = event.click.pos;
        in_click_level(host, event.click, |lv| {
            let dim = lv.dim;
            let running = with_world(|w| w.dims[dim].windmills.contains_key(&pos)) == Some(true);
            if running {
                let state = lv.block(pos);
                if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten()
                {
                    put_back(lv, pos, &w);
                }
            } else {
                with_world(|w| {
                    if !w.dims[dim].windmill_assemble.contains(&pos) {
                        w.dims[dim].windmill_assemble.push(pos);
                    }
                });
            }
        });
        Some(InteractionResult::Success)
    }
}

struct MechanicalBearingPart;

#[mod_api::horizon_part]
impl BlockPart for MechanicalBearingPart {
    /// `MechanicalBearingBlock.useItemOn` with an empty hand, not sneaking, for a player who
    /// may build: a turning structure comes apart now (`disassemble`, turned to the nearest
    /// quarter), otherwise it assembles on the next tick when the bearing turns
    /// (`assembleNextTick`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = clicker(&*host, event.click);
        if c.player.is_none_or(|p| p.may_build(&*host) != Some(true)) || c.sneaking {
            return Some(InteractionResult::Fail);
        }
        if c.held.is_some() || c.after_empty_main {
            return None;
        }
        let pos = event.click.pos;
        in_click_level(host, event.click, |lv| {
            let dim = lv.dim;
            let now = lv.game_time();
            let turn = with_world(|w| w.dims[dim].windmills.get(&pos).map(|m| m.bearing)).flatten();
            match turn {
                Some(Some(turn)) => {
                    let Some(facing) = facing_of(lv, pos, Block::MechanicalBearing) else {
                        return;
                    };
                    if let Some((mut c, _)) =
                        BEARING_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos)))
                    {
                        actors::cancel_actors(lv, pos, &mut c);
                    }
                    let quarter = contraption::quarter(turn.angle_at(now));
                    let state = lv.block(pos);
                    if let Some(w) =
                        with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten()
                    {
                        put_back_turned(lv, pos, facing, quarter, &w);
                    }
                }
                Some(None) => {}
                None => {
                    with_world(|w| w.dims[dim].bearing_checks.push(pos));
                }
            }
        });
        Some(InteractionResult::Success)
    }
}

/// `ControlledContraptionEntity.disassemble` at angle 0 (a windmill turns back first): each
/// block where it was, or dropped where something else took its place.
fn put_back(lv: &mut ServerLevel<'_>, bearing: [i32; 3], w: &create_engine::world::Windmill) {
    let blocks: Vec<([i32; 3], u32)> = w
        .blocks
        .iter()
        .map(|(o, s)| (add(bearing, *o), *s))
        .collect();
    actors::expect_back(lv, bearing, |o| add(bearing, o));
    put_back_blocks(lv, &blocks, |lv, p| lv.is_free(p));
    actors::release_storage(lv, bearing, |o| add(bearing, o));
    actors::release_extras(lv, &mut w.extras.clone(), |o| add(bearing, o));
}

/// The facing of the Create block `block` at `pos`.
fn facing_of(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block) -> Option<Dir> {
    super::create_info(lv.block(pos))
        .filter(|(b, _)| *b == block)
        .and_then(|(_, i)| i.facing)
}

/// Whether a stopped mechanical bearing at `angle` puts its structure back, by its movement
/// mode (`RotationMode`): always (`ROTATE_PLACE`), near its initial angle
/// (`ROTATE_PLACE_RETURNED`, `isNearInitialAngle`), or only when the bearing goes
/// (`ROTATE_NEVER_PLACE`).
fn may_place(dim: usize, pos: [i32; 3], angle: f32) -> bool {
    let a = angle.rem_euclid(360.0);
    match super::option_at(dim, pos) {
        0 => true,
        1 => !(22.5..=360.0 - 22.5).contains(&a),
        _ => false,
    }
}

fn speed(dim: usize, pos: [i32; 3]) -> f32 {
    with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
}

/// The key of a bearing's or piston's last assembly's failure in its block entity's data
/// (`AssemblyException.write`'s `LastException`), for its clients' overlay
/// (`IDisplayAssemblyExceptions`): the message's key, its arguments and, for an unmovable block,
/// the block's id.
pub const LAST_EXCEPTION: &str = "LastException";

/// `lastException`: what the last assembly of the bearing or piston at `pos` refused, or none
/// for one that worked; nothing changes for a structure that wasn't there to assemble
/// (`assemble` returning false).
pub(super) fn set_exception(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    refused: Option<&contraption::Refused>,
) {
    use contraption::Refused;
    let c = match refused {
        Some(Refused::Empty) => return,
        Some(r) => {
            let (key, args, block) = match r {
                Refused::Unmovable(p) => (
                    "unmovableBlock",
                    p.iter().map(i32::to_string).collect(),
                    lv.name_props(*p).map(|(n, _)| n),
                ),
                Refused::TooLarge => (
                    "structureTooLarge",
                    vec![contraption::MAX_BLOCKS.to_string()],
                    None,
                ),
                Refused::NotEnoughSails(n) => (
                    "not_enough_sails",
                    vec![n.to_string(), contraption::MIN_SAILS.to_string()],
                    None,
                ),
                Refused::Empty => return,
            };
            let mut c = Compound::new();
            c.insert("Key".into(), Tag::String(key.into()));
            c.insert(
                "Args".into(),
                Tag::List(
                    args.into_iter()
                        .map(|a: String| Tag::String(a.as_str().into()))
                        .collect(),
                ),
            );
            if let Some(b) = block {
                c.insert("Block".into(), Tag::String(b.as_str().into()));
            }
            Some(c)
        }
        None => None,
    };
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let Some(mut data) = entity.data(&*lv.host) else {
        return;
    };
    let changed = match c {
        Some(c) => {
            let tag = Tag::Compound(c);
            data.insert(LAST_EXCEPTION.into(), tag.clone()) != Some(tag)
        }
        None => data.remove(&LAST_EXCEPTION.into()).is_some(),
    };
    if changed {
        let _ = entity.replace_data(&mut *lv.host, data);
    }
}

/// `sequenceContext.getEffectiveValue` of the turn (`distance`: by metres, else by degrees) a
/// sequenced gearshift or valve handle hands the block at `pos`, as its speed changes.
fn sequence_limit(dim: usize, pos: [i32; 3], distance: bool) -> Option<f32> {
    with_world(|w| w.dims[dim].kinetics.sequence_limit(pos, 0, distance)).flatten()
}

/// Windmills: structures whose bearing broke go back; right-clicked bearings assemble their
/// sails (`assemble`), the blocks leaving the world; clients see the changes. Nothing runs
/// without either.
pub(super) fn tick_windmills(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((orphans, queued)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.windmill_orphans),
            std::mem::take(&mut d.windmill_assemble),
        )
    }) else {
        return;
    };
    for (pos, w) in orphans {
        // A mechanical bearing broken mid-turn leaves its structure where it had turned to
        // (`remove` → `disassemble`: the entity keeps its angle); its first block is the one in
        // front of it.
        let front = w.blocks.first().and_then(|(o, _)| Dir::from_offset(*o));
        match (w.bearing, front) {
            (Some(turn), Some(facing)) => {
                let quarter = contraption::quarter(turn.angle_at(lv.game_time()));
                put_back_turned(lv, pos, facing, quarter, &w);
            }
            _ => put_back(lv, pos, &w),
        }
    }
    for pos in queued {
        let state = lv.block(pos);
        let Some(facing) = facing_of(lv, pos, Block::WindmillBearing) else {
            continue;
        };
        let gathered = contraption::gather_windmill(pos, facing, |p| found(lv, p));
        let (blocks, sails) = match gathered {
            Ok(g) => g,
            Err(r) => {
                set_exception(lv, pos, Some(&r));
                continue;
            }
        };
        set_exception(lv, pos, None);
        crate::glue::advancements::windmill_assembled(lv, pos, &blocks, sails);
        actors::carry_storage(lv, pos, pos, &blocks);
        let extras = actors::carry_extras(lv, pos, &blocks, |p| p == pos);
        take_blocks(lv, pos, &blocks);
        // `RotationDirection` on its value box: clockwise unless set otherwise.
        let w = create_engine::world::Windmill {
            blocks,
            sails,
            clockwise: super::option_at(dim, pos) == 0,
            bearing: None,
            extras,
        };
        with_world(|world| world.windmill_assembled(dim, pos, state, w));
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].windmill_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    for pos in changed {
        show_windmill(lv, pos);
    }
}

/// The angle a windmill with actors has turned to, and the speed it turns at; None for others.
fn windmill_turn(dim: usize, pos: [i32; 3]) -> Option<Turn> {
    BEARING_CARRIED.with_borrow(|c| c.get(&(dim, pos)).and_then(|c| c.1))
}

/// A bearing's structure to its block entity, for its clients: its blocks turning about the
/// bearing's axis from its angle now, at the speed it turns (a mechanical bearing's: none while
/// an actor stalls it; a windmill with no actors turns at its block's own speed). None when it
/// came apart.
fn show_windmill(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let w = with_world(|w| w.dims[dim].windmills.get(&pos).cloned()).flatten();
    let Some(w) = w else {
        draw::hide(lv, pos);
        return;
    };
    let facing = super::create_info(lv.block(pos))
        .and_then(|(_, i)| i.facing)
        .unwrap_or(Dir::Up);
    let turn = w.bearing.or_else(|| windmill_turn(dim, pos));
    draw::show_turn(lv, pos, &w.blocks, facing.axis(), turn);
}

/// `tickActors` for turning bearings and windmills with actors: each works where the structure
/// has turned to; while one stalls the structure holds its angle, and turns on at its speed
/// after (`isStalled`). A structure's actors are found once, the first tick it turns (also after
/// a load). Nothing runs without a turning structure.
pub(super) fn tick_bearing_actors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let structures = with_world(|w| w.dims[dim].windmills.len()).unwrap_or(0);
    if structures > 0 || BEARINGS_LOOKED.with_borrow(|l| !l.is_empty()) {
        let fresh: Vec<([i32; 3], Blocks, bool, Carried)> = with_world(|w| {
            let mut fresh = Vec::new();
            BEARINGS_LOOKED.with_borrow_mut(|l| {
                let d = &w.dims[dim];
                // One taken apart is looked at again when it is put together again.
                l.retain(|k| k.0 != dim || d.windmills.contains_key(&k.1));
                for (p, m) in &d.windmills {
                    if l.insert((dim, *p)) {
                        fresh.push((*p, m.blocks.clone(), m.bearing.is_none(), m.extras.clone()));
                    }
                }
            });
            fresh
        })
        .unwrap_or_default();
        for (pos, blocks, windmill, extras) in fresh {
            let carried = actors::with_extras(actors::bearing_actors(&blocks), &extras);
            if carried.actors.is_empty() && carried.controls.is_empty() {
                continue;
            }
            let own = windmill.then(|| Turn {
                angle: 0.0,
                since: now,
                rpm: speed(dim, pos),
                limit: None,
            });
            BEARING_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), (carried, own)));
            with_world(|w| w.dims[dim].windmill_changes.push(pos));
        }
    }
    let keys: Vec<[i32; 3]> =
        BEARING_CARRIED.with_borrow(|c| c.keys().filter(|k| k.0 == dim).map(|k| k.1).collect());
    for pos in keys {
        let facing = super::create_info(lv.block(pos)).and_then(|(_, i)| i.facing);
        let Some((mut carried, own)) = BEARING_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos)))
        else {
            continue;
        };
        let turn = match own {
            Some(t) => (with_world(|w| w.dims[dim].windmills.contains_key(&pos)) == Some(true))
                .then_some(t),
            None => {
                with_world(|w| w.dims[dim].windmills.get(&pos).and_then(|m| m.bearing)).flatten()
            }
        };
        let (Some(facing), Some(mut turn)) = (facing, turn) else {
            actors::cancel_actors(lv, pos, &mut carried);
            continue;
        };
        let rpm = speed(dim, pos);
        // A windmill's speed changed (its sails, or the network's): on from here at it.
        if own.is_some() && !carried.stalled && turn.rpm != rpm {
            turn.rebase(now, rpm);
            with_world(|w| w.dims[dim].windmill_changes.push(pos));
        }
        let deg = f64::from(turn.angle_at(now));
        let was = carried.stalled;
        let stalled = actors::tick_turning_actors(lv, pos, &mut carried, pos, facing.axis(), deg);
        carried.stalled = stalled;
        super::actor_draw::show_actors(lv, pos, pos, &carried);
        if stalled != was {
            let rpm = if stalled { 0.0 } else { rpm };
            if own.is_some() {
                turn.rebase(now, rpm);
            }
            with_world(|w| {
                if own.is_none()
                    && let Some(m) = w.dims[dim].windmills.get_mut(&pos)
                    && let Some(t) = m.bearing.as_mut()
                {
                    t.rebase(now, rpm);
                }
                w.dims[dim].windmill_changes.push(pos);
            });
        }
        BEARING_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), (carried, own.map(|_| turn))));
    }
}

/// What the turning structure of the bearing or windmill at `pos` carries (its actors, controls,
/// the kinds turned off), for a click on its contraption controls, and where each of its blocks
/// is in the world now (`toGlobalVector` of its centre); None for no turning structure.
pub(super) fn take_turning(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<(Carried, actors::Place)> {
    let dim = lv.dim;
    let now = lv.game_time();
    let w = with_world(|w| w.dims[dim].windmills.get(&pos).cloned()).flatten()?;
    let facing = super::create_info(lv.block(pos)).and_then(|(_, i)| i.facing)?;
    let carried = BEARING_CARRIED
        .with_borrow(|c| c.get(&(dim, pos)).map(|c| c.0.clone()))
        .unwrap_or_else(|| actors::with_extras(actors::bearing_actors(&w.blocks), &w.extras));
    let angle = w
        .bearing
        .or_else(|| windmill_turn(dim, pos))
        .map_or(0.0, |t| f64::from(t.angle_at(now)));
    let axis = facing.axis();
    let place = move |o: [i32; 3]| {
        let v = create_engine::actors::turn_vec(o.map(f64::from), axis, angle);
        [0, 1, 2].map(|i| (f64::from(pos[i]) + 0.5 + v[i]).floor() as i32)
    };
    Some((carried, Box::new(place)))
}

/// What [`take_turning`] took, changed by its controls, goes back: to its actors while it
/// turns, and to the structure, which saves it and puts it back with its blocks.
pub(super) fn put_turning(lv: &mut ServerLevel<'_>, pos: [i32; 3], carried: Carried) {
    let dim = lv.dim;
    with_world(|w| {
        if let Some(m) = w.dims[dim].windmills.get_mut(&pos) {
            m.extras.controls.clone_from(&carried.controls);
            m.extras.disabled.clone_from(&carried.disabled);
            w.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
    });
    BEARING_CARRIED.with_borrow_mut(|c| {
        if let Some(e) = c.get_mut(&(dim, pos)) {
            e.0 = carried;
        }
    });
}

/// Mechanical bearings whose speed changed (`tick` after `onSpeedChanged`): a still one starts
/// turning what is in front of it (`assemble`); a turning one stopped puts it back turned to the
/// nearest quarter turn (`disassemble`, `ROTATE_PLACE`); otherwise the angle runs on at the new
/// speed. Nothing runs without a change.
pub(super) fn tick_bearings(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some(mut checks) = with_world(|w| std::mem::take(&mut w.dims[dim].bearing_checks)) else {
        return;
    };
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let now = lv.game_time();
    for pos in checks {
        let state = lv.block(pos);
        let Some(facing) = facing_of(lv, pos, Block::MechanicalBearing) else {
            continue;
        };
        // One loaded turning has no speed until its network's first solve, which looks at it
        // again: Create's block entity loads with its speed, and goes on turning.
        if super::unsolved(dim, pos) {
            continue;
        }
        let rpm = speed(dim, pos);
        let running = with_world(|w| w.dims[dim].windmills.get(&pos).map(|m| m.bearing)).flatten();
        match running {
            Some(Some(turn)) if rpm == 0.0 && may_place(dim, pos, turn.angle_at(now)) => {
                if let Some((mut c, _)) = BEARING_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos)))
                {
                    actors::cancel_actors(lv, pos, &mut c);
                }
                let quarter = contraption::quarter(turn.angle_at(now));
                if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten()
                {
                    put_back_turned(lv, pos, facing, quarter, &w);
                }
            }
            Some(Some(mut turn)) => {
                // A stalled structure stays still at any speed until its actor is done.
                let stalled = BEARING_CARRIED
                    .with_borrow(|c| c.get(&(dim, pos)).is_some_and(|c| c.0.stalled));
                turn.rebase(now, if stalled { 0.0 } else { rpm });
                // `onSpeedChanged`: the limit starts again from the context it has now (not
                // for the speed it is loaded with).
                if !super::keeps_loaded_limit(dim, pos) {
                    turn.limit = sequence_limit(dim, pos, false);
                }
                with_world(|w| {
                    if let Some(m) = w.dims[dim].windmills.get_mut(&pos) {
                        m.bearing = Some(turn);
                    }
                    w.dims[dim].windmill_changes.push(pos);
                });
            }
            Some(None) => {}
            None if rpm != 0.0 => {
                let gathered = contraption::gather_bearing(pos, facing, 0, |p| found(lv, p));
                let (blocks, sails) = match gathered {
                    Ok(g) => g,
                    Err(r) => {
                        set_exception(lv, pos, Some(&r));
                        continue;
                    }
                };
                set_exception(lv, pos, None);
                crate::glue::advancements::assembled(lv, pos, &blocks);
                actors::carry_storage(lv, pos, pos, &blocks);
                let extras = actors::carry_extras(lv, pos, &blocks, |p| p == pos);
                take_blocks(lv, pos, &blocks);
                let w = create_engine::world::Windmill {
                    extras,
                    blocks,
                    sails,
                    clockwise: true,
                    bearing: Some(Turn {
                        angle: 0.0,
                        since: now,
                        rpm,
                        limit: sequence_limit(dim, pos, false),
                    }),
                };
                with_world(|world| world.bearing_assembled(dim, pos, w));
            }
            None => {}
        }
    }
}

/// `ControlledContraptionEntity.disassemble` with its `StructureTransform`: each block turned
/// `quarter`° about the bearing's axis through the block in front of it ([`turned_state`]). A
/// block whose place is taken drops.
fn put_back_turned(
    lv: &mut ServerLevel<'_>,
    bearing: [i32; 3],
    facing: Dir,
    quarter: i32,
    w: &create_engine::world::Windmill,
) {
    let axis = facing.axis();
    let f = facing.offset();
    let anchor = add(bearing, f);
    let turned: Vec<([i32; 3], u32)> = w
        .blocks
        .iter()
        .map(|(o, state)| {
            let local = [o[0] - f[0], o[1] - f[1], o[2] - f[2]];
            let r = contraption::rotate(local, quarter, axis);
            (
                add(anchor, r),
                turned_state(&*lv.host, *state, quarter, axis),
            )
        })
        .collect();
    let turn = |o: [i32; 3]| {
        let local = [o[0] - f[0], o[1] - f[1], o[2] - f[2]];
        add(anchor, contraption::rotate(local, quarter, axis))
    };
    actors::expect_back(lv, bearing, turn);
    put_back_blocks(lv, &turned, |lv, p| lv.is_free(p));
    actors::release_storage(lv, bearing, turn);
    actors::release_extras(lv, &mut w.extras.clone(), turn);
}

// ---------------------------------------------------------------------------------------------
// Gantries

/// A gantry shaft's facing, part and power; None for any other block.
fn gantry_shaft(
    lv: &ServerLevel<'_>,
    p: [i32; 3],
) -> Option<(Dir, create_engine::gantry::Part, bool)> {
    let (name, props) = lv.name_props(p)?;
    if name != "create:gantry_shaft" {
        return None;
    }
    let facing = prop(&props, "facing").and_then(dir_of)?;
    let part = create_engine::gantry::Part::read(prop(&props, "part")?)?;
    Some((facing, part, flag(&props, "powered")))
}

/// `checkAttachedCarriageBlocks`: carriages beside the shaft facing away from it, off its axis,
/// try to start on the next tick.
pub(super) fn gantry_carriages_on(lv: &ServerLevel<'_>, shaft: [i32; 3]) {
    let Some((facing, ..)) = gantry_shaft(lv, shaft) else {
        return;
    };
    for d in Dir::ALL {
        if d.axis() == facing.axis() {
            continue;
        }
        let c = add(shaft, d.offset());
        let carriage = lv.name_props(c).is_some_and(|(n, p)| {
            n == "create:gantry_carriage" && prop(&p, "facing") == Some(dir_name(d))
        });
        if carriage {
            with_world(|w| w.dims[lv.dim].gantry_assemble.push(c));
        }
    }
}

/// Gantries: shafts take their place in their line (`updateShape`); queued carriages on a shaft
/// they may start from take their structure out of the world (`tryAssemble`); moving ones run
/// along the shafts at their speed (`checkPinionShaft`) and go back into the world, at the
/// nearest whole block, where they stop. Nothing runs without one of them.
pub(super) fn tick_gantries(lv: &mut ServerLevel<'_>) {
    use create_engine::gantry::{self, Moving, Shaft, Step};
    let dim = lv.dim;
    let Some((mut checks, mut queued, moving)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.gantry_checks),
            std::mem::take(&mut d.gantry_assemble),
            !d.gantries.is_empty(),
        )
    }) else {
        return;
    };
    if checks.is_empty() && queued.is_empty() && !moving {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((facing, part, _)) = gantry_shaft(lv, pos) else {
            continue;
        };
        let along =
            |d: Dir| gantry_shaft(lv, add(pos, d.offset())).is_some_and(|(f, ..)| f == facing);
        let now = gantry::Part::of(along(facing), along(facing.opposite()));
        if now != part {
            lv.set_property(pos, "part", now.name(), update_all());
        }
    }
    queued.sort();
    queued.dedup();
    for c in queued {
        let Some(cf) = lv
            .name_props(c)
            .filter(|(n, _)| n == "create:gantry_carriage")
            .and_then(|(_, p)| prop(&p, "facing").and_then(dir_of))
        else {
            continue;
        };
        let shaft = add(c, cf.opposite().offset());
        let Some((sf, part, powered)) = gantry_shaft(lv, shaft) else {
            continue;
        };
        let v = gantry::pinion_speed(speed(dim, shaft));
        if !gantry::can_assemble_on(part, powered, v) {
            continue;
        }
        let moving_dir = if v > 0.0 { sf } else { sf.opposite() };
        let gathered = contraption::gather_gantry(c, cf, moving_dir, |p| found(lv, p));
        let Ok(blocks) = gathered else {
            continue;
        };
        let carried = actors::carry(lv, c, c, &blocks, |p| p == shaft);
        crate::glue::advancements::assembled(lv, c, &blocks);
        take_blocks(lv, c, &blocks);
        let limit = sequence_limit(dim, shaft, true);
        with_world(|w| {
            w.dims[dim].gantries.push(Moving {
                start: c,
                carriage_facing: cf,
                shaft_facing: sf,
                offset: 0.0,
                per_tick: 0.0,
                blocks,
                carried,
                limit,
            })
        });
    }
    let list = with_world(|w| std::mem::take(&mut w.dims[dim].gantries)).unwrap_or_default();
    let mut keep = Vec::new();
    for mut g in list {
        // Its actors first; while one stalls, the gantry holds (`isStalled`).
        if !g.carried.actors.is_empty() {
            let o = g.shaft_facing.offset();
            let at =
                [0, 1, 2].map(|a| f64::from(g.start[a]) + f64::from(o[a]) * f64::from(g.offset));
            let stalled = actors::tick_actors(lv, g.start, &mut g.carried, at);
            g.carried.stalled = stalled;
            let shaft = add(g.start, g.carriage_facing.opposite().offset());
            super::actor_draw::show_actors(lv, shaft, g.start, &g.carried);
            if stalled {
                if g.per_tick != 0.0 {
                    g.per_tick = 0.0;
                    show_gantry(lv, &g, false);
                }
                keep.push(g);
                continue;
            }
        }
        let sp = g.shaft_pos();
        // Loaded moving, it goes on once its shaft has a speed.
        if super::unsolved(dim, sp) {
            keep.push(g);
            continue;
        }
        let shaft = gantry_shaft(lv, sp).map(|(facing, part, powered)| Shaft {
            facing,
            part,
            powered,
            speed: speed(dim, sp),
        });
        // `ContraptionCollider.collideBlocks`: run into the world, it comes apart.
        let step = match g.step(shaft) {
            Step::Move(v) if gantry_collides(lv, &g, v) => Step::Stop,
            step => step,
        };
        match step {
            Step::Move(v) => {
                if v != g.per_tick {
                    g.per_tick = v;
                    show_gantry(lv, &g, false);
                }
                g.moved(v);
                // Saved with the column it started from as it starts, now and then while it
                // moves, and as it stops (`gantries`).
                if v != 0.0 && (g.offset == v || lv.game_time() % 10 == 0) {
                    super::super::persist::mark_pos_unsaved(dim, g.start);
                }
                keep.push(g);
            }
            Step::Stop => {
                // `disassemble` at `BlockPos.containing(anchor + .5)`.
                let at = g.carriage();
                let shift = [at[0] - g.start[0], at[1] - g.start[1], at[2] - g.start[2]];
                let blocks: Vec<([i32; 3], u32)> = g
                    .blocks
                    .iter()
                    .map(|(o, id)| (add(add(g.start, *o), shift), *id))
                    .collect();
                actors::expect_back(lv, g.start, |o| add(at, o));
                put_back_blocks(lv, &blocks, |lv, p| lv.is_free(p));
                actors::release(lv, g.start, at, &mut g.carried);
                show_gantry(lv, &g, true);
                super::super::persist::mark_pos_unsaved(dim, g.start);
            }
        }
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        keep.extend(std::mem::take(&mut d.gantries));
        d.gantries = keep;
    });
}

/// Whether gantry `g`, moving `v` a tick along its shaft, meets the world where it is
/// (`isCollidingWithWorld`, as a piston's structure does).
fn gantry_collides(lv: &ServerLevel<'_>, g: &create_engine::gantry::Moving, v: f32) -> bool {
    use create_engine::piston::{colliders, entered_cells};
    let dir = if v > 0.0 {
        g.shaft_facing
    } else {
        g.shaft_facing.opposite()
    };
    let leading = colliders(&g.blocks, dir);
    entered_cells(g.start, g.shaft_facing, g.offset, dir, &leading)
        .into_iter()
        .any(|(cell, local)| {
            !lv.moved_through(cell) && !actors::breaker_clears(lv, g.start, &g.carried, local, cell)
        })
}

/// A moving gantry to its clients, through the shaft its carriage started on: gliding along the
/// shaft line from its offset at its speed; `gone` clears it.
fn show_gantry(lv: &mut ServerLevel<'_>, g: &create_engine::gantry::Moving, gone: bool) {
    let shaft = add(g.start, g.carriage_facing.opposite().offset());
    if gone {
        draw::hide(lv, shaft);
        return;
    }
    let origin = g.carriage_facing.offset();
    draw::show_glide(
        lv,
        shaft,
        draw::Glide {
            key: g.start,
            origin,
            blocks: &g.blocks,
            dir: g.shaft_facing.offset(),
            offset: g.offset,
            per_tick: g.per_tick,
            clamp: None,
            rope: false,
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Mechanical pistons

/// A block's world position on a piston's structure at `offset` blocks out.
fn piston_world(anchor: [i32; 3], local: [i32; 3], facing: Dir, offset: i32) -> [i32; 3] {
    let o = facing.offset();
    [0, 1, 2].map(|a| anchor[a] + local[a] + o[a] * offset)
}

/// Mechanical pistons (`LinearActuatorBlockEntity.tick`): a speed change makes a still piston
/// assemble or a moving one stop (`assembleNextTick`); moving ones slide `getMovementSpeed`
/// a tick and are put back at their end (`tryDisassemble`). Clients glide the structure.
pub(super) fn tick_pistons(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let orphans =
        with_world(|w| std::mem::take(&mut w.dims[dim].piston_orphans)).unwrap_or_default();
    for (pos, m) in orphans {
        put_back_piston(lv, pos, m, false);
        draw::hide(lv, pos);
    }
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].piston_checks)).unwrap_or_default();
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].pistons.keys().copied().collect()).unwrap_or_default();
    if checks.is_empty() && moving.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let mut later = Vec::new();
    for &pos in &checks {
        // Loaded moving, it goes on once its network has a speed.
        if super::unsolved(dim, pos) {
            later.push(pos);
            continue;
        }
        let rpm = speed(dim, pos);
        // `onSpeedChanged`: a piston kept together by its mode moves again.
        let running = with_world(|w| {
            let limit = w.dims[dim].kinetics.sequence_limit(pos, 0, true);
            let m = w.dims[dim].pistons.get_mut(&pos)?;
            m.waiting = false;
            // The limit starts again from the context it has now (not for the speed it is
            // loaded with).
            if !super::keeps_loaded_limit(dim, pos) {
                m.limit = limit;
            }
            Some(())
        })
        .flatten()
        .is_some();
        let now = lv.game_time();
        let unchanged = PISTONS_STOPPED.with_borrow_mut(|m| {
            let same = m
                .get(&(dim, pos))
                .is_some_and(|&(r, t)| r == rpm && now - t <= 3);
            if !same {
                m.remove(&(dim, pos));
            }
            same && !running
        });
        match (running, rpm != 0.0) {
            (false, true) if unchanged => {}
            (false, true) => assemble_piston(lv, pos, rpm),
            (true, false) => try_disassemble_piston(lv, pos),
            (true, true) => show_piston(lv, pos),
            _ => {}
        }
    }
    if !later.is_empty() {
        with_world(|w| w.dims[dim].piston_checks.extend(later));
    }
    for pos in moving {
        // One whose speed changed this tick only answers it (`assembleNextTick` returns).
        if checks.contains(&pos) || super::unsolved(dim, pos) {
            continue;
        }
        let waiting = with_world(|w| w.dims[dim].pistons.get(&pos).map(|m| m.waiting)).flatten();
        if waiting != Some(false) {
            continue;
        }
        let rpm = speed(dim, pos);
        let Some(facing) = super::create_info(lv.block(pos)).and_then(|(_, i)| i.facing) else {
            continue;
        };
        // Its actors first (the contraption entity ticks before the piston's block entity);
        // while one stalls, the piston holds (`isStalled`).
        let stalled = tick_piston_actors(lv, pos, facing);
        // `moveAndCollideContraption`: moved into the world, it stops at the whole offset it
        // came from (`collided`).
        if !stalled && piston_collides(lv, pos, facing, rpm) {
            with_world(|w| {
                if let Some(m) = w.dims[dim].pistons.get_mut(&pos) {
                    use create_engine::piston::grid_offset;
                    let v = m.step(rpm, facing);
                    m.offset = grid_offset(grid_offset(m.offset, m.range) - v, m.range);
                }
            });
            try_disassemble_piston(lv, pos);
            continue;
        }
        let mut locked = false;
        let end = with_world(|w| {
            let m = w.dims[dim].pistons.get_mut(&pos)?;
            let v = m.step(rpm, facing);
            // `LinearActuatorBlockEntity.tick`: a sequenced limit is used up as it moves; at
            // its end the structure is set where it is (`forceMove`).
            if let Some(l) = m.limit.filter(|l| *l > 0.0) {
                let left = (l - v.abs()).max(0.0);
                m.limit = Some(left);
                locked = left == 0.0;
            }
            if !stalled {
                m.offset += v;
            }
            let end = m.offset <= 0.0 || m.offset >= m.range as f32;
            if end {
                m.offset = m.offset.clamp(0.0, m.range as f32);
            }
            Some(end)
        })
        .flatten();
        if locked {
            show_piston(lv, pos);
        }
        if end == Some(true) {
            try_disassemble_piston(lv, pos);
        } else if lv.game_time() % 10 == 0 {
            // Its offset is saved now and then while it moves (and whenever it stops).
            super::super::persist::mark_pos_unsaved(dim, pos);
        }
    }
}

/// Whether piston `pos`'s structure, moved on a tick at `rpm`, meets the world
/// (`ContraptionCollider.isCollidingWithWorld`): a cell its leading blocks enter holds a block
/// with a collision shape that can't be replaced, and that no breaker there can break.
fn piston_collides(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir, rpm: f32) -> bool {
    use create_engine::piston::{colliders, entered_cells};
    let dim = lv.dim;
    let Some(m) = with_world(|w| w.dims[dim].pistons.get(&pos).cloned()).flatten() else {
        return false;
    };
    let v = m.step(rpm, facing);
    if v == 0.0 {
        return false;
    }
    let dir = if v > 0.0 { facing } else { facing.opposite() };
    let to = m.offset + v;
    let leading = colliders(&m.blocks, dir);
    entered_cells(m.anchor, facing, to, dir, &leading)
        .into_iter()
        .any(|(cell, local)| {
            !lv.moved_through(cell) && !actors::breaker_clears(lv, pos, &m.carried, local, cell)
        })
}

/// `LinearActuatorBlockEntity.tryDisassemble`: by its movement mode, a stopped piston puts its
/// structure back (`MOVE_PLACE`, or `MOVE_PLACE_RETURNED` at its starting offset) or keeps it
/// together, still, until its speed changes (`waitingForSpeedChange`).
fn try_disassemble_piston(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let mode = super::option_at(dim, pos);
    let keep = with_world(|w| {
        let m = w.dims[dim].pistons.get_mut(&pos)?;
        let keep = mode == 2 || (mode == 1 && (m.offset + 0.5) as i32 != m.initial);
        m.waiting = keep;
        Some(keep)
    })
    .flatten();
    match keep {
        Some(true) => show_piston(lv, pos),
        Some(false) => disassemble_piston(lv, pos),
        None => {}
    }
    super::super::persist::mark_pos_unsaved(dim, pos);
}

/// A moving piston's actors at its offset now; clients hear when it stalls or moves on. Only
/// pistons carrying actors do anything.
fn tick_piston_actors(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let dim = lv.dim;
    let Some((mut carried, anchor, offset)) = with_world(|w| {
        let m = w.dims[dim].pistons.get_mut(&pos)?;
        if m.carried.actors.is_empty() {
            return None;
        }
        Some((std::mem::take(&mut m.carried), m.anchor, m.offset))
    })
    .flatten() else {
        return false;
    };
    let o = facing.offset();
    let at = [0, 1, 2].map(|a| f64::from(anchor[a]) + f64::from(o[a]) * f64::from(offset));
    let stalled = actors::tick_actors(lv, pos, &mut carried, at);
    let changed = stalled != carried.stalled;
    carried.stalled = stalled;
    super::actor_draw::show_actors(lv, pos, pos, &carried);
    let back = with_world(|w| match w.dims[dim].pistons.get_mut(&pos) {
        Some(m) => {
            m.carried = carried;
            true
        }
        None => false,
    })
    .unwrap_or(false);
    if back && changed {
        show_piston(lv, pos);
    }
    stalled
}

/// `MechanicalPistonBlockEntity.assemble`: the head, poles and structure leave the world.
fn assemble_piston(lv: &mut ServerLevel<'_>, pos: [i32; 3], rpm: f32) {
    use create_engine::piston::{self, PoleCell};
    let dim = lv.dim;
    let Some((block, Some(facing))) = super::create_info(lv.block(pos)).map(|(b, i)| (b, i.facing))
    else {
        return;
    };
    let extended = lv
        .name_props(pos)
        .is_some_and(|(_, p)| prop(&p, "state") == Some("extended"));
    let cell = |p: [i32; 3]| match super::create_info(lv.block(p)).map(|(b, i)| (b, i.facing)) {
        Some((Block::PistonExtensionPole, Some(f))) => PoleCell::Pole(f.axis()),
        Some((Block::MechanicalPistonHead, Some(f))) => PoleCell::Head(f),
        _ => PoleCell::Other,
    };
    let Some(c) = piston::collect(pos, facing, extended, cell) else {
        return;
    };
    let m = piston::movement(rpm, facing, c.progress as f32, c.range);
    let result = c.progress as f32 + m.signum() * 0.5;
    if m == 0.0 || result <= 0.0 || result >= c.range as f32 {
        return;
    }
    // The poles' and head's own states, then the structure in front (searched from the anchor,
    // pushed the way it moves).
    let sticky = block == Block::StickyMechanicalPiston;
    let head = state_with(
        &*lv.host,
        "create:mechanical_piston_head",
        &[
            ("facing", dir_name(facing)),
            ("type", if sticky { "sticky" } else { "normal" }),
        ],
    );
    let pole = state_with(
        &*lv.host,
        "create:piston_extension_pole",
        &[("facing", dir_name(facing))],
    );
    let (Some(head), Some(pole)) = (head, pole) else {
        return;
    };
    let pole_worlds: Vec<[i32; 3]> = c
        .poles
        .iter()
        .map(|(l, _)| piston_world(c.anchor, *l, facing, c.progress))
        .collect();
    let mut exclude = pole_worlds;
    exclude.push(pos);
    let gathered = piston::gather_piston(c.anchor, facing, m < 0.0, sticky, &exclude, |p| {
        found(lv, p)
    });
    let structure = match gathered {
        Ok(s) => s,
        Err(r) => {
            set_exception(lv, pos, Some(&r));
            return;
        }
    };
    set_exception(lv, pos, None);
    let o = facing.offset();
    let mut blocks: Vec<([i32; 3], u32)> = c
        .poles
        .iter()
        .map(|(l, is_head)| (*l, if *is_head { head } else { pole }))
        .collect();
    for (off, s) in &structure {
        blocks.push(([0, 1, 2].map(|a| off[a] - o[a] * c.progress), *s));
    }
    // What rides with it (actors, storage, glue; offsets as the blocks', from the anchor at
    // offset 0), then off the world: everything but the piston.
    let origin = piston_world(c.anchor, [0; 3], facing, c.progress);
    let local: Vec<([i32; 3], u32)> = blocks
        .iter()
        .filter(|(l, _)| piston_world(c.anchor, *l, facing, c.progress) != pos)
        .copied()
        .collect();
    let carried = actors::carry(lv, pos, origin, &local, |p| p == pos);
    let cells: Vec<([i32; 3], u32)> = blocks
        .iter()
        .map(|(l, s)| (piston_world(c.anchor, *l, facing, c.progress), *s))
        .filter(|(p, _)| *p != pos)
        .collect();
    super::remove_cells(lv, &cells);
    crate::glue::advancements::assembled(lv, pos, &blocks);
    lv.set_property(pos, "state", "moving", update_all());
    super::super::persist::mark_pos_unsaved(dim, pos);
    let limit = sequence_limit(dim, pos, true);
    with_world(|w| {
        w.dims[dim].pistons.insert(
            pos,
            create_engine::world::Moving {
                anchor: c.anchor,
                blocks,
                offset: c.progress as f32,
                range: c.range,
                carried,
                initial: c.progress,
                facing: Some(facing),
                waiting: false,
                limit,
            },
        )
    });
    show_piston(lv, pos);
}

/// `tryDisassemble`: the structure goes back into the world at the nearest whole offset; the
/// piston is extended unless all the way in.
fn disassemble_piston(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(m) = with_world(|w| w.dims[dim].pistons.remove(&pos)).flatten() else {
        return;
    };
    put_back_piston(lv, pos, m, true);
}

/// The structure of piston `pos` back in the world at its nearest whole offset, and the piston
/// (when `own`: still there) extended or retracted.
fn put_back_piston(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    mut m: create_engine::world::Moving,
    own: bool,
) {
    let Some(facing) = m.facing else {
        return;
    };
    let grid = m.offset.round() as i32;
    let cells: Vec<([i32; 3], u32)> = m
        .blocks
        .iter()
        .map(|(l, s)| (piston_world(m.anchor, *l, facing, grid), *s))
        .filter(|(p, _)| *p != pos)
        .collect();
    let o = facing.offset();
    let origin = [0, 1, 2].map(|a| m.anchor[a] + o[a] * grid);
    actors::expect_back(lv, pos, |l| add(origin, l));
    put_back_blocks(lv, &cells, |lv, p| lv.passable(p));
    if own {
        let rpm = speed(lv.dim, pos);
        let now = lv.game_time();
        PISTONS_STOPPED.with_borrow_mut(|s| s.insert((lv.dim, pos), (rpm, now)));
        let state = if grid > 0 { "extended" } else { "retracted" };
        lv.set_property(pos, "state", state, update_all());
    }
    actors::release(lv, pos, origin, &mut m.carried);
    draw::hide(lv, pos);
    super::super::persist::mark_pos_unsaved(lv.dim, pos);
}

/// A moving piston to its clients: its blocks (offset from the anchor at offset 0) gliding
/// along its facing from its offset now at the blocks a tick it moves (none while stalled,
/// `ContraptionStallPacket`).
fn show_piston(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(m) = with_world(|w| w.dims[dim].pistons.get(&pos).cloned()).flatten() else {
        return;
    };
    let Some(facing) = super::create_info(lv.block(pos)).and_then(|(_, i)| i.facing) else {
        return;
    };
    let per_tick = if m.carried.stalled || m.waiting {
        0.0
    } else {
        m.step(speed(dim, pos), facing)
    };
    let origin = [
        m.anchor[0] - pos[0],
        m.anchor[1] - pos[1],
        m.anchor[2] - pos[2],
    ];
    draw::show_glide(
        lv,
        pos,
        draw::Glide {
            key: pos,
            origin,
            blocks: &m.blocks,
            dir: facing.offset(),
            offset: m.offset,
            per_tick,
            clamp: m.limit.map(|l| (m.offset - l, m.offset + l)),
            rope: false,
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Saved with the column

/// Windmills and mechanical bearings (`windmills`), chassis ranges (`chassis`) and the options
/// picked on value boxes (`modes`).
pub(super) fn chunk_load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let dim = lv.dim;
    let windmills = windmills_from_nbt(lv, data);
    let ranges: Vec<([i32; 3], i32)> = match data.get(&"chassis".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| ([c[0], c[1], c[2]], c[3].clamp(1, 16)))
            .collect(),
        _ => Vec::new(),
    };
    let modes: Vec<([i32; 3], u8)> = match data.get(&"modes".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| ([c[0], c[1], c[2]], c[3].clamp(0, 2) as u8))
            .collect(),
        _ => Vec::new(),
    };
    let pistons = pistons_from_nbt(lv, data);
    let gantries = gantries_from_nbt(lv, data);
    with_world(|w| {
        let d = &mut w.dims[dim];
        for g in gantries {
            // A column loaded again while its gantry still runs keeps the running one.
            if !d.gantries.iter().any(|m| m.start == g.start) {
                d.gantries.push(g);
            }
        }
    });
    with_world(|w| {
        w.load_windmills(dim, windmills.into_iter());
        w.dims[dim].chassis_range.extend(ranges);
        w.dims[dim].scroll_options.extend(modes);
        for (p, m) in pistons {
            w.dims[dim].pistons.insert(p, m);
            // Clients see it again, and its speed tells it to move on or stop.
            w.dims[dim].piston_checks.push(p);
        }
    });
}

pub(super) fn chunk_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let dim = lv.dim;
    let in_chunk = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let Some((windmills, ranges, modes, pistons, gantries)) = with_world(|w| {
        let d = &w.dims[dim];
        let mut ranges: Vec<([i32; 3], i32)> = d
            .chassis_range
            .iter()
            .filter(|(p, _)| in_chunk(p))
            .map(|(p, r)| (*p, *r))
            .collect();
        ranges.sort();
        let mut modes: Vec<([i32; 3], i32)> = d
            .scroll_options
            .iter()
            .filter(|(p, _)| in_chunk(p))
            .map(|(p, m)| (*p, i32::from(*m)))
            .collect();
        modes.sort();
        let mut pistons: Vec<([i32; 3], create_engine::world::Moving)> = d
            .pistons
            .iter()
            .filter(|(p, _)| in_chunk(p))
            .map(|(p, m)| (*p, m.clone()))
            .collect();
        pistons.sort_by_key(|(p, _)| *p);
        let gantries: Vec<create_engine::gantry::Moving> = d
            .gantries
            .iter()
            .filter(|g| in_chunk(&g.start))
            .cloned()
            .collect();
        (
            w.column_windmills(dim, chunk[0], chunk[1]),
            ranges,
            modes,
            pistons,
            gantries,
        )
    }) else {
        return;
    };
    if !gantries.is_empty() {
        data.insert("gantries".into(), gantries_to_nbt(lv, &gantries));
    }
    if !pistons.is_empty() {
        data.insert("pistons".into(), pistons_to_nbt(lv, &pistons));
    }
    if !modes.is_empty() {
        let v = modes
            .iter()
            .flat_map(|(p, m)| [p[0], p[1], p[2], *m])
            .collect();
        data.insert("modes".into(), Tag::IntArray(v));
    }
    if !ranges.is_empty() {
        let v = ranges
            .iter()
            .flat_map(|(p, r)| [p[0], p[1], p[2], *r])
            .collect();
        data.insert("chassis".into(), Tag::IntArray(v));
    }
    if !windmills.is_empty() {
        data.insert("windmills".into(), windmills_to_nbt(lv, &windmills));
    }
}

fn windmills_to_nbt(
    lv: &ServerLevel<'_>,
    all: &[([i32; 3], create_engine::world::Windmill)],
) -> Tag {
    let now = lv.game_time();
    Tag::List(
        all.iter()
            .map(|(p, w)| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(p.to_vec()));
                if let Some(t) = actors::storage_to_nbt(lv, *p) {
                    c.insert("storage".into(), t);
                }
                actors::extras_to_nbt(&mut c, &w.extras);
                c.insert(
                    "blocks".into(),
                    Tag::IntArray(w.blocks.iter().flat_map(|(o, _)| *o).collect()),
                );
                c.insert(
                    "states".into(),
                    Tag::List(
                        w.blocks
                            .iter()
                            .map(|(_, s)| {
                                let text = state_text(&*lv.host, *s).unwrap_or_default();
                                Tag::String(text.as_str().into())
                            })
                            .collect(),
                    ),
                );
                // A mechanical bearing's `Angle` (now) and that it is one.
                let angle = w.bearing.map_or(0.0, |t| t.angle_at(now));
                // Its column is written when its turn changes, not each tick: the turn as it
                // follows from then (the angle, game time and speed it was set at, and its
                // sequenced limit, `SequencedAngleLimit`), so a load finds both where they are at
                // the save's game time.
                if let Some(t) = w.bearing {
                    let since = t.since.to_le_bytes();
                    let word = |i: usize| i32::from_le_bytes(since[i..i + 4].try_into().unwrap());
                    c.insert(
                        "turn".into(),
                        Tag::IntArray(vec![
                            t.angle.to_bits() as i32,
                            word(0),
                            word(4),
                            t.rpm.to_bits() as i32,
                        ]),
                    );
                    super::limit_to_nbt(&mut c, t.limit);
                }
                c.insert(
                    "sails".into(),
                    Tag::IntArray(vec![
                        w.sails as i32,
                        i32::from(w.clockwise),
                        angle.to_bits() as i32,
                        i32::from(w.bearing.is_some()),
                    ]),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

/// Blocks (offsets and states) as a save keeps them: `blocks` offsets, `states` texts.
fn structure_to_nbt(lv: &ServerLevel<'_>, c: &mut Compound, blocks: &[([i32; 3], u32)]) {
    c.insert(
        "blocks".into(),
        Tag::IntArray(blocks.iter().flat_map(|(o, _)| *o).collect()),
    );
    let states = blocks
        .iter()
        .map(|(_, s)| {
            Tag::String(
                state_text(&*lv.host, *s)
                    .unwrap_or_default()
                    .as_str()
                    .into(),
            )
        })
        .collect();
    c.insert("states".into(), Tag::List(states));
}

fn structure_from_nbt(lv: &ServerLevel<'_>, c: &Compound) -> Option<Vec<([i32; 3], u32)>> {
    let Some(Tag::IntArray(offsets)) = c.get(&"blocks".into()) else {
        return None;
    };
    let Some(Tag::List(states)) = c.get(&"states".into()) else {
        return None;
    };
    Some(
        offsets
            .as_chunks::<3>()
            .0
            .iter()
            .zip(states)
            .filter_map(|(o, s)| {
                let Tag::String(s) = s else { return None };
                Some((
                    [o[0], o[1], o[2]],
                    state_of_text(&*lv.host, &s.to_string_lossy())?,
                ))
            })
            .collect(),
    )
}

/// Moving and waiting pistons (`pistons`), as `LinearActuatorBlockEntity` and its contraption
/// entity save them: anchor, offset, range, starting offset, waiting, facing, and the blocks.
/// Their actors are found again from the blocks; their storage, controls, the kinds turned off
/// and glue are saved with them.
fn pistons_to_nbt(lv: &ServerLevel<'_>, all: &[([i32; 3], create_engine::world::Moving)]) -> Tag {
    Tag::List(
        all.iter()
            .filter_map(|(p, m)| {
                let facing = Dir::ALL.iter().position(|d| Some(*d) == m.facing)?;
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(p.to_vec()));
                if let Some(t) = actors::storage_to_nbt(lv, *p) {
                    c.insert("storage".into(), t);
                }
                structure_to_nbt(lv, &mut c, &m.blocks);
                actors::extras_to_nbt(&mut c, &m.carried);
                super::limit_to_nbt(&mut c, m.limit);
                c.insert(
                    "piston".into(),
                    Tag::IntArray(vec![
                        m.anchor[0],
                        m.anchor[1],
                        m.anchor[2],
                        m.offset.to_bits() as i32,
                        m.range,
                        m.initial,
                        i32::from(m.waiting),
                        facing as i32,
                    ]),
                );
                Some(Tag::Compound(c))
            })
            .collect(),
    )
}

fn pistons_from_nbt(
    lv: &ServerLevel<'_>,
    data: &Compound,
) -> Vec<([i32; 3], create_engine::world::Moving)> {
    let Some(Tag::List(list)) = data.get(&"pistons".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(v)) = c.get(&"piston".into()) else {
                return None;
            };
            let [ax, ay, az, offset, range, initial, waiting, facing] = v[..] else {
                return None;
            };
            let blocks = structure_from_nbt(lv, c)?;
            let carried =
                actors::with_extras(actors::bearing_actors(&blocks), &actors::extras_from_nbt(c));
            let pos = [*p.first()?, *p.get(1)?, *p.get(2)?];
            actors::storage_from_nbt(lv, pos, c.get(&"storage".into()));
            let limit = super::limit_from_nbt(lv.dim, pos, c);
            Some((
                pos,
                create_engine::world::Moving {
                    anchor: [ax, ay, az],
                    blocks,
                    offset: f32::from_bits(offset as u32),
                    range,
                    carried,
                    initial,
                    facing: Dir::ALL.get(facing as usize).copied(),
                    waiting: waiting == 1,
                    limit,
                },
            ))
        })
        .collect()
}

/// Moving gantries (`gantries`, in the column their carriage started from), as
/// `GantryContraptionEntity` saves itself: where it started, its carriage's and shaft line's
/// facings, how far it went, what is left of its sequenced limit, and the blocks. What they
/// carry is saved and found again as a piston's is.
fn gantries_to_nbt(lv: &ServerLevel<'_>, all: &[create_engine::gantry::Moving]) -> Tag {
    let index = |d: Dir| Dir::ALL.iter().position(|x| *x == d).unwrap_or(0) as i32;
    Tag::List(
        all.iter()
            .map(|g| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(g.start.to_vec()));
                if let Some(t) = actors::storage_to_nbt(lv, g.start) {
                    c.insert("storage".into(), t);
                }
                structure_to_nbt(lv, &mut c, &g.blocks);
                actors::extras_to_nbt(&mut c, &g.carried);
                super::limit_to_nbt(&mut c, g.limit);
                c.insert(
                    "gantry".into(),
                    Tag::IntArray(vec![
                        index(g.carriage_facing),
                        index(g.shaft_facing),
                        g.offset.to_bits() as i32,
                    ]),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

fn gantries_from_nbt(lv: &ServerLevel<'_>, data: &Compound) -> Vec<create_engine::gantry::Moving> {
    let Some(Tag::List(list)) = data.get(&"gantries".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(v)) = c.get(&"gantry".into()) else {
                return None;
            };
            let [carriage, shaft, offset] = v[..] else {
                return None;
            };
            let blocks = structure_from_nbt(lv, c)?;
            let start = [*p.first()?, *p.get(1)?, *p.get(2)?];
            actors::storage_from_nbt(lv, start, c.get(&"storage".into()));
            Some(create_engine::gantry::Moving {
                start,
                carriage_facing: *Dir::ALL.get(carriage as usize)?,
                shaft_facing: *Dir::ALL.get(shaft as usize)?,
                offset: f32::from_bits(offset as u32),
                // Shown again at the speed of its first tick.
                per_tick: 0.0,
                carried: actors::with_extras(
                    actors::bearing_actors(&blocks),
                    &actors::extras_from_nbt(c),
                ),
                // Its limit is set as it starts only (`limitMovement`), so no speed check resets it.
                limit: super::saved_limit(c),
                blocks,
            })
        })
        .collect()
}

fn windmills_from_nbt(
    lv: &ServerLevel<'_>,
    data: &Compound,
) -> Vec<([i32; 3], create_engine::world::Windmill)> {
    let Some(Tag::List(list)) = data.get(&"windmills".into()) else {
        return Vec::new();
    };
    let now = lv.game_time();
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(offsets)) = c.get(&"blocks".into()) else {
                return None;
            };
            let Some(Tag::List(states)) = c.get(&"states".into()) else {
                return None;
            };
            let Some(Tag::IntArray(sails)) = c.get(&"sails".into()) else {
                return None;
            };
            let blocks = offsets
                .as_chunks::<3>()
                .0
                .iter()
                .zip(states)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(&*lv.host, &s.to_string_lossy())?,
                    ))
                })
                .collect();
            let pos = [*p.first()?, *p.get(1)?, *p.get(2)?];
            actors::storage_from_nbt(lv, pos, c.get(&"storage".into()));
            let limit = super::limit_from_nbt(lv.dim, pos, c);
            let bearing = (sails.get(3) == Some(&1)).then(|| {
                let angle = f32::from_bits(sails.get(2).copied().unwrap_or(0) as u32);
                match c.get(&"turn".into()) {
                    Some(Tag::IntArray(t)) if t.len() == 4 => {
                        let since = (i64::from(t[2]) << 32) | i64::from(t[1] as u32);
                        let mut turn = Turn {
                            angle: f32::from_bits(t[0] as u32),
                            since: since.min(now),
                            rpm: f32::from_bits(t[3] as u32),
                            limit,
                        };
                        // Where it had turned to by now, and what is left of its limit; its speed
                        // comes back with its network's.
                        turn.rebase(now, 0.0);
                        turn
                    }
                    _ => Turn {
                        angle,
                        since: now,
                        rpm: 0.0,
                        limit,
                    },
                }
            });
            Some((
                pos,
                create_engine::world::Windmill {
                    blocks,
                    sails: (*sails.first()?).max(0) as u32,
                    clockwise: sails.get(1) != Some(&0),
                    bearing,
                    extras: actors::extras_from_nbt(c),
                },
            ))
        })
        .collect()
}
