//! The schedule (`ScheduleItem`, `ScheduleScreen`, `ScheduleEditPacket`): using it opens its
//! screen, where entries (a station to travel to, a title, a top speed) and the conditions to
//! wait on at a station are added, edited, moved and removed; closing it writes the schedule
//! back onto the item in the hand it was opened from, under Create's component name in its
//! custom data. The schedule's screen is the mod's own: Create's opens a menu with the player's
//! inventory for item and fluid conditions, which the port doesn't run.

use super::gui::{Reader, click_sound, icon_button, inside, millis, put_i32, put_str, tooltip, tr};
use crate::glue::net;
use create_engine::schedule::{Condition, Entry, Instruction, Schedule, TimeUnit};
use mod_api::{
    ClientHost, Compound, DrawContext, Flow, HostSide, InteractionResult, ItemPart, ItemStack,
    PlayerRef, Registrar, ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, Tag, UseItemEvent,
};
use std::cell::RefCell;

/// Server to client: open the screen; client to server: the schedule written.
pub const CHANNEL: &str = "create:schedule_screen";
const MOD: &str = "create";
const CUSTOM_DATA: &str = "minecraft:custom_data";
/// `AllDataComponents.TRAIN_SCHEDULE`.
const SCHEDULE_KEY: &str = "create:train_schedule";

pub fn register(r: &mut dyn Registrar) {
    let key = r.item_key("create:schedule");
    r.item_part(key, ScheduleItem);
}

// ---------------------------------------------------------------------------------------------
// The schedule on a stack

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.components()?.as_nbt().get(&CUSTOM_DATA.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// The stack's schedule (`ScheduleItem.getSchedule`), None without one.
pub fn schedule_of(stack: &ItemStack) -> Option<Schedule> {
    match custom(stack)?.get(&SCHEDULE_KEY.into()) {
        Some(Tag::Compound(c)) => Some(super::saved::schedule_from_nbt(c)),
        _ => None,
    }
}

/// The stack with `schedule` on it.
pub fn with_schedule(stack: &ItemStack, schedule: &Schedule) -> ItemStack {
    let mut stack = stack.clone();
    let mut patch = stack
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    let mut c = custom(&stack).cloned().unwrap_or_default();
    c.insert(
        SCHEDULE_KEY.into(),
        Tag::Compound(super::saved::schedule_to_nbt(schedule)),
    );
    patch.insert(CUSTOM_DATA.into(), Tag::Compound(c));
    if let Err(e) = stack.set_components(patch) {
        crate::create_log!("[create] the schedule could not be written: {e:?}");
    }
    stack
}

// ---------------------------------------------------------------------------------------------
// The wire

fn put_schedule(out: &mut Vec<u8>, s: &Schedule) {
    out.push(u8::from(s.cyclic));
    put_i32(out, s.saved_progress as i32);
    put_i32(out, s.entries.len() as i32);
    for e in &s.entries {
        match &e.instruction {
            Instruction::Destination(t) => {
                out.push(0);
                put_str(out, t);
            }
            Instruction::Rename(t) => {
                out.push(1);
                put_str(out, t);
            }
            Instruction::Throttle(v) => {
                out.push(2);
                put_i32(out, *v);
            }
        }
        put_i32(out, e.conditions.len() as i32);
        for g in &e.conditions {
            put_i32(out, g.len() as i32);
            for c in g {
                put_condition(out, c);
            }
        }
    }
}

fn put_condition(out: &mut Vec<u8>, c: &Condition) {
    let (kind, a, b, d) = match *c {
        Condition::Delay { value, unit } => (0, value, unit.ordinal(), 0),
        Condition::TimeOfDay {
            hour,
            minute,
            rotation,
        } => (1, hour, minute, rotation),
        Condition::PlayerCount { count, exact } => (2, count, i32::from(exact), 0),
        Condition::Idle { value, unit } => (3, value, unit.ordinal(), 0),
        Condition::Powered => (4, 0, 0, 0),
    };
    out.push(kind);
    for v in [a, b, d] {
        put_i32(out, v);
    }
}

fn read_condition(r: &mut Reader<'_>) -> Option<Condition> {
    let kind = r.u8()?;
    let (a, b, d) = (r.i32()?, r.i32()?, r.i32()?);
    Some(match kind {
        0 => Condition::Delay {
            value: a,
            unit: TimeUnit::from_ordinal(b),
        },
        1 => Condition::TimeOfDay {
            hour: a,
            minute: b,
            rotation: d,
        },
        2 => Condition::PlayerCount {
            count: a,
            exact: b != 0,
        },
        3 => Condition::Idle {
            value: a,
            unit: TimeUnit::from_ordinal(b),
        },
        _ => Condition::Powered,
    })
}

fn read_schedule(r: &mut Reader<'_>) -> Option<Schedule> {
    let cyclic = r.bool()?;
    let saved_progress = r.i32()?.max(0) as usize;
    let n = r.i32()?.clamp(0, 1000);
    let mut entries = Vec::new();
    for _ in 0..n {
        let instruction = match r.u8()? {
            0 => Instruction::Destination(r.str()?),
            1 => Instruction::Rename(r.str()?),
            _ => Instruction::Throttle(r.i32()?.clamp(5, 100)),
        };
        let groups = r.i32()?.clamp(0, 100);
        let mut conditions = Vec::new();
        for _ in 0..groups {
            let k = r.i32()?.clamp(0, 100);
            conditions.push(
                (0..k)
                    .map(|_| read_condition(r))
                    .collect::<Option<Vec<_>>>()?,
            );
        }
        entries.push(Entry {
            instruction,
            conditions,
        });
    }
    Some(Schedule {
        entries,
        cyclic,
        saved_progress,
    })
}

/// The hotbar slot of the schedule and the schedule.
fn encode(slot: u8, s: &Schedule) -> Vec<u8> {
    let mut out = vec![slot];
    put_schedule(&mut out, s);
    out
}

fn decode(data: &[u8]) -> Option<(u8, Schedule)> {
    let mut r = Reader(data);
    let slot = r.u8()?;
    Some((slot, read_schedule(&mut r)?))
}

// ---------------------------------------------------------------------------------------------
// Server

struct ScheduleItem;

#[mod_api::horizon_part]
impl ItemPart for ScheduleItem {
    /// `ScheduleItem.use`: not sneaking, the main hand's schedule opens its screen.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        if event.hand != mod_api::Hand::Main {
            return None;
        }
        let HostSide::Server(host) = host else {
            return Some(InteractionResult::Success);
        };
        let player = host.player(event.player)?;
        if crate::glue::items::sneaking(&*host, player) {
            return None;
        }
        let slot = player.selected_slot(&*host).unwrap_or(0) as u8;
        let schedule = schedule_of(event.stack).unwrap_or_default();
        net::send_to_player(host, player, CHANNEL, &encode(slot, &schedule));
        Some(InteractionResult::SuccessServer)
    }

    /// `ScheduleItem.appendHoverText`: each destination's filter, the one at the saved
    /// progress marked when there are several entries.
    fn tooltip(&self, _host: &dyn ClientHost, event: &mut mod_api::TooltipEvent<'_>) {
        let Some(s) = schedule_of(event.stack) else {
            return;
        };
        let styled = |text: &str, colour: &str| {
            let mut style = mod_api::text::TextStyle::default();
            style.color = Some(colour.to_owned());
            mod_api::Text::literal(text.to_owned()).with_style(style)
        };
        for (i, e) in s.entries.iter().enumerate() {
            let Instruction::Destination(filter) = &e.instruction else {
                continue;
            };
            let current = i == s.saved_progress && s.entries.len() > 1;
            let (prefix, colour) = if current {
                ("-> ", "yellow")
            } else {
                ("> ", "gold")
            };
            let line = styled(prefix, "gray").append(styled(filter, colour));
            event.lines.push((line, u32::MAX));
        }
    }
}

/// `ScheduleEditPacket`: the schedule written onto the schedule in that hotbar slot.
pub fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some((slot, schedule)) = decode(data) else {
        return;
    };
    let slot = u32::from(slot);
    if slot >= 9 {
        return;
    }
    let stack = player
        .inventory_slots(&*host)
        .unwrap_or_default()
        .into_iter()
        .find(|(i, _)| *i == slot)
        .map(|(_, s)| s);
    if let Some(stack) = stack
        && stack.item().id(&*host) == "create:schedule"
    {
        let written = with_schedule(&stack, &schedule);
        if let Err(e) = player.set_inventory_slot(host, slot, Some(written)) {
            crate::create_log!("[create] the schedule could not be kept: {e:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Client

/// What the editor edits (`IScheduleInput`).
#[derive(Clone, Debug, PartialEq)]
enum Field {
    Instruction(Instruction),
    Condition(Condition),
}

/// Where the edited part goes back when confirmed (`onEditorClose`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A new entry (`add_entry`), or entry `i`'s instruction.
    NewEntry,
    Instruction(usize),
    /// Condition `row` of group `col` of entry `entry`; a new one at the end of the group, or
    /// a new group.
    Condition {
        entry: usize,
        col: usize,
        row: usize,
    },
    AppendCondition {
        entry: usize,
        col: usize,
    },
    NewGroup {
        entry: usize,
    },
}

struct Editing {
    field: Field,
    target: Target,
    allow_delete: bool,
    /// The text box has the focus.
    typing: bool,
}

struct Open {
    schedule: Schedule,
    slot: u8,
    scroll: f32,
    editing: Option<Editing>,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

pub fn server_message(host: &mut dyn ClientHost, data: &[u8]) {
    let Some((slot, schedule)) = decode(data) else {
        return;
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            schedule,
            slot,
            scroll: 0.0,
            editing: None,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

pub fn tick_client(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for d in send {
        net::send_to_server(host, CHANNEL, &d);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &encode(open.slot, &open.schedule));
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

const W: f32 = 256.0;
const H: f32 = 226.0;
const CARD_HEADER: f32 = 22.0;
const CARD_WIDTH: f32 = 195.0;
const SHEET: &str = "create:gui/schedule";
const SHEET_2: &str = "create:gui/schedule_2";

/// `AllGuiTextures.SCHEDULE_*` in `schedule.png`.
const CARD_DARK: [f32; 4] = [5.0, 233.0, 1.0, 1.0];
const CARD_MEDIUM: [f32; 4] = [6.0, 233.0, 1.0, 1.0];
const CARD_LIGHT: [f32; 4] = [7.0, 233.0, 1.0, 1.0];
const CARD_MOVE_UP: [f32; 4] = [51.0, 230.0, 12.0, 12.0];
const CARD_MOVE_DOWN: [f32; 4] = [65.0, 230.0, 12.0, 12.0];
const CARD_REMOVE: [f32; 4] = [51.0, 243.0, 12.0, 12.0];
const CARD_DUPLICATE: [f32; 4] = [65.0, 243.0, 12.0, 12.0];
const CARD_NEW: [f32; 4] = [79.0, 239.0, 16.0, 16.0];
const CONDITION_NEW: [f32; 4] = [96.0, 239.0, 19.0, 16.0];
const CONDITION_LEFT: [f32; 4] = [116.0, 239.0, 6.0, 16.0];
const CONDITION_LEFT_CLEAN: [f32; 4] = [147.0, 239.0, 2.0, 16.0];
const CONDITION_MIDDLE: [f32; 4] = [123.0, 239.0, 1.0, 16.0];
const CONDITION_ITEM: [f32; 4] = [125.0, 239.0, 18.0, 16.0];
const CONDITION_RIGHT: [f32; 4] = [144.0, 239.0, 2.0, 16.0];
const CONDITION_APPEND: [f32; 4] = [150.0, 245.0, 10.0, 10.0];
const STRIP_DARK: [f32; 4] = [5.0, 235.0, 3.0, 1.0];
const STRIP_LIGHT: [f32; 4] = [5.0, 237.0, 3.0, 1.0];
const STRIP_WAIT: [f32; 4] = [1.0, 239.0, 11.0, 16.0];
const STRIP_TRAVEL: [f32; 4] = [12.0, 239.0, 11.0, 16.0];
const STRIP_DOTTED: [f32; 4] = [23.0, 239.0, 11.0, 16.0];
const STRIP_END: [f32; 4] = [34.0, 239.0, 11.0, 16.0];
const STRIP_ACTION: [f32; 4] = [209.0, 239.0, 11.0, 16.0];
const POINTER: [f32; 4] = [185.0, 239.0, 21.0, 16.0];
/// `SCHEDULE_EDITOR` in `schedule_2.png`, the data boxes in `display_link.png`.
const EDITOR: [f32; 4] = [0.0, 0.0, 256.0, 89.0];
const INACTIVE_SLOT: [f32; 4] = [0.0, 91.0, 18.0, 18.0];
const DATA_AREA_SPEECH: [f32; 4] = [8.0, 163.0, 5.0, 18.0];
const DATA_AREA_START: [f32; 4] = [0.0, 163.0, 2.0, 18.0];
const DATA_AREA: [f32; 4] = [3.0, 163.0, 1.0, 18.0];
const DATA_AREA_END: [f32; 4] = [5.0, 163.0, 2.0, 18.0];
/// `AllIcons`.
const I_CONFIRM: [f32; 2] = [0.0, 16.0];
const I_TRASH: [f32; 2] = [16.0, 0.0];
const I_REFRESH: [f32; 2] = [48.0, 16.0];
const I_PRIORITY_VERY_HIGH: [f32; 2] = [112.0, 0.0];
const I_PRIORITY_LOW: [f32; 2] = [80.0, 0.0];

fn corner(size: [f32; 2]) -> (f32, f32) {
    (((size[0] - W) / 2.0).floor(), ((size[1] - H) / 2.0).floor())
}

fn sprite(draw: &mut dyn DrawContext, sheet: &str, src: [f32; 4], at: [f32; 2]) {
    draw.sprite(sheet, src, [at[0], at[1], src[2], src[3]], None);
}

/// `UIRenderHelper.drawStretched`.
fn stretched(draw: &mut dyn DrawContext, src: [f32; 4], rect: [f32; 4]) {
    draw.sprite(SHEET, src, rect, None);
}

fn item(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

fn two(v: i32) -> String {
    format!("{v:02}")
}

/// `TimeOfDayCondition.getDigitalDisplay`.
fn digital(host: &dyn ClientHost, hour: i32, minute: i32) -> String {
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let ampm = tr(
        host,
        if hour > 11 {
            "create.generic.daytime.pm"
        } else {
            "create.generic.daytime.am"
        },
        &[],
    );
    tr(
        host,
        "create.schedule.condition.time_of_day.digital_format",
        &[h12.to_string(), hour.to_string(), two(minute), ampm],
    )
}

/// `getSummary`: the item drawn in a field and its text.
fn summary(host: &dyn ClientHost, f: &Field) -> (Option<&'static str>, String) {
    match f {
        Field::Instruction(Instruction::Destination(t)) => {
            (Some("create:track_station"), t.clone())
        }
        Field::Instruction(Instruction::Rename(t)) => (Some("minecraft:name_tag"), t.clone()),
        Field::Instruction(Instruction::Throttle(v)) => (Some("create:controls"), format!("{v}%")),
        Field::Condition(c) => match *c {
            Condition::Delay { value, unit } => (
                None,
                tr(
                    host,
                    "create.schedule.condition.delay_short",
                    &[format!("{value}{}", unit.suffix())],
                ),
            ),
            Condition::Idle { value, unit } => (
                None,
                tr(
                    host,
                    "create.schedule.condition.idle_short",
                    &[format!("{value}{}", unit.suffix())],
                ),
            ),
            Condition::TimeOfDay { hour, minute, .. } => {
                (Some("minecraft:clock"), digital(host, hour, minute))
            }
            Condition::PlayerCount { count, .. } => (
                Some("create:yellow_seat"),
                tr(
                    host,
                    if count == 1 {
                        "create.schedule.condition.player_count.summary"
                    } else {
                        "create.schedule.condition.player_count.summary_plural"
                    },
                    &[count.to_string()],
                ),
            ),
            Condition::Powered => (None, tr(host, "create.schedule.condition.powered", &[])),
        },
    }
}

/// `getFieldSize`.
fn field_size(draw: &dyn DrawContext, min: f32, s: &(Option<&str>, String)) -> f32 {
    (draw.text_width(&s.1) + if s.0.is_some() { 20.0 } else { 0.0 } + 16.0).max(min)
}

/// `renderInput`: a field with its item and text; its width.
fn render_input(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    s: &(Option<&str>, String),
    at: [f32; 2],
    clean: bool,
    min: f32,
) -> f32 {
    let size = field_size(&*draw, min, s).min(150.0);
    stretched(draw, CONDITION_MIDDLE, [at[0], at[1], size, 16.0]);
    if clean {
        sprite(draw, SHEET, CONDITION_LEFT_CLEAN, at);
    } else {
        sprite(draw, SHEET, CONDITION_LEFT, [at[0] - 3.0, at[1]]);
    }
    sprite(draw, SHEET, CONDITION_RIGHT, [at[0] + size - 2.0, at[1]]);
    if let Some(id) = s.0 {
        sprite(draw, SHEET, CONDITION_ITEM, [at[0] + 3.0, at[1]]);
        if let Some(k) = item(host, id) {
            draw.item(k, [at[0] + 4.0, at[1]], 16.0);
        }
    }
    let x = if s.0.is_some() { 28.0 } else { 8.0 };
    let mut text = s.1.clone();
    while draw.text_width(&text) > 120.0 {
        text.pop();
    }
    draw.text(&text, [at[0] + x, at[1] + 4.0], 0xFFF2_F2EE, true);
    size
}

fn card_height(e: &Entry) -> f32 {
    let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
    CARD_HEADER
        + if e.instruction.supports_conditions() {
            24.0 + rows * 18.0
        } else {
            4.0
        }
}

fn column_width(host: &dyn ClientHost, draw: &dyn DrawContext, g: &[Condition]) -> f32 {
    g.iter()
        .map(|c| field_size(draw, 32.0, &summary(host, &Field::Condition(*c))))
        .fold(0.0, f32::max)
}

pub fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        o.size = size;
        let mut tips = Vec::new();
        draw_schedule(host, draw, o, mouse, &mut tips);
        if o.editing.is_some() {
            tips.clear();
            draw_editor(host, draw, o, mouse, &mut tips);
        }
        if !tips.is_empty() {
            tooltip(draw, &tips, mouse);
        }
    });
}

/// `renderBg` and `renderSchedule`.
fn draw_schedule(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &Open,
    mouse: [f32; 2],
    tips: &mut Vec<String>,
) {
    let (left, top) = corner(o.size);
    sprite(draw, SHEET, [0.0, 0.0, W, H], [left, top]);
    let title = tr(host, "item.create.schedule", &[]);
    let center = left + ((W - 8.0) / 2.0).floor();
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [center - (tw / 2.0).floor(), top + 4.0],
        0xFF50_5050,
        false,
    );
    stretched(draw, STRIP_DARK, [left + 33.0, top + 16.0, 3.0, 173.0]);
    let s = &o.schedule;
    let mut y_offset = 25.0;
    let scroll = -o.scroll;
    let visible = |y: f32, h: f32| y >= 16.0 && y + h <= 189.0;
    for i in 0..=s.entries.len() {
        if s.saved_progress == i && !s.entries.is_empty() {
            let expected = scroll + y_offset + 4.0;
            let actual = expected.clamp(18.0, 170.0);
            sprite(draw, SHEET, POINTER, [left, top + actual]);
        }
        if (i == 0 || s.entries.is_empty()) && visible(16.0 + scroll, 10.0) {
            stretched(
                draw,
                STRIP_LIGHT,
                [left + 33.0, top + 16.0 + scroll, 3.0, 10.0],
            );
        }
        if i == s.entries.len() {
            if i > 0 {
                y_offset += 9.0;
            }
            if visible(y_offset + scroll, 16.0) {
                sprite(
                    draw,
                    SHEET,
                    STRIP_END,
                    [left + 29.0, top + y_offset + scroll],
                );
                sprite(
                    draw,
                    SHEET,
                    CARD_NEW,
                    [left + 43.0, top + y_offset + scroll],
                );
            }
            break;
        }
        let e = &s.entries[i];
        let h = card_height(e);
        let card_y = y_offset + scroll;
        if visible(card_y, h) {
            draw_entry(host, draw, s, i, [left + 25.0, top + card_y]);
        }
        y_offset += h;
        if i + 1 < s.entries.len() {
            if visible(y_offset - 3.0 + scroll, 16.0) {
                sprite(
                    draw,
                    SHEET,
                    STRIP_DOTTED,
                    [left + 29.0, top + y_offset - 3.0 + scroll],
                );
            }
            y_offset += 10.0;
        }
    }
    // The buttons (hidden while editing).
    if o.editing.is_none() {
        let c = [left + W - 42.0, top + H - 30.0];
        icon_button(
            draw,
            c,
            I_CONFIRM,
            true,
            inside([c[0], c[1], 18.0, 18.0], mouse),
        );
        let b = [left + 21.0, top + 196.0];
        let hovered = inside([b[0], b[1], 18.0, 18.0], mouse);
        if s.cyclic && !hovered {
            // `IconButton.green`: `BUTTON_GREEN`.
            draw.sprite(
                "create:gui/widgets",
                [72.0, 0.0, 18.0, 18.0],
                [b[0], b[1], 18.0, 18.0],
                None,
            );
            draw.sprite(
                "create:gui/icons",
                [I_REFRESH[0], I_REFRESH[1], 16.0, 16.0],
                [b[0] + 1.0, b[1] + 1.0, 16.0, 16.0],
                None,
            );
        } else {
            icon_button(draw, b, I_REFRESH, true, hovered);
        }
        if hovered {
            tips.push(tr(host, "create.schedule.loop", &[]));
            tips.push(tr(
                host,
                if s.cyclic {
                    "create.gui.schematicannon.optionEnabled"
                } else {
                    "create.gui.schematicannon.optionDisabled"
                },
                &[],
            ));
            tips.push(tr(host, "create.schedule.loop1", &[]));
            tips.push(tr(host, "create.schedule.loop2", &[]));
        }
        let r = [left + 45.0, top + 196.0];
        let hovered = inside([r[0], r[1], 18.0, 18.0], mouse);
        icon_button(draw, r, I_PRIORITY_VERY_HIGH, s.saved_progress > 0, hovered);
        if hovered {
            tips.push(tr(host, "create.schedule.reset", &[]));
        }
        let k = [left + 63.0, top + 196.0];
        let hovered = inside([k[0], k[1], 18.0, 18.0], mouse);
        icon_button(draw, k, I_PRIORITY_LOW, s.entries.len() > 1, hovered);
        if hovered {
            tips.push(tr(host, "create.schedule.skip", &[]));
        }
        if tips.is_empty()
            && let Some(t) = action(host, o, mouse, None)
        {
            tips.extend(t);
        }
    }
    // The schedule itself, three times as large, beside the window.
    if let Some(k) = item(host, "create:schedule") {
        draw.item(k, [left + W, top + H - 56.0], 48.0);
    }
}

/// `renderScheduleEntry` and `renderScheduleConditions` for entry `i` at `at`.
fn draw_entry(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    s: &Schedule,
    i: usize,
    at: [f32; 2],
) {
    let e = &s.entries[i];
    let h = card_height(e);
    let (x, y) = (at[0], at[1]);
    let conditions = e.instruction.supports_conditions();
    stretched(draw, CARD_LIGHT, [x, y + 1.0, CARD_WIDTH, h - 2.0]);
    stretched(draw, CARD_LIGHT, [x + 1.0, y, CARD_WIDTH - 2.0, h]);
    stretched(
        draw,
        CARD_DARK,
        [x + 1.0, y + 1.0, CARD_WIDTH - 2.0, h - 2.0],
    );
    stretched(
        draw,
        CARD_MEDIUM,
        [x + 2.0, y + 2.0, CARD_WIDTH - 4.0, h - 4.0],
    );
    stretched(
        draw,
        if conditions { CARD_LIGHT } else { CARD_MEDIUM },
        [x + 2.0, y + 2.0, CARD_WIDTH - 4.0, CARD_HEADER],
    );
    sprite(draw, SHEET, CARD_REMOVE, [x + CARD_WIDTH - 14.0, y + 2.0]);
    sprite(
        draw,
        SHEET,
        CARD_DUPLICATE,
        [x + CARD_WIDTH - 14.0, y + h - 14.0],
    );
    if i > 0 {
        sprite(
            draw,
            SHEET,
            CARD_MOVE_UP,
            [x + CARD_WIDTH, y + CARD_HEADER - 14.0],
        );
    }
    if i + 1 < s.entries.len() {
        sprite(
            draw,
            SHEET,
            CARD_MOVE_DOWN,
            [x + CARD_WIDTH, y + CARD_HEADER],
        );
    }
    stretched(draw, STRIP_LIGHT, [x + 8.0, y, 3.0, h + 10.0]);
    sprite(
        draw,
        SHEET,
        if conditions {
            STRIP_TRAVEL
        } else {
            STRIP_ACTION
        },
        [x + 4.0, y + 6.0],
    );
    if conditions {
        sprite(draw, SHEET, STRIP_WAIT, [x + 4.0, y + 28.0]);
    }
    let sum = summary(host, &Field::Instruction(e.instruction.clone()));
    render_input(host, draw, &sum, [x + 26.0, y + 5.0], false, 100.0);
    if !conditions {
        return;
    }
    let mut x_offset = 26.0;
    for g in &e.conditions {
        let w = column_width(host, &*draw, g);
        for (r, c) in g.iter().enumerate() {
            let sum = summary(host, &Field::Condition(*c));
            render_input(
                host,
                draw,
                &sum,
                [x + x_offset, y + 29.0 + r as f32 * 18.0],
                r != 0,
                w,
            );
        }
        sprite(
            draw,
            SHEET,
            CONDITION_APPEND,
            [
                x + x_offset + ((w - 10.0) / 2.0).floor(),
                y + 29.0 + g.len() as f32 * 18.0,
            ],
        );
        x_offset += w + 10.0;
    }
    sprite(draw, SHEET, CONDITION_NEW, [x + x_offset - 3.0, y + 29.0]);
}

/// What a click at `mouse` does on the schedule (`action`): with `click` None only its
/// tooltip; else the change. The tooltip's lines.
fn action(
    host: &dyn ClientHost,
    o: &Open,
    mouse: [f32; 2],
    click: Option<(u8, &mut Vec<Change>)>,
) -> Option<Vec<String>> {
    let (left, top) = corner(o.size);
    let mut x = mouse[0] - left - 25.0;
    let mut y = mouse[1] - top - 25.0;
    if !(0.0..205.0).contains(&x) || !(0.0..173.0).contains(&y) {
        return None;
    }
    y += o.scroll;
    let entries = &o.schedule.entries;
    let lmb = tr(host, "create.gui.schedule.lmb_edit", &[]);
    let rmb = tr(host, "create.gui.schedule.rmb_remove", &[]);
    let mut out = click;
    let mut push = |c: Change, button: u8| {
        if let Some((b, list)) = out.as_mut()
            && *b == button
        {
            list.push(c);
        }
    };
    for (i, e) in entries.iter().enumerate() {
        let h = card_height(e);
        if y >= h + 5.0 {
            y -= h + 10.0;
            if y < 0.0 {
                return None;
            }
            continue;
        }
        let sum = summary(host, &Field::Instruction(e.instruction.clone()));
        // `getFieldSize(100, ...)` with a font: the draw context isn't here, so the width is
        // measured as the client's font does.
        let text_w = host
            .text_width(&mod_api::Text::literal(sum.1.as_str()))
            .unwrap_or(0) as f32;
        let field = (text_w + 20.0 + 16.0).max(100.0);
        if x > 25.0 && x <= 25.0 + field && y > 4.0 && y <= 20.0 {
            push(Change::Edit(Target::Instruction(i)), 0);
            return Some(vec![
                tr(
                    host,
                    &format!("create.schedule.instruction.{}", e.instruction.id()),
                    &[],
                ),
                String::new(),
                lmb,
            ]);
        }
        if x > 180.0 && x <= 192.0 {
            if y > 0.0 && y <= 14.0 {
                push(Change::Remove(i), 0);
                return Some(vec![tr(host, "create.gui.schedule.remove_entry", &[])]);
            }
            if y > h - 14.0 {
                push(Change::Duplicate(i), 0);
                return Some(vec![tr(host, "create.gui.schedule.duplicate", &[])]);
            }
        }
        if x > 194.0 {
            if y > 7.0 && y <= 20.0 && i > 0 {
                push(Change::Move(i, i - 1), 0);
                return Some(vec![tr(host, "create.gui.schedule.move_up", &[])]);
            }
            if y > 20.0 && y <= 33.0 && i + 1 < entries.len() {
                push(Change::Move(i, i + 1), 0);
                return Some(vec![tr(host, "create.gui.schedule.move_down", &[])]);
            }
        }
        if !e.instruction.supports_conditions() {
            return None;
        }
        x -= 18.0;
        y -= 28.0;
        if x < 0.0 || y < 0.0 || x > 160.0 {
            return None;
        }
        x -= 8.0;
        for (j, g) in e.conditions.iter().enumerate() {
            if x < 0.0 {
                return None;
            }
            let w = g
                .iter()
                .map(|c| {
                    let s = summary(host, &Field::Condition(*c));
                    let tw = host
                        .text_width(&mod_api::Text::literal(s.1.as_str()))
                        .unwrap_or(0) as f32;
                    (tw + if s.0.is_some() { 20.0 } else { 0.0 } + 16.0).max(32.0)
                })
                .fold(0.0, f32::max);
            if x >= w {
                x -= w + 10.0;
                continue;
            }
            let row = (y / 18.0).floor() as i32;
            if row >= 0 && (row as usize) < g.len() {
                let row = row as usize;
                let can_remove = g.len() > 1 || e.conditions.len() > 1;
                let c = g[row];
                let mut lines = vec![tr(host, "create.schedule.condition_type", &[])];
                lines.push(tr(
                    host,
                    &format!("create.schedule.condition.{}", c.id()),
                    &[],
                ));
                lines.push(String::new());
                lines.push(lmb);
                if can_remove {
                    lines.push(rmb);
                    push(Change::RemoveCondition(i, j, row), 1);
                }
                push(
                    Change::Edit(Target::Condition {
                        entry: i,
                        col: j,
                        row,
                    }),
                    0,
                );
                return Some(lines);
            }
            let n = g.len() as f32;
            if y > 18.0 * n && y <= 18.0 * n + 10.0 && x >= w / 2.0 - 5.0 && x < w / 2.0 + 5.0 {
                push(
                    Change::Edit(Target::AppendCondition { entry: i, col: j }),
                    0,
                );
                return Some(vec![tr(host, "create.gui.schedule.add_condition", &[])]);
            }
            return None;
        }
        if x < 0.0 || x > 15.0 || y > 20.0 {
            return None;
        }
        push(Change::Edit(Target::NewGroup { entry: i }), 0);
        return Some(vec![tr(
            host,
            "create.gui.schedule.alternative_condition",
            &[],
        )]);
    }
    if x < 18.0 || x > 33.0 || y > 14.0 {
        return None;
    }
    push(Change::Edit(Target::NewEntry), 0);
    Some(vec![tr(host, "create.gui.schedule.add_entry", &[])])
}

/// A change a click asks for.
enum Change {
    Edit(Target),
    Remove(usize),
    Duplicate(usize),
    Move(usize, usize),
    RemoveCondition(usize, usize, usize),
}

fn apply(o: &mut Open, change: Change) {
    let s = &mut o.schedule;
    match change {
        Change::Remove(i) => {
            s.entries.remove(i);
        }
        Change::Duplicate(i) => {
            let e = s.entries[i].clone();
            s.entries.insert(i, e);
        }
        Change::Move(a, b) => {
            let e = s.entries.remove(a);
            s.entries.insert(b, e);
        }
        Change::RemoveCondition(i, j, row) => {
            let groups = &mut s.entries[i].conditions;
            groups[j].remove(row);
            if groups[j].is_empty() {
                groups.remove(j);
            }
        }
        Change::Edit(target) => {
            // `startEditing`: a copy of what is clicked, or a new part of Create's defaults.
            let (field, allow_delete) = match target {
                Target::NewEntry => (Field::Instruction(Instruction::default()), true),
                Target::Instruction(i) => {
                    (Field::Instruction(s.entries[i].instruction.clone()), false)
                }
                Target::Condition { entry, col, row } => {
                    let groups = &s.entries[entry].conditions;
                    let can_remove = groups[col].len() > 1 || groups.len() > 1;
                    (Field::Condition(groups[col][row]), can_remove)
                }
                Target::AppendCondition { .. } | Target::NewGroup { .. } => (
                    Field::Condition(Condition::of("delay").unwrap_or(Condition::Powered)),
                    true,
                ),
            };
            o.editing = Some(Editing {
                field,
                target,
                allow_delete,
                typing: false,
            });
        }
    }
    let n = o.schedule.entries.len();
    o.schedule.saved_progress = if n == 0 {
        0
    } else {
        o.schedule.saved_progress.min(n - 1)
    };
}

/// `onEditorClose`: confirmed, the part goes where it was edited; deleted, it goes away.
fn close_editor(o: &mut Open, confirmed: bool) {
    let Some(ed) = o.editing.take() else {
        return;
    };
    let s = &mut o.schedule;
    match (ed.target, ed.field) {
        (Target::NewEntry, Field::Instruction(i)) if confirmed => {
            let delay = Condition::of("delay").unwrap_or(Condition::Powered);
            s.entries.push(Entry {
                instruction: i,
                conditions: vec![vec![delay]],
            });
        }
        (Target::Instruction(n), Field::Instruction(i)) if confirmed => {
            if let Some(e) = s.entries.get_mut(n) {
                e.instruction = i;
            }
        }
        (Target::Condition { entry, col, row }, Field::Condition(c)) => {
            let groups = &mut s.entries[entry].conditions;
            if confirmed {
                groups[col][row] = c;
            } else {
                groups[col].remove(row);
                if groups[col].is_empty() {
                    groups.remove(col);
                }
            }
        }
        (Target::AppendCondition { entry, col }, Field::Condition(c)) if confirmed => {
            s.entries[entry].conditions[col].push(c);
        }
        (Target::NewGroup { entry }, Field::Condition(c)) if confirmed => {
            s.entries[entry].conditions.push(vec![c]);
        }
        _ => {}
    }
}

/// A value of the editor's line (`ModularGuiLineBuilder`): its x from the line's start, width,
/// and what it holds.
#[derive(Clone, Copy)]
enum Input {
    Text,
    /// A number in `min..max` stepped by `step`, shown with a suffix.
    Number {
        min: i32,
        max: i32,
        step: i32,
    },
    /// One of the options (lang keys).
    Select(&'static [&'static str]),
}

const UNITS: [&str; 3] = [
    "create.generic.unit.ticks",
    "create.generic.unit.seconds",
    "create.generic.unit.minutes",
];
const ROTATION_KEYS: [&str; 10] = [
    "create.schedule.condition.time_of_day.rotation.every_24",
    "create.schedule.condition.time_of_day.rotation.every_12",
    "create.schedule.condition.time_of_day.rotation.every_6",
    "create.schedule.condition.time_of_day.rotation.every_4",
    "create.schedule.condition.time_of_day.rotation.every_3",
    "create.schedule.condition.time_of_day.rotation.every_2",
    "create.schedule.condition.time_of_day.rotation.every_1",
    "create.schedule.condition.time_of_day.rotation.every_0_45",
    "create.schedule.condition.time_of_day.rotation.every_0_30",
    "create.schedule.condition.time_of_day.rotation.every_0_15",
];
const PLAYER_COUNT: [&str; 2] = [
    "create.schedule.condition.player_count.exactly",
    "create.schedule.condition.player_count.or_above",
];

/// `initConfigurationWidgets` of each part: `(x, width, input)`.
fn inputs(f: &Field) -> Vec<(f32, f32, Input)> {
    match f {
        Field::Instruction(Instruction::Destination(_) | Instruction::Rename(_)) => {
            vec![(0.0, 121.0, Input::Text)]
        }
        Field::Instruction(Instruction::Throttle(_)) => vec![(
            0.0,
            50.0,
            Input::Number {
                min: 5,
                max: 101,
                step: 5,
            },
        )],
        Field::Condition(Condition::Delay { .. } | Condition::Idle { .. }) => vec![
            (
                0.0,
                31.0,
                Input::Number {
                    min: 0,
                    max: 121,
                    step: 1,
                },
            ),
            (36.0, 85.0, Input::Select(&UNITS)),
        ],
        Field::Condition(Condition::TimeOfDay { .. }) => vec![
            (
                0.0,
                16.0,
                Input::Number {
                    min: 0,
                    max: 24,
                    step: 1,
                },
            ),
            (
                18.0,
                16.0,
                Input::Number {
                    min: 0,
                    max: 60,
                    step: 1,
                },
            ),
            (52.0, 62.0, Input::Select(&ROTATION_KEYS)),
        ],
        Field::Condition(Condition::PlayerCount { .. }) => vec![
            (
                0.0,
                31.0,
                Input::Number {
                    min: 0,
                    max: 21,
                    step: 1,
                },
            ),
            (36.0, 85.0, Input::Select(&PLAYER_COUNT)),
        ],
        Field::Condition(Condition::Powered) => Vec::new(),
    }
}

/// The value of input `k` of `f`.
fn value(f: &Field, k: usize) -> i32 {
    match (f, k) {
        (Field::Instruction(Instruction::Throttle(v)), 0) => *v,
        (Field::Condition(Condition::Delay { value, .. } | Condition::Idle { value, .. }), 0) => {
            *value
        }
        (Field::Condition(Condition::Delay { unit, .. } | Condition::Idle { unit, .. }), 1) => {
            unit.ordinal()
        }
        (Field::Condition(Condition::TimeOfDay { hour, .. }), 0) => *hour,
        (Field::Condition(Condition::TimeOfDay { minute, .. }), 1) => *minute,
        (Field::Condition(Condition::TimeOfDay { rotation, .. }), 2) => *rotation,
        (Field::Condition(Condition::PlayerCount { count, .. }), 0) => *count,
        (Field::Condition(Condition::PlayerCount { exact, .. }), 1) => i32::from(!*exact),
        _ => 0,
    }
}

fn set_value(f: &mut Field, k: usize, v: i32) {
    match (f, k) {
        (Field::Instruction(Instruction::Throttle(x)), 0) => *x = v,
        (Field::Condition(Condition::Delay { value, .. } | Condition::Idle { value, .. }), 0) => {
            *value = v;
        }
        (Field::Condition(Condition::Delay { unit, .. } | Condition::Idle { unit, .. }), 1) => {
            *unit = TimeUnit::from_ordinal(v);
        }
        (Field::Condition(Condition::TimeOfDay { hour, .. }), 0) => *hour = v,
        (Field::Condition(Condition::TimeOfDay { minute, .. }), 1) => *minute = v,
        (Field::Condition(Condition::TimeOfDay { rotation, .. }), 2) => *rotation = v,
        (Field::Condition(Condition::PlayerCount { count, .. }), 0) => *count = v,
        (Field::Condition(Condition::PlayerCount { exact, .. }), 1) => *exact = v == 0,
        _ => {}
    }
}

fn text_mut(f: &mut Field) -> Option<&mut String> {
    match f {
        Field::Instruction(Instruction::Destination(t) | Instruction::Rename(t)) => Some(t),
        _ => None,
    }
}

/// The editor's type options (`Schedule.INSTRUCTION_TYPES` / `CONDITION_TYPES` the port runs).
fn types(f: &Field) -> (&'static str, Vec<&'static str>) {
    match f {
        Field::Instruction(_) => ("instruction", Instruction::IDS.to_vec()),
        Field::Condition(_) => ("condition", Condition::IDS.to_vec()),
    }
}

fn type_index(f: &Field) -> usize {
    let id = match f {
        Field::Instruction(i) => i.id(),
        Field::Condition(c) => c.id(),
    };
    types(f).1.iter().position(|t| *t == id).unwrap_or(0)
}

fn with_type(f: &Field, i: usize) -> Field {
    let (_, ids) = types(f);
    let id = ids[i.min(ids.len() - 1)];
    match f {
        Field::Instruction(_) => Field::Instruction(Instruction::of(id).unwrap_or_default()),
        Field::Condition(_) => Field::Condition(Condition::of(id).unwrap_or(Condition::Powered)),
    }
}

/// The editor's rectangles from the window's corner: the type selector, confirm, delete, and
/// each input (`[x, y, w, h]`).
const TYPE_INPUT: [f32; 4] = [56.0, 65.0, 143.0, 16.0];
const EDITOR_CONFIRM: [f32; 4] = [224.0, 87.0, 18.0, 18.0];
const EDITOR_DELETE: [f32; 4] = [11.0, 87.0, 18.0, 18.0];

fn input_rect(x: f32, w: f32) -> [f32; 4] {
    [77.0 + x, 88.0, w, 18.0]
}

fn at(r: [f32; 4], left: f32, top: f32) -> [f32; 4] {
    [left + r[0], top + r[1], r[2], r[3]]
}

/// The editor over the schedule (`renderBg` while editing, its widgets).
fn draw_editor(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &Open,
    mouse: [f32; 2],
    tips: &mut Vec<String>,
) {
    let Some(ed) = &o.editing else {
        return;
    };
    let (left, top) = corner(o.size);
    let size = draw.size();
    draw.fill([0.0, 0.0, size[0], size[1]], 0xC010_1010);
    sprite(draw, SHEET_2, EDITOR, [left - 2.0, top + 40.0]);
    let (kind, ids) = types(&ed.field);
    let title = tr(host, &format!("create.schedule.{kind}.editor"), &[]);
    let center = left + ((W - 8.0) / 2.0).floor();
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [center - (tw / 2.0).floor(), top + 44.0],
        0xFF50_5050,
        false,
    );
    // The type (`scrollInputLabel`).
    let name = tr(
        host,
        &format!("create.schedule.{kind}.{}", ids[type_index(&ed.field)]),
        &[],
    );
    draw.text(&name, [left + 59.0, top + 69.0], 0xFFFF_FFFF, true);
    if inside(at(TYPE_INPUT, left, top), mouse) {
        tips.push(tr(host, &format!("create.schedule.{kind}_type"), &[]));
    }
    // Its icon.
    let (icon, _) = summary(host, &ed.field);
    let icon = icon.or(match ed.field {
        Field::Condition(Condition::Delay { .. } | Condition::Idle { .. }) => {
            Some("minecraft:repeater")
        }
        _ => None,
    });
    match icon.and_then(|i| item(host, i)) {
        Some(k) => {
            draw.item(k, [left + 54.0, top + 88.0], 16.0);
        }
        None => sprite(draw, SHEET_2, INACTIVE_SLOT, [left + 53.0, top + 87.0]),
    }
    // The line's boxes (`ModularGuiLine.renderWidgetBG`, a speech bubble first) and values.
    for (k, (x, w, input)) in inputs(&ed.field).into_iter().enumerate() {
        let r = at(input_rect(x, w), left, top);
        let (bx, bw) = match input {
            Input::Text => (r[0] - 5.0 + 5.0, w + 9.0 - 9.0),
            _ => (r[0], w),
        };
        let by = top + 87.0;
        draw.sprite(
            "create:gui/display_link",
            DATA_AREA,
            [bx, by, bw, 18.0],
            None,
        );
        if k == 0 {
            draw.sprite(
                "create:gui/display_link",
                DATA_AREA_SPEECH,
                [bx - 3.0, by, 5.0, 18.0],
                None,
            );
        } else {
            draw.sprite(
                "create:gui/display_link",
                DATA_AREA_START,
                [bx, by, 2.0, 18.0],
                None,
            );
        }
        draw.sprite(
            "create:gui/display_link",
            DATA_AREA_END,
            [bx + bw - 2.0, by, 2.0, 18.0],
            None,
        );
        let label_at = [r[0] + 5.0, top + 92.0];
        match input {
            Input::Text => {
                let mut t = match &ed.field {
                    Field::Instruction(Instruction::Destination(t) | Instruction::Rename(t)) => {
                        t.clone()
                    }
                    _ => String::new(),
                };
                if ed.typing && (millis() / 300).is_multiple_of(2) {
                    t.push('_');
                }
                while draw.text_width(&t) > w - 9.0 && !t.is_empty() {
                    t.remove(0);
                }
                draw.text(&t, label_at, 0xFFFF_FFFF, true);
                if inside(r, mouse) {
                    let base = match ed.field {
                        Field::Instruction(Instruction::Rename(_)) => {
                            "create.schedule.instruction.name_edit_box"
                        }
                        _ => "create.schedule.instruction.filter_edit_box",
                    };
                    tips.push(tr(host, base, &[]));
                    for n in 1..=3 {
                        let line = tr(host, &format!("{base}_{n}"), &[]);
                        if !line.starts_with("create.") {
                            tips.push(line);
                        }
                    }
                }
            }
            Input::Number { .. } => {
                let v = value(&ed.field, k);
                let text = match ed.field {
                    Field::Instruction(Instruction::Throttle(_)) => format!("{v}%"),
                    Field::Condition(Condition::TimeOfDay { .. }) => two(v),
                    _ => v.to_string(),
                };
                draw.text(&text, label_at, 0xFFFF_FFFF, true);
                if inside(r, mouse) {
                    let key = match ed.field {
                        Field::Instruction(Instruction::Throttle(_)) => {
                            "create.schedule.instruction.throttle_edit_box"
                        }
                        Field::Condition(Condition::TimeOfDay { .. }) if k == 0 => {
                            "create.generic.daytime.hour"
                        }
                        Field::Condition(Condition::TimeOfDay { .. }) => {
                            "create.generic.daytime.minute"
                        }
                        Field::Condition(Condition::PlayerCount { .. }) => {
                            "create.schedule.condition.player_count.players"
                        }
                        _ => "create.generic.duration",
                    };
                    tips.push(tr(host, key, &[]));
                }
            }
            Input::Select(options) => {
                let v = value(&ed.field, k).clamp(0, options.len() as i32 - 1) as usize;
                draw.text(&tr(host, options[v], &[]), label_at, 0xFFFF_FFFF, true);
                if inside(r, mouse) {
                    let key = match ed.field {
                        Field::Condition(Condition::TimeOfDay { .. }) => {
                            "create.schedule.condition.time_of_day.rotation"
                        }
                        Field::Condition(Condition::PlayerCount { .. }) => {
                            "create.schedule.condition.player_count.condition"
                        }
                        _ => "create.generic.timeUnit",
                    };
                    tips.push(tr(host, key, &[]));
                }
            }
        }
    }
    let c = at(EDITOR_CONFIRM, left, top);
    icon_button(draw, [c[0], c[1]], I_CONFIRM, true, inside(c, mouse));
    if ed.allow_delete {
        let d = at(EDITOR_DELETE, left, top);
        icon_button(draw, [d[0], d[1]], I_TRASH, true, inside(d, mouse));
    }
}

/// A step of the editor's input under `mouse`; true when one took it.
fn step_editor(o: &mut Open, mouse: [f32; 2], up: bool) -> bool {
    let (left, top) = corner(o.size);
    let Some(ed) = o.editing.as_mut() else {
        return false;
    };
    if inside(at(TYPE_INPUT, left, top), mouse) {
        let n = types(&ed.field).1.len();
        let i = type_index(&ed.field);
        let next = if up {
            i.saturating_sub(1)
        } else {
            (i + 1).min(n - 1)
        };
        if next != i {
            ed.field = with_type(&ed.field, next);
            ed.typing = false;
        }
        return true;
    }
    for (k, (x, w, input)) in inputs(&ed.field).into_iter().enumerate() {
        if !inside(at(input_rect(x, w), left, top), mouse) {
            continue;
        }
        match input {
            Input::Text => return false,
            Input::Number { min, max, step } => {
                let v = value(&ed.field, k);
                let v = if up { v + step } else { v - step };
                set_value(&mut ed.field, k, v.clamp(min, max - 1));
            }
            Input::Select(options) => {
                let v = value(&ed.field, k);
                let v = if up { v - 1 } else { v + 1 };
                set_value(&mut ed.field, k, v.clamp(0, options.len() as i32 - 1));
            }
        }
        return true;
    }
    false
}

pub fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let up = event.scroll[1] > 0.0;
    let mouse = event.screen.mouse;
    let stepped = OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return false;
        };
        if o.editing.is_some() {
            return step_editor(o, mouse, up);
        }
        // `mouseScrolled`: the list scrolls 12 a notch, within its length.
        let mut max = 40.0 - 173.0;
        for e in &o.schedule.entries {
            let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
            max += CARD_HEADER + 24.0 + rows * 18.0 + 10.0;
        }
        o.scroll = if max > 0.0 {
            (o.scroll - event.scroll[1] as f32 * 12.0).clamp(0.0, max)
        } else {
            0.0
        };
        false
    });
    if stepped {
        click_sound(host);
    }
    Flow::Cancel
}

pub fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let button = event.button as u8;
    let mut close = false;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (left, top) = corner(o.size);
        if let Some(ed) = o.editing.as_mut() {
            if button != 0 {
                return;
            }
            if inside(at(EDITOR_CONFIRM, left, top), mouse) {
                click_sound(host);
                close_editor(o, true);
                return;
            }
            if ed.allow_delete && inside(at(EDITOR_DELETE, left, top), mouse) {
                click_sound(host);
                close_editor(o, false);
                return;
            }
            let text_at = inputs(&ed.field)
                .into_iter()
                .find(|(_, _, i)| matches!(i, Input::Text))
                .map(|(x, w, _)| at(input_rect(x, w), left, top));
            ed.typing = text_at.is_some_and(|r| inside(r, mouse));
            if !ed.typing && step_editor(o, mouse, false) {
                click_sound(host);
            }
            return;
        }
        let s = &mut o.schedule;
        let b = |x: f32, y: f32| inside([left + x, top + y, 18.0, 18.0], mouse);
        if button == 0 {
            if b(W - 42.0, H - 30.0) {
                click_sound(host);
                close = true;
                return;
            }
            if b(21.0, 196.0) {
                click_sound(host);
                s.cyclic = !s.cyclic;
                return;
            }
            if b(45.0, 196.0) && s.saved_progress > 0 {
                click_sound(host);
                s.saved_progress = 0;
                return;
            }
            if b(63.0, 196.0) && s.entries.len() > 1 {
                click_sound(host);
                s.saved_progress = (s.saved_progress + 1) % s.entries.len();
                return;
            }
        }
        let mut changes = Vec::new();
        action(host, o, mouse, Some((button, &mut changes)));
        for c in changes {
            click_sound(host);
            apply(o, c);
        }
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| {
            l.0.push(encode(open.slot, &open.schedule));
            l.1 = true;
        });
    }
    Flow::Cancel
}

pub fn screen_key_pressed(host: &dyn ClientHost, event: &mut ScreenKeyPressedPreEvent<'_>) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let editing = OPEN.with_borrow(|o| o.as_ref().map(|o| o.editing.as_ref().map(|e| e.typing)));
    let Some(Some(typing)) = editing else {
        return Flow::Continue;
    };
    match event.key {
        "key.keyboard.backspace" if typing => {
            OPEN.with_borrow_mut(|o| {
                if let Some(t) = o
                    .as_mut()
                    .and_then(|o| o.editing.as_mut())
                    .and_then(|e| text_mut(&mut e.field))
                {
                    t.pop();
                }
            });
            Flow::Cancel
        }
        "key.keyboard.enter" | "key.keyboard.keypad.enter" if typing => {
            // `hitEnter`: the editor closes, confirmed.
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    close_editor(o, true);
                }
            });
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        _ if typing => Flow::Cancel,
        _ => Flow::Continue,
    }
}

pub fn screen_char_typed(host: &dyn ClientHost, event: &mut ScreenCharTypedPreEvent<'_>) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    OPEN.with_borrow_mut(|o| {
        if let Some(e) = o.as_mut().and_then(|o| o.editing.as_mut())
            && e.typing
            && super::gui::allowed(c)
            && let Some(t) = text_mut(&mut e.field)
            && t.chars().count() < 256
        {
            t.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: the schedule goes to the server (`ScheduleEditPacket`), at the next tick.
pub fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(encode(open.slot, &open.schedule)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_cross_the_wire() {
        let s = Schedule {
            entries: vec![
                Entry::station(
                    "Mine*",
                    vec![
                        vec![Condition::Delay {
                            value: 5,
                            unit: TimeUnit::Minutes,
                        }],
                        vec![
                            Condition::Powered,
                            Condition::PlayerCount {
                                count: 2,
                                exact: false,
                            },
                            Condition::TimeOfDay {
                                hour: 6,
                                minute: 15,
                                rotation: 3,
                            },
                        ],
                    ],
                ),
                Entry {
                    instruction: Instruction::Throttle(55),
                    conditions: Vec::new(),
                },
            ],
            cyclic: true,
            saved_progress: 1,
        };
        assert_eq!(decode(&encode(3, &s)), Some((3, s)));
    }
}
