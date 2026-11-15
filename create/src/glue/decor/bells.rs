//! Peculiar and haunted bells ringing (`AbstractBellBlock`, `AbstractBellBlockEntity`,
//! `BellRenderer`). They are declared on the bell's template, whose own ringing looks for a
//! vanilla bell's block entity and so never rings; here a click on the bell's side (any item,
//! `canRingFrom`) or a rising redstone signal (the template keeps `powered`) rings it: Create's
//! toll (`PECULIAR_BELL_USE`, `HAUNTED_BELL_USE` at volume 4), and the bell body (Create's
//! partial, as the template draws none) swings from the struck side for `RING_DURATION` ticks.
//! A haunted bell rings again only after `RECHARGE_TICKS`.
//!
//! The server puts each ring's tick and side in the bell's block entity data (`Ringing`).
//!
//! A rung haunted bell's soul pulse and souls are `souls`'.

use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockClick, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost, Compound,
    InteractionResult, Registrar, ServerHost, Tag, UseItemOnEvent, UseWithoutItemEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The last ring in a bell's block entity data: `[game tick, side]` (`Direction` index).
pub const RINGING: &str = "Ringing";

/// `AbstractBellBlockEntity.RING_DURATION`.
const RING_DURATION: i64 = 74;
/// `HauntedBellBlockEntity.RECHARGE_TICKS`.
const RECHARGE_TICKS: i64 = 65;

thread_local! {
    /// Bells powered this tick (`neighborChanged` setting `powered`), by level, to ring after it.
    static POWERED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    /// Client: each bell's ring drawn to its end, by position.
    static DONE: RefCell<HashMap<[i32; 3], i64>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    POWERED.with_borrow_mut(Vec::clear);
}

pub(super) fn client_forget() {
    DONE.with_borrow_mut(HashMap::clear);
}

pub fn is_bell(b: Block) -> bool {
    matches!(b, Block::PeculiarBell | Block::HauntedBell)
}

pub(super) fn register(r: &mut dyn Registrar) {
    for b in [Block::PeculiarBell, Block::HauntedBell] {
        r.block_part(states::key(b), BellBlock);
    }
}

/// A bell's state changed: one newly powered rings after the tick.
pub(super) fn changed(dim: usize, pos: [i32; 3], was_powered: Option<bool>, powered: bool) {
    if powered && was_powered == Some(false) {
        POWERED.with_borrow_mut(|p| p.push((dim, pos)));
    }
}

/// The bells powered in this level's tick ring (`neighborChanged`): from the side they face
/// on the floor or ceiling, else from the one clockwise of it.
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due: Vec<[i32; 3]> = POWERED.with_borrow_mut(|p| {
        let mine = p.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
        p.retain(|e| e.0 != dim);
        mine
    });
    for pos in due {
        let Some((facing, attachment)) = bell_state(&*lv.host, lv.block_state(pos)) else {
            continue;
        };
        let side = if matches!(attachment.as_str(), "ceiling" | "floor") {
            facing
        } else {
            clockwise(facing)
        };
        ring(lv, pos, side);
    }
}

/// `Direction.getClockWise` about up.
fn clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::East,
        Dir::East => Dir::South,
        Dir::South => Dir::West,
        Dir::West => Dir::North,
        other => other,
    }
}

/// A bell state's facing and attachment.
fn bell_state(
    host: &dyn mod_api::Host,
    state: Option<mod_api::BlockState>,
) -> Option<(Dir, String)> {
    let text = host.block_state_string(state?)?;
    let (_, props) = states::split(&text);
    let get = |k: &str| props.iter().find(|(p, _)| *p == k).map(|(_, v)| *v);
    let facing = match get("facing")? {
        "north" => Dir::North,
        "south" => Dir::South,
        "west" => Dir::West,
        _ => Dir::East,
    };
    Some((facing, get("attachment").unwrap_or("floor").to_owned()))
}

fn ring_of(nbt: &Compound) -> Option<(i64, Dir)> {
    match nbt.get(&RINGING.into()) {
        Some(Tag::List(l)) => match l.as_slice() {
            [Tag::Long(t), Tag::Byte(d)] => Some((*t, *Dir::ALL.get(*d as usize)?)),
            _ => None,
        },
        _ => None,
    }
}

/// `AbstractBellBlock.ring`: the bell's block entity rings (a haunted one not while
/// recharging) and its toll plays.
fn ring(lv: &mut ServerLevel<'_>, pos: [i32; 3], side: Dir) -> bool {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return false;
    };
    let haunted = lv.block_name(pos).as_deref() == Some("create:haunted_bell");
    let now = lv.game_time();
    if haunted
        && let Some((t, _)) = entity.data(&*lv.host).as_ref().and_then(ring_of)
        && now - t < RECHARGE_TICKS
    {
        return false;
    }
    let data = Compound::from([(
        RINGING.into(),
        Tag::List(vec![Tag::Long(now), Tag::Byte(side as i8)]),
    )]);
    let _ = entity.merge_data(&mut *lv.host, data);
    if haunted {
        super::souls::rung(lv.dim, pos);
        lv.block_sound("create:haunted_bell_use", pos, 4.0, 1.0);
    } else {
        lv.block_sound("create:peculiar_bell_use", pos, 1.0, 1.0);
    }
    true
}

/// `AbstractBellBlock.onHit` from a click: on a side the bell hangs to be struck from, below its
/// top (`canRingFrom`).
fn clicked(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let side = Dir::from_offset(click.hit.face)?;
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let (facing, attachment) = bell_state(&*lv.host, lv.block_state(click.pos))?;
    let height = click.hit.point[1] - f64::from(click.pos[1]);
    let can = side.axis() != create_engine::kinetics::Axis::Y
        && height <= 0.8124
        && match attachment.as_str() {
            "floor" | "ceiling" => facing.axis() == side.axis(),
            _ => facing.axis() != side.axis(),
        };
    if !can {
        return None;
    }
    // `HauntedBellBlock.ring`: the player who rang a haunted bell.
    if ring(&mut lv, click.pos, side)
        && lv.block_name(click.pos).as_deref() == Some("create:haunted_bell")
        && let Some(player) = lv.host.player(click.player)
    {
        super::super::advancements::award(lv.host, player, "haunted_bell");
    }
    Some(InteractionResult::Success)
}

struct BellBlock;

#[mod_api::horizon_part]
impl BlockPart for BellBlock {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        clicked(host, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        clicked(host, event.click)
    }
}

/// Whether the bell at `pos` swings now.
pub(super) fn animated(pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
    ring_of(&entity.nbt).is_some_and(|(t, _)| DONE.with_borrow(|d| d.get(&pos) != Some(&t)))
}

/// `BellRenderer.getSwingAngle`, radians.
fn swing_angle(time: f32) -> f32 {
    let t = time / 1.5;
    1.2 * (t / std::f32::consts::PI).sin() / (2.5 + t / 3.0)
}

pub(super) struct BellDraw;

#[mod_api::horizon_part]
impl BlockPart for BellDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for BellDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let name = match event.block {
            "create:peculiar_bell" => "part_peculiar_bell",
            "create:haunted_bell" => "part_haunted_bell",
            _ => return,
        };
        let Some(part) = states::partial(name) else {
            return;
        };
        let Some((facing, attachment)) = bell_state(host, Some(event.state)) else {
            return;
        };
        let draw = &mut *event.draw;
        let pos = event.pos;
        draw.push();
        if let Some((t, side)) = ring_of(&event.entity.nbt) {
            let ticks = (draw.game_time() - t) as f32 + draw.partial();
            if ticks >= 0.0 && ticks < RING_DURATION as f32 {
                // `rotateCentered(angle, ringDirection.getCounterClockWise())`.
                let axis = clockwise(clockwise(clockwise(side)))
                    .offset()
                    .map(|v| v as f32);
                draw.rotate_about(axis, swing_angle(ticks).to_degrees(), [0.5; 3]);
            } else {
                DONE.with_borrow_mut(|d| d.insert(pos, t));
            }
        }
        let mut y = create_engine::decor::horizontal_angle(facing);
        if matches!(attachment.as_str(), "single_wall" | "double_wall") {
            y += 90.0;
        }
        draw.rotate_about([0.0, 1.0, 0.0], y, [0.5; 3]);
        draw.block(part, &[("facing", "up")]);
        draw.pop();
    }
}
