//! The sequenced gearshift's instructions: its screen (`SequencedGearshiftScreen`), opened by a
//! click on the block (`SequencedGearshiftBlock.useItemOn`), and what it sends back
//! (`ConfigureSequencedGearshiftPacket`), kept in the block entity's data as Create keeps them
//! (`Instructions`, read again when the block entity loads), with where its sequence is
//! (`InstructionIndex` and the rest, published after all level subsystems finish).
//!
//! The screen draws Create's `sequencer` texture, a row per instruction with its three inputs
//! (kind, value, speed and direction) that the mouse wheel changes (`ScrollInput`), their
//! tooltips, the confirm button and the block's item; closing it sends the instructions.

use super::net;
use super::persist;
use super::server::ServerLevel;
use super::states;
use super::world::{level_index, with_world};
use create_engine::Block;
use create_engine::sequencer::{self as seq, Instruction, Kind, Speed};
use mod_api::text::TextStyle;
use mod_api::{
    BlockEntityRef, ClientHost, Compound, DrawContext, Flow, Host, InteractionResult, PlayerRef,
    Registrar, ScreenClosingEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, ServerHost, SoundPlay, Tag, Text, UseItemOnEvent,
};
use std::cell::RefCell;

/// The channel: the server opens the screen with the block's instructions, the client sends
/// them back.
pub const CHANNEL: &str = "create:sequencer";
/// The mod's own screen.
const MOD: &str = "create";
/// The block entity data key of the instructions (`SequencedGearshiftBlockEntity.write`).
const INSTRUCTIONS: &str = "Instructions";

const TEXTURE: &str = "create:gui/sequencer";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
/// `AllGuiTextures.SEQUENCER`'s size, and the window's offset (`setWindowOffset(-20, 0)`).
const SIZE: [f32; 2] = [173.0, 161.0];
const OFFSET_X: i32 = -20;
const ROW: f32 = 22.0;

pub fn register(r: &mut dyn Registrar) {
    r.block_part(states::key(Block::SequencedGearshift), Gearshift);
}

fn words(data: &[u8]) -> Vec<i32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| i32::from_le_bytes(*c))
        .collect()
}

fn bytes(words: &[i32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The instructions as `Instruction.serializeAll` writes them.
fn to_tag(list: &[Instruction]) -> Tag {
    Tag::List(
        list.iter()
            .map(|i| {
                Tag::Compound(Compound::from([
                    ("Type".into(), Tag::String(i.kind.name().into())),
                    ("Modifier".into(), Tag::String(i.speed.name().into())),
                    ("Value".into(), Tag::Int(i.value)),
                ]))
            })
            .collect(),
    )
}

/// `Instruction.deserializeAll`: none read gives none (the default list then).
fn of_tag(data: &Compound) -> Vec<Instruction> {
    let Some(Tag::List(l)) = data.get(&INSTRUCTIONS.into()) else {
        return Vec::new();
    };
    l.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else {
                return None;
            };
            let text = |k: &str| match c.get(&k.into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            let value = match c.get(&"Value".into()) {
                Some(Tag::Int(v)) => *v,
                _ => 0,
            };
            Some(Instruction {
                kind: Kind::of_name(&text("Type")),
                speed: Speed::of_name(&text("Modifier")),
                value,
            })
        })
        .take(seq::ROWS)
        .collect()
}

/// A sequenced gearshift's block entity loaded on the server: its saved instructions are its
/// own again.
pub fn loaded(host: &dyn Host, dim: usize, pos: [i32; 3], entity: BlockEntityRef) {
    let Some(data) = entity.data(host) else {
        return;
    };
    let list = of_tag(&data);
    if !list.is_empty() {
        with_world(|w| w.set_sequencer_instructions(dim, pos, list, false));
    }
    if let Some(run) = run_of_tag(&data) {
        with_world(|w| w.restore_sequencer(dim, pos, run));
    }
}

/// `InstructionIndex` and the rest of `SequencedGearshiftBlockEntity.write`'s running state.
const RUN_KEYS: [&str; 5] = [
    "InstructionIndex",
    "InstructionDuration",
    "InstructionProgress",
    "Timer",
    "PrevPowered",
];

fn run_of_tag(data: &Compound) -> Option<seq::Run> {
    let int = |k: &str| match data.get(&k.into()) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    };
    let progress = match data.get(&RUN_KEYS[2].into()) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    Some(seq::Run {
        current: int(RUN_KEYS[0])?,
        duration: int(RUN_KEYS[1]).unwrap_or(-1),
        progress,
        timer: int(RUN_KEYS[3]).unwrap_or(0),
        powered_before: matches!(data.get(&RUN_KEYS[4].into()), Some(Tag::Byte(b)) if *b != 0),
    })
}

/// Publish `SequencedGearshiftBlockEntity.write`'s current run, including progress and timer.
/// Called after all level mutations, as well as immediate instruction transitions.
pub fn save_run(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(run) = with_world(|w| w.sequencer_run(dim, pos)).flatten() else {
        return;
    };
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let saved = entity.data(&*lv.host).as_ref().and_then(run_of_tag);
    if saved == Some(run) {
        return;
    }
    let data = Compound::from([
        (RUN_KEYS[0].into(), Tag::Int(run.current)),
        (RUN_KEYS[1].into(), Tag::Int(run.duration)),
        (RUN_KEYS[2].into(), Tag::Float(run.progress.to_bits())),
        (RUN_KEYS[3].into(), Tag::Int(run.timer)),
        (RUN_KEYS[4].into(), Tag::Byte(i8::from(run.powered_before))),
    ]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

struct Gearshift;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Gearshift {
    /// `SequencedGearshiftBlock.useItemOn`: a wrench, or a kinetic block held against a shaft
    /// end, pass; else the screen opens.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        if let Some(stack) = click.stack {
            let item = stack.item().id(&*host);
            if item == "create:wrench" {
                return None;
            }
            let face = create_engine::kinetics::Dir::from_offset(click.hit.face)?;
            let kinetic_block = item
                .strip_prefix("create:")
                .and_then(|n| Block::ALL.iter().find(|b| b.name() == n))
                .is_some_and(|b| create_engine::world::keeps_state(*b));
            let dim = level_index(click.dimension.key().id())?;
            if kinetic_block
                && with_world(|w| w.has_shaft_towards(dim, click.pos, face)) == Some(true)
            {
                return None;
            }
        }
        let dim = level_index(click.dimension.key().id())?;
        let list = with_world(|w| w.sequencer_instructions(dim, click.pos)).flatten()?;
        let mut message = click.pos.to_vec();
        message.extend(seq::encode(&list));
        net::send_to_player(host, player, CHANNEL, &bytes(&message));
        Some(InteractionResult::Success)
    }
}

/// `ConfigureSequencedGearshiftPacket` on the server: from a player in reach of the block, the
/// instructions it sent are the gearshift's (its sequence stopped), and saved with it.
pub fn client_message(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    channel: &str,
    data: &[u8],
) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let w = words(data);
    let (Some(pos), Some(list)) = (
        w.get(..3).map(|p| [p[0], p[1], p[2]]),
        w.get(3..).and_then(seq::decode),
    ) else {
        return true;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return true;
    };
    // `BlockEntityConfigurationPacket`: within 20 blocks of it.
    let near = player.entity().position(&*host).is_some_and(|at| {
        (0..3)
            .map(|i| (at[i] - (f64::from(pos[i]) + 0.5)).powi(2))
            .sum::<f64>()
            <= 20.0 * 20.0
    });
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return true;
    };
    let dim = lv.dim;
    if !near || with_world(|w| w.block_at(dim, pos)).flatten() != Some(Block::SequencedGearshift) {
        return true;
    }
    let Some(step) =
        with_world(|w| w.set_sequencer_instructions(dim, pos, list.clone(), true)).flatten()
    else {
        return true;
    };
    // ConfigureSequencedGearshiftPacket.applySettings runs and commits run(-1) now,
    // before the next KineticBlockEntity.tick can attach the cleared block.
    super::kinetics::apply_sequencer_steps(&mut lv, vec![(pos, step)]);
    if let Some(entity) = lv.host.block_entity(lv.level, pos) {
        let data = Compound::from([(INSTRUCTIONS.into(), to_tag(&list))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
    persist::mark_pos_unsaved(dim, pos);
    true
}

// ---------------------------------------------------------------------------------------------
// The screen, on the client

struct Open {
    pos: [i32; 3],
    list: Vec<Instruction>,
    /// The screen's size when last drawn: where the window is for the mouse.
    size: [f32; 2],
}

#[derive(Default)]
struct Later {
    close: bool,
    send: Option<Open>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<Later> = RefCell::new(Later::default());
}

pub fn client_reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = Later::default());
}

/// The server opened the screen on a gearshift's instructions.
pub fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let w = words(data);
    let (Some(pos), Some(list)) = (
        w.get(..3).map(|p| [p[0], p[1], p[2]]),
        w.get(3..).and_then(seq::decode),
    ) else {
        return true;
    };
    if let Some(old) = OPEN.with_borrow_mut(Option::take) {
        send(host, old);
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            pos,
            list,
            size: [0.0; 2],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

/// `removed` → `sendPacket`.
fn send(host: &mut dyn ClientHost, open: Open) {
    let mut message = open.pos.to_vec();
    message.extend(seq::encode(&open.list));
    net::send_to_server(host, CHANNEL, &bytes(&message));
}

/// What the screen's hooks left for a tick, where the host may change.
pub fn client_tick(host: &mut dyn ClientHost) {
    let later = LATER.with_borrow_mut(std::mem::take);
    if later.close {
        host.mod_screen_close(MOD);
    }
    if let Some(open) = later.send {
        send(host, open);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        send(host, open);
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// `AbstractSimiScreen.init`: the window's corner.
fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] as i32 - SIZE[0] as i32) / 2 + OFFSET_X) as f32,
        ((size[1] as i32 - SIZE[1] as i32) / 2) as f32,
    )
}

/// The three inputs of row `row` (`initInputsOfRow`), as `[x, y, w, h]`.
fn inputs(left: f32, top: f32, row: usize) -> [[f32; 4]; 3] {
    let (x, y) = (left + 30.0, top + 20.0 + ROW * row as f32);
    [
        [x, y, 50.0, 18.0],
        [x + 58.0, y, 28.0, 18.0],
        [x + 88.0, y, 28.0, 18.0],
    ]
}

fn confirm(left: f32, top: f32) -> [f32; 4] {
    [left + SIZE[0] - 33.0, top + SIZE[1] - 24.0, 18.0, 18.0]
}

fn inside(r: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The input under `at`: its row and which of the three, if that input shows
/// (`updateParamsOfRow`: no value or speed for an instruction without one).
fn hovered(open: &Open, left: f32, top: f32, at: [f32; 2]) -> Option<(usize, usize)> {
    open.list.iter().enumerate().find_map(|(row, i)| {
        let shown = [true, i.kind.value_range().is_some(), i.kind.has_speed()];
        inputs(left, top, row)
            .iter()
            .enumerate()
            .find(|(n, r)| shown[*n] && inside(**r, at))
            .map(|(n, _)| (row, n))
    })
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(&format!("create.{key}"))
        .unwrap_or_else(|| key.to_owned())
}

fn styled(text: String, color: &str, italic: bool) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text).with_style(style)
}

pub fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(open) = o {
            open.size = event.draw.size();
            draw(host, event.draw, event.screen.mouse, open);
        }
    });
}

/// `renderWindow`, the confirm button, the block's item, and the hovered input's tooltip.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], open: &Open) {
    let (x, y) = corner(draw.size());
    draw.sprite(
        TEXTURE,
        [0.0, 0.0, SIZE[0], SIZE[1]],
        [x, y, SIZE[0], SIZE[1]],
        None,
    );
    for row in 0..seq::ROWS {
        let top = y + 16.0 + ROW * row as f32;
        // `SEQUENCER_EMPTY`, then each instruction's own background.
        let v = match open.list.get(row).map(|i| i.kind) {
            None => 104.0,
            Some(Kind::TurnAngle | Kind::TurnDistance) => 16.0,
            Some(Kind::Delay) => 60.0,
            Some(Kind::End) => 82.0,
            Some(Kind::Await) => 162.0,
        };
        draw.sprite(TEXTURE, [0.0, v, 162.0, 22.0], [x, top, 162.0, 22.0], None);
        let Some(i) = open.list.get(row) else {
            continue;
        };
        let y_offset = ROW * row as f32;
        let label = |draw: &mut dyn DrawContext, dx: f32, text: &str| {
            draw.text(
                text,
                [x + dx, y + 26.0 + y_offset - 1.0],
                0xFFFF_FFEE,
                false,
            );
        };
        let name = tr(
            host,
            &format!("gui.sequenced_gearshift.instruction.{}", i.kind.id()),
        );
        label(draw, 36.0, &name);
        if i.kind.value_range().is_some() {
            let text = i.kind.format_value(i.value);
            let w = (draw.text_width(&text) as i32 / 2) as f32;
            label(draw, 90.0 + (12.0 - w), &text);
        }
        if i.kind.has_speed() {
            label(draw, 127.0, i.speed.label());
        }
    }
    let title = tr(host, "gui.sequenced_gearshift.title");
    let tw = draw.text_width(&title);
    let tx = x + ((SIZE[0] as i32 - 8) / 2) as f32 - (tw as i32 / 2) as f32;
    draw.text(&title, [tx, y + 4.0], 0xFF59_2424, false);
    // The confirm button (`IconButton`, `I_CONFIRM`).
    let c = confirm(x, y);
    let u = if inside(c, mouse) { 18.0 } else { 0.0 };
    draw.sprite(WIDGETS, [u, 0.0, 18.0, 18.0], c, None);
    draw.sprite(
        ICONS,
        [0.0, 16.0, 16.0, 16.0],
        [c[0] + 1.0, c[1] + 1.0, 16.0, 16.0],
        None,
    );
    // `renderAdditional`: the block's item, five times its size, right of the window.
    if let Some(key) = mod_api::Identifier::parse("create:sequenced_gearshift")
        .ok()
        .and_then(|id| host.item_key(&id))
    {
        draw.item(key, [x + SIZE[0] + 6.0, y + SIZE[1] - 56.0], 80.0);
    }
    if let Some((row, n)) = hovered(open, x, y, mouse) {
        let lines = tooltip(host, &open.list[row], n);
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// `ScrollInput.updateTooltip` / `SelectionScrollInput.updateTooltip` of input `n` of a row.
fn tooltip(host: &dyn ClientHost, i: &Instruction, n: usize) -> Vec<Text> {
    const HEADER: &str = "#5391E1";
    let mut lines = Vec::new();
    let options = |title: String, names: Vec<String>, state: usize, lines: &mut Vec<Text>| {
        lines.push(styled(title, HEADER, false));
        for (k, name) in names.into_iter().enumerate() {
            if k == state {
                lines.push(styled(format!("-> {name}"), "white", false));
            } else {
                lines.push(styled(format!("> {name}"), "gray", false));
            }
        }
        lines.push(styled(
            tr(host, "gui.scrollInput.scrollToSelect"),
            "dark_gray",
            true,
        ));
    };
    match n {
        0 => {
            let names = Kind::ALL
                .iter()
                .map(|k| {
                    tr(
                        host,
                        &format!("gui.sequenced_gearshift.instruction.{}.descriptive", k.id()),
                    )
                })
                .collect();
            let state = Kind::ALL.iter().position(|k| *k == i.kind).unwrap_or(0);
            options(
                tr(host, "gui.sequenced_gearshift.instruction"),
                names,
                state,
                &mut lines,
            );
        }
        1 => {
            let param = i.kind.parameter().unwrap_or_default();
            lines.push(styled(
                tr(
                    host,
                    &format!(
                        "gui.sequenced_gearshift.instruction.{}.{param}",
                        i.kind.id()
                    ),
                ),
                HEADER,
                false,
            ));
            for key in [
                "gui.scrollInput.scrollToModify",
                "gui.scrollInput.shiftScrollsFaster",
            ] {
                lines.push(styled(tr(host, key), "dark_gray", true));
            }
        }
        _ => {
            let names = Speed::ALL
                .iter()
                .map(|s| {
                    let id = s.name().to_ascii_lowercase();
                    tr(host, &format!("gui.sequenced_gearshift.speed.{id}"))
                })
                .collect();
            let state = Speed::ALL.iter().position(|s| *s == i.speed).unwrap_or(0);
            options(
                tr(host, "gui.sequenced_gearshift.speed"),
                names,
                state,
                &mut lines,
            );
        }
    }
    lines
}

/// `ScrollInput.mouseScrolled` on the hovered input.
pub fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let up = event.scroll[1] > 0.0;
    let shift = event.modifiers & 1 != 0;
    let mouse = event.screen.mouse;
    let changed = OPEN.with_borrow_mut(|o| {
        let open = o.as_mut()?;
        // The screen's size is what the last drawing used: the window's corner from it.
        let (x, y) = corner(open.size);
        let (row, n) = hovered(open, x, y, mouse)?;
        let i = open.list[row];
        match n {
            // The selection inputs are inverted: the wheel down moves on.
            0 => {
                let state = Kind::ALL.iter().position(|k| *k == i.kind)? as i32;
                let next = seq::scroll(state, !up, false, (0, Kind::ALL.len() as i32), 1, false);
                (next != state).then(|| {
                    seq::set_kind(&mut open.list, row, Kind::ALL[next as usize]);
                    (next, Kind::ALL.len() as i32)
                })
            }
            1 => {
                let (max, shift_step, _) = i.kind.value_range()?;
                let delay = i.kind == Kind::Delay;
                let next = seq::scroll(i.value, up, shift, (1, max + 1), shift_step, delay);
                (next != i.value).then(|| {
                    open.list[row].value = next;
                    (next - 1, max)
                })
            }
            _ => {
                let state = Speed::ALL.iter().position(|s| *s == i.speed)? as i32;
                let next = seq::scroll(state, !up, false, (0, Speed::ALL.len() as i32), 1, false);
                (next != state).then(|| {
                    open.list[row].speed = Speed::ALL[next as usize];
                    (next, Speed::ALL.len() as i32)
                })
            }
        }
    });
    // `AllSoundEvents.SCROLL_VALUE` at `1.5 + 0.1 * (state - min) / (max - min)`.
    if let Some((state, span)) = changed {
        let pitch = 1.5 + 0.1 * state as f32 / span.max(1) as f32;
        host.play_local_sound(&SoundPlay::ui("create:scroll_value").with_pitch(pitch), 0);
    }
    Flow::Cancel
}

/// The confirm button closes the screen (`onClose`, which sends the instructions).
pub fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.button != 0 {
        return Flow::Continue;
    }
    let Some(size) = OPEN.with_borrow(|o| o.as_ref().map(|o| o.size)) else {
        return Flow::Continue;
    };
    let (x, y) = corner(size);
    if inside(confirm(x, y), event.screen.mouse) {
        host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
        if let Some(open) = OPEN.with_borrow_mut(Option::take) {
            LATER.with_borrow_mut(|l| {
                l.send = Some(open);
                l.close = true;
            });
        }
    }
    Flow::Cancel
}

/// `removed`: the instructions go to the server at the next tick.
pub fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.send = Some(open));
    }
}
