//! Track: the track item's placement (`TrackBlockItem`, `TrackPlacement`), the curves it lays
//! and the fake track above them (`manageFakeTracksAlong`), and what clients draw of the
//! railway on its own blocks: each track block's curves (`TrackRenderer.renderBezierTurn`, with
//! a girder curve's beams, `renderGirder`) and each signal's light (`SignalRenderer`), carried
//! in the block entity's data. Metal girders in the off hand lay a girder curve, paving girders
//! under its straight runs (`TrackPaver`); a curve taken up drops its tracks and girders
//! (`BezierConnection.spawnItems`). Any other block in the off hand paves too: slabs, full
//! blocks and walls (under the rails only), each placed where the block is free.
//!
//! The signal item picks its track first (`TrackTargetingBlockItem.useOn`): a click on straight
//! track keeps the track and the way the player looked on the stack, and the signal placed next,
//! within 16 blocks, keeps them in its block entity (`TargetTrack`, `TargetDirection`), which
//! the railway reads as it is placed and loaded. Create's highlight of the picked track
//! (`TrackTargetingClient`) is not drawn, and a pick on diagonal, sloped or curved track is
//! refused as invalid (the port binds signals to straight track along x or z only).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::{level_index, with_world};
use super::saved;
use crate::glue::items;
use create_engine::Block;
use create_engine::network::Target;
use create_engine::track::{self, End, Failure, Request, TrackShape};
use create_engine::track_curves::Saved;
use create_engine::track_render::{Mat4, Piece, girder_parts, segment_parts};
use create_engine::trains::SignalState;
use mod_api::{
    BlockEntityDrawContext, BlockEntityOnLoadEvent, BlockEntityPart,
    BlockEntityRenderBoundingBoxEvent, BlockEntitySubmitEvent, BlockEntityUpdateTagEvent,
    BlockFace, BlockPart, BlockState, ClientHost, Compound, Host, HostBlockStates, HostSide,
    InteractionResult, ItemPart, OnBlockPlacedByEvent, Registrar, ServerHost, SupportType, Tag,
    Text, UseOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key in a track block entity's data holding the curves drawn from it.
pub const CURVES: &str = "Curves";
/// The key in a signal's block entity data: 1 while it shows red.
pub const LIGHT: &str = "Light";

thread_local! {
    /// The track each player picked first with the track item (`TRACK_CONNECTING_FROM`, kept
    /// in Create on the item; here per player until used or cleared), with its level.
    static TRACK_FROM: RefCell<HashMap<u128, (usize, End)>> = RefCell::new(HashMap::new());
    /// Track and signal block entities that came into a level, by level: their data is set
    /// from the railway at the level's next tick.
    static LOADED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    /// The pick a player's signal click is placing with (`useOn` sets the block entity data
    /// before `BlockItem.useOn` places it), by player: its level and the pick.
    static PLACING: RefCell<HashMap<u128, (usize, Target)>> = RefCell::new(HashMap::new());
}

/// The world closed.
pub fn forget() {
    TRACK_FROM.with_borrow_mut(HashMap::clear);
    LOADED.with_borrow_mut(Vec::clear);
    PLACING.with_borrow_mut(HashMap::clear);
}

pub fn register(r: &mut dyn Registrar) {
    let item = r.item_key("create:track");
    r.item_part(item, TrackItem);
    let signal = r.item_key("create:track_signal");
    r.item_part(signal, SignalItem);
}

/// The track shape of `state`, if it is Create's track.
fn shape_of(host: &dyn Host, state: BlockState) -> Option<TrackShape> {
    if state.block() != states::key(Block::Track) {
        return None;
    }
    TrackShape::read(&super::property(host, state, "shape")?)
}

/// `TrackBlockItem.useOn` with a track in hand, on the server: the first click on a track picks
/// it (`select`; a junction can't start a curve); the next, on another track or where a new
/// one goes (`getPlacementState`), joins the two as `TrackPlacement.tryConnect` decides and lays
/// the track (`placeTracks`); sneaking with a pick clears it. Create's message for a failure
/// shows on the action bar. Outside creative the tracks laid are paid for (`pay_tracks`).
struct TrackItem;

#[mod_api::horizon_part]
impl ItemPart for TrackItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let host = match host {
            HostSide::Server(host) => host,
            // The client's `useOn`: a pick or a join is the server's, so the client places no
            // block of its own for it (the stack shows a pick; a click on track makes one).
            HostSide::Client(host) => {
                let picked = items::custom(event.stack).contains_key(&SELECTION.into());
                let on_track = host
                    .level(host.dimension())
                    .and_then(|l| l.block_state(&*host, event.pos))
                    .is_some_and(|s| s.block() == states::key(Block::Track));
                return (picked || on_track).then_some(InteractionResult::Success);
            }
        };
        let player = host.player(event.player)?;
        let dimension = player.level(&*host)?.dimension().id(&*host)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        use_track(&mut lv, player, event.pos, event.face)
            .then_some(InteractionResult::SuccessServer)
    }

    /// `TrackBlock.getStateForPlacement` with a player: the straight or diagonal shape nearest
    /// their horizontal look, an orthogonal one becoming a ramp up toward a side whose block has
    /// a sturdy top under no sturdy bottom.
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(player) = event.placer else {
            return;
        };
        if shape_of(&*host, event.state).is_none() {
            return;
        }
        let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let pos = event.pos;
        let Some(shape) = placement_shape(&lv, player, pos) else {
            return;
        };
        if let Some(next) = super::with_props(&*lv.host, event.state, &[("shape", shape.name())])
            && next != event.state
        {
            lv.set_state(pos, next, update_all());
        }
    }
}

/// `TrackBlock.getStateForPlacement` for `player` placing track at `pos`: the straight or
/// diagonal shape nearest their horizontal look, an orthogonal one becoming a ramp up toward a
/// side whose block has a sturdy top under no sturdy bottom.
fn placement_shape(
    lv: &ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
) -> Option<TrackShape> {
    let look = player.entity().view_vector(&*lv.host)?;
    let flat = placement_look(look, player.entity().rotation(&*lv.host));
    let straight = TrackShape::for_placement(flat, |_| false);
    let sturdy = |p: [i32; 3], face: BlockFace| {
        lv.level
            .block_face_sturdy(&*lv.host, p, face, SupportType::Full)
            .unwrap_or(false)
    };
    Some(TrackShape::for_placement(flat, |negative| {
        let Some(axis) = straight.axes().first() else {
            return false;
        };
        let k = if negative { -1.0 } else { 1.0 };
        let o: [i32; 3] = std::array::from_fn(|i| pos[i] + (axis[i] * k).round() as i32);
        sturdy(o, BlockFace::Up) && !sturdy([o[0], o[1] + 1, o[2]], BlockFace::Down)
    }))
}

/// `getStateForPlacement`'s look: the horizontal part of the view, normalized; straight up or
/// down, (0, 0, 1) turned by the yaw.
fn placement_look(view: [f64; 3], rotation: Option<[f32; 2]>) -> [f64; 3] {
    let len = (view[0] * view[0] + view[2] * view[2]).sqrt();
    if len > 1.0e-5 {
        return [view[0] / len, 0.0, view[2] / len];
    }
    let yaw = f64::from(rotation.map_or(0.0, |r| r[0])).to_radians();
    [-yaw.sin(), 0.0, yaw.cos()]
}

fn say(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, key: &str) {
    super::action_bar(
        lv.host,
        player,
        Text::translate(format!("create.{key}"), Vec::new()),
    );
}

/// The track item's click; whether it was the track's.
fn use_track(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    face: [i32; 3],
) -> bool {
    let uuid = player.uuid();
    let entity = player.entity();
    let Some(look) = entity.view_vector(&*lv.host) else {
        return false;
    };
    let Some(here) = lv.block_state(pos) else {
        return false;
    };
    let dim = lv.dim;
    let picked = TRACK_FROM
        .with_borrow(|m| m.get(&uuid).copied())
        .filter(|(d, _)| *d == dim)
        .map(|(_, e)| e);
    let Some(from) = picked else {
        let Some(shape) = shape_of(&*lv.host, here) else {
            return false;
        };
        if shape.is_junction() {
            say(lv, player, "track.junction_start");
            return true;
        }
        let end = End::select(pos, shape, look);
        TRACK_FROM.with_borrow_mut(|m| m.insert(uuid, (dim, end)));
        mark_selection(lv, player, Some(pos));
        return true;
    };
    if entity.is_shift_key_down(&*lv.host) == Some(true) {
        TRACK_FROM.with_borrow_mut(|m| m.remove(&uuid));
        mark_selection(lv, player, None);
        say(lv, player, "track.selection_cleared");
        return true;
    }
    // The second end: the track clicked, else a new one where the click places it.
    let (pos2, state2) = match shape_of(&*lv.host, here) {
        Some(_) => (pos, here),
        None => {
            let replaceable = lv.host.block_states().replaceable(here) == Some(true);
            let p = if replaceable {
                pos
            } else {
                [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]]
            };
            let Some(shape) = placement_shape(lv, player, p) else {
                return true;
            };
            let Some(s) = lv
                .host
                .block_state_default(states::key(Block::Track))
                .and_then(|base| super::with_props(&*lv.host, base, &[("shape", shape.name())]))
            else {
                return true;
            };
            (p, s)
        }
    };
    let laid = lay_track(lv, from, pos2, state2, look, player);
    if let Ok(Some(m)) | Err(Some(m)) = laid {
        say(lv, player, &format!("track.{}", m.key()));
    }
    if laid.is_ok() {
        TRACK_FROM.with_borrow_mut(|m| m.remove(&uuid));
        mark_selection(lv, player, None);
    }
    true
}

/// `TrackPlacement.tryConnect` and `placeTracks` from end `from` to the track (or the track
/// state to place) `state2` at `pos2`, picked along `look`: the straight runs and the curve
/// between them laid. Ok with Create's message, if any; Err with the failure's message when
/// they can't be joined (None when there is nothing to join).
fn lay_track(
    lv: &mut ServerLevel<'_>,
    from: End,
    pos2: [i32; 3],
    state2: BlockState,
    look: [f64; 3],
    player: mod_api::PlayerRef,
) -> Result<Option<Failure>, Option<Failure>> {
    let Some(shape2) = shape_of(&*lv.host, state2) else {
        return Err(None);
    };
    let Some(state1) = lv.block_state(from.pos) else {
        return Err(None);
    };
    let info = track::try_connect(
        from,
        End::select(pos2, shape2, look),
        Request {
            original_present: shape_of(&*lv.host, state1).is_some(),
            second_tilted: false,
            max_length: track::MAX_PLACEMENT_LENGTH,
            maximise_turn: false,
            preview: false,
        },
    );
    if !info.valid {
        return Err(info.message);
    }
    // `TrackBlockItem.useOn`: metal girders in the off hand lay a girder curve. A block in the
    // off hand paves under the run (`paveTracks`), costing what it lays.
    let girder = items::held(&*lv.host, player, mod_api::Hand::Off)
        .is_some_and(|s| s.item().id(&*lv.host) == "create:metal_girder");
    let (paving, pavement) = match paver(lv, player, info.positions[0]) {
        Some(p) => pave(lv, &info, &p),
        None => (Vec::new(), 0),
    };
    // `tryConnect` outside creative: the player pays a track for each one laid where the block
    // is replaceable, and half a track per sleeper segment of the curve, from their stacks of
    // the held track (the selected slot after the others, then the off hand), or nothing is
    // laid.
    if player.is_creative(&*lv.host) != Some(true) {
        let facts = lv.host.block_states();
        let straight: i32 = (0..2)
            .flat_map(|i| info.straight(i).collect::<Vec<_>>())
            .filter(|&p| {
                lv.block_state(p)
                    .is_some_and(|s| facts.replaceable(s) == Some(true))
            })
            .count() as i32;
        let need = straight + info.turn_tracks();
        if pavement > 0 && count_offhand_kind(lv, player) < pavement {
            return Err(Some(Failure::NotEnoughPavement));
        }
        if !pay_tracks(lv, player, need) {
            return Err(Some(Failure::NotEnoughTracks));
        }
        if pavement > 0 {
            pay_offhand_kind(lv, player, pavement);
        }
    }
    for (p, state) in paving {
        lv.set_state(p, state, update_all());
    }
    // `placeTracks`: each end's state (a T junction's straight part) along its run.
    for (i, state) in [state1, state2].into_iter().enumerate() {
        let shape = match shape_of(&*lv.host, state).map(TrackShape::name) {
            Some("te" | "tw") => "xo",
            Some("tn" | "ts") => "zo",
            Some(other) => other,
            None => continue,
        };
        let Some(place) =
            super::with_props(&*lv.host, state, &[("shape", shape), ("turn", "false")])
        else {
            continue;
        };
        for p in info.straight(i).collect::<Vec<_>>() {
            let Some(at) = lv.block_state(p) else {
                continue;
            };
            let replaceable = lv.host.block_states().replaceable(at) == Some(true);
            if replaceable || shape_of(&*lv.host, at).is_some() {
                lv.set_state(p, place, update_all());
            }
        }
    }
    let Some(curve) = info.curve.as_ref() else {
        return Ok(info.message);
    };
    for (t, state) in info.targets.into_iter().zip([state1, state2]) {
        let base = match lv.block_state(t) {
            Some(at) if shape_of(&*lv.host, at).is_some() => at,
            _ => state,
        };
        if let Some(s) = super::with_props(&*lv.host, base, &[("turn", "true")]) {
            lv.set_state(t, s, update_all());
        }
    }
    let saved = Saved {
        from: info.targets[0],
        to: info.targets[1],
        starts: curve.starts,
        axes: curve.axes,
        normals: curve.normals,
        girder,
    };
    let dim = lv.dim;
    with_world(|w| {
        w.dims[dim].track_curves.add(saved);
        w.mark_unsaved(dim, saved.from);
        w.mark_unsaved(dim, saved.to);
    });
    Ok(info.message)
}

/// The block the off hand paves with (`paveTracks`): its default state (a slab's double), and
/// whether it is a slab (`SlabBlock.TYPE`), wall-like (a wall or a metal girder) and a girder.
struct Paver {
    state: BlockState,
    slab: bool,
    wall: bool,
    girder: bool,
}

/// `tryConnect`'s `shouldPave` and `paveTracks`' refusals: a block item in the off hand but the
/// brackets (`INVALID_FOR_TRACK_PAVING`), whose block has no block entity and collides (at `at`).
fn paver(lv: &ServerLevel<'_>, player: mod_api::PlayerRef, at: [i32; 3]) -> Option<Paver> {
    let host = &*lv.host;
    let stack = items::held(host, player, mod_api::Hand::Off)?;
    let id = stack.item().id(host);
    if matches!(
        id.as_str(),
        "create:wooden_bracket" | "create:metal_bracket"
    ) {
        return None;
    }
    let block = host.item_block(stack.item())?;
    let mut state = host.block_state_default(block)?;
    let facts = host.block_states();
    if facts.has_block_entity(state) == Some(true)
        || block
            .id(host)
            .strip_prefix("create:")
            .and_then(Block::from_name)
            .is_some_and(Block::has_block_entity)
    {
        return None;
    }
    let collides = lv
        .level
        .block_shape(host, state, at, mod_api::BlockShapeKind::Collision)
        .is_some_and(|s| !s.is_empty());
    if !collides {
        return None;
    }
    let girder = block.id(host) == "create:metal_girder";
    let slab = super::with_props(host, state, &[("type", "double")]);
    if let Some(double) = slab {
        state = double;
    }
    Some(Paver {
        state,
        slab: slab.is_some(),
        wall: girder || host.block_in_tag(block, "minecraft:walls") == Some(true),
        girder,
    })
}

/// `TrackPaver.placeBlockIfFree`: another block that can be replaced is at `p`; what goes
/// there holds the water source it replaces (`ProperWaterloggedBlock.withWater`).
fn free_for(lv: &ServerLevel<'_>, p: [i32; 3], state: BlockState) -> Option<BlockState> {
    let host = &*lv.host;
    let at = lv.block_state(p)?;
    if at.block() == state.block() || host.block_states().replaceable(at) != Some(true) {
        return None;
    }
    let water = host
        .block_state_id("minecraft:water")
        .and_then(|w| host.state_fluid_state(w));
    let source = host
        .state_fluid_state(at)
        .is_some_and(|f| f.source && water.is_some_and(|w| w.fluid == f.fluid));
    let has = state
        .property(&host.block_states(), "waterlogged")
        .ok()
        .flatten()
        .is_some();
    Some(if has {
        super::with_props(
            host,
            state,
            &[("waterlogged", if source { "true" } else { "false" })],
        )
        .unwrap_or(state)
    } else {
        state
    })
}

/// `paveTracks`: under each end's straight run (`paveStraight`) and the curve (`paveCurve`),
/// the blocks to place and the items they cost (a slab's double two, its half one). A girder
/// run along an axis lies along it (`AXIS`, its `x` or `z` side, neither top nor bottom); a
/// wall-like curve paves nothing but costs a girder curve two girders per two sleeper segments.
fn pave(
    lv: &ServerLevel<'_>,
    info: &track::Placement,
    paver: &Paver,
) -> (Vec<([i32; 3], BlockState)>, i32) {
    let host = &*lv.host;
    let mut out: Vec<([i32; 3], BlockState)> = Vec::new();
    let mut visited: Vec<[i32; 3]> = Vec::new();
    let mut needed = 0;
    for i in 0..2 {
        let a = info.axes[i];
        let mut state = paver.state;
        if paver.girder {
            let side = match a {
                [x, y, z] if y == 0.0 && z == 0.0 && x.abs() == 1.0 => Some(("x", "x")),
                [x, y, z] if y == 0.0 && x == 0.0 && z.abs() == 1.0 => Some(("z", "z")),
                _ => None,
            };
            if let Some((axis, flag)) = side {
                state = super::with_props(
                    host,
                    state,
                    &[
                        ("top", "false"),
                        ("bottom", "false"),
                        ("axis", axis),
                        (flag, "true"),
                    ],
                )
                .unwrap_or(state);
            }
        }
        for p in info.pave_straight(i, paver.wall) {
            if visited.contains(&p) {
                continue;
            }
            visited.push(p);
            if let Some(s) = free_for(lv, p, state) {
                out.push((p, s));
                needed += if paver.slab { 2 } else { 1 };
            }
        }
    }
    let Some(curve) = info.curve.as_ref() else {
        return (out, needed);
    };
    if paver.wall {
        if paver.girder {
            needed += (curve.segments as i32 + 1) / 2 * 2;
        }
        return (out, needed);
    }
    let half = |t: &str| super::with_props(host, paver.state, &[("type", t)]);
    for (key, y) in curve.pave_levels() {
        let floor = y.floor() as i32;
        let slab = paver.slab && y - f64::from(floor) >= 0.5;
        let target = [key[0], floor + i32::from(slab), key[1]];
        let state = if slab {
            half("bottom").unwrap_or(paver.state)
        } else {
            paver.state
        };
        if visited.contains(&target) {
            continue;
        }
        visited.push(target);
        if let Some(s) = free_for(lv, target, state) {
            out.push((target, s));
            needed += if slab { 1 } else { 2 };
        }
        if slab {
            let below = [target[0], target[1] - 1, target[2]];
            if visited.contains(&below) {
                continue;
            }
            visited.push(below);
            if let Some(s) = half("top").and_then(|top| free_for(lv, below, top)) {
                out.push((below, s));
                needed += 1;
            }
        }
    }
    (out, needed)
}

/// How many of the off hand's item `player` has (its stacks anywhere and the off hand's).
fn count_offhand_kind(lv: &ServerLevel<'_>, player: mod_api::PlayerRef) -> i32 {
    use mod_api::Hand;
    let Some(item) = items::held(&*lv.host, player, Hand::Off).map(|s| s.item()) else {
        return 0;
    };
    let slots: i32 = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, s)| s.item() == item)
        .map(|(_, s)| s.count() as i32)
        .sum();
    slots + items::held(&*lv.host, player, Hand::Off).map_or(0, |s| s.count() as i32)
}

/// Takes `need` of the off hand's item from `player`: the inventory's stacks of it, then the
/// off hand (`tryConnect`'s pavement pass).
fn pay_offhand_kind(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, need: i32) {
    use items::{held, set_held};
    use mod_api::Hand;
    let Some(item) = held(&*lv.host, player, Hand::Off).map(|s| s.item()) else {
        return;
    };
    let mut left = need;
    for (i, s) in player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, s)| s.item() == item)
    {
        if left <= 0 {
            return;
        }
        let take = left.min(s.count() as i32);
        left -= take;
        let _ = player.set_inventory_slot(lv.host, i, s.shrink(take as u32));
    }
    if left > 0
        && let Some(s) = held(&*lv.host, player, Hand::Off)
    {
        set_held(lv.host, player, Hand::Off, s.shrink(left as u32));
    }
}

/// `BezierConnection.spawnItems` for curves taken up with their track: a track at every other
/// sleeper segment but the last, and two metal girders beside it on a girder curve.
fn drop_curves(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let dropped =
        with_world(|w| std::mem::take(&mut w.dims[dim].track_curves.dropped)).unwrap_or_default();
    for s in dropped {
        let curve = s.curve();
        for i in (0..curve.segments).step_by(2) {
            let p = curve.position(f64::from(curve.segment_t(i)));
            let at = [p[0], p[1] + f64::from(3.0f32 / 16.0), p[2]];
            let ids: &[&str] = if s.girder {
                &["create:track", "create:metal_girder", "create:metal_girder"]
            } else {
                &["create:track"]
            };
            for id in ids {
                if let Some(stack) = crate::glue::packages::body::new_stack(&*lv.host, id, 1) {
                    let _ = lv
                        .level
                        .spawn_item(&mut *lv.host, at[0], at[1], at[2], stack, 10);
                }
            }
        }
    }
}

/// The key in the track stack's custom data marking its pick (`TRACK_CONNECTING_FROM`).
const SELECTION: &str = "create:track_connecting_from";

/// `minecraft:enchantment_glint_override`: a picking track shines (`isFoil`).
struct Glint;

impl mod_api::Component for Glint {
    type Value = bool;
    fn id(&self) -> &str {
        "minecraft:enchantment_glint_override"
    }
    fn decode(&self, value: &Tag) -> Option<bool> {
        match value {
            Tag::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }
    fn encode(&self, value: bool) -> Tag {
        Tag::Byte(i8::from(value))
    }
}

/// Marks the track in `player`'s main hand as picking the track at `from`
/// (`TRACK_CONNECTING_FROM`, which makes it shine), or not.
fn mark_selection(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, from: Option<[i32; 3]>) {
    use mod_api::Hand;
    let Some(mut stack) = items::held(&*lv.host, player, Hand::Main) else {
        return;
    };
    if stack.item().id(&*lv.host) != "create:track" {
        return;
    }
    items::set_custom(
        &mut stack,
        &[(SELECTION, from.map(|p| Tag::IntArray(p.to_vec())))],
    );
    if from.is_some() {
        let _ = stack.set(&Glint, true);
    } else {
        stack.remove(&Glint);
    }
    items::set_held(lv.host, player, Hand::Main, Some(stack));
}

/// Takes `need` of the held track item from `player` (`tryConnect`'s two passes: counted first,
/// then taken from the inventory with the selected slot last, then the off hand). False, with
/// nothing taken, when they have too few.
fn pay_tracks(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, need: i32) -> bool {
    use items::{held, set_held};
    use mod_api::Hand;
    let Some(item) = held(&*lv.host, player, Hand::Main).map(|s| s.item()) else {
        return false;
    };
    let selected = player.selected_slot(&*lv.host).map(|s| s as u32);
    let mut slots: Vec<(u32, mod_api::ItemStack)> = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, s)| s.item() == item)
        .collect();
    slots.sort_by_key(|(i, _)| Some(*i) == selected);
    let offhand = held(&*lv.host, player, Hand::Off).filter(|s| s.item() == item);
    let have: i32 = slots.iter().map(|(_, s)| s.count() as i32).sum::<i32>()
        + offhand.as_ref().map_or(0, |s| s.count() as i32);
    if have < need {
        return false;
    }
    let mut left = need;
    for (i, s) in slots {
        if left <= 0 {
            break;
        }
        let take = left.min(s.count() as i32);
        left -= take;
        let _ = player.set_inventory_slot(lv.host, i, s.shrink(take as u32));
    }
    if left > 0
        && let Some(s) = offhand
    {
        set_held(lv.host, player, Hand::Off, s.shrink(left as u32));
    }
    true
}

/// Curves that changed since the last tick: a track left with none is no longer `turn`
/// (`HAS_BE`), and each track's block entity carries what it draws now; then the track and
/// signal block entities that came in get their data.
pub fn sync(lv: &mut ServerLevel<'_>) {
    lay_fake_track(lv);
    drop_curves(lv);
    let dim = lv.dim;
    let changed = with_world(|w| {
        let c = &mut w.dims[dim].track_curves.changed;
        if c.is_empty() {
            return Vec::new();
        }
        let mut c = std::mem::take(c);
        c.sort();
        c.dedup();
        c
    })
    .unwrap_or_default();
    for pos in changed {
        let none = with_world(|w| !w.dims[dim].track_curves.at.contains_key(&pos)).unwrap_or(true);
        if none
            && let Some(state) = lv.block_state(pos)
            && shape_of(&*lv.host, state).is_some()
            && super::property(&*lv.host, state, "turn").as_deref() == Some("true")
            && let Some(s) = super::with_props(&*lv.host, state, &[("turn", "false")])
        {
            lv.set_state(pos, s, update_all());
        }
        set_curves(lv, pos);
    }
    if LOADED.with_borrow(Vec::is_empty) {
        return;
    }
    let loaded: Vec<[i32; 3]> = LOADED.with_borrow_mut(|l| {
        let (here, rest): (Vec<_>, Vec<_>) =
            std::mem::take(l).into_iter().partition(|e| e.0 == dim);
        *l = rest;
        here.into_iter().map(|e| e.1).collect()
    });
    for pos in loaded {
        let state = lv.block(pos);
        match with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten() {
            Some(Block::Track) => set_curves(lv, pos),
            Some(Block::TrackSignal) => {
                // Its pick, if it was placed with one (else it binds to the track beside it).
                let target = lv
                    .host
                    .block_entity(lv.level, pos)
                    .and_then(|e| e.data(&*lv.host))
                    .and_then(|d| data_target(pos, &d));
                with_world(|w| w.dims[dim].railway.set_signal_target(pos, target));
                let state = with_world(|w| w.dims[dim].railway.lights.get(&pos).copied()).flatten();
                set_light(lv, pos, state.unwrap_or_default());
            }
            _ => {}
        }
    }
}

/// The curves drawn from the track at `pos`, into its block entity's data (sent to clients).
fn set_curves(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let curves = with_world(|w| w.dims[dim].track_curves.drawn_from(pos)).unwrap_or_default();
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let has = entity
        .data(&*lv.host)
        .is_some_and(|d| d.contains_key(&CURVES.into()));
    if curves.is_empty() && !has {
        return;
    }
    let data = Compound::from([(CURVES.into(), saved::curves_to_nbt(&curves))]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// The keys in the signal stack's custom data holding its pick
/// (`TRACK_TARGETING_ITEM_SELECTED_POS`, `TRACK_TARGETING_ITEM_SELECTED_DIRECTION`).
const PICK_POS: &str = "create:track_targeting_item_selected_pos";
const PICK_DIRECTION: &str = "create:track_targeting_item_selected_direction";
/// The keys in a signal's block entity data (`TrackTargetingBehaviour.write`): its track, from
/// the signal, and whether it was picked `POSITIVE`.
pub const TARGET_TRACK: &str = "TargetTrack";
pub const TARGET_DIRECTION: &str = "TargetDirection";

/// The pick a signal stack holds.
fn stack_pick(stack: &mod_api::ItemStack) -> Option<Target> {
    let c = items::custom(stack);
    let Some(Tag::IntArray(p)) = c.get(&PICK_POS.into()) else {
        return None;
    };
    let &[x, y, z] = &p[..] else {
        return None;
    };
    let front = matches!(c.get(&PICK_DIRECTION.into()), Some(Tag::Byte(b)) if *b != 0);
    Some(Target {
        track: [x, y, z],
        front,
    })
}

/// Sets (or clears) the pick on the signal stack in `player`'s `hand`.
fn set_stack_pick(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    hand: mod_api::Hand,
    pick: Option<Target>,
) {
    let Some(mut stack) = items::held(&*lv.host, player, hand) else {
        return;
    };
    if stack.item().id(&*lv.host) != "create:track_signal" {
        return;
    }
    items::set_custom(
        &mut stack,
        &[
            (PICK_POS, pick.map(|t| Tag::IntArray(t.track.to_vec()))),
            (PICK_DIRECTION, pick.map(|t| Tag::Byte(i8::from(t.front)))),
        ],
    );
    items::set_held(lv.host, player, hand, Some(stack));
}

/// The pick kept in a signal's block entity data, if it has one.
pub fn data_target(pos: [i32; 3], data: &Compound) -> Option<Target> {
    let Some(Tag::IntArray(t)) = data.get(&TARGET_TRACK.into()) else {
        return None;
    };
    let &[x, y, z] = &t[..] else {
        return None;
    };
    let front = matches!(data.get(&TARGET_DIRECTION.into()), Some(Tag::Byte(b)) if *b != 0);
    Some(Target {
        track: [pos[0] + x, pos[1] + y, pos[2] + z],
        front,
    })
}

/// A `create.track_target.*` line on the action bar, coloured as Create's.
fn say_target(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    key: &str,
    color: Option<&str>,
) {
    let mut line = Text::translate(format!("create.track_target.{key}"), Vec::new());
    if let Some(c) = color {
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some(c.to_owned());
        line = line.with_style(style);
    }
    super::action_bar(lv.host, player, line);
}

fn click_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], id: &str, volume: f32, pitch: f32) {
    let at = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    lv.level.play_sound(
        &mut *lv.host,
        id,
        at,
        mod_api::SoundSource::Blocks,
        volume,
        pitch,
    );
}

/// `TrackTargetingBlockItem.useOn` with a signal, on the server: sneaking with a pick clears
/// it; a click on track picks it and the way the player looks along it (refused on a junction,
/// where another point stands, or off the graph); with no pick nothing is placed; else the
/// signal goes where the click places it if the pick is within 16 blocks, keeping it (None: the
/// block item places it).
fn use_signal(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    event: &UseOnEvent<'_>,
) -> Option<InteractionResult> {
    let pos = event.pos;
    let picked = stack_pick(event.stack);
    if picked.is_some() && player.entity().is_shift_key_down(&*lv.host) == Some(true) {
        say_target(lv, player, "clear", None);
        set_stack_pick(lv, player, event.hand, None);
        click_sound(lv, pos, "create:controller_click", 1.0, 0.5);
        return Some(InteractionResult::SuccessServer);
    }
    let here = lv.block_state(pos)?;
    if let Some(shape) = shape_of(&*lv.host, here) {
        let deny = |lv: &mut ServerLevel<'_>, key: &str| {
            say_target(lv, player, key, Some("red"));
            click_sound(lv, pos, "create:deny", 0.5, 1.0);
            Some(InteractionResult::Fail)
        };
        if shape.is_junction() {
            return deny(lv, "no_junctions");
        }
        let look = player.entity().view_vector(&*lv.host)?;
        let Some(target) = Target::pick(pos, shape, look) else {
            return deny(lv, "invalid");
        };
        let dim = lv.dim;
        let overlap = with_world(|w| w.dims[dim].railway.net.signal_overlap(pos)).flatten();
        match overlap {
            None => return deny(lv, "invalid"),
            Some(true) => return deny(lv, "occupied"),
            Some(false) => {}
        }
        set_stack_pick(lv, player, event.hand, Some(target));
        say_target(lv, player, "set", None);
        click_sound(lv, pos, "create:controller_click", 1.0, 1.0);
        return Some(InteractionResult::SuccessServer);
    }
    let Some(target) = picked else {
        say_target(lv, player, "missing", Some("red"));
        return Some(InteractionResult::Fail);
    };
    let replaceable = lv.host.block_states().replaceable(here) == Some(true);
    let placed = if replaceable {
        pos
    } else {
        std::array::from_fn(|i| pos[i] + event.face[i])
    };
    let far: i64 = (0..3)
        .map(|i| i64::from(target.track[i] - placed[i]).pow(2))
        .sum();
    if far >= 16 * 16 {
        say_target(lv, player, "too_far", Some("red"));
        return Some(InteractionResult::Fail);
    }
    let dim = lv.dim;
    PLACING.with_borrow_mut(|m| m.insert(player.uuid(), (dim, target)));
    None
}

/// The signal item (`TrackTargetingBlockItem`).
struct SignalItem;

#[mod_api::horizon_part]
impl ItemPart for SignalItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let host = match host {
            HostSide::Server(host) => host,
            // The client's `useOn`: a clear or a pick is the server's; with no pick nothing is
            // placed.
            HostSide::Client(host) => {
                let picked = stack_pick(event.stack).is_some();
                let sneaking = host
                    .local_player()
                    .and_then(|p| p.is_secondary_use_active(&*host))
                    == Some(true);
                let on_track = host
                    .level(host.dimension())
                    .and_then(|l| l.block_state(&*host, event.pos))
                    .is_some_and(|s| s.block() == states::key(Block::Track));
                if (picked && sneaking) || on_track {
                    return Some(InteractionResult::Success);
                }
                return (!picked).then_some(InteractionResult::Fail);
            }
        };
        PLACING.with_borrow_mut(|m| m.remove(&event.player));
        let player = host.player(event.player)?;
        let dimension = player.level(&*host)?.dimension().id(&*host)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        use_signal(&mut lv, player, event)
    }

    /// The rest of `useOn` once the block is placed: the pick into its block entity
    /// (`BLOCK_ENTITY_DATA`: `TargetTrack` from the signal, `TargetDirection`), off the stack, and
    /// `track_target.success`.
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(player) = event.placer else {
            return;
        };
        let Some((dim, target)) = PLACING.with_borrow_mut(|m| m.remove(&player.uuid())) else {
            return;
        };
        if event.state.block() != states::key(Block::TrackSignal) {
            return;
        }
        let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        if lv.dim != dim {
            return;
        }
        let pos = event.pos;
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            return;
        };
        let rel: Vec<i32> = (0..3).map(|i| target.track[i] - pos[i]).collect();
        let data = Compound::from([
            (TARGET_TRACK.into(), Tag::IntArray(rel)),
            (TARGET_DIRECTION.into(), Tag::Byte(i8::from(target.front))),
        ]);
        if entity.merge_data(&mut *lv.host, data).is_err() {
            return;
        }
        with_world(|w| w.dims[dim].railway.set_signal_target(pos, Some(target)));
        for hand in [mod_api::Hand::Main, mod_api::Hand::Off] {
            if items::held(&*lv.host, player, hand).is_some_and(|s| stack_pick(&s).is_some()) {
                set_stack_pick(&mut lv, player, hand, None);
            }
        }
        say_target(&mut lv, player, "success", Some("green"));
    }
}

/// Whether a signal's data shows an invalid signal's blink.
pub fn blinking(nbt: &Compound) -> bool {
    matches!(nbt.get(&LIGHT.into()), Some(Tag::Byte(l)) if *l as u8 == create_engine::trains::LIGHT_BLINK)
}

/// A signal's light into its block entity's data: dark, red or blinking (`SignalRenderer`,
/// `SignalState::light`), and its whole state (`SIGNAL_STATE`, `SignalState`'s ordinal) for a
/// nixie tube in front of it.
fn set_light(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: SignalState) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let data = Compound::from([
        (LIGHT.into(), Tag::Byte(state.light() as i8)),
        (SIGNAL_STATE.into(), Tag::Byte(state.index() as i8)),
    ]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// The key of a signal's state in its block entity's data (`SignalBlockEntity.getState`'s
/// ordinal).
pub const SIGNAL_STATE: &str = "SignalState";

/// A signal's state as its block entity's data says (`INVALID` when it says none).
pub fn signal_state_of(data: &Compound) -> SignalState {
    match data.get(&SIGNAL_STATE.into()) {
        Some(Tag::Byte(b)) => SignalState::from_index(*b as u8),
        _ => SignalState::Invalid,
    }
}

/// Signal lights that changed in the railway's tick (None: the signal is gone).
pub fn lights_changed(lv: &mut ServerLevel<'_>, lights: &[([i32; 3], Option<SignalState>)]) {
    for &(pos, state) in lights {
        if let Some(state) = state {
            set_light(lv, pos, state);
        }
    }
}

/// `TrackBlockEntity.manageFakeTracksAlong` for curves added or gone: fake track (`Track
/// Marker for Maps`) laid in the replaceable blocks above a curve (waterlogged in a water
/// source, never in another fluid), taken up with it. Create also lays it again every lazy tick
/// and lets it decay by random ticks (`FakeTrackBlockEntity.keepAlive`); here it is laid once and
/// taken up when its curve goes, with nothing per tick.
fn lay_fake_track(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let fake = with_world(|w| {
        let f = &mut w.dims[dim].track_curves.fake;
        if f.is_empty() {
            Vec::new()
        } else {
            std::mem::take(f)
        }
    })
    .unwrap_or_default();
    if fake.is_empty() {
        return;
    }
    let Some(dry) = lv.host.block_state_default(states::key(Block::FakeTrack)) else {
        return;
    };
    let wet = super::with_props(&*lv.host, dry, &[("waterlogged", "true")]).unwrap_or(dry);
    for (p, lay) in fake {
        if !lv.is_loaded(p) {
            continue;
        }
        let Some(here) = lv.block_state(p) else {
            continue;
        };
        let present = here == dry || here == wet;
        if !lay {
            if present {
                // `removeBlock`: its water stays.
                let name = if here == wet {
                    "minecraft:water"
                } else {
                    "minecraft:air"
                };
                if let Some(s) = super::default_state(&*lv.host, name) {
                    lv.set_state(p, s, update_all());
                }
            }
            continue;
        }
        if present {
            continue;
        }
        let fluid = lv.host.state_fluid_state(here);
        let water = lv
            .host
            .block_state_string(here)
            .is_some_and(|t| t == "minecraft:water[level=0]" || t == "minecraft:water");
        let wet_here = fluid.is_some_and(|f| !f.is_empty());
        if (wet_here && !water) || lv.host.block_states().replaceable(here) != Some(true) {
            continue;
        }
        lv.set_state(p, if water { wet } else { dry }, update_all());
    }
}

/// Create's track and signal blocks' part on the kept blocks' block entity: their data is set
/// when they come in, carried to clients, and drawn there.
pub struct TrackEntity;

#[mod_api::horizon_part]
impl BlockPart for TrackEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for TrackEntity {
    fn block_entity_on_load(&self, host: HostSide, event: &mut BlockEntityOnLoadEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let entity = event.entity;
        let (Some(level), Some(pos)) = (entity.level(&*host), entity.position(&*host)) else {
            return;
        };
        let Some(state) = level.block_state(&*host, pos) else {
            return;
        };
        let ours = [Block::Track, Block::TrackSignal]
            .into_iter()
            .any(|b| state.block() == states::key(b));
        if !ours {
            return;
        }
        let Some(dim) = level.dimension().id(&*host).and_then(|d| level_index(&d)) else {
            return;
        };
        LOADED.with_borrow_mut(|l| l.push((dim, pos)));
    }

    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let nbt = &event.entity.nbt;
        let mut out = Compound::new();
        for key in [
            CURVES,
            LIGHT,
            SIGNAL_STATE,
            super::station_draw::KEY,
            super::station_draw::MARK,
        ] {
            if let Some(v) = nbt.get(&key.into()) {
                out.insert(key.into(), v.clone());
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        if event.state.block() == states::key(Block::TrackSignal) {
            // Its marker, on its track up to 16 blocks away.
            if let Some(Tag::Compound(c)) =
                crate::glue::kinetics::synced(&event.entity.nbt, super::station_draw::MARK)
                && let Some(Tag::IntArray(t)) = c.get(&"t".into())
                && let &[x, y, z] = &t[..]
            {
                for (k, d) in [x, y, z].into_iter().enumerate() {
                    let d = d as f32;
                    event.bounds[k] = event.bounds[k].min(d - 1.0);
                    event.bounds[k + 3] = event.bounds[k + 3].max(d + 2.0);
                }
            }
            return;
        }
        if event.state.block() != states::key(Block::Track) {
            return;
        }
        let curves = saved::curves_of(event.entity.nbt.get(&CURVES.into()));
        let origin = event.pos;
        for s in curves {
            for p in [s.from, s.to] {
                for k in 0..3 {
                    let d = (p[k] - origin[k]) as f32;
                    event.bounds[k] = event.bounds[k].min(d - 1.0);
                    event.bounds[k + 3] = event.bounds[k + 3].max(d + 2.0);
                }
            }
        }
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        // Hot: every kept block's draw; three compares for any other block.
        let block = event.state.block();
        if block == states::key(Block::TrackStation) {
            super::station_draw::draw(host, event.draw, event.pos, &event.entity.nbt);
            return;
        }
        if block == states::key(Block::TrackObserver) {
            super::station_draw::draw_mark(host, event.draw, &event.entity.nbt);
            return;
        }
        if block != states::key(Block::Track) && block != states::key(Block::TrackSignal) {
            return;
        }
        let nbt = &event.entity.nbt;
        if block == states::key(Block::TrackSignal) {
            super::station_draw::draw_mark(host, event.draw, nbt);
        }
        if let Some(Tag::Byte(light)) = nbt.get(&LIGHT.into()) {
            // `SignalRenderer`: Create's `indicator_on` partial, full bright, while red (and an
            // invalid signal's blink, the first 3 ticks of every 40); `indicator_off` else.
            let light = *light as u8;
            let blink = light == create_engine::trains::LIGHT_BLINK;
            let on = light == create_engine::trains::LIGHT_RED
                || (blink && event.draw.animation_time().rem_euclid(40.0) < 3.0);
            let name = if on {
                "part_signal_indicator_on"
            } else {
                "part_signal_indicator_off"
            };
            if let Some(b) = states::partial(name) {
                let lit = event.draw.light();
                if on {
                    event.draw.set_light(0x00F0_00F0);
                }
                event.draw.block(b, &[("facing", "up")]);
                event.draw.set_light(lit);
            }
            if !blink {
                event.draw.keep();
            }
            return;
        }
        let curves = saved::curves_of(nbt.get(&CURVES.into()));
        if curves.is_empty() {
            return;
        }
        let part = states::partial;
        let (Some(tie), Some(left), Some(right)) = (
            part("part_track_tie"),
            part("part_track_segment_left"),
            part("part_track_segment_right"),
        ) else {
            return;
        };
        // `renderBezierTurn`: a girder curve's beams and caps (`renderGirder`), then the
        // sleepers and rails.
        let girders = [
            "part_girder_segment_middle",
            "part_girder_segment_top",
            "part_girder_segment_bottom",
        ]
        .map(part);
        for s in &curves {
            let curve = s.curve();
            let mut pieces = if s.girder {
                girder_parts(&curve, event.pos)
            } else {
                Vec::new()
            };
            pieces.extend(segment_parts(&curve, event.pos));
            for (piece, m) in pieces {
                let key = match piece {
                    Piece::Tie => Some(tie),
                    Piece::RailLeft => Some(left),
                    Piece::RailRight => Some(right),
                    Piece::GirderMiddle => girders[0],
                    Piece::GirderTop => girders[1],
                    Piece::GirderBottom => girders[2],
                };
                let Some(key) = key else {
                    continue;
                };
                draw_posed(event.draw, &m, |d| {
                    d.block(key, &[("facing", "south")]);
                });
            }
        }
        event.draw.keep();
    }
}

/// Draws what `f` draws under transform `m` (a translation, a rotation and a stretch along the
/// piece's own z, as `segment_parts` builds each piece), in the draw context's own operations.
fn draw_posed(
    draw: &mut dyn BlockEntityDrawContext,
    m: &Mat4,
    f: impl FnOnce(&mut dyn BlockEntityDrawContext),
) {
    let (t, axis, degrees, stretch) = decompose(m);
    draw.push();
    draw.translate(t);
    if degrees != 0.0 {
        draw.rotate_about(axis, degrees, [0.0; 3]);
    }
    draw.scale([1.0, 1.0, stretch]);
    f(draw);
    draw.pop();
}

/// A column-major transform `translate · rotate · scale(1, 1, z)` as its translation, the
/// rotation's axis and angle in degrees (counterclockwise seen from the axis' positive end),
/// and `z`.
fn decompose(m: &Mat4) -> ([f32; 3], [f32; 3], f32, f32) {
    let t = [m[3][0], m[3][1], m[3][2]];
    let len = |c: &[f32; 4]| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let stretch = len(&m[2]);
    let col = |j: usize| {
        let s = if j == 2 {
            stretch.max(1e-9)
        } else {
            len(&m[j]).max(1e-9)
        };
        [m[j][0] / s, m[j][1] / s, m[j][2] / s]
    };
    let c = [col(0), col(1), col(2)];
    // `r(i, j)`: row i, column j.
    let r = |i: usize, j: usize| f64::from(c[j][i]);
    let trace = r(0, 0) + r(1, 1) + r(2, 2);
    let angle = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos();
    if angle < 1e-6 {
        return (t, [0.0, 1.0, 0.0], 0.0, stretch);
    }
    let axis = if std::f64::consts::PI - angle < 1e-4 {
        // Half a turn: the axis from the diagonal, its signs from the largest component.
        let d = [r(0, 0), r(1, 1), r(2, 2)].map(|v| ((v + 1.0) / 2.0).max(0.0).sqrt());
        let k = (0..3).max_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap_or(0);
        let mut a = d;
        for (j, v) in a.iter_mut().enumerate() {
            if j != k && r(k, j) + r(j, k) < 0.0 {
                *v = -*v;
            }
        }
        a
    } else {
        let s = 2.0 * angle.sin();
        [
            (r(2, 1) - r(1, 2)) / s,
            (r(0, 2) - r(2, 0)) / s,
            (r(1, 0) - r(0, 1)) / s,
        ]
    };
    (
        t,
        axis.map(|v| v as f32),
        angle.to_degrees() as f32,
        stretch,
    )
}

#[cfg(test)]
mod tests {
    use super::decompose;

    /// Applies a decomposed pose to `v`: scale z, rotate (Rodrigues), translate.
    fn apply(t: [f32; 3], axis: [f32; 3], degrees: f32, z: f32, v: [f32; 3]) -> [f32; 3] {
        let v = [v[0], v[1], v[2] * z];
        let (s, c) = degrees.to_radians().sin_cos();
        let k = axis;
        let dot = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
        let cross = [
            k[1] * v[2] - k[2] * v[1],
            k[2] * v[0] - k[0] * v[2],
            k[0] * v[1] - k[1] * v[0],
        ];
        std::array::from_fn(|i| v[i] * c + cross[i] * s + k[i] * dot * (1.0 - c) + t[i])
    }

    #[test]
    fn a_turned_stretched_piece_comes_apart_and_back() {
        // Turned 90° about up (x goes to -z), stretched 3 along z, moved by (1, 2, 3).
        let m = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0, 0.0],
            [1.0, 2.0, 3.0, 1.0],
        ];
        let (t, axis, degrees, z) = decompose(&m);
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let want: [f32; 3] =
                std::array::from_fn(|i| m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2] + m[3][i]);
            let got = apply(t, axis, degrees, z, v);
            for i in 0..3 {
                assert!((want[i] - got[i]).abs() < 1e-4, "{want:?} {got:?}");
            }
        }
    }
}
