//! What a client draws of the machines beyond their turning parts (`kinetics`): the press's
//! head on its stroke (`MechanicalPressRenderer`), the deployer's pole and hand
//! (`DeployerRenderer`), the mixer's pole and head (`MechanicalMixerRenderer`).
//!
//! The press's cycle and the deployer's stroke come from the server when they change
//! (`create:press`, `create:deployer`); the client runs them on between messages, as Create's
//! does, and draws them each frame. The mixer's pole and head stand at their resting height
//! (`getRenderedHeadOffset` is 7/16 when not running), the head turning with the mixer.

use crate::glue::kinetics::{self, dir_name};
use crate::glue::states;
use create_engine::client::{self as parts, Model};
use create_engine::kinetics::Dir;
use create_engine::{Block, deployer, press};
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockKey, BlockPart,
    ClientHost, Registrar,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

thread_local! {
    /// Running presses: their cycle, run on from the server's last word, and whether the head
    /// waits at its lowest point for the server to say it pressed.
    static PRESSES: RefCell<HashMap<[i32; 3], (press::Press, bool)>> = RefCell::new(HashMap::new());
    /// Each deployer's stroke as last sent, and the client tick it came at.
    static DEPLOYERS: RefCell<HashMap<[i32; 3], (deployer::Shown, u64)>> = RefCell::new(HashMap::new());
    /// This client's ticks, for the strokes' glide between messages.
    static TICKS: Cell<u64> = const { Cell::new(0) };
}

/// The partial models drawn here: blocks of the content (`create:part_*`, one model per
/// facing) that are never placed, so not among the engine's `Block`s.
#[derive(Clone, Copy)]
enum Partial {
    PressHead,
    DeployerPole,
    DeployerHandHolding,
    DeployerHandPointing,
    MixerPole,
    MixerHead,
}

const PARTIALS: [&str; 6] = [
    "create:part_press_head",
    "create:part_deployer_pole",
    "create:part_deployer_hand_holding",
    "create:part_deployer_hand_pointing",
    "create:part_mixer_pole",
    "create:part_mixer_head",
];

/// Their keys, by `Partial as usize`, requested at registration.
static PARTIAL_KEYS: OnceLock<[BlockKey; 6]> = OnceLock::new();

/// Attaches the drawing part to the kinetic block entity type.
pub fn register(r: &mut dyn Registrar) {
    let _ = PARTIAL_KEYS.set(PARTIALS.map(|name| r.block_key(name)));
    let kind = r.block_entity_type_key(kinetics::TYPE);
    if let Err(e) = r.block_entity_part(&kind, MachineDraw) {
        eprintln!("[create] the machines' drawing part was refused: {e:?}");
    }
}

/// Whether the client draws `block` moving each frame: its block entity's drawing is not kept.
pub fn moving(block: Block) -> bool {
    matches!(block, Block::MechanicalPress | Block::Deployer)
}

/// The server's word on presses or deployers. True for those channels.
pub fn server_message(_host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel == press::CHANNEL {
        presses_from_server(data);
        return true;
    }
    if channel == deployer::CHANNEL {
        let now = TICKS.get();
        DEPLOYERS.with_borrow_mut(|d| {
            for s in deployer::decode(data) {
                if s.at.is_nan() {
                    d.remove(&s.pos);
                } else {
                    d.insert(s.pos, (s, now));
                }
            }
        });
        return true;
    }
    false
}

/// The client left its world.
pub fn reset() {
    PRESSES.with_borrow_mut(HashMap::clear);
    DEPLOYERS.with_borrow_mut(HashMap::clear);
}

fn presses_from_server(data: &[u8]) {
    for (pos, p) in press::decode(data) {
        // At the lowest point the server has pressed: the head goes on up.
        let waiting = p.running && p.ticks < press::CYCLE / 2;
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.insert(pos, (p, waiting));
            } else {
                c.remove(&pos);
            }
        });
    }
}

/// The client's next cycle tick for a press at `speed`: `PressingBehaviour.tick` on the client,
/// which waits at the lowest point for the server.
fn client_press_step(p: &press::Press, speed: f32, waiting: bool) -> i32 {
    use press::CYCLE;
    if waiting && p.ticks >= CYCLE / 2 {
        return CYCLE / 2;
    }
    let next = p.ticks + press::tick_speed(speed);
    if p.ticks < CYCLE / 2 && next >= CYCLE / 2 {
        return CYCLE / 2;
    }
    next
}

/// Each client tick: running presses move on a cycle step.
pub fn tick(host: &mut dyn ClientHost) {
    TICKS.set(TICKS.get() + 1);
    let running: Vec<([i32; 3], press::Press, bool)> =
        PRESSES.with_borrow(|c| c.iter().map(|(k, v)| (*k, v.0, v.1)).collect());
    for (pos, mut p, waiting) in running {
        let speed = press_speed(host, pos);
        p.ticks = client_press_step(&p, speed, waiting);
        if p.ticks > press::CYCLE {
            p.running = false;
        }
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.insert(pos, (p, waiting));
            } else {
                c.remove(&pos);
            }
        });
    }
}

/// The press's shaft speed, as its block entity shows it.
fn press_speed(host: &dyn ClientHost, pos: [i32; 3]) -> f32 {
    let Some(level) = host.local_player().and_then(|p| p.level(host)) else {
        return 0.0;
    };
    host.block_entity(level, pos)
        .and_then(|e| e.data(host))
        .map_or(0.0, |d| kinetics::speeds_of(&d)[0])
}

/// Draws the press head, the deployer's pole and hand, the mixer's pole and head.
struct MachineDraw;

#[mod_api::horizon_part]
impl BlockPart for MachineDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for MachineDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            crate::glue::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        match block {
            Block::MechanicalPress => {
                if let Some(f) = info.facing {
                    let speed = kinetics::speeds_of(&event.entity.nbt)[0];
                    draw_press_head(event.draw, event.pos, f, speed);
                }
            }
            Block::Deployer => draw_deployer(event.draw, event.pos),
            Block::MechanicalMixer => {
                let speeds = kinetics::speeds_of(&event.entity.nbt);
                for d in parts::draws(block, &info, event.pos, speeds) {
                    if matches!(d.model, Model::MixerPole | Model::MixerHead) {
                        draw_mixer_part(event.draw, &d);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A partial model of the content (`create:part_*`, one block per facing), turned to `facing`.
fn draw_partial(draw: &mut dyn BlockEntityDrawContext, partial: Partial, facing: &str) {
    if let Some(keys) = PARTIAL_KEYS.get() {
        draw.block(keys[partial as usize], &[("facing", facing)]);
    }
}

/// The press head where its cycle has it, gliding over the frame toward the next tick's place.
fn draw_press_head(draw: &mut dyn BlockEntityDrawContext, pos: [i32; 3], facing: Dir, speed: f32) {
    let offset = PRESSES.with_borrow(|c| {
        let &(p, waiting) = c.get(&pos)?;
        Some((p, waiting))
    });
    let y = match offset {
        Some((p, waiting)) if p.running => {
            let depth = p.mode.head_offset();
            let step = client_press_step(&p, speed, waiting);
            let now = press::head_offset(p.ticks as f32) * depth;
            let to = press::head_offset(step as f32) * depth;
            now + (to - now) * draw.partial()
        }
        _ => 0.0,
    };
    draw.push();
    draw.translate([0.0, -y, 0.0]);
    draw_partial(draw, Partial::PressHead, dir_name(facing));
    draw.pop();
}

/// The deployer's pole and hand (pointing, or holding) at the hand's offset along its facing,
/// moving on at the speed the server gave.
fn draw_deployer(draw: &mut dyn BlockEntityDrawContext, pos: [i32; 3]) {
    let Some((s, at_tick)) = DEPLOYERS.with_borrow(|d| d.get(&pos).cloned()) else {
        return;
    };
    let elapsed = (TICKS.get().saturating_sub(at_tick)) as f32 + draw.partial();
    let at = s.at + s.per_tick * elapsed;
    let o = s.facing.offset().map(|v| v as f32);
    let hand = if s.held.is_some() {
        Partial::DeployerHandHolding
    } else {
        Partial::DeployerHandPointing
    };
    draw.push();
    draw.translate([o[0] * at, o[1] * at, o[2] * at]);
    draw_partial(draw, Partial::DeployerPole, dir_name(s.facing));
    draw_partial(draw, hand, dir_name(s.facing));
    draw.pop();
}

/// The mixer's pole or head at rest, 7/16 down; the head turns with the mixer.
fn draw_mixer_part(draw: &mut dyn BlockEntityDrawContext, d: &parts::Draw) {
    let partial = if d.model == Model::MixerPole {
        Partial::MixerPole
    } else {
        Partial::MixerHead
    };
    draw.push();
    draw.translate([0.0, -7.0 / 16.0, 0.0]);
    if d.rpm != 0.0 {
        draw.spin([0.0, 1.0, 0.0], [0.5; 3], d.rpm * 0.3, d.phase);
    }
    draw_partial(draw, partial, "up");
    draw.still();
    draw.pop();
}
