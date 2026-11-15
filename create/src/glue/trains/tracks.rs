//! Track: the track item's placement (`TrackBlockItem`, `TrackPlacement`), the curves it lays
//! and the fake track above them (`manageFakeTracksAlong`), and what clients draw of the
//! railway on its own blocks: each track block's curves (`TrackRenderer.renderBezierTurn`) and
//! each signal's light (`SignalRenderer`), carried in the block entity's data.

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::{level_index, with_world};
use super::saved;
use crate::glue::items;
use create_engine::Block;
use create_engine::track::{self, End, Failure, Request, TrackShape};
use create_engine::track_curves::Saved;
use create_engine::track_render::{Mat4, Piece, segment_parts};
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
const CURVES: &str = "Curves";
/// The key in a signal's block entity data: 1 while it shows red.
const LIGHT: &str = "Light";

thread_local! {
    /// The track each player picked first with the track item (`TRACK_CONNECTING_FROM`, kept
    /// in Create on the item; here per player until used or cleared), with its level.
    static TRACK_FROM: RefCell<HashMap<u128, (usize, End)>> = RefCell::new(HashMap::new());
    /// Track and signal block entities that came into a level, by level: their data is set
    /// from the railway at the level's next tick.
    static LOADED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// The world closed.
pub fn forget() {
    TRACK_FROM.with_borrow_mut(HashMap::clear);
    LOADED.with_borrow_mut(Vec::clear);
}

pub fn register(r: &mut dyn Registrar) {
    let item = r.item_key("create:track");
    r.item_part(item, TrackItem);
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
        if !pay_tracks(lv, player, need) {
            return Err(Some(Failure::NotEnoughTracks));
        }
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
    };
    let dim = lv.dim;
    with_world(|w| {
        w.dims[dim].track_curves.add(saved);
        w.mark_unsaved(dim, saved.from);
        w.mark_unsaved(dim, saved.to);
    });
    Ok(info.message)
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

/// A signal's light into its block entity's data: red or not (`SignalRenderer`), and its whole
/// state (`SIGNAL_STATE`, `SignalState`'s ordinal) for a nixie tube in front of it.
fn set_light(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: SignalState) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let data = Compound::from([
        (LIGHT.into(), Tag::Byte(i8::from(state.red_light()))),
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
        for key in [CURVES, LIGHT, SIGNAL_STATE, super::station_draw::KEY] {
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
        if block != states::key(Block::Track) && block != states::key(Block::TrackSignal) {
            return;
        }
        let nbt = &event.entity.nbt;
        if let Some(Tag::Byte(red)) = nbt.get(&LIGHT.into()) {
            // `SignalRenderer`: Create's `indicator_on` partial while red, `indicator_off` else.
            let name = if *red != 0 {
                "part_signal_indicator_on"
            } else {
                "part_signal_indicator_off"
            };
            if let Some(b) = states::partial(name) {
                event.draw.block(b, &[("facing", "up")]);
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
        for s in &curves {
            for (piece, m) in segment_parts(&s.curve(), event.pos) {
                let key = match piece {
                    Piece::Tie => tie,
                    Piece::RailLeft => left,
                    Piece::RailRight => right,
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
