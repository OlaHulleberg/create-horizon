//! Nixie tubes and display boards on a client (`NixieTubeRenderer`, `FlapDisplayRenderer`,
//! `FlapDisplayBlockEntity.tick`): what the server sent of each (`create:display`), the boards'
//! flaps turning toward their text with their sounds, and the text drawn in the world through
//! each block entity's draw context, every frame.

use crate::glue::pipelines::{Pose, Textured};
use crate::glue::redstone::pose;
use crate::glue::{client_table, kinetics, models, states};
use create_engine::Block;
use create_engine::display::{self, Board, Face, Lang, Shown};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntityRenderBoundingBoxEvent,
    BlockEntitySubmitEvent, BlockPart, ClientHost, Registrar, SoundPlay, Tag, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) fn register(r: &mut dyn Registrar) {
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, DisplayEntity) {
        crate::create_log!("[create] the display block entity's part was refused: {e:?}");
    }
}

/// A board on a client: its lines' sections spinning toward their text.
#[derive(Clone, Debug, Default)]
struct ClientBoard {
    x_size: i32,
    y_size: i32,
    lines: Vec<(String, Vec<display::ShownSection>)>,
    colour: Vec<Option<u8>>,
    glowing: Vec<bool>,
    /// Whether it turns fast enough for its flaps to move (`isSpeedRequirementFulfilled`), as
    /// last ticked.
    running: bool,
    /// Its speed and facing as last drawn.
    speed: f32,
    facing: Option<Dir>,
}

/// What a client knows of tubes and boards.
#[derive(Default)]
struct ClientDisplays {
    /// Each tube's two characters.
    nixies: HashMap<[i32; 3], (String, String)>,
    boards: HashMap<[i32; 3], ClientBoard>,
    /// Boards with flaps still turning.
    active: Vec<[i32; 3]>,
    /// Each cycle's flap options in the client's language, read once.
    options: Option<Vec<Vec<String>>>,
    rng: Option<create_engine::processing::Rng>,
}

thread_local! {
    static DISPLAYS: RefCell<ClientDisplays> = RefCell::new(ClientDisplays::default());
}

pub(super) fn reset() {
    DISPLAYS.with_borrow_mut(|d| *d = ClientDisplays::default());
}

/// Each cycle's flap options, in the client's language.
fn options(host: &dyn ClientHost) -> Vec<Vec<String>> {
    DISPLAYS.with_borrow_mut(|d| {
        d.options
            .get_or_insert_with(|| {
                let resolve = |k: &str| host.translate(k);
                let lang = Lang {
                    resolve: Some(&resolve),
                };
                display::Cycle::ALL
                    .iter()
                    .map(|c| c.options(&lang))
                    .collect()
            })
            .clone()
    })
}

/// `NixieTubeBlockEntity.read` / `FlapDisplayBlockEntity.read` on a client.
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != display::CHANNEL {
        return false;
    }
    let counts: Vec<usize> = options(&*host).iter().map(Vec::len).collect();
    DISPLAYS.with_borrow_mut(|c| {
        for s in display::decode(data) {
            match s {
                Shown::Nixie {
                    pos,
                    text,
                    index,
                    redstone,
                } => {
                    let n = display::Nixie {
                        custom: text,
                        index,
                        redstone,
                    };
                    c.nixies.insert(pos, n.displayed());
                }
                Shown::Board {
                    pos,
                    x_size,
                    y_size,
                    lines,
                    colour,
                    glowing,
                } => {
                    let b = c.boards.entry(pos).or_default();
                    // A new size forgets the lines (`lines = null`).
                    if b.x_size != x_size || b.y_size != y_size {
                        let max = Board {
                            x_size,
                            ..Board::default()
                        }
                        .max_chars();
                        let fresh = display::Layout::new(max);
                        b.lines = (0..lines.len())
                            .map(|_| {
                                let n = counts[fresh.sections[0].cycle as usize];
                                (
                                    fresh.key.clone(),
                                    fresh
                                        .sections
                                        .iter()
                                        .map(|s| display::ShownSection::new(s.clone(), n))
                                        .collect(),
                                )
                            })
                            .collect();
                    }
                    b.x_size = x_size;
                    b.y_size = y_size;
                    b.colour = colour;
                    b.glowing = glowing;
                    b.lines.resize_with(lines.len(), Default::default);
                    for (shown, layout) in b.lines.iter_mut().zip(lines) {
                        if shown.0 != layout.key || shown.1.len() != layout.sections.len() {
                            // FlapDisplayLayout.read: another layout's sections are new.
                            shown.0 = layout.key;
                            shown.1 = layout
                                .sections
                                .into_iter()
                                .map(|s| {
                                    let n = counts[s.cycle as usize];
                                    display::ShownSection::new(s, n)
                                })
                                .collect();
                        } else {
                            // FlapDisplaySection.update.
                            for (old, new) in shown.1.iter_mut().zip(layout.sections) {
                                if new.text.as_deref().is_some_and(|t| !t.is_empty()) {
                                    old.section.text = new.text;
                                }
                                old.refresh(new.transition);
                            }
                        }
                    }
                    if b.lines.iter().any(|l| l.1.iter().any(|s| !s.settled()))
                        && !c.active.contains(&pos)
                    {
                        c.active.push(pos);
                    }
                }
                Shown::Forget { pos } => {
                    c.nixies.remove(&pos);
                    c.boards.remove(&pos);
                    c.active.retain(|p| *p != pos);
                }
            }
        }
    });
    true
}

/// Whether the board at `pos` has flaps turning on a running board: drawn again every frame
/// (`FlapDisplayRenderer` turns them by the render time), its last frame kept once they settle.
pub(super) fn animated(pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
    speed_of(&entity.nbt).abs() >= create_engine::gauge::MEDIUM_SPEED
        && DISPLAYS.with_borrow(|d| d.active.contains(&pos))
}

/// `FlapDisplayBlockEntity.tick` on a client, for boards with flaps still turning (while
/// running: `isSpeedRequirementFulfilled`): instant over 128 rpm; the flaps' sounds.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let active = DISPLAYS.with_borrow(|d| d.active.clone());
    if active.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for pos in active {
        let ticked = DISPLAYS.with_borrow_mut(|d| {
            let mut rng = d
                .rng
                .take()
                .unwrap_or_else(|| create_engine::processing::Rng::new(0x9E37_79B9));
            let mut next = |n: i32| (rng.next_float() * n.max(1) as f32) as i32;
            let out = d.boards.get_mut(&pos).map(|b| {
                let speed = b.speed;
                b.running = speed.abs() >= create_engine::gauge::MEDIUM_SPEED;
                if !b.running {
                    // Spinning flaps stop where they are and start again with the board.
                    return (0, false, (b.x_size, b.y_size), b.facing);
                }
                let instant = speed.abs() > 128.0;
                let mut flaps = 0;
                for l in &mut b.lines {
                    for s in &mut l.1 {
                        flaps += s.tick(instant, &mut next);
                    }
                }
                let settled = b.lines.iter().all(|l| l.1.iter().all(|s| s.settled()));
                (flaps, settled, (b.x_size, b.y_size), b.facing)
            });
            d.rng = Some(rng);
            out
        });
        let Some((flaps, settled, size, facing)) = ticked else {
            continue;
        };
        if !settled {
            keep.push(pos);
        }
        if flaps == 0 {
            continue;
        }
        // At the middle of the board: `getDirection().getClockWise()` by xSize / 2, down by
        // ySize / 2.
        let side = display::clockwise(facing.unwrap_or(Dir::North).opposite()).offset();
        let middle = [
            f64::from(pos[0] + side[0] * (size.0 / 2)),
            f64::from(pos[1] - size.1 / 2),
            f64::from(pos[2] + side[2] * (size.0 / 2)),
        ];
        let volume = (flaps as f32 / 20.0).clamp(0.25, 1.5);
        let bg = (flaps as f32 / 40.0).clamp(0.25, 1.0);
        let at = |id: &str, volume: f32, pitch: f32| {
            SoundPlay::at(id, mod_api::SoundSource::Blocks, middle)
                .with_volume(volume)
                .with_pitch(pitch)
        };
        host.play_local_sound(&at("create:scroll_value", volume, 0.56), 0);
        host.play_local_sound(&at("minecraft:block.calcite.hit", 0.35 * bg, 1.95), 0);
    }
    DISPLAYS.with_borrow_mut(|d| d.active = keep);
}

/// The first part's speed the kinetic block entity's data holds (`Speeds`).
fn speed_of(nbt: &mod_api::Compound) -> f32 {
    match nbt.get(&"Speeds".into()) {
        Some(Tag::List(l)) => match l.first() {
            Some(Tag::Float(bits)) => f32::from_bits(*bits),
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// The width of `s` in font pixels.
fn width(draw: &dyn BlockEntityDrawContext, s: &str) -> f32 {
    draw.text_width(&Text::literal(s)).unwrap_or(0.0)
}

/// Draws `text` at `pose` (font pixels from the block's corner), `rgb`, glowing or not.
fn draw_text(
    draw: &mut dyn BlockEntityDrawContext,
    pose_at: &create_engine::math::Mat4,
    text: &str,
    rgb: u32,
    glowing: bool,
) {
    draw.push();
    pose::apply(draw, pose_at);
    let _ = draw.text(
        &Text::literal(text),
        [0.0, 0.0],
        rgb & 0xFF_FFFF,
        glowing,
        None,
    );
    draw.pop();
}

/// The tubes' and boards' renderers, on the kinetic block entity type.
struct DisplayEntity;

#[mod_api::horizon_part]
impl BlockPart for DisplayEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for DisplayEntity {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, facing)) =
            client_table(host, |t| t.get(id).map(|(b, i)| (b, i.facing))).flatten()
        else {
            return;
        };
        let pos = event.pos;
        if display::is_nixie(block) {
            let Some(facing) = facing else {
                return;
            };
            draw_nixie(host, event.draw, event.state, block, facing, pos);
        } else if block == Block::DisplayBoard {
            let speed = speed_of(&event.entity.nbt);
            let known = DISPLAYS.with_borrow_mut(|d| {
                d.boards.get_mut(&pos).map(|b| {
                    b.speed = speed;
                    b.facing = facing;
                })
            });
            if known.is_some()
                && let Some(facing) = facing
            {
                draw_board(host, event.draw, pos, facing);
            }
        }
    }

    /// A controller draws across its whole board: `xSize` along it and `ySize` down.
    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        let size = DISPLAYS.with_borrow(|d| d.boards.get(&event.pos).map(|b| (b.x_size, b.y_size)));
        if let Some((x, y)) = size {
            let (x, y) = (x as f32, y as f32);
            let b = &mut *event.bounds;
            b[0] -= x;
            b[2] -= x;
            b[3] += x;
            b[5] += x;
            b[1] -= y;
        }
    }
}

/// `NixieTubeRenderer`: the tube's two characters, front and back with their shadows.
fn draw_nixie(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    state: mod_api::BlockState,
    block: Block,
    facing: Dir,
    pos: [i32; 3],
) {
    if draw_nixie_signal(host, draw, state, facing, pos) {
        return;
    }
    signal_gone(pos);
    let Some((left, right)) = DISPLAYS.with_borrow(|d| d.nixies.get(&pos).cloned()) else {
        return;
    };
    draw_nixie_text(host, draw, state, block, facing, &left, &right);
}

/// `NixieTubeBlockEntity.tick` on a client (`signalState`): the state of the train signal the
/// tube at `pos` is on (behind it, against `getFacing`), if it is on one.
fn signal_behind(
    host: &dyn ClientHost,
    pos: [i32; 3],
    face: Face,
    facing: Dir,
) -> Option<create_engine::trains::SignalState> {
    let getfacing = match face {
        Face::Ceiling => Dir::Down,
        Face::Floor => Dir::Up,
        _ => facing,
    };
    let o = getfacing.opposite().offset();
    let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let level = host.level(host.dimension())?;
    let id = states::id(level.block_state(host, at)?)?;
    client_table(host, |t| t.get(id).map(|(b, _)| b))
        .flatten()
        .filter(|b| *b == Block::TrackSignal)?;
    let data = host.block_entity(level, at)?.data(host)?;
    Some(crate::glue::trains::signal_state_of(&data))
}

/// `NixieTubeRenderer.renderAsSignal`: on a train signal, the tube shows the signal's panel
/// (`AllPartialModels.SIGNAL_PANEL`); its lights (the red tube while it is red or blinking
/// invalid, the yellow or white one while it is yellow or green) and their glows are drawn
/// additively by [`render_signal_glows`] from the pose noted here. Whether it drew as a signal.
fn draw_nixie_signal(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    state: mod_api::BlockState,
    facing: Dir,
    pos: [i32; 3],
) -> bool {
    let text = host.block_state_string(state).unwrap_or_default();
    let (_, props) = states::split(&text);
    let Some(face) = props
        .iter()
        .find(|(k, _)| *k == "double_face")
        .and_then(|(_, v)| Face::read(v))
    else {
        return false;
    };
    let Some(signal) = signal_behind(host, pos, face, facing) else {
        return false;
    };
    let getfacing = match face {
        Face::Ceiling => Dir::Down,
        Face::Floor => Dir::Up,
        _ => facing,
    };
    let y_rot = display::horizontal_angle(facing) - 90.0
        + if face == Face::WallReversed {
            180.0
        } else {
            0.0
        };
    let x_rot = match face {
        Face::Wall => -90.0,
        Face::WallReversed => 90.0,
        _ => 0.0,
    };
    let centre = [0.5f32; 3];
    draw.push();
    draw.rotate_about([0.0, 1.0, 0.0], y_rot, centre);
    draw.rotate_about([0.0, 0.0, 1.0], x_rot, centre);
    if getfacing == Dir::Down {
        draw.rotate_about([0.0, 0.0, 1.0], 180.0, centre);
    }
    let invert = getfacing == Dir::Down || face == Face::WallReversed;
    if let Some(panel) = states::partial("part_signal_panel") {
        draw.block(panel, &[("facing", "up")]);
    }
    let mut pose = Pose::at(pos.map(f64::from))
        .rotate([0.0, 1.0, 0.0], f64::from(y_rot), [0.5; 3])
        .rotate([0.0, 0.0, 1.0], f64::from(x_rot), [0.5; 3]);
    if getfacing == Dir::Down {
        pose = pose.rotate([0.0, 0.0, 1.0], 180.0, [0.5; 3]);
    }
    let pose = pose.translate([0.5, 7.5 / 16.0, 0.5]);
    SIGNALS.with_borrow_mut(|s| {
        s.insert(
            pos,
            SignalLights {
                pose,
                invert,
                horizontal: getfacing.axis() != create_engine::kinetics::Axis::Y,
                signal,
            },
        )
    });
    draw.pop();
    true
}

thread_local! {
    static FLICKER: std::cell::Cell<u64> = const { std::cell::Cell::new(0x9E37_79B9_7F4A_7C15) };
}

/// `random.nextFloat()` for a tube's flicker, drawn anew each frame.
fn flicker() -> f32 {
    FLICKER.with(|r| {
        let mut x = r.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        r.set(x);
        (x >> 40) as f32 / (1u64 << 24) as f32
    })
}

/// A nixie signal's lights as its block entity last drew it.
struct SignalLights {
    /// The pose at the middle between the two tubes.
    pose: Pose,
    invert: bool,
    /// `NixieTubeBlock.getFacing` is horizontal.
    horizontal: bool,
    signal: create_engine::trains::SignalState,
}

thread_local! {
    static SIGNALS: RefCell<HashMap<[i32; 3], SignalLights>> = RefCell::new(HashMap::new());
}

/// The nixie tube at `pos` no longer shows a signal.
fn signal_gone(pos: [i32; 3]) {
    SIGNALS.with_borrow_mut(|s| s.remove(&pos));
}

/// `GLOW_VIEW_DISTANCE`: the glows show within 96 blocks of the eye.
const GLOW_VIEW_DISTANCE: f64 = 96.0;

/// `renderAsSignal`'s lights, each frame (the red one blinks): for each lit light, within 96
/// blocks the white cube (`SIGNAL_WHITE_CUBE`, translucent) and the glow (`SIGNAL_*_GLOW`,
/// additive) stretched along it, and the tube (`SIGNAL_*`, additive, scaled by 17/16), full
/// bright and without diffuse shading: Create's models (read by `models`) as quads of the
/// glue's pipelines. Drawn after opaque terrain and before translucent terrain, as Create's
/// block entity buffers are: the tube's glass (translucent terrain, which writes depth) then
/// blends over the lights inside it instead of hiding them.
pub(super) fn render_signal_glows(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    if event.stage != mod_api::RenderLevelStage::AfterOpaqueBlocks {
        return;
    }
    let time = event.view.game_time as f64 + f64::from(event.view.partial_tick);
    let eye = event.view.camera;
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let model = |id| crate::glue::models::get(host, id);
    let (Some(white_cube), Some(red_glow), Some(yellow_glow), Some(white_glow)) = (
        model(models::SIGNAL_WHITE_CUBE),
        model(models::SIGNAL_RED_GLOW),
        model(models::SIGNAL_YELLOW_GLOW),
        model(models::SIGNAL_WHITE_GLOW),
    ) else {
        return;
    };
    let (Some(red), Some(yellow_tube), Some(white)) = (
        model(models::SIGNAL_RED),
        model(models::SIGNAL_YELLOW),
        model(models::SIGNAL_WHITE),
    ) else {
        return;
    };
    let (mut cubes, mut glows, mut glows2) = (Vec::new(), Vec::new(), Vec::new());
    SIGNALS.with_borrow_mut(|s| {
        s.retain(|pos, l| {
            let block = level
                .block_state(host, *pos)
                .and_then(states::id)
                .and_then(|id| client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten());
            if !block.is_some_and(|b| create_engine::dye::NIXIE_TUBES.contains(&b)) {
                return false;
            }
            let d2: f64 = (0..3)
                .map(|i| (f64::from(pos[i]) + 0.5 - eye[i]).powi(2))
                .sum();
            let near = d2 < GLOW_VIEW_DISTANCE * GLOW_VIEW_DISTANCE;
            let white_color = 0xFFFF_FFFFu32;
            for first in [true, false] {
                use create_engine::trains::SignalState;
                let yellow = l.signal == SignalState::Yellow;
                let lit = if first {
                    l.signal.red_light_at(time)
                } else {
                    yellow || l.signal == SignalState::Green
                };
                if !lit {
                    continue;
                }
                let flip = first == l.invert;
                let at = l
                    .pose
                    .translate([if flip { 4.0 / 16.0 } else { -4.0 / 16.0 }, 0.0, 0.0]);
                // The yellow light's glow and tube sample `signal_glow_2`, the others
                // `signal_glow`.
                let out = if !first && yellow {
                    &mut glows2
                } else {
                    &mut glows
                };
                if near {
                    let vert = first ^ l.horizontal;
                    let (long, long_glow) = if yellow { (1.0, 2.0) } else { (4.0, 5.125) };
                    let cube = at.scale(if vert {
                        [long, 1.0, 1.0]
                    } else {
                        [1.0, long, 1.0]
                    });
                    models::quads(&cube, &white_cube, white_color, &mut cubes);
                    let glow = at.scale(if vert {
                        [long_glow, 2.0, 2.0]
                    } else {
                        [2.0, long_glow, 2.0]
                    });
                    let glow_model = if first {
                        &red_glow
                    } else if yellow {
                        &yellow_glow
                    } else {
                        &white_glow
                    };
                    models::quads(&glow, glow_model, white_color, out);
                }
                let tube = at.scale([1.0 + 1.0 / 16.0; 3]);
                let tube_model = if first {
                    &red
                } else if yellow {
                    &yellow_tube
                } else {
                    &white
                };
                models::quads(&tube, tube_model, white_color, out);
            }
            true
        });
    });
    for (t, quads) in [
        (Textured::SignalCube, cubes),
        (Textured::SignalGlow, glows),
        (Textured::SignalGlow2, glows2),
    ] {
        if !quads.is_empty()
            && let Some(p) = crate::glue::pipelines::resolve(host, t)
        {
            let _ = event.draw.pipeline_quads(p, &quads);
        }
    }
}

/// `NixieTubeRenderer`: the tube's two characters, `left` and `right`, in its colour, each
/// flickering toward its dark colour (`drawTube`).
fn draw_nixie_text(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    state: mod_api::BlockState,
    block: Block,
    facing: Dir,
    left: &str,
    right: &str,
) {
    let Some(i) = create_engine::dye::NIXIE_TUBES
        .iter()
        .position(|b| *b == block)
    else {
        return;
    };
    let text = host.block_state_string(state).unwrap_or_default();
    let (_, props) = states::split(&text);
    let Some(face) = props
        .iter()
        .find(|(k, _)| *k == "double_face")
        .and_then(|(_, v)| Face::read(v))
    else {
        return;
    };
    let colour = create_engine::dye::DyeColor::ALL[i];
    let texts = {
        let draw = &*draw;
        let flicker = [flicker(), flicker()];
        display::nixie_flickering(
            face,
            facing,
            colour,
            [left, right],
            |s| width(draw, s),
            flicker,
        )
    };
    for t in texts {
        draw_text(draw, &t.pose, &t.text, t.color, true);
    }
}

/// `FlapDisplayRenderer`: the board's lines laid out from its controller.
fn draw_board(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    facing: Dir,
) {
    let options = options(host);
    let ticks = draw.game_time();
    let texts = DISPLAYS.with_borrow(|d| {
        let b = d.boards.get(&pos)?;
        let draw = &*draw;
        Some(display::board_texts(
            facing,
            b.x_size,
            &b.lines,
            &b.colour,
            &b.glowing,
            &options,
            b.running,
            ticks,
            |s| width(draw, s),
        ))
    });
    for t in texts.unwrap_or_default() {
        draw_text(draw, &t.pose, &t.text, t.color, t.full_bright);
    }
}

/// A nixie tube riding a moving structure: the characters it showed when the structure took it
/// (`NixieTubeRenderer` in `renderBlockEntities`).
pub(super) fn draw_carried(
    host: &dyn ClientHost,
    block: Block,
    event: &mut BlockEntitySubmitEvent<'_>,
) {
    let carried = crate::glue::contraptions::actors::SHOWN_CARRIED;
    let Some(Tag::Compound(c)) = event.entity.nbt.get(&carried.into()) else {
        return;
    };
    if block == Block::DisplayBoard {
        draw_carried_board(host, event, c);
        return;
    }
    if !display::is_nixie(block) {
        return;
    }
    let Some(Tag::List(shown)) = c.get(&super::carried::SHOWN.into()) else {
        return;
    };
    let text = |i: usize| match shown.get(i) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let Some(id) = states::id(event.state) else {
        return;
    };
    let Some(facing) = client_table(host, |t| t.get(id).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    draw_nixie_text(
        host,
        event.draw,
        event.state,
        block,
        facing,
        &text(0),
        &text(1),
    );
}

/// A controlling display board riding a moving structure: its lines as it showed them when the
/// structure took it, settled (`FlapDisplayRenderer` in `renderBlockEntities`; a board on a
/// structure has no speed, so its flaps stand still).
fn draw_carried_board(
    host: &dyn ClientHost,
    event: &mut BlockEntitySubmitEvent<'_>,
    c: &mod_api::Compound,
) {
    let Some(b) = super::carried::board_of(c) else {
        return;
    };
    let Some(id) = states::id(event.state) else {
        return;
    };
    let Some(facing) = client_table(host, |t| t.get(id).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let options = options(host);
    let lines: Vec<(String, Vec<display::ShownSection>)> = b
        .lines
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            let sections = l
                .sections
                .into_iter()
                .map(|mut s| {
                    s.transition = false;
                    let n = options.get(s.cycle as usize).map_or(1, Vec::len);
                    display::ShownSection::new(s, n)
                })
                .collect();
            (l.key, sections)
        })
        .collect();
    let draw = &mut *event.draw;
    let ticks = draw.game_time();
    let texts = {
        let draw = &*draw;
        display::board_texts(
            facing,
            b.x_size,
            &lines,
            &b.colour,
            &b.glowing,
            &options,
            false,
            ticks,
            |s| width(draw, s),
        )
    };
    for t in texts {
        draw_text(draw, &t.pose, &t.text, t.color, t.full_bright);
    }
}
