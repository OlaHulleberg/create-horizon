//! What a client draws of the machines beyond their turning parts (`kinetics`): the press's
//! head on its stroke (`MechanicalPressRenderer`), the deployer's pole, hand and held item
//! (`DeployerRenderer`), the mixer's pole and head (`MechanicalMixerRenderer`), a basin's
//! contents (`BasinRenderer`), the saw's blade (`SawRenderer.renderBlade`) and the mechanical
//! crafter's cog, lid, belts or arrow and slot item (`MechanicalCrafterRenderer`).
//!
//! The press's cycle and the deployer's stroke come from the server when they change
//! (`create:press`, `create:deployer`); the client runs them on between messages, as Create's
//! does, and draws them each frame. The mixer's head goes down and up with its cycle
//! (`getRenderedHeadOffset`), which the server keeps in its block entity's data, the head turning
//! with the mixer. A basin's fluids and items and a crafter's phase and item come with their
//! block entity's data too (a client's block entity has no slots): a change of it draws them
//! again.

use super::basin as basin_glue;
use super::stacks;
use crate::glue::kinetics::{self, dir_name};
use crate::glue::states;
use create_engine::client::{self as parts, Model};
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use create_engine::tank::Fluid;
use create_engine::{Block, deployer, press};
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockKey, BlockPart,
    ClientHost, Compound, ItemDisplayContext, ModBlockEntity, Registrar, Tag,
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
    /// Each basin's ingredient swirl: angle, speed, and the client tick they are at.
    static SPINS: RefCell<HashMap<[i32; 3], (f32, f32, u64)>> = RefCell::new(HashMap::new());
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
        crate::create_log!("[create] the machines' drawing part was refused: {e:?}");
    }
}

/// Whether the client draws `block` moving each frame: its block entity's drawing is not kept.
/// The press and deployer always; a mixer while it runs; a basin whose items bob on its fluid.
pub fn animated(block: Block, entity: &ModBlockEntity) -> bool {
    match block {
        Block::MechanicalPress | Block::Deployer | Block::BlazeBurner => true,
        // A crafter's grid moves while it exports or crafts.
        Block::MechanicalCrafter => matches!(
            entity.nbt.get(&super::crafter::CRAFTER_KEY.into()),
            Some(Tag::Compound(c)) if matches!(c.get(&"P".into()), Some(Tag::Int(3 | 5)))
        ),
        Block::MechanicalSaw => matches!(
            entity.nbt.get(&super::saw::SAW_KEY.into()),
            Some(Tag::Compound(c)) if c.contains_key(&"Items".into())
        ),
        Block::MechanicalMixer => matches!(
            entity.nbt.get(&basin_glue::MIXER_KEY.into()),
            Some(Tag::Compound(c)) if c.contains_key(&"T".into())
        ),
        Block::Basin => {
            // Items bob on fluid or swirl under a mixer.
            !basin_items(&entity.nbt).is_empty()
                || !super::spoutput::visualized(&entity.nbt).is_empty()
        }
        _ => false,
    }
}

/// A basin's stacks its block entity's data holds (`ITEMS_KEY`), in slot order.
fn basin_items(nbt: &Compound) -> Vec<(String, u32)> {
    basin_items_of(nbt.get(&basin_glue::ITEMS_KEY.into()))
}

/// The stacks (`id`, `count`) of a list of them.
fn basin_items_of(tag: Option<&Tag>) -> Vec<(String, u32)> {
    let Some(Tag::List(list)) = tag else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else {
                return None;
            };
            let Some(Tag::String(id)) = c.get(&"id".into()) else {
                return None;
            };
            let Some(Tag::Int(n)) = c.get(&"count".into()) else {
                return None;
            };
            Some((id.to_string().ok()?, (*n).max(0) as u32))
        })
        .collect()
}

/// A basin's tanks its block entity's data holds: (fluid, amount) of the inputs, then outputs.
fn basin_fluids(nbt: &Compound) -> Vec<(Fluid, u32)> {
    let Some(Tag::IntArray(v)) = nbt.get(&basin_glue::FLUIDS_KEY.into()) else {
        return Vec::new();
    };
    let keys = crate::glue::fluids::fluid_keys(nbt, basin_glue::FLUIDS_KEY);
    v.as_chunks::<2>()
        .0
        .iter()
        .filter_map(|c| {
            let fluid = create_engine::tank::Codes::fluid(&keys, c[0])?;
            (c[1] > 0).then_some((fluid, c[1] as u32))
        })
        .collect()
}

/// `BasinBlockEntity.ingredientRotation`: the items in a basin swirl while its mixer works the
/// bottom (`setAreFluidsMoving`: running, at most 20 running ticks), their speed chasing 20°
/// (or 0) a tick by a tenth of the gap each tick; this frame's angle.
fn ingredient_rotation(
    host: &dyn ClientHost,
    draw: &dyn BlockEntityDrawContext,
    pos: [i32; 3],
) -> f32 {
    let mixer = [pos[0], pos[1] + 2, pos[2]];
    let moving = host
        .local_player()
        .and_then(|p| p.level(host))
        .and_then(|l| host.block_entity(l, mixer))
        .and_then(|e| e.data(host))
        .is_some_and(|d| match d.get(&basin_glue::MIXER_KEY.into()) {
            Some(Tag::Compound(c)) => match (c.get(&"T".into()), c.get(&"S".into())) {
                (Some(Tag::Int(t)), Some(Tag::Long(at))) => {
                    create_engine::basin::shown_ticks(*t, draw.game_time() - at) <= 20
                }
                _ => false,
            },
            _ => false,
        });
    let target = if moving { 20.0 } else { 0.0 };
    let now = TICKS.get();
    SPINS.with_borrow_mut(|s| {
        let e = s.entry(pos).or_insert((0.0, 0.0, now));
        let steps = now.saturating_sub(e.2).min(200);
        for _ in 0..steps {
            e.1 += (target - e.1) * 0.1;
            e.0 = (e.0 + e.1) % 360.0;
        }
        e.2 = now;
        e.0 + e.1 * draw.partial()
    })
}

/// `BasinRenderer`: the fluids side by side across the basin, as high as they fill it, and the
/// items in a ring on the bottom or bobbing on the fluid, a stack's every eight one more.
fn draw_basin(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    nbt: &Compound,
) {
    // `renderFluids`.
    let fluids = basin_fluids(nbt);
    let total: u32 = fluids.iter().map(|f| f.1).sum();
    let mut fluid_top = 0.0f32;
    if total >= 1 {
        let level = (total as f32 / 2000.0).clamp(0.0, 1.0);
        let level = 1.0 - (1.0 - level) * (1.0 - level);
        let (y_min, z_min, z_max) = (2.0 / 16.0, 2.0 / 16.0, 14.0 / 16.0);
        let y_max = y_min + 12.0 / 16.0 * level;
        let mut x_min = 2.0 / 16.0;
        for (fluid, units) in &fluids {
            let partial = (*units as f32 / total as f32).clamp(0.0, 1.0);
            let x_max = x_min + partial * 12.0 / 16.0;
            crate::glue::fluids::client::fluid_box(
                host,
                draw,
                *fluid,
                *units,
                [x_min, y_min, z_min],
                [x_max, y_max, z_max],
            );
            x_min = x_max;
        }
        fluid_top = y_max;
    }
    let rotation = ingredient_rotation(host, draw, pos);
    draw_basin_items(host, draw, pos, nbt, fluid_top, rotation);
}

/// `BasinRenderer`'s items: in a ring on the bottom or bobbing on the fluid, turned by
/// `rotation` (`ingredientRotation`), a stack's every eight one more.
fn draw_basin_items(
    host: &dyn mod_api::Host,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    nbt: &Compound,
    fluid_top: f32,
    rotation: f32,
) {
    let items = basin_items(nbt);
    if items.is_empty() {
        return;
    }
    let level = (fluid_top - 0.3).clamp(0.125, 0.6);
    // `RandomSource.create(pos.hashCode())`.
    let hash = (pos[1].wrapping_add(pos[2].wrapping_mul(31)))
        .wrapping_mul(31)
        .wrapping_add(pos[0]);
    let mut r = create_engine::depot::JavaRandom::new(i64::from(hash));
    let mut count = items.len();
    let base_x = if count == 1 { 0.0 } else { 0.125 };
    let partition = 360.0 / count as f32;
    let time = draw.game_time() as f32 + draw.partial();
    draw.push();
    draw.translate([0.5, 0.2, 0.5]);
    draw.rotate_about([0.0, 1.0, 0.0], rotation, [0.0; 3]);
    for (id, n) in items {
        let Some(stack) = stacks::stack(host, &id, 1) else {
            count -= 1;
            continue;
        };
        let angle = partition * count as f32;
        draw.push();
        if fluid_top > 0.0 {
            let bob = ((time / 12.0 + angle).sin() + 1.5) / 32.0;
            draw.translate([0.0, bob, 0.0]);
        }
        // `VecHelper.rotate(baseVector, angle, Y)`.
        let (s, c) = angle.to_radians().sin_cos();
        draw.translate([base_x * c, level, -base_x * s]);
        draw.rotate_about([0.0, 1.0, 0.0], angle + 35.0, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], 65.0, [0.0; 3]);
        for _ in 0..=n / 8 {
            let mut o = || (r.next_float() - 0.5) * 2.0 / 16.0;
            let v = [o(), o(), o()];
            draw.push();
            draw.translate(v);
            draw.item(&stack, ItemDisplayContext::Ground, None, 0);
            draw.pop();
        }
        draw.pop();
        count -= 1;
    }
    draw.pop();
}

/// `BasinRenderer`'s `visualizedOutputItems`: the items leaving a basin's spout, falling out of
/// it over their moment (only the first third of it unless another basin is there).
fn draw_spout_items(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    facing: Dir,
    nbt: &Compound,
) {
    if facing == Dir::Down || facing == Dir::Up {
        return;
    }
    let seen = super::spoutput::visualized(nbt);
    if seen.is_empty() {
        return;
    }
    let o = facing.offset().map(|v| v as f32);
    let out = [0.5 + o[0] * 0.55, 0.0, 0.5 + o[2] * 0.55];
    let next = [pos[0] + o[0] as i32, pos[1], pos[2] + o[2] as i32];
    let out_to_basin = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, next))
        .and_then(states::id)
        .and_then(|id| crate::glue::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten())
        == Some(Block::Basin);
    let time = draw.game_time() as f32 + draw.partial();
    let span = super::spoutput::OUTPUT_ANIMATION_TIME as f32;
    for (id, at) in seen {
        let progress = (time - at as f32) / span;
        if !(0.0..=1.0).contains(&progress) || (!out_to_basin && progress > 0.35) {
            continue;
        }
        let Some(stack) = stacks::stack(host, &id, 1) else {
            continue;
        };
        draw.push();
        draw.translate(out);
        draw.translate([0.0, (-(progress * progress * 2.0)).max(-0.55), 0.0]);
        draw.translate([o[0] * progress * 0.5, 0.0, o[2] * progress * 0.5]);
        draw.rotate_about([0.0, 1.0, 0.0], horizontal_angle(facing), [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], progress * 180.0, [0.0; 3]);
        draw.item(&stack, ItemDisplayContext::Ground, None, 0);
        draw.pop();
    }
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
    super::burner_draw::reset();
    PRESSES.with_borrow_mut(HashMap::clear);
    DEPLOYERS.with_borrow_mut(HashMap::clear);
    SPINS.with_borrow_mut(HashMap::clear);
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
    super::burner_draw::tick(&*host);
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

/// `BlazeBurnerBlockEntity.getStockTicker`: a stock ticker on one of the burner's sides makes
/// it a stock keeper.
fn beside_ticker(host: &dyn ClientHost, pos: [i32; 3]) -> bool {
    let Some(level) = host.level(host.dimension()) else {
        return false;
    };
    [[1, 0], [-1, 0], [0, 1], [0, -1]].iter().any(|[dx, dz]| {
        level
            .block_state(host, [pos[0] + dx, pos[1], pos[2] + dz])
            .and_then(states::id)
            .and_then(|id| crate::glue::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten())
            == Some(Block::StockTicker)
    })
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
            Block::Deployer => {
                let rpm = kinetics::speeds_of(&event.entity.nbt)[0];
                draw_deployer(host, event.draw, event.pos, rpm);
            }
            Block::MechanicalMixer => {
                let speeds = kinetics::speeds_of(&event.entity.nbt);
                let offset = mixer_head_offset(&event.entity.nbt, &*event.draw);
                for d in parts::draws(block, &info, event.pos, speeds) {
                    if matches!(d.model, Model::MixerPole | Model::MixerHead) {
                        draw_mixer_part(event.draw, &d, offset);
                    }
                }
            }
            Block::Basin => {
                draw_basin(host, event.draw, event.pos, &event.entity.nbt);
                let facing = info.facing.unwrap_or(Dir::Down);
                draw_spout_items(host, event.draw, event.pos, facing, &event.entity.nbt);
            }
            Block::BlazeBurner => super::burner_draw::draw(
                event.draw,
                event.pos,
                info.heat.unwrap_or(create_engine::fan::Heat::None),
                info.facing.unwrap_or(Dir::South),
                event.entity.nbt.get(&basin_glue::GOGGLES_KEY.into()) == Some(&Tag::Byte(1)),
                beside_ticker(host, event.pos)
                    || event
                        .entity
                        .nbt
                        .get(&crate::glue::stock::keeper::KEEPER_KEY.into())
                        == Some(&Tag::Byte(1)),
            ),
            Block::MechanicalCrafter => {
                draw_crafter(host, event.draw, event.pos, &event.entity.nbt);
            }
            Block::MechanicalSaw => {
                let speed = kinetics::speeds_of(&event.entity.nbt)[0];
                draw_saw_blade(event.draw, &info, speed);
                draw_saw_items(host, event.draw, &info, speed, &event.entity.nbt);
            }
            _ => {}
        }
        if matches!(block, Block::Basin | Block::MechanicalSaw | Block::Deployer) {
            draw_machine_filter(
                host,
                event.draw,
                block,
                &info,
                event.state,
                &event.entity.nbt,
            );
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
/// moving on at the speed the server gave, and what it holds (`DeployerRenderer.renderItem`).
fn draw_deployer(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    rpm: f32,
) {
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
    // `getHandPose`: a fist in punch mode.
    match states::partial("part_deployer_hand_punching").filter(|_| s.punch) {
        Some(fist) => {
            draw.block(fist, &[("facing", dir_name(s.facing))]);
        }
        None => draw_partial(draw, hand, dir_name(s.facing)),
    }
    draw.pop();
    let Some(stack) = s.held.as_deref().and_then(|id| stacks::stack(host, id, 1)) else {
        return;
    };
    // `AngleHelper.horizontalAngle(facing) + 180`; up and down tip the item over x.
    let horizontal = match s.facing {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        Dir::Up | Dir::Down => 270.0,
    };
    let x_rot = match s.facing {
        Dir::Up => 90.0,
        Dir::Down => 270.0,
        _ => 0.0,
    };
    // Facing up and still, it shows its item turning on top (`displayMode`).
    let display = s.facing == Dir::Up && rpm == 0.0 && !s.punch;
    let block = host.item_block(stack.item()).is_some();
    draw.push();
    draw.translate([o[0] * at + 0.5, o[1] * at + 0.5, o[2] * at + 0.5]);
    draw.rotate_about([0.0, 1.0, 0.0], horizontal + 180.0, [0.0; 3]);
    if !display {
        draw.rotate_about([1.0, 0.0, 0.0], x_rot, [0.0; 3]);
        draw.translate([0.0, 0.0, -11.0 / 16.0]);
    }
    if s.punch {
        draw.translate([0.0, 1.0 / 8.0, -1.0 / 16.0]);
    }
    let context = if display {
        let scale = if block { 1.25 } else { 1.0 };
        draw.translate([0.0, if block { 9.0 / 16.0 } else { 11.0 / 16.0 }, 0.0]);
        draw.scale([scale; 3]);
        let turn = (draw.game_time() as f32 + draw.partial()) % 360.0;
        draw.rotate_about([0.0, 1.0, 0.0], turn, [0.0; 3]);
        ItemDisplayContext::Ground
    } else {
        let scale = if s.punch {
            0.75
        } else if block {
            0.75 - 1.0 / 64.0
        } else {
            0.5
        };
        draw.scale([scale; 3]);
        if s.punch {
            ItemDisplayContext::ThirdPersonRightHand
        } else {
            ItemDisplayContext::Fixed
        }
    };
    draw.item(&stack, context, None, 0);
    draw.pop();
}

/// A crafter's facing and pointing as the client sees the block at `pos`; None for any other.
fn crafter_seen(host: &dyn ClientHost, pos: [i32; 3]) -> Option<(Dir, &'static str)> {
    let state = host.level(host.dimension())?.block_state(host, pos)?;
    let id = states::id(state)?;
    let (block, info) = crate::glue::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i)))??;
    if block != Block::MechanicalCrafter {
        return None;
    }
    let text = host.block_state_string(state)?;
    let pointing = states::split(&text)
        .1
        .into_iter()
        .find(|(k, _)| *k == "pointing")
        .map(|(_, v)| match v {
            "left" => "left",
            "down" => "down",
            "right" => "right",
            _ => "up",
        })?;
    Some((info.facing?, pointing))
}

/// `Pointing.getXRotation`.
fn pointing_x(pointing: &str) -> f32 {
    match pointing {
        "left" => 270.0,
        "down" => 180.0,
        "right" => 90.0,
        _ => 0.0,
    }
}

/// `AngleHelper.horizontalAngle`: 0 for south, turning clockwise from above, x negated.
fn horizontal_angle(d: Dir) -> f32 {
    match d {
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `MechanicalCrafterRenderer`: its cog turning on its facing's axis; its lid while covered or
/// at work (not crafting or putting out); the belt toward the crafter it points at, else its
/// arrow (`renderAndTransform`); the item in its slot while idle, the grid moving between
/// crafters while they work (`crafter_items`).
fn draw_crafter(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    nbt: &Compound,
) {
    let Some((facing, pointing)) = crafter_seen(host, pos) else {
        return;
    };
    let (phase, covered, item) = match nbt.get(&super::crafter::CRAFTER_KEY.into()) {
        Some(Tag::Compound(c)) => (
            match c.get(&"P".into()) {
                Some(Tag::Int(p)) => *p,
                _ => 0,
            },
            matches!(c.get(&"C".into()), Some(Tag::Byte(1))),
            match c.get(&"I".into()) {
                Some(Tag::String(s)) => s.to_string().ok(),
                _ => None,
            },
        ),
        _ => (0, false, None),
    };
    const IDLE: i32 = 0;
    const CRAFTING: i32 = 5;
    const INSERTING: i32 = 6;
    let axis = facing.axis();
    // The shaftless cog, its y axis turned onto the facing's.
    if let Some(cog) = states::partial("part_cogwheel_shaftless") {
        let rpm = kinetics::speeds_of(nbt)[0];
        let phase = create_engine::client::rotation_offset(Block::MechanicalCrafter, axis, pos);
        draw.push();
        if rpm != 0.0 {
            draw.spin(kinetics::axis_vector(axis), [0.5; 3], rpm * 0.3, phase);
        }
        match axis {
            create_engine::kinetics::Axis::X => draw.rotate_about([0.0, 0.0, 1.0], 90.0, [0.5; 3]),
            create_engine::kinetics::Axis::Z => draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.5; 3]),
            create_engine::kinetics::Axis::Y => {}
        }
        draw.block(cog, &[("facing", "up")]);
        draw.still();
        draw.pop();
    }
    let posed = |draw: &mut dyn BlockEntityDrawContext, name: &str| {
        if let Some(part) = states::partial(name) {
            draw.push();
            draw.rotate_about([0.0, 1.0, 0.0], horizontal_angle(facing) + 90.0, [0.5; 3]);
            draw.rotate_about([1.0, 0.0, 0.0], pointing_x(pointing), [0.5; 3]);
            draw.block(part, &[("facing", "up")]);
            draw.pop();
        }
    };
    if (covered || phase != IDLE) && phase != CRAFTING && phase != INSERTING {
        posed(draw, "part_crafter_lid");
    }
    // `isValidTarget`: a crafter facing the same way, not pointing straight back.
    let target = create_engine::crafter::target(
        facing,
        create_engine::crafter::Pointing::read(pointing)
            .unwrap_or(create_engine::crafter::Pointing::Up),
    );
    let o = target.offset();
    let valid = crafter_seen(host, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
        .is_some_and(|(f, p)| f == facing && (pointing_x(p) - pointing_x(pointing)).abs() != 180.0);
    if valid {
        posed(draw, "part_crafter_belt");
        posed(draw, "part_crafter_belt_frame");
    } else {
        posed(draw, "part_crafter_arrow");
    }
    crafter_items(host, draw, facing, pointing, phase, item.as_deref(), nbt);
}

/// `MechanicalCrafterRenderer.renderItems`: the grid at work, else the item in its slot.
fn crafter_items(
    host: &dyn mod_api::Host,
    draw: &mut dyn BlockEntityDrawContext,
    facing: Dir,
    pointing: &str,
    phase: i32,
    item: Option<&str>,
    nbt: &Compound,
) {
    const IDLE: i32 = 0;
    if phase != IDLE {
        draw_crafter_grid(host, draw, facing, pointing, phase, nbt);
        return;
    }
    if let Some(stack) = item.and_then(|i| stacks::stack(host, i, 1)) {
        let f = facing.offset().map(|v| v as f32 * 0.58 + 0.5);
        draw.push();
        draw.translate(f);
        draw.scale([0.5; 3]);
        draw.rotate_about([0.0, 1.0, 0.0], horizontal_angle(facing), [0.0; 3]);
        draw.translate([0.0, 0.0, -1.0 / 256.0]);
        draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
}

/// The cells of a crafter's grid its block entity data holds under `key` of `CRAFTER_KEY`.
fn crafter_cells(c: &Compound, key: &str) -> Vec<((i32, i32), String)> {
    let Some(Tag::List(list)) = c.get(&key.into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(e) = t else { return None };
            let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::String(id))) =
                (e.get(&"x".into()), e.get(&"y".into()), e.get(&"id".into()))
            else {
                return None;
            };
            Some(((*x, *y), id.to_string().ok()?))
        })
        .collect()
}

/// `MechanicalCrafterRenderer.renderSafe`/`renderItems` past `IDLE`: the grid's items on the
/// crafter's face, sliding on to the crafter it points at while exporting, and while crafting
/// gathering, shrinking away and the result spinning up in their place. Its `countDown` runs on
/// from the server's word at the phase's start (`S`, `D`) at the crafter's speed.
fn draw_crafter_grid(
    host: &dyn mod_api::Host,
    draw: &mut dyn BlockEntityDrawContext,
    facing: Dir,
    pointing: &str,
    phase: i32,
    nbt: &Compound,
) {
    const EXPORTING: i32 = 3;
    const CRAFTING: i32 = 5;
    const INSERTING: i32 = 6;
    let Some(Tag::Compound(c)) = nbt.get(&super::crafter::CRAFTER_KEY.into()) else {
        return;
    };
    let began = match c.get(&"S".into()) {
        Some(Tag::Long(t)) => *t,
        _ => draw.game_time(),
    };
    let start = match c.get(&"D".into()) {
        Some(Tag::Int(d)) => *d,
        _ => 0,
    };
    let speed = create_engine::crafter::count_down_speed(kinetics::speeds_of(nbt)[0]) as f32;
    let count_down = (start as f32 - speed * (draw.game_time() - began).max(0) as f32).max(0.0);
    let partial = draw.partial();
    let pointing_v = create_engine::crafter::Pointing::read(pointing)
        .unwrap_or(create_engine::crafter::Pointing::Up);
    let o = facing.offset().map(|v| v as f32);
    let mut vec = [0.5 + o[0] * 0.58, 0.5 + o[1] * 0.58, 0.5 + o[2] * 0.58];
    if phase == EXPORTING {
        let t = create_engine::crafter::target(facing, pointing_v).offset();
        let progress = ((1000.0 - count_down + speed * partial) / 1000.0).clamp(0.0, 1.0);
        for i in 0..3 {
            vec[i] += t[i] as f32 * progress * 0.75;
        }
    }
    draw.push();
    draw.translate(vec);
    draw.scale([0.5; 3]);
    draw.rotate_about([0.0, 1.0, 0.0], horizontal_angle(facing), [0.0; 3]);
    let mut items = crafter_cells(c, "G");
    let mut distance = 0.5f32;
    draw.push();
    if phase == CRAFTING {
        items = crafter_cells(c, "B");
        let progress = ((2000.0 - count_down + speed * partial) / 1000.0).clamp(0.0, 1.0);
        let early = (progress * 2.0).clamp(0.0, 1.0);
        let late = (progress * 2.0 - 1.0).clamp(0.0, 1.0);
        draw.scale([1.0 - late; 3]);
        // `calcStats`: the grid's extent.
        let min_x = items.iter().map(|((x, _), _)| *x).min().unwrap_or(0) as f32;
        let max_x = items.iter().map(|((x, _), _)| *x).max().unwrap_or(0) as f32;
        let min_y = items.iter().map(|((_, y), _)| *y).min().unwrap_or(0) as f32;
        let max_y = items.iter().map(|((_, y), _)| *y).max().unwrap_or(0) as f32;
        let (width, height) = (max_x - min_x + 1.0, max_y - min_y + 1.0);
        let cx = (-min_x + (-width + 1.0) / 2.0) * early;
        let cy = (-min_y + (-height + 1.0) / 2.0) * early;
        draw.translate([cx * 0.5, cy * 0.5, 0.0]);
        distance += (-4.0 * (progress - 0.5) * (progress - 0.5) + 1.0) * 0.25;
    }
    let only_first = phase == INSERTING || (phase == CRAFTING && count_down < 1000.0);
    let offset = if phase == EXPORTING {
        match pointing_v {
            create_engine::crafter::Pointing::Up => -1,
            create_engine::crafter::Pointing::Left => 2,
            create_engine::crafter::Pointing::Right => -2,
            create_engine::crafter::Pointing::Down => 1,
        }
    } else {
        0
    };
    for ((x, y), id) in &items {
        if only_first && (*x != 0 || *y != 0) {
            continue;
        }
        let Some(stack) = stacks::stack(host, id, 1) else {
            continue;
        };
        draw.push();
        draw.translate([*x as f32 * distance, *y as f32 * distance, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
        draw.translate([0.0, 0.0, (x + y * 3 + offset * 9) as f32 / 1024.0]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
    draw.pop();
    if phase == CRAFTING {
        let progress = ((1000.0 - count_down + speed * partial) / 1000.0).clamp(0.0, 1.0);
        let early = (progress * 2.0).clamp(0.0, 1.0);
        let late = (progress * 2.0 - 1.0).clamp(0.0, 1.0);
        draw.rotate_about([0.0, 0.0, 1.0], early * 2.0 * 360.0, [0.0; 3]);
        draw.scale([early * 1.125; 3]);
        draw.scale([1.0 + (1.0 - late) * 0.125; 3]);
        for ((x, y), id) in crafter_cells(c, "G") {
            if x != 0 || y != 0 {
                continue;
            }
            let Some(stack) = stacks::stack(host, &id, 1) else {
                continue;
            };
            draw.push();
            draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
            draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
            draw.pop();
        }
    }
    draw.pop();
}

/// Whether `click` on the processing block `block` lands on its filter slot and is the slot's
/// (not a wrench's, an arm's or a clipboard's), which the client then takes as used.
pub fn filter_slot_click(
    host: &dyn ClientHost,
    block: Block,
    click: &mod_api::BlockClick<'_>,
) -> bool {
    if !matches!(block, Block::Basin | Block::MechanicalSaw | Block::Deployer) {
        return false;
    }
    let Some(player) = host.player(click.player) else {
        return false;
    };
    if !crate::glue::interaction::values::may_use_value_box(host, player) {
        return false;
    }
    if click.stack.is_some_and(|s| {
        matches!(
            stacks::id(host, s).as_str(),
            "create:wrench" | "create:mechanical_arm" | "create:clipboard"
        )
    }) {
        return false;
    }
    let Some(state) = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, click.pos))
    else {
        return false;
    };
    let Some((_, info)) = states::id(state).and_then(|id| {
        crate::glue::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
    }) else {
        return false;
    };
    let flipped = host
        .block_state_string(state)
        .is_some_and(|s| s.contains("flipped=true"));
    let Some(face) = Dir::from_offset(click.hit.face) else {
        return false;
    };
    super::filtering::hit(block, &info, flipped, click.pos, face, click.hit.point)
}

/// `FilteringRenderer` for the basin, upward saw and deployer: the filter (`logistics::FILTER`)
/// in each slot, at half the value box's half scale for a flat item and its half scale for a
/// block.
fn draw_machine_filter(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    block: Block,
    info: &StateInfo,
    state: mod_api::BlockState,
    nbt: &Compound,
) {
    let Some(Tag::String(id)) = nbt.get(&crate::glue::logistics::FILTER.into()) else {
        return;
    };
    let id = id.to_string_lossy();
    if id.is_empty() {
        return;
    }
    let Some(stack) = stacks::stack(host, &id, 1) else {
        return;
    };
    let flipped = host
        .block_state_string(state)
        .is_some_and(|s| s.contains("flipped=true"));
    let solid = host.item_block(stack.item()).is_some();
    let scale = 0.5 * (if solid { 1.0 } else { 0.5 } + 1.0 / 64.0);
    for (_, slot, y_rot, x_rot) in super::filtering::drawn(block, info, flipped) {
        draw.push();
        draw.translate(slot.map(|v| v as f32));
        draw.rotate_about([0.0, 1.0, 0.0], y_rot, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], x_rot, [0.0; 3]);
        draw.scale([scale; 3]);
        if !solid {
            draw.translate([0.0, 0.0, -0.15]);
        }
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
}

/// `SawRenderer.renderItems`: what an upward saw holds (`SAW_KEY`), lying on it and moving across
/// it as it cuts, run on from the server's last word by the game time (`ProcessingInventory`'s
/// `remainingTime`, which a client counts down too and holds above 5 until the server cuts);
/// its results side by side.
fn draw_saw_items(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    info: &StateInfo,
    speed: f32,
    nbt: &Compound,
) {
    if info.facing != Some(Dir::Up) {
        return;
    }
    let Some(Tag::Compound(c)) = nbt.get(&super::saw::SAW_KEY.into()) else {
        return;
    };
    let float = |k: &str| match c.get(&k.into()) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    let applied = matches!(c.get(&"A".into()), Some(Tag::Byte(1)));
    let told = match c.get(&"S".into()) {
        Some(Tag::Long(t)) => *t,
        _ => draw.game_time(),
    };
    let items = basin_items_of(c.get(&"Items".into()));
    if items.is_empty() {
        return;
    }
    let along_z = !info.along_first;
    let duration = float("D");
    let mut remaining = float("R");
    if remaining >= 0.0 {
        let elapsed = (draw.game_time() - told).max(0) as f32;
        remaining -= (speed.abs() / 24.0).clamp(1.0, 128.0) * elapsed;
        remaining = remaining.max(if applied { 0.0 } else { 4.0 });
    }
    let moving = duration != 0.0;
    let mut offset = if moving { remaining / duration } else { 0.0 };
    let processing_speed = (speed.abs() / 32.0).clamp(1.0, 128.0);
    if moving {
        offset = (offset + (-draw.partial() + 0.5) * processing_speed / duration).clamp(0.125, 1.0);
        if !applied {
            offset += 1.0;
        }
        offset /= 2.0;
    }
    if speed == 0.0 {
        offset = 0.5;
    }
    if (speed < 0.0) ^ along_z {
        offset = 1.0 - offset;
    }
    // The results are in slots 1 and on, the input in slot 0.
    let outputs = if applied { items.len() } else { 0 };
    draw.push();
    if along_z {
        draw.rotate_about([0.0, 1.0, 0.0], 90.0, [0.0; 3]);
    }
    draw.translate([if outputs <= 1 { 0.5 } else { 0.25 }, 0.0, offset]);
    draw.translate([if along_z { -1.0 } else { 0.0 }, 0.0, 0.0]);
    for (rendered, (id, _)) in items.iter().enumerate() {
        let Some(stack) = stacks::stack(host, id, 1) else {
            continue;
        };
        let block = host.item_block(stack.item()).is_some();
        draw.push();
        draw.translate([0.0, if block { 0.925 } else { 13.0 / 16.0 }, 0.0]);
        if applied && outputs > 1 {
            draw.translate([0.5 / (outputs - 1) as f32 * rendered as f32, 0.0, 0.0]);
        }
        draw.scale([0.5; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0; 3]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
    draw.pop();
}

/// `SawRenderer.renderBlade`: the blade turned to the saw's facing, its texture running forward,
/// backward or still with the speed's sign; facing up or down along x, turned a quarter.
fn draw_saw_blade(draw: &mut dyn BlockEntityDrawContext, info: &StateInfo, speed: f32) {
    let Some(facing) = info.facing else {
        return;
    };
    let vertical = facing.axis() == create_engine::kinetics::Axis::Y;
    let kind = if speed > 0.0 {
        "active"
    } else if speed < 0.0 {
        "reversed"
    } else {
        "inactive"
    };
    let name = format!(
        "part_saw_blade_{}_{kind}",
        if vertical { "vertical" } else { "horizontal" }
    );
    let Some(part) = states::partial(&name) else {
        return;
    };
    draw.push();
    if vertical && info.along_first {
        draw.rotate_about([0.0, 1.0, 0.0], 90.0, [0.5; 3]);
    }
    draw.block(part, &[("facing", dir_name(facing))]);
    draw.pop();
}

/// The mixer's head lowered as its cycle has it this frame (`getRenderedHeadOffset`), from what
/// the server last told of it (`MIXER_KEY`) run on by the game time; 7/16 at rest.
fn mixer_head_offset(nbt: &Compound, draw: &dyn BlockEntityDrawContext) -> f32 {
    let Some(Tag::Compound(c)) = nbt.get(&basin_glue::MIXER_KEY.into()) else {
        return create_engine::basin::head_offset(false, 0, 0.0);
    };
    let (Some(Tag::Int(told)), Some(Tag::Long(at))) = (c.get(&"T".into()), c.get(&"S".into()))
    else {
        return create_engine::basin::head_offset(false, 0, 0.0);
    };
    let ticks = create_engine::basin::shown_ticks(*told, draw.game_time() - at);
    // At the bottom the head waits for the server; past 40 the cycle is over.
    let partial = if ticks == 20 || ticks >= 40 {
        0.0
    } else {
        draw.partial()
    };
    create_engine::basin::head_offset(ticks < 40, ticks, partial)
}

/// The mixer's pole or head `offset` down; the head turns with the mixer.
fn draw_mixer_part(draw: &mut dyn BlockEntityDrawContext, d: &parts::Draw, offset: f32) {
    let partial = if d.model == Model::MixerPole {
        Partial::MixerPole
    } else {
        Partial::MixerHead
    };
    draw.push();
    draw.translate([0.0, -offset, 0.0]);
    if d.rpm != 0.0 {
        draw.spin([0.0, 1.0, 0.0], [0.5; 3], d.rpm * 0.3, d.phase);
    }
    draw_partial(draw, partial, "up");
    draw.still();
    draw.pop();
}

/// What a basin (its items and fluids) or a mechanical crafter (its item) shows while it rides
/// a moving structure (`BasinRenderer`, `MechanicalCrafterRenderer` in
/// `renderBlockEntities`), from the data it had when the structure took it.
pub(super) fn draw_carried(
    host: &dyn ClientHost,
    block: Block,
    event: &mut BlockEntitySubmitEvent<'_>,
) {
    if matches!(block, Block::Basin | Block::MechanicalCrafter) {
        MachineDraw.block_entity_submit(host, event);
    }
}

/// The items a riding basin or crafter's renderer draws, from its block entity data `nbt` as the
/// structure took it (its fluids are left out: they are no item): a basin's ring of items at the
/// height its fluids give them, at rest; a crafter's grid as it stood in its phase, or the item in
/// its slot. Needs no
/// client: the trains' riding displays are posed by it.
pub(crate) fn draw_riding_items(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn mod_api::Host,
    block: Block,
    facing: Option<Dir>,
    pointing: Option<&str>,
    nbt: &Compound,
) {
    match block {
        Block::Basin => {
            let total: u32 = basin_fluids(nbt).iter().map(|f| f.1).sum();
            let fluid_top = if total >= 1 {
                let level = (total as f32 / 2000.0).clamp(0.0, 1.0);
                2.0 / 16.0 + 12.0 / 16.0 * (1.0 - (1.0 - level) * (1.0 - level))
            } else {
                0.0
            };
            draw_basin_items(host, draw, [0; 3], nbt, fluid_top, 0.0);
        }
        Block::MechanicalCrafter => {
            let (Some(facing), Some(pointing)) = (facing, pointing) else {
                return;
            };
            let Some(Tag::Compound(c)) = nbt.get(&super::crafter::CRAFTER_KEY.into()) else {
                return;
            };
            let phase = match c.get(&"P".into()) {
                Some(Tag::Int(p)) => *p,
                _ => 0,
            };
            let item = match c.get(&"I".into()) {
                Some(Tag::String(s)) => s.to_string().ok(),
                _ => None,
            };
            crafter_items(host, draw, facing, pointing, phase, item.as_deref(), nbt);
        }
        _ => {}
    }
}
