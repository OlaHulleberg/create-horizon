//! The blaze in a blaze burner, as a client draws it (`BlazeBurnerRenderer.renderShared`): its
//! head by heat, bobbing and turned to the nearest player (or to its facing while it heats a
//! basin), the rods floating around it, and the flame scrolling under a basin. The head's turn
//! and its sinking under a basin are run on in client ticks (`BlazeBurnerBlockEntity
//! .tickAnimation`); the cage is the block's own model.

use crate::glue::states;
use create_engine::Block;
use create_engine::fan::Heat;
use create_engine::kinetics::Dir;
use mod_api::{BlockEntityDrawContext, ClientHost, ModelRef, TerrainModelOptions, TerrainUvShift};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// `AllPartialModels.BLAZE_BURNER_FLAME`, a standalone model (its texture scrolls).
pub const FLAME_MODEL: &str = "create:block/blaze_burner/flame";
/// Our model naming the scrolling sprites, so they are stitched (`flame_scroll_model`).
pub const FLAME_SCROLLS_MODEL: &str = "create:block/blaze_burner_flame_scrolls";
/// The flame's sprite and the sprites scrolled over it (`AllSpriteShifts.BURNER_FLAME`,
/// `SUPER_BURNER_FLAME`), twice its size.
const FLAME_SPRITE: &str = "create:block/blaze_burner_flame";
const FLAME_SCROLL: &str = "create:block/blaze_burner_flame_scroll";
const SUPER_FLAME_SCROLL: &str = "create:block/blaze_burner_flame_superheated_scroll";
/// `LightTexture.FULL_BRIGHT`.
const FULL_BRIGHT: u32 = 0xF0F0;

/// One burner's animation: `headAngle` and `headAnimation`, this tick's and the last.
#[derive(Clone, Copy, Debug)]
struct Head {
    angle: f32,
    last_angle: f32,
    sink: f32,
    last_sink: f32,
}

thread_local! {
    static HEADS: RefCell<HashMap<[i32; 3], Head>> = RefCell::new(HashMap::new());
    static FLAME: Cell<Option<ModelRef>> = const { Cell::new(None) };
}

/// The flame's model, resolved after each resource load.
pub fn models_baked(host: &mut dyn ClientHost) {
    let model = mod_api::Identifier::parse(FLAME_MODEL)
        .ok()
        .and_then(|id| host.render_assets().model(&id).ok());
    if model.is_none() {
        crate::create_log!("[create] the blaze burner's flame model did not resolve");
    }
    FLAME.set(model);
}

/// The client left its world.
pub fn reset() {
    HEADS.with_borrow_mut(HashMap::clear);
}

/// `AngleHelper.horizontalAngle(facing) + 180`: the angle the head faces its front at.
fn front_angle(facing: Dir) -> f32 {
    let horizontal = match facing {
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    };
    (horizontal + 180.0f32).rem_euclid(360.0)
}

/// `AngleHelper.getShortestAngleDiff`.
fn shortest(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Whether a basin or a fluid tank is over `pos` (`isValidBlockAbove`).
fn heats_above(host: &dyn ClientHost, pos: [i32; 3]) -> bool {
    let above = [pos[0], pos[1] + 1, pos[2]];
    let block = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, above))
        .and_then(states::id)
        .and_then(|id| crate::glue::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten());
    matches!(block, Some(Block::Basin | Block::FluidTank))
}

/// `tickAnimation` for every burner drawn: the head turns to the local player while idle, to
/// its front while it heats something; it sinks under a basin.
pub fn tick(host: &dyn ClientHost) {
    let positions: Vec<[i32; 3]> = HEADS.with_borrow(|h| h.keys().copied().collect());
    if positions.is_empty() {
        return;
    }
    let level = host.level(host.dimension());
    let player = host.local_player().and_then(|p| {
        let e = p.entity();
        (e.is_invisible(host) != Some(true))
            .then(|| e.position(host))
            .flatten()
    });
    for pos in positions {
        let info = level
            .and_then(|l| l.block_state(host, pos))
            .and_then(states::id)
            .and_then(|id| {
                crate::glue::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
            });
        let Some((Block::BlazeBurner, info)) = info else {
            HEADS.with_borrow_mut(|h| h.remove(&pos));
            continue;
        };
        let heat = info.heat.unwrap_or(Heat::None);
        let active = heat >= Heat::Fading && heats_above(host, pos);
        HEADS.with_borrow_mut(|h| {
            let Some(head) = h.get_mut(&pos) else {
                return;
            };
            head.last_angle = head.angle;
            head.last_sink = head.sink;
            if active {
                // `chase(.., .125f, Chaser.EXP)`.
                let target = front_angle(info.facing.unwrap_or(Dir::South));
                head.angle += shortest(head.angle, target) * 0.125;
            } else {
                // `chase(target, .25f, Chaser.exp(5))`.
                let target = player.map_or(0.0, |p| {
                    let dx = p[0] - (f64::from(pos[0]) + 0.5);
                    let dz = p[2] - (f64::from(pos[2]) + 0.5);
                    (-dz.atan2(dx)).to_degrees() as f32 - 90.0
                });
                head.angle += (shortest(head.angle, target) * 0.25).clamp(-5.0, 5.0);
            }
            // `headAnimation.chase(active ? 1 : 0, .25f, Chaser.exp(.25f))`.
            let to = if active { 1.0 } else { 0.0 };
            head.sink += ((to - head.sink) * 0.25).clamp(-0.25, 0.25);
        });
    }
}

/// `renderShared` for the burner at `pos` of `heat` facing `facing`; beside a stock ticker it is
/// its keeper (`stockKeeper`): lit at least as a fading blaze (`getHeatLevelForRender`) and
/// wearing the logistics hat.
pub fn draw(
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    heat: Heat,
    facing: Dir,
    goggles: bool,
    keeper: bool,
) {
    if heat == Heat::None {
        return;
    }
    let head = HEADS.with_borrow_mut(|h| {
        *h.entry(pos).or_insert_with(|| {
            let a = front_angle(facing);
            Head {
                angle: a,
                last_angle: a,
                sink: 0.0,
                last_sink: 0.0,
            }
        })
    });
    let partial = draw.partial();
    let angle = head.last_angle + (head.angle - head.last_angle) * partial;
    let animation = (head.last_sink + (head.sink - head.last_sink) * partial) * 0.175;
    // `renderTick`: the render time, set apart per burner (`hashCode % 13`).
    let hash = (pos[0].wrapping_mul(31).wrapping_add(pos[1]))
        .wrapping_mul(31)
        .wrapping_add(pos[2]);
    let heat = if keeper { heat.max(Heat::Fading) } else { heat };
    let wear = Wear {
        goggles,
        hat: keeper,
    };
    shared(draw, heat, angle, animation, true, wear, hash);
}

/// `renderInContraption`: a burner carried by a moving structure, its blaze at least fading,
/// facing its front (`headAngle`), with no flame and no goggles (its block entity's data stays
/// behind). `hash` sets its bobbing apart.
pub fn draw_moving(draw: &mut dyn BlockEntityDrawContext, heat: Heat, facing: Dir, hash: i32) {
    if heat == Heat::None {
        return;
    }
    let heat = heat.max(Heat::Fading);
    let wear = Wear {
        goggles: false,
        hat: false,
    };
    shared(draw, heat, front_angle(facing), 0.0, false, wear, hash);
}

/// What the blaze wears: goggles (`drawGoggles`) and the logistics hat (`drawHat`).
#[derive(Clone, Copy)]
struct Wear {
    goggles: bool,
    hat: bool,
}

/// `renderShared`: the head at `angle`, sunk by `animation`, its rods, and (when it may and
/// something is above) its flame.
fn shared(
    draw: &mut dyn BlockEntityDrawContext,
    heat: Heat,
    angle: f32,
    animation: f32,
    can_draw_flame: bool,
    wear: Wear,
    hash: i32,
) {
    let Wear { goggles, hat } = wear;
    let above = animation > 0.125;
    let time = draw.animation_time() as f32;
    let tick = time + (hash % 13) as f32 * 16.0;
    let mult = if heat >= Heat::Fading { 64.0 } else { 16.0 };
    let tau = std::f32::consts::TAU;
    let bob = |phase: f32| ((tick / 16.0 + phase) % tau).sin() / mult;
    let pi = std::f32::consts::PI;
    let (offset, offset1, offset2) = (bob(0.0), bob(pi), bob(pi / 2.0));
    let head_y = offset - animation * 0.75;

    let model = match heat {
        Heat::Seething if above => "part_blaze_super_active",
        Heat::Seething => "part_blaze_super",
        Heat::Kindled if above => "part_blaze_active",
        Heat::Fading | Heat::Kindled => "part_blaze_idle",
        _ => "part_blaze_inert",
    };
    draw.set_light(FULL_BRIGHT);
    if let Some(part) = states::partial(model) {
        draw.push();
        draw.translate([0.0, head_y, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5; 3]);
        draw.block(part, &[("facing", "up")]);
        draw.pop();
    }
    // `drawGoggles`: on the head, smaller on the inert one.
    let goggles = goggles
        .then(|| {
            states::partial(if model == "part_blaze_inert" {
                "part_blaze_goggles_small"
            } else {
                "part_blaze_goggles"
            })
        })
        .flatten();
    if let Some(part) = goggles {
        draw.push();
        draw.translate([0.0, head_y + 0.5, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5; 3]);
        draw.block(part, &[("facing", "up")]);
        draw.pop();
    }
    // `drawHat`: the logistics hat on the head, smaller on the inert one.
    if hat && let Some(part) = states::partial("part_logistics_hat") {
        draw.push();
        if model == "part_blaze_inert" {
            draw.translate([0.0, head_y + 0.5, 0.0]);
            draw.translate([0.5, 0.5, 0.5]);
            draw.scale([0.75, 0.75, 0.75]);
            draw.translate([-0.5, -0.5, -0.5]);
        } else {
            draw.translate([0.0, head_y + 0.75, 0.0]);
        }
        draw.rotate_about([0.0, 1.0, 0.0], angle + 180.0, [0.5; 3]);
        draw.translate([0.5, 0.0, 0.5]);
        draw.block(part, &[("facing", "up")]);
        draw.pop();
    }
    if heat >= Heat::Fading {
        let seething = heat == Heat::Seething;
        let (small, large) = if seething {
            ("part_blaze_super_rods_small", "part_blaze_super_rods_large")
        } else {
            ("part_blaze_rods_small", "part_blaze_rods_large")
        };
        for (name, y) in [
            (small, offset1 + animation + 0.125),
            (large, offset2 + animation - 3.0 / 16.0),
        ] {
            if let Some(part) = states::partial(name) {
                draw.push();
                draw.translate([0.0, y, 0.0]);
                draw.block(part, &[("facing", "up")]);
                draw.pop();
            }
        }
        if can_draw_flame
            && above
            && let Some(flame) = FLAME.get()
        {
            // `shiftUVScrolling`: half the scrolling sprite slides by, at a speed by heat.
            let speed = 1.0 / 32.0 + 1.0 / 64.0 * heat as u8 as f32;
            let v = (speed * time).fract() / 2.0;
            let u = (speed * time / 2.0).fract() / 2.0;
            let target = if seething {
                SUPER_FLAME_SCROLL
            } else {
                FLAME_SCROLL
            };
            let shift = TerrainUvShift::new(FLAME_SPRITE, target, [0.5, 0.5], [u, v]);
            draw.push();
            draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5; 3]);
            let options = TerrainModelOptions::default().with_uv_shift(shift);
            let _ = draw.terrain_model(flame, options);
            draw.pop();
        }
    }
}
