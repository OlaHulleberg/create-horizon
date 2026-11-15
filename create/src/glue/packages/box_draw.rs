//! Packagers and postboxes drawn. `PackagerRenderer`: the hatch (open while a package passes)
//! and the tray sliding the package out or the unpacked one in over the packager's `CYCLE`, the
//! package on it. `PostboxRenderer`: the flag, raised (with a wiggle) while the postbox holds
//! anything and lowered when it is empty (`PostboxBlockEntity.flag`, chasing its target at 0.1 a
//! tick), with Create's sounds as it goes up and down.
//!
//! The server puts what clients need in each block entity's data when it changes: a packager's
//! animation start, way and package (`Tray`), a postbox's flag target, its value then and when
//! (`Flag`).

use super::super::server::ServerLevel;
use super::super::states;
use super::{body, with};
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::packager::CYCLE;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost,
    Compound, ItemDisplayContext, SoundSource, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// A packager's animation in its block entity's data: `[start tick, inward, package id]`.
pub const TRAY: &str = "Tray";
/// A postbox's flag in its block entity's data: `[target, value then, tick]`.
pub const FLAG: &str = "Flag";

/// `PostboxBlockEntity.flag`'s chase speed, a tick.
const FLAG_SPEED: f32 = 0.1;

#[derive(Clone, Debug, PartialEq)]
struct Tray {
    start: i64,
    inward: bool,
    item: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Flag {
    target: f32,
    from: f32,
    since: i64,
}

impl Flag {
    /// The flag's value at `t` (ticks with the partial tick).
    fn at(&self, t: f64) -> f32 {
        let moved = (FLAG_SPEED as f64 * (t - self.since as f64).max(0.0)) as f32;
        if self.target > self.from {
            (self.from + moved).min(self.target)
        } else {
            (self.from - moved).max(self.target)
        }
    }

    fn settled(&self, t: f64) -> bool {
        (self.at(t) - self.target).abs() < 1e-4
    }
}

thread_local! {
    static TRAYS: RefCell<HashMap<(usize, [i32; 3]), Tray>> = RefCell::new(HashMap::new());
    static FLAGS: RefCell<HashMap<(usize, [i32; 3]), Flag>> = RefCell::new(HashMap::new());
    /// The package each packager unpacked last (`previouslyUnwrapped`), by level and position.
    static UNWRAPPED: RefCell<HashMap<(usize, [i32; 3]), String>> = RefCell::new(HashMap::new());
    /// Client: the animations drawn to their end, by position (their start tick or flag tick).
    static ENDED: RefCell<HashMap<[i32; 3], i64>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    TRAYS.with_borrow_mut(HashMap::clear);
    FLAGS.with_borrow_mut(HashMap::clear);
    UNWRAPPED.with_borrow_mut(HashMap::clear);
}

pub(super) fn client_reset() {
    ENDED.with_borrow_mut(HashMap::clear);
}

/// The packager at `pos` unpacked `id` (`previouslyUnwrapped = box`).
pub(super) fn unwrapped(dim: usize, pos: [i32; 3], id: String) {
    UNWRAPPED.with_borrow_mut(|u| u.insert((dim, pos), id));
}

fn merge(lv: &mut ServerLevel<'_>, pos: [i32; 3], key: &str, value: Tag) -> bool {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return false;
    };
    let data = Compound::from([(key.into(), value)]);
    entity.merge_data(&mut *lv.host, data).is_ok()
}

/// Each packager's tray and each postbox's flag to its block entity's data, when it changed.
pub(super) fn sync(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let host: &dyn mod_api::Host = &*lv.host;
    let id = |s: &mod_api::ItemStack| s.item().id(host);
    let trays: Vec<([i32; 3], u32, bool, Option<String>)> = with(dim, |pk| {
        pk.packagers
            .iter()
            .map(|(pos, p)| {
                let shown = if p.inward {
                    None
                } else {
                    p.sliding.as_ref().or(p.held.as_ref()).map(|b| id(&b.stack))
                };
                (*pos, p.animation_ticks, p.inward, shown)
            })
            .collect()
    });
    for (pos, ticks, inward, out) in trays {
        let item = if inward {
            UNWRAPPED.with_borrow(|u| u.get(&(dim, pos)).cloned())
        } else {
            out
        }
        .unwrap_or_default();
        // The tick the animation started, `CYCLE` ticks before it ends; one that ended keeps
        // its start.
        let before = TRAYS.with_borrow(|t| t.get(&(dim, pos)).cloned());
        let ended = now - i64::from(CYCLE);
        let start = if ticks > 0 {
            now - i64::from(CYCLE - ticks.min(CYCLE))
        } else {
            before
                .as_ref()
                .filter(|b| b.inward == inward)
                .map_or(ended, |b| b.start.min(ended))
        };
        let tray = Tray {
            start,
            inward,
            item,
        };
        if before.as_ref() == Some(&tray) {
            continue;
        }
        let value = Tag::List(vec![
            Tag::Long(tray.start),
            Tag::Byte(i8::from(tray.inward)),
            Tag::String(tray.item.as_str().into()),
        ]);
        if merge(lv, pos, TRAY, value) {
            TRAYS.with_borrow_mut(|t| t.insert((dim, pos), tray));
        }
    }
    // `PostboxBlockEntity.tick`: a new target once the flag is down or settled.
    let boxes: Vec<([i32; 3], bool)> = with(dim, |pk| {
        pk.ports
            .iter()
            .filter(|(_, p)| !p.frog)
            .map(|(pos, p)| (*pos, p.items.iter().any(Option::is_some)))
            .collect()
    });
    let t = now as f64;
    for (pos, full) in boxes {
        let target = if full { 1.0 } else { 0.0 };
        let before = FLAGS.with_borrow(|f| f.get(&(dim, pos)).copied());
        let flag = before.unwrap_or(Flag {
            target: 0.0,
            from: 0.0,
            since: now,
        });
        // The sound of a flag going down past 0.15 (`settled` turning false).
        if flag.target == 0.0 && flag.at(t - 1.0) > 0.15 && flag.at(t) <= 0.15 {
            sound(
                lv,
                pos,
                "minecraft:block.iron_trapdoor.close",
                0.35 * 0.75,
                0.75 * 1.5,
            );
        }
        let may = flag.target == 0.0 || flag.settled(t);
        if !may || flag.target == target {
            if before.is_none() {
                FLAGS.with_borrow_mut(|f| f.insert((dim, pos), flag));
                send_flag(lv, pos, flag);
            }
            continue;
        }
        let next = Flag {
            target,
            from: flag.at(t),
            since: now,
        };
        if target == 1.0 {
            // `CONTRAPTION_ASSEMBLE` at pitch 2.
            sound(
                lv,
                pos,
                "minecraft:block.wooden_trapdoor.open",
                0.5,
                0.5 * 2.0,
            );
            sound(lv, pos, "minecraft:block.chest.open", 0.045, 0.74 * 2.0);
        }
        FLAGS.with_borrow_mut(|f| f.insert((dim, pos), next));
        send_flag(lv, pos, next);
    }
}

fn send_flag(lv: &mut ServerLevel<'_>, pos: [i32; 3], f: Flag) {
    let value = Tag::List(vec![
        Tag::Float(f.target.to_bits()),
        Tag::Float(f.from.to_bits()),
        Tag::Long(f.since),
    ]);
    merge(lv, pos, FLAG, value);
}

fn sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], id: &str, volume: f32, pitch: f32) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv
        .level
        .play_sound(&mut *lv.host, id, at, SoundSource::Blocks, volume, pitch);
}

fn tray_of(nbt: &Compound) -> Option<Tray> {
    match super::super::kinetics::synced(nbt, TRAY) {
        Some(Tag::List(l)) => match l.as_slice() {
            [Tag::Long(s), Tag::Byte(i), Tag::String(item)] => Some(Tray {
                start: *s,
                inward: *i != 0,
                item: item.to_string_lossy(),
            }),
            _ => None,
        },
        _ => None,
    }
}

fn flag_of(nbt: &Compound) -> Option<Flag> {
    match super::super::kinetics::synced(nbt, FLAG) {
        Some(Tag::List(l)) => match l.as_slice() {
            [Tag::Float(t), Tag::Float(f), Tag::Long(s)] => Some(Flag {
                target: f32::from_bits(*t),
                from: f32::from_bits(*f),
                since: *s,
            }),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a packager's tray or a postbox's flag at `pos` moves now: an animation not yet drawn
/// to its end.
pub(super) fn animated(block: Block, pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
    let key = if matches!(block, Block::Packager | Block::Repackager) {
        tray_of(&entity.nbt).map(|t| t.start)
    } else if create_engine::dye::is_postbox(block) {
        flag_of(&entity.nbt).map(|f| f.since)
    } else {
        None
    };
    key.is_some_and(|k| ENDED.with_borrow(|e| e.get(&pos) != Some(&k)))
}

/// `Direction.toYRot`.
fn y_rot(d: Dir) -> f32 {
    match d {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0,
    }
}

const UP: [f32; 3] = [0.0, 1.0, 0.0];
const EAST: [f32; 3] = [1.0, 0.0, 0.0];
const CENTRE: [f32; 3] = [0.5; 3];

fn part(draw: &mut dyn BlockEntityDrawContext, name: &str) {
    if let Some(p) = states::partial(name) {
        draw.block(p, &[("facing", "up")]);
    }
}

pub(super) struct BoxDraw;

#[mod_api::horizon_part]
impl BlockPart for BoxDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for BoxDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        if matches!(block, Block::Packager | Block::Repackager) {
            draw_packager(host, block, info.facing, event);
        } else if create_engine::dye::is_postbox(block) {
            draw_flag(info.facing, event);
        }
    }
}

/// `PackagerRenderer.renderSafe`.
fn draw_packager(
    host: &dyn ClientHost,
    block: Block,
    facing: Option<Dir>,
    event: &mut BlockEntitySubmitEvent<'_>,
) {
    let pos = event.pos;
    let tray = tray_of(&event.entity.nbt);
    let draw = &mut *event.draw;
    // `FACING.getOpposite()`.
    let facing = facing.unwrap_or(Dir::Down).opposite();
    let t = draw.game_time() as f32 + draw.partial();
    // `animationTicks` (whole) and `animationTicks - partialTicks`.
    let (ticks, cycle, inward) = match &tray {
        Some(tr) => {
            let since = (draw.game_time() - tr.start).max(0);
            let ticks = (i64::from(CYCLE) - since).max(0) as i32;
            let exact = (CYCLE as f32 - (t - tr.start as f32)).max(0.0);
            (ticks, exact, tr.inward)
        }
        None => (0, 0.0, true),
    };
    if ticks == 0
        && let Some(tr) = &tray
    {
        ENDED.with_borrow_mut(|e| e.insert(pos, tr.start));
    }
    // `getTrayOffset`.
    let tick_cycle = if inward { cycle } else { cycle - 5.0 };
    let progress = (tick_cycle / (CYCLE - 5) as f32 * 2.0 - 1.0).clamp(-1.0, 1.0);
    let progress = 1.0 - progress * progress;
    let tray_offset = if ticks == 0 { 0.0 } else { progress * progress };
    // `isHatchOpen`.
    let cycle_i = CYCLE as i32;
    let open = ticks > if inward { 1 } else { 5 } && ticks < cycle_i - if inward { 5 } else { 1 };
    let n = facing.offset().map(|v| v as f32);
    let horizontal = create_engine::decor::horizontal_angle(facing);
    let vertical = match facing {
        Dir::Up => -90.0,
        Dir::Down => 90.0,
        _ => 0.0,
    };
    draw.push();
    draw.translate(n.map(|v| v * 0.49999));
    draw.rotate_about(UP, horizontal, CENTRE);
    draw.rotate_about(EAST, vertical, CENTRE);
    part(
        draw,
        if open {
            "part_packager_hatch_open"
        } else {
            "part_packager_hatch_closed"
        },
    );
    draw.pop();
    draw.push();
    draw.translate(n.map(|v| v * tray_offset));
    draw.rotate_about(UP, y_rot(facing), CENTRE);
    part(
        draw,
        if block == Block::Packager {
            "part_packager_tray"
        } else {
            "part_repackager_tray"
        },
    );
    draw.pop();
    // `getRenderedBox`.
    let shown = if inward {
        ticks > cycle_i / 2
    } else {
        ticks < cycle_i / 2
    };
    if shown
        && let Some(tr) = &tray
        && !tr.item.is_empty()
        && let Some(stack) = body::new_stack(host, &tr.item, 1)
    {
        draw.push();
        draw.translate(n.map(|v| v * tray_offset));
        draw.translate(CENTRE);
        draw.rotate_about(UP, y_rot(facing), [0.0; 3]);
        draw.translate([0.0, 2.0 / 16.0, 0.0]);
        draw.scale([1.49; 3]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
}

/// `PostboxRenderer.renderSafe`: the flag turned to the postbox's facing and raised by
/// `transformFlag`.
fn draw_flag(facing: Option<Dir>, event: &mut BlockEntitySubmitEvent<'_>) {
    let pos = event.pos;
    let flag = flag_of(&event.entity.nbt);
    let draw = &mut *event.draw;
    let t = draw.game_time() as f64 + f64::from(draw.partial());
    let (value, moving) = match flag {
        Some(f) => (f.at(t), !f.settled(t)),
        None => (0.0, false),
    };
    if !moving && let Some(f) = flag {
        ENDED.with_borrow_mut(|e| e.insert(pos, f.since));
    }
    let mut progress = (value * 5.0).min(1.0).powi(2);
    if flag.is_some_and(|f| f.target > 0.0) && moving && progress == 1.0 {
        let wiggle = (value - 0.2) / 0.8;
        progress += ((wiggle * std::f32::consts::TAU * 4.0).sin() / 8.0) / (8.0 * wiggle).max(1.0);
    }
    let facing = facing.unwrap_or(Dir::North);
    let Some(p) = states::partial("part_postbox_flag") else {
        return;
    };
    draw.push();
    draw.rotate_about(UP, 180.0 - y_rot(facing), CENTRE);
    draw.rotate_about(EAST, -progress * 90.0, [0.0, 10.0 / 16.0, 2.0 / 16.0]);
    draw.block(p, &[("facing", "up")]);
    draw.pop();
}
