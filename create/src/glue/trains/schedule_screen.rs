//! The schedule (`ScheduleItem`, `ScheduleScreen`, `ScheduleEditPacket`): using it opens its
//! screen, where entries (a station to travel to, a title, a top speed) and the conditions to
//! wait on at a station are added, edited, moved and removed; closing it writes the schedule
//! back onto the item in the hand it was opened from, under Create's component name in its
//! custom data. While a part is edited, the player's inventory is shown under the editor and
//! the part's ghost slots (an item threshold's item, a fluid threshold's bucket, a redstone
//! link's two frequencies) take ghost copies of its stacks (`ScheduleMenu`).
//!
//! Left out (no API for it): an item-opened mod menu has no screen layout (`player_open_mod_menu`
//! without a block gives the client no `Screen`), so the screen is the mod's own and the
//! inventory shown is a copy the server sent when it opened: clicking a slot carries a ghost
//! copy of its stack, never the stack itself (as the filters' screens do).

use super::gui::{Reader, click_sound, icon_button, inside, millis, put_i32, tooltip, tr};
use crate::glue::items::filter_screen::{click_inventory, stack_bytes, stack_of};
use crate::glue::net;
use create_engine::schedule::{Condition, Entry, Instruction, Op, Schedule, Stack, TimeUnit};
use mod_api::{
    ClientHost, Compound, DrawContext, Flow, Host, HostSide, InteractionResult, ItemPart,
    ItemStack, PlayerRef, Registrar, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, ServerHost, Tag, UseItemEvent,
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

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_i32(out, b.len() as i32);
    out.extend_from_slice(b);
}

fn read_bytes(r: &mut Reader<'_>) -> Option<Vec<u8>> {
    let n = usize::try_from(r.i32()?).ok()?;
    if r.0.len() < n {
        return None;
    }
    let (b, rest) = r.0.split_at(n);
    r.0 = rest;
    Some(b.to_vec())
}

/// A schedule on the wire: its NBT as Create saves it (`Schedule.write`), ghost stacks and all.
fn put_schedule(out: &mut Vec<u8>, s: &Schedule) {
    let b = Tag::Compound(super::saved::schedule_to_nbt(s))
        .to_network_bytes()
        .unwrap_or_default();
    put_bytes(out, &b);
}

fn read_schedule(r: &mut Reader<'_>) -> Option<Schedule> {
    let b = read_bytes(r)?;
    match Tag::from_network_bytes(&b, None).ok()?.0 {
        Tag::Compound(c) => Some(super::saved::schedule_from_nbt(&c)),
        _ => None,
    }
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

/// The screen opened: the schedule, and the player's inventory (slot 0..35, the stack's
/// network NBT) for the ghost slots.
fn encode_open(slot: u8, s: &Schedule, inventory: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = encode(slot, s);
    out.push(inventory.len() as u8);
    for (i, b) in inventory {
        out.push(*i);
        put_bytes(&mut out, b);
    }
    out
}

type Opened = (u8, Schedule, Vec<(u8, Vec<u8>)>);

fn decode_open(data: &[u8]) -> Option<Opened> {
    let mut r = Reader(data);
    let slot = r.u8()?;
    let s = read_schedule(&mut r)?;
    let n = r.u8()?;
    let mut inventory = Vec::new();
    for _ in 0..n {
        let i = r.u8()?;
        inventory.push((i, read_bytes(&mut r)?));
    }
    Some((slot, s, inventory))
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
        let h: &dyn Host = &*host;
        let inventory: Vec<(u8, Vec<u8>)> = player
            .inventory_slots(h)
            .unwrap_or_default()
            .into_iter()
            .filter(|(i, _)| *i < 36)
            .filter_map(|(i, s)| Some((i as u8, stack_bytes(h, &s)?)))
            .collect();
        net::send_to_player(
            host,
            player,
            CHANNEL,
            &encode_open(slot, &schedule, &inventory),
        );
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
    /// The ghost slots (`ScheduleMenu.ghostInventory`), given to the part when it closes.
    ghosts: [Option<ItemStack>; 2],
}

struct Open {
    schedule: Schedule,
    slot: u8,
    scroll: f32,
    /// Each entry's condition columns scrolled across (`horizontalScrolls`): the column index
    /// shown first, chased, as (previous tick, now, target).
    columns: Vec<[f32; 3]>,
    editing: Option<Editing>,
    size: [f32; 2],
    /// The player's inventory as the server sent it, and the slot whose ghost is carried.
    inventory: Vec<(u8, ItemStack)>,
    cursor: Option<u8>,
}

impl Open {
    fn stack(&self, slot: u8) -> Option<&ItemStack> {
        self.inventory
            .iter()
            .find(|(i, _)| *i == slot)
            .map(|(_, s)| s)
    }
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
    let Some((slot, schedule, inventory)) = decode_open(data) else {
        return;
    };
    let h: &dyn Host = &*host;
    let inventory = inventory
        .iter()
        .filter_map(|(i, b)| Some((*i, stack_of(h, b)?)))
        .collect();
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            schedule,
            slot,
            scroll: 0.0,
            columns: Vec::new(),
            editing: None,
            size: [0.0, 0.0],
            inventory,
            cursor: None,
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

pub fn tick_client(host: &mut dyn ClientHost) {
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            // `LerpedFloat.tickChaser` with `Chaser.EXP` at 0.5.
            for c in &mut o.columns {
                c[0] = c[1];
                let d = c[2] - c[1];
                c[1] = if d.abs() < 1.0 / 128.0 {
                    c[2]
                } else {
                    c[1] + d * 0.5
                };
            }
        }
    });
    flush(host);
}

/// What the hooks left: closing and sending, and the schedule once the screen is gone.
pub fn flush(host: &mut dyn ClientHost) {
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
const POINTER_OFFSCREEN: [f32; 4] = [171.0, 239.0, 13.0, 16.0];
const SCROLL_LEFT: [f32; 4] = [161.0, 247.0, 4.0, 8.0];
const SCROLL_RIGHT: [f32; 4] = [166.0, 247.0, 4.0, 8.0];
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

/// A stack of the item `id`.
fn stack_named(host: &dyn ClientHost, id: &str) -> Option<ItemStack> {
    item(host, id).map(ItemStack::of)
}

/// A ghost slot's stack from the bytes a condition keeps.
fn ghost(host: &dyn ClientHost, s: &Stack) -> Option<ItemStack> {
    if s.is_empty() {
        None
    } else {
        stack_of(host, s)
    }
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
fn summary(host: &dyn ClientHost, f: &Field) -> (Option<ItemStack>, String) {
    let named = |id: &str| stack_named(host, id);
    match f {
        Field::Instruction(Instruction::Destination(t)) => {
            (named("create:track_station"), t.clone())
        }
        Field::Instruction(Instruction::DeliverPackages) => (
            named("create:white_postbox"),
            tr(host, "create.schedule.instruction.package_delivery", &[]),
        ),
        Field::Instruction(Instruction::FetchPackages(_)) => (
            named("create:cardboard_package_12x12"),
            tr(host, "create.schedule.instruction.package_retrieval", &[]),
        ),
        Field::Instruction(Instruction::Rename(t)) => (named("minecraft:name_tag"), t.clone()),
        Field::Instruction(Instruction::Throttle(v)) => (named("create:controls"), format!("{v}%")),
        Field::Condition(c) => match *c {
            // `CargoThresholdCondition.getSummary`: the item, the operator and the threshold
            // with its unit.
            Condition::ItemThreshold {
                op,
                threshold,
                stacks,
                ref item,
            } => (
                ghost(host, item),
                format!(
                    "{} {threshold}{}",
                    op.symbol(),
                    if stacks { "\u{25A4}" } else { "" }
                ),
            ),
            Condition::FluidThreshold {
                op,
                threshold,
                ref item,
            } => (ghost(host, item), format!("{} {threshold}b", op.symbol())),
            Condition::RedstoneLink { inverted, .. } => (
                named("create:redstone_link"),
                tr(
                    host,
                    if inverted {
                        "create.schedule.condition.redstone_link_off"
                    } else {
                        "create.schedule.condition.redstone_link_on"
                    },
                    &[],
                ),
            ),
            Condition::Unloaded => (None, tr(host, "create.schedule.condition.unloaded", &[])),
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
                (named("minecraft:clock"), digital(host, hour, minute))
            }
            Condition::PlayerCount { count, .. } => (
                named("create:yellow_seat"),
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
fn field_size(draw: &dyn DrawContext, min: f32, s: &(Option<ItemStack>, String)) -> f32 {
    (draw.text_width(&s.1) + if s.0.is_some() { 20.0 } else { 0.0 } + 16.0).max(min)
}

/// `renderInput`: a field with its item and text; its width.
fn render_input(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    s: &(Option<ItemStack>, String),
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
    if let Some(stack) = &s.0 {
        sprite(draw, SHEET, CONDITION_ITEM, [at[0] + 3.0, at[1]]);
        draw.item_stack(stack, [at[0] + 4.0, at[1]], 16.0);
    }
    let _ = host;
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
        .map(|c| field_size(draw, 32.0, &summary(host, &Field::Condition(c.clone()))))
        .fold(0.0, f32::max)
}

/// `getConditionColumnWidth` for each of `e`'s columns, measured as the client's font does.
fn column_widths(host: &dyn ClientHost, e: &Entry) -> Vec<f32> {
    e.conditions
        .iter()
        .map(|g| {
            g.iter()
                .map(|c| {
                    let s = summary(host, &Field::Condition(c.clone()));
                    let tw = host
                        .text_width(&mod_api::Text::literal(s.1.as_str()))
                        .unwrap_or(0) as f32;
                    (tw + if s.0.is_some() { 20.0 } else { 0.0 } + 16.0).max(32.0)
                })
                .fold(0.0, f32::max)
        })
        .collect()
}

/// `isConditionAreaScrollable`.
fn scrollable(widths: &[f32]) -> bool {
    26.0 + widths.iter().map(|w| w + 10.0).sum::<f32>() + 16.0 > CARD_WIDTH - 26.0
}

/// `getConditionScroll`: how far `widths`' columns are scrolled at column index `index`.
fn column_scroll(widths: &[f32], mut index: f32) -> f32 {
    let mut out = 0.0;
    for w in widths {
        let part = index.min(1.0);
        out += (w + 10.0) * part;
        index -= part;
    }
    out
}

impl Open {
    /// Entry `i`'s column chaser (`horizontalScrolls.get(i)`), at rest at 0 until scrolled.
    fn column(&self, i: usize) -> [f32; 3] {
        self.columns.get(i).copied().unwrap_or([0.0; 3])
    }

    fn chase_column(&mut self, i: usize, target: f32) {
        if self.columns.len() <= i {
            self.columns.resize(i + 1, [0.0; 3]);
        }
        self.columns[i][2] = target;
    }
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
    let partial = crate::glue::stock::keeper::partial_tick();
    // The cards are cut to the list (`enableScissor(leftPos + 16, topPos + 16, leftPos + 236,
    // topPos + 189)`).
    let _ = draw.push_clip([left + 16.0, top + 16.0, 220.0, 173.0]);
    for i in 0..=s.entries.len() {
        if s.saved_progress == i && !s.entries.is_empty() {
            let expected = scroll + y_offset + 4.0;
            let actual = expected.clamp(18.0, 170.0);
            let pointer = if expected == actual {
                POINTER
            } else {
                POINTER_OFFSCREEN
            };
            sprite(draw, SHEET, pointer, [left, top + actual]);
        }
        if i == 0 || s.entries.is_empty() {
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
            break;
        }
        let e = &s.entries[i];
        let h = card_height(e);
        let card_y = y_offset;
        draw_entry(host, draw, s, i, [left + 25.0, top + card_y + scroll]);
        y_offset += h;
        if i + 1 < s.entries.len() {
            sprite(
                draw,
                SHEET,
                STRIP_DOTTED,
                [left + 29.0, top + y_offset - 3.0 + scroll],
            );
            y_offset += 10.0;
        }
        if !e.instruction.supports_conditions() {
            continue;
        }
        // The conditions, scrolled across and cut to the card (`enableScissor(leftPos + 43,
        // 0, leftPos + 204, 400)` inside the list's).
        let widths = column_widths(host, e);
        let c = o.column(i);
        let index = c[0] + (c[1] - c[0]) * partial;
        let _ = draw.push_clip([left + 43.0, 0.0, 161.0, 400.0]);
        draw_conditions(
            host,
            draw,
            e,
            [left + 25.0, top + card_y + scroll],
            column_scroll(&widths, index),
            h,
        );
        let _ = draw.pop_clip();
        if scrollable(&widths) {
            let center = ((h - 8.0 + CARD_HEADER) / 2.0).floor();
            if c[2] != 0.0 {
                sprite(
                    draw,
                    SHEET,
                    SCROLL_LEFT,
                    [left + 40.0, top + card_y + scroll + center],
                );
            }
            if c[2] != (e.conditions.len() as f32 - 1.0) {
                sprite(
                    draw,
                    SHEET,
                    SCROLL_RIGHT,
                    [left + 203.0, top + card_y + scroll + center],
                );
            }
        }
    }
    let _ = draw.pop_clip();
    draw.fill_gradient([left + 16.0, top + 16.0, 220.0, 10.0], 0x7700_0000, 0);
    draw.fill_gradient([left + 16.0, top + 179.0, 220.0, 10.0], 0, 0x7700_0000);
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
}

/// `renderScheduleConditions` for entry `e`'s card at `at`, its columns scrolled `shift` across.
fn draw_conditions(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    e: &Entry,
    at: [f32; 2],
    shift: f32,
    h: f32,
) {
    let (x, y) = (at[0] - shift, at[1]);
    let mut x_offset = 26.0;
    for g in &e.conditions {
        let w = column_width(host, &*draw, g);
        for (r, c) in g.iter().enumerate() {
            let sum = summary(host, &Field::Condition(c.clone()));
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
    // Shadows at the card's edges when the columns don't fit across it (the gradients turned
    // a quarter: dark at the left edge and at the right).
    if x_offset + 16.0 > CARD_WIDTH - 26.0 {
        let (cx, band) = (at[0], [y + 2.0 + CARD_HEADER, h - 4.0 - CARD_HEADER]);
        for (gx, from, to) in [
            (cx + 18.0, 0x4400_0000u32, 0u32),
            (cx + CARD_WIDTH - 26.0, 0, 0x4400_0000),
        ] {
            for k in 0..10 {
                let t = (k as f32 + 0.5) / 10.0;
                let a = ((from >> 24) as f32 * (1.0 - t) + (to >> 24) as f32 * t) as u32;
                draw.fill([gx + k as f32, band[0], 1.0, band[1]], a << 24);
            }
        }
    }
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
        let widths = column_widths(host, e);
        let center = ((h - 8.0 + CARD_HEADER) / 2.0).floor();
        if y > center - 1.0 && y <= center + 7.0 && scrollable(&widths) {
            let target = o.column(i)[2];
            if x > 12.0 && x <= 19.0 && target != 0.0 {
                push(Change::Column(i, target - 1.0), 0);
                return Some(Vec::new());
            }
            if x > 177.0 && x <= 184.0 && target != e.conditions.len() as f32 - 1.0 {
                push(Change::Column(i, target + 1.0), 0);
                return Some(Vec::new());
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
        x += column_scroll(&widths, o.column(i)[1]) - 8.0;
        for (j, g) in e.conditions.iter().enumerate() {
            if x < 0.0 {
                return None;
            }
            let w = widths[j];
            if x >= w {
                x -= w + 10.0;
                continue;
            }
            let row = (y / 18.0).floor() as i32;
            if row >= 0 && (row as usize) < g.len() {
                let row = row as usize;
                let can_remove = g.len() > 1 || e.conditions.len() > 1;
                let c = &g[row];
                let mut lines = vec![tr(host, "create.schedule.condition_type", &[])];
                lines.extend(title_lines(host, c));
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

/// `getTitleAs`: a condition's lines in its tooltip.
fn title_lines(host: &dyn ClientHost, c: &Condition) -> Vec<String> {
    let name = |s: &Stack| {
        ghost(host, s).map_or_else(
            || tr(host, "create.schedule.condition.threshold.anything", &[]),
            |st| {
                let id = st.item().id(host);
                if matches!(
                    id.as_str(),
                    "create:filter" | "create:attribute_filter" | "create:package_filter"
                ) {
                    tr(
                        host,
                        "create.schedule.condition.threshold.matching_content",
                        &[],
                    )
                } else {
                    host.hover_name(&st)
                }
            },
        )
    };
    let holds = |op: Op| {
        tr(
            host,
            "create.schedule.condition.threshold.train_holds",
            &[tr(
                host,
                &format!("create.schedule.condition.threshold.{}", op.key()),
                &[],
            )],
        )
    };
    match c {
        Condition::ItemThreshold {
            op,
            threshold,
            stacks,
            item,
        } => vec![
            holds(*op),
            tr(
                host,
                "create.schedule.condition.threshold.x_units_of_item",
                &[
                    threshold.to_string(),
                    tr(
                        host,
                        if *stacks {
                            "create.schedule.condition.threshold.stacks"
                        } else {
                            "create.schedule.condition.threshold.items"
                        },
                        &[],
                    ),
                    name(item),
                ],
            ),
        ],
        Condition::FluidThreshold {
            op,
            threshold,
            item,
        } => vec![
            holds(*op),
            tr(
                host,
                "create.schedule.condition.threshold.x_units_of_item",
                &[
                    threshold.to_string(),
                    tr(host, "create.schedule.condition.threshold.buckets", &[]),
                    name(item),
                ],
            ),
        ],
        Condition::RedstoneLink { freq, inverted } => {
            let freq_name = |s: &Stack| {
                ghost(host, s).map_or_else(
                    || tr(host, "block.minecraft.air", &[]),
                    |st| host.hover_name(&st),
                )
            };
            vec![
                tr(
                    host,
                    if *inverted {
                        "create.schedule.condition.redstone_link.frequency_unpowered"
                    } else {
                        "create.schedule.condition.redstone_link.frequency_powered"
                    },
                    &[],
                ),
                format!(" #1 {}", freq_name(&freq[0])),
                format!(" #2 {}", freq_name(&freq[1])),
            ]
        }
        _ => vec![tr(
            host,
            &format!("create.schedule.condition.{}", c.id()),
            &[],
        )],
    }
}

/// `getSecondLineTooltip`: what an empty ghost slot says.
fn slot_tip(host: &dyn ClientHost, c: &Condition, slot: usize) -> Vec<String> {
    match c {
        Condition::RedstoneLink { .. } => vec![tr(
            host,
            if slot == 0 {
                "create.logistics.firstFrequency"
            } else {
                "create.logistics.secondFrequency"
            },
            &[],
        )],
        _ => (1..=3)
            .map(|n| {
                let suffix = if n == 1 {
                    String::new()
                } else {
                    format!("_{n}")
                };
                tr(
                    host,
                    &format!("create.schedule.condition.threshold.place_item{suffix}"),
                    &[],
                )
            })
            .collect(),
    }
}

/// A change a click asks for.
enum Change {
    Edit(Target),
    Remove(usize),
    Duplicate(usize),
    Move(usize, usize),
    RemoveCondition(usize, usize, usize),
    /// Entry `i`'s columns chased to a column index.
    Column(usize, f32),
}

fn apply(host: &dyn ClientHost, o: &mut Open, change: Change) {
    let s = &mut o.schedule;
    match change {
        Change::Column(i, target) => {
            o.chase_column(i, target);
            return;
        }
        // These `init()` the screen again, which starts every column scroll at 0.
        Change::Remove(i) => {
            s.entries.remove(i);
            o.columns.clear();
        }
        Change::Duplicate(i) => {
            let e = s.entries[i].clone();
            s.entries.insert(i, e);
            o.columns.clear();
        }
        Change::Move(a, b) => {
            let e = s.entries.remove(a);
            s.entries.insert(b, e);
            o.columns.clear();
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
                    (Field::Condition(groups[col][row].clone()), can_remove)
                }
                Target::AppendCondition { .. } | Target::NewGroup { .. } => (
                    Field::Condition(Condition::of("delay").unwrap_or(Condition::Powered)),
                    true,
                ),
            };
            // `startEditing`: the ghost slots take the part's stacks.
            let ghosts = match &field {
                Field::Condition(c) => [0, 1].map(|i| c.item(i).and_then(|b| ghost(host, b))),
                Field::Instruction(_) => [None, None],
            };
            o.cursor = None;
            o.editing = Some(Editing {
                field,
                target,
                allow_delete,
                typing: false,
                ghosts,
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

/// `onEditorClose`: confirmed, the part goes where it was edited; deleted, it goes away. The
/// ghost slots' stacks go to the part first (`stopEditing`).
fn close_editor(host: &dyn ClientHost, o: &mut Open, confirmed: bool) {
    let Some(mut ed) = o.editing.take() else {
        return;
    };
    o.cursor = None;
    if let Field::Condition(c) = &mut ed.field {
        for i in 0..c.slots() {
            let b = ed.ghosts[i]
                .as_ref()
                .and_then(|s| stack_bytes(host, s))
                .unwrap_or_default();
            c.set_item(i, b);
        }
    }
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
    /// A number typed in (`addIntegerTextInput`).
    Digits,
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
const OPERATORS: [&str; 3] = [
    "create.schedule.condition.threshold.greater",
    "create.schedule.condition.threshold.less",
    "create.schedule.condition.threshold.equal",
];
const ITEM_MEASURES: [&str; 2] = [
    "create.schedule.condition.threshold.items",
    "create.schedule.condition.threshold.stacks",
];
const FLUID_MEASURES: [&str; 1] = ["create.schedule.condition.threshold.buckets"];
const LINK_STATES: [&str; 2] = [
    "create.schedule.condition.redstone_link.powered",
    "create.schedule.condition.redstone_link.unpowered",
];

/// `initConfigurationWidgets` of each part: `(x, width, input)`.
fn inputs(f: &Field) -> Vec<(f32, f32, Input)> {
    match f {
        Field::Instruction(
            Instruction::Destination(_) | Instruction::Rename(_) | Instruction::FetchPackages(_),
        ) => {
            vec![(0.0, 121.0, Input::Text)]
        }
        Field::Instruction(Instruction::DeliverPackages) => Vec::new(),
        // `CargoThresholdCondition.initConfigurationWidgets` and the measure of each.
        Field::Condition(Condition::ItemThreshold { .. }) => vec![
            (0.0, 24.0, Input::Select(&OPERATORS)),
            (29.0, 41.0, Input::Digits),
            (71.0, 50.0, Input::Select(&ITEM_MEASURES)),
        ],
        Field::Condition(Condition::FluidThreshold { .. }) => vec![
            (0.0, 24.0, Input::Select(&OPERATORS)),
            (29.0, 41.0, Input::Digits),
            (71.0, 50.0, Input::Select(&FLUID_MEASURES)),
        ],
        Field::Condition(Condition::RedstoneLink { .. }) => {
            vec![(20.0, 101.0, Input::Select(&LINK_STATES))]
        }
        Field::Condition(Condition::Unloaded) => Vec::new(),
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
        (
            Field::Condition(
                Condition::ItemThreshold { op, .. } | Condition::FluidThreshold { op, .. },
            ),
            0,
        ) => op.ordinal(),
        (
            Field::Condition(
                Condition::ItemThreshold { threshold, .. }
                | Condition::FluidThreshold { threshold, .. },
            ),
            1,
        ) => *threshold,
        (Field::Condition(Condition::ItemThreshold { stacks, .. }), 2) => i32::from(*stacks),
        (Field::Condition(Condition::RedstoneLink { inverted, .. }), 0) => i32::from(*inverted),
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
        (
            Field::Condition(
                Condition::ItemThreshold { op, .. } | Condition::FluidThreshold { op, .. },
            ),
            0,
        ) => *op = Op::from_ordinal(v),
        (
            Field::Condition(
                Condition::ItemThreshold { threshold, .. }
                | Condition::FluidThreshold { threshold, .. },
            ),
            1,
        ) => *threshold = v,
        (Field::Condition(Condition::ItemThreshold { stacks, .. }), 2) => *stacks = v == 1,
        (Field::Condition(Condition::RedstoneLink { inverted, .. }), 0) => *inverted = v == 1,
        _ => {}
    }
}

/// The number a `Digits` input of `f` holds, if any (`Threshold`).
fn digits_mut(f: &mut Field) -> Option<&mut i32> {
    match f {
        Field::Condition(
            Condition::ItemThreshold { threshold, .. }
            | Condition::FluidThreshold { threshold, .. },
        ) => Some(threshold),
        _ => None,
    }
}

fn text_mut(f: &mut Field) -> Option<&mut String> {
    match f {
        Field::Instruction(
            Instruction::Destination(t) | Instruction::Rename(t) | Instruction::FetchPackages(t),
        ) => Some(t),
        _ => None,
    }
}

/// The editor's type options (`Schedule.INSTRUCTION_TYPES` / `CONDITION_TYPES`).
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

/// `SCHEDULE_EDITOR_ADDITIONAL_SLOT` in `schedule_2.png`.
const ADDITIONAL_SLOT: [f32; 4] = [55.0, 47.0, 32.0, 18.0];
/// `PLAYER_INVENTORY`, drawn at (38, 122) under the editor.
const INVENTORY_PANEL: [f32; 4] = [0.0, 0.0, 176.0, 108.0];

/// Ghost slot `i`'s item corner (`ScheduleMenu.addSlots`: 54 + 20i, 88).
fn ghost_slot(left: f32, top: f32, i: usize) -> [f32; 2] {
    [left + 54.0 + 20.0 * i as f32, top + 88.0]
}

/// An inventory slot's item corner (`addPlayerSlots(46, 140)`).
fn inventory_slot(left: f32, top: f32, slot: u8) -> [f32; 2] {
    let s = f32::from(slot);
    if slot < 9 {
        [left + 46.0 + s * 18.0, top + 198.0]
    } else {
        let i = s - 9.0;
        [
            left + 46.0 + (i % 9.0) * 18.0,
            top + 140.0 + (i / 9.0).floor() * 18.0,
        ]
    }
}

fn over_slot(at: [f32; 2], p: [f32; 2]) -> bool {
    p[0] >= at[0] - 1.0 && p[0] < at[0] + 17.0 && p[1] >= at[1] - 1.0 && p[1] < at[1] + 17.0
}

/// The ghost slots the part offers (`targetSlotsActive`).
fn slots_of(f: &Field) -> usize {
    match f {
        Field::Condition(c) => c.slots(),
        Field::Instruction(_) => 0,
    }
}

/// The inventory under the editor, the ghost slots and what is carried (`ScheduleMenu`'s
/// slots, drawn while editing); the hovered slot's name or the empty ghost slot's tip.
fn draw_slots(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &Open,
    ed: &Editing,
    mouse: [f32; 2],
    tips: &mut Vec<String>,
) {
    let (left, top) = corner(o.size);
    sprite(
        draw,
        "create:gui/player_inventory",
        INVENTORY_PANEL,
        [left + 38.0, top + 122.0],
    );
    draw.text(
        &tr(host, "container.inventory", &[]),
        [left + 46.0, top + 128.0],
        0xFF50_5050,
        false,
    );
    for slot in 0..36u8 {
        let p = inventory_slot(left, top, slot);
        if let Some(s) = o.stack(slot) {
            // With its count and bar, as a slot draws it.
            draw.gui_draw_item_with(s, p, None);
        }
        if over_slot(p, mouse) {
            draw.fill([p[0], p[1], 16.0, 16.0], 0x80FF_FFFF);
            if o.cursor.is_none()
                && let Some(s) = o.stack(slot)
            {
                tips.push(host.hover_name(s));
            }
        }
    }
    let n = slots_of(&ed.field);
    for i in 0..n {
        let p = ghost_slot(left, top, i);
        if let Some(s) = &ed.ghosts[i] {
            draw.item_stack(s, p, 16.0);
        }
        if over_slot(p, mouse) {
            draw.fill([p[0], p[1], 16.0, 16.0], 0x80FF_FFFF);
            match (&ed.ghosts[i], &ed.field) {
                (Some(s), _) if o.cursor.is_none() => tips.push(host.hover_name(s)),
                (None, Field::Condition(c)) => tips.extend(slot_tip(host, c, i)),
                _ => {}
            }
        }
    }
    // The ghost on the cursor (`renderFloatingItem`).
    if let Some(s) = o.cursor.and_then(|slot| o.stack(slot)) {
        draw.item_stack(s, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
    }
}

/// A click on the inventory or a ghost slot while editing (`GhostItemMenu.clicked`,
/// `quickMoveStack`); true when one took it.
fn click_slots(o: &mut Open, mouse: [f32; 2], shift: bool) -> bool {
    let (left, top) = corner(o.size);
    let selected = o.slot;
    for slot in 0..36u8 {
        if !over_slot(inventory_slot(left, top, slot), mouse) {
            continue;
        }
        let stack = o.stack(slot).cloned();
        if shift {
            // `quickMoveStack`: a copy into the first empty ghost slot.
            if let (Some(s), Some(ed)) = (stack, o.editing.as_mut())
                && slot != selected
            {
                let n = slots_of(&ed.field);
                if let Some(g) = ed.ghosts[..n].iter_mut().find(|g| g.is_none()) {
                    *g = Some(one_of(&s));
                }
            }
        } else {
            o.cursor = click_inventory(o.cursor, slot, stack.is_some(), selected);
        }
        return true;
    }
    let carried = o.cursor.and_then(|slot| o.stack(slot)).map(one_of);
    let Some(ed) = o.editing.as_mut() else {
        return false;
    };
    for i in 0..slots_of(&ed.field) {
        if over_slot(ghost_slot(left, top, i), mouse) {
            ed.ghosts[i] = if shift { None } else { carried };
            return true;
        }
    }
    if !inside([left, top, W, H], mouse) {
        o.cursor = None;
    }
    false
}

/// A one-item copy (`GhostItemMenu`: `insert.setCount(1)`).
fn one_of(s: &ItemStack) -> ItemStack {
    let mut s = s.clone();
    let _ = s.set_count(1);
    s
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
    // Its ghost slots, or its icon (`getSummary`, else `getSecondLineIcon`).
    let n = slots_of(&ed.field);
    for i in 0..n {
        sprite(
            draw,
            SHEET_2,
            ADDITIONAL_SLOT,
            [left + 53.0 + 20.0 * i as f32, top + 87.0],
        );
    }
    if n == 0 {
        let (icon, _) = summary(host, &ed.field);
        let icon = icon.or(match ed.field {
            Field::Condition(Condition::Delay { .. } | Condition::Idle { .. }) => {
                stack_named(host, "minecraft:repeater")
            }
            _ => None,
        });
        match icon {
            Some(s) => {
                draw.item_stack(&s, [left + 54.0, top + 88.0], 16.0);
            }
            None => sprite(draw, SHEET_2, INACTIVE_SLOT, [left + 53.0, top + 87.0]),
        }
        // `getSecondLineTooltip` of a package retrieval: its address filter.
        if matches!(ed.field, Field::Instruction(Instruction::FetchPackages(_)))
            && over_slot([left + 54.0, top + 88.0], mouse)
        {
            let base = "create.schedule.instruction.address_filter_edit_box";
            tips.push(tr(host, base, &[]));
            for k in 1..=3 {
                tips.push(tr(host, &format!("{base}_{k}"), &[]));
            }
        }
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
                    Field::Instruction(
                        Instruction::Destination(t)
                        | Instruction::Rename(t)
                        | Instruction::FetchPackages(t),
                    ) => t.clone(),
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
                        Field::Instruction(Instruction::FetchPackages(_)) => {
                            "create.schedule.instruction.address_filter_edit_box"
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
            Input::Digits => {
                let mut t = value(&ed.field, k).to_string();
                if ed.typing && (millis() / 300).is_multiple_of(2) {
                    t.push('_');
                }
                draw.text(&t, label_at, 0xFFFF_FFFF, true);
            }
            Input::Select(options) => {
                let v = value(&ed.field, k).clamp(0, options.len() as i32 - 1) as usize;
                // The operator shows its sign (`format`: " >").
                let label = if std::ptr::eq(options, &OPERATORS[..]) {
                    format!(" {}", Op::from_ordinal(v as i32).symbol())
                } else {
                    tr(host, options[v], &[])
                };
                draw.text(&label, label_at, 0xFFFF_FFFF, true);
                if inside(r, mouse) {
                    let key = match ed.field {
                        Field::Condition(Condition::TimeOfDay { .. }) => {
                            Some("create.schedule.condition.time_of_day.rotation")
                        }
                        Field::Condition(Condition::PlayerCount { .. }) => {
                            Some("create.schedule.condition.player_count.condition")
                        }
                        Field::Condition(
                            Condition::ItemThreshold { .. } | Condition::FluidThreshold { .. },
                        ) if k == 0 => Some("create.schedule.condition.threshold.train_holds"),
                        Field::Condition(Condition::ItemThreshold { .. }) => {
                            Some("create.schedule.condition.threshold.item_measure")
                        }
                        Field::Condition(Condition::FluidThreshold { .. }) => None,
                        Field::Condition(Condition::RedstoneLink { .. }) => {
                            Some("create.schedule.condition.redstone_link.frequency_state")
                        }
                        _ => Some("create.generic.timeUnit"),
                    };
                    if let Some(key) = key {
                        tips.push(tr(host, key, &[String::new()]).trim_end().to_owned());
                    }
                    // `forOptions`: each option, the chosen one marked.
                    for (i, opt) in options.iter().enumerate() {
                        let mark = if i == v { "-> " } else { "> " };
                        tips.push(format!("{mark}{}", tr(host, opt, &[])));
                    }
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
    draw_slots(host, draw, o, ed, mouse, tips);
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
            Input::Text | Input::Digits => return false,
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
        // Shift scrolls the columns of the card under the cursor.
        if host.modifiers().shift() {
            let (left, top) = corner(o.size);
            let mut y = (mouse[1] - top - 25.0 + o.scroll).floor();
            let mut over = None;
            for (i, e) in o.schedule.entries.iter().enumerate() {
                let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
                let h = CARD_HEADER + 24.0 + rows * 18.0;
                if y >= h {
                    y -= h + 9.0;
                    if y < 0.0 {
                        break;
                    }
                    continue;
                }
                if scrollable(&column_widths(host, e))
                    && y >= 24.0
                    && mouse[0] >= left + 25.0
                    && mouse[0] <= left + 205.0
                {
                    over = Some((i, e.conditions.len() as f32 - 1.0));
                }
                break;
            }
            if let Some((i, last)) = over {
                let target = o.column(i)[2];
                if up && target != 0.0 {
                    o.chase_column(i, target - 1.0);
                } else if !up && target != last {
                    o.chase_column(i, target + 1.0);
                }
                return false;
            }
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
        if o.editing.is_some() {
            if button <= 1 && click_slots(o, mouse, host.modifiers().shift()) {
                return;
            }
            let Some(ed) = o.editing.as_mut() else {
                return;
            };
            if button != 0 {
                return;
            }
            if inside(at(EDITOR_CONFIRM, left, top), mouse) {
                click_sound(host);
                close_editor(host, o, true);
                return;
            }
            if ed.allow_delete && inside(at(EDITOR_DELETE, left, top), mouse) {
                click_sound(host);
                close_editor(host, o, false);
                return;
            }
            let text_at = inputs(&ed.field)
                .into_iter()
                .find(|(_, _, i)| matches!(i, Input::Text | Input::Digits))
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
            apply(host, o, c);
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
                let Some(e) = o.as_mut().and_then(|o| o.editing.as_mut()) else {
                    return;
                };
                if let Some(t) = text_mut(&mut e.field) {
                    t.pop();
                } else if let Some(n) = digits_mut(&mut e.field) {
                    *n /= 10;
                }
            });
            Flow::Cancel
        }
        "key.keyboard.enter" | "key.keyboard.keypad.enter" if typing => {
            // `hitEnter`: the editor closes, confirmed.
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    close_editor(host, o, true);
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
        let Some(e) = o.as_mut().and_then(|o| o.editing.as_mut()) else {
            return;
        };
        if !e.typing {
            return;
        }
        // A package retrieval's filter takes three `*` at most (`modifyEditBox`).
        let fetch = matches!(e.field, Field::Instruction(Instruction::FetchPackages(_)));
        if let Some(t) = text_mut(&mut e.field) {
            if super::gui::allowed(c)
                && t.chars().count() < 256
                && !(fetch && c == '*' && t.matches('*').count() >= 3)
            {
                t.push(c);
            }
        } else if let Some(n) = digits_mut(&mut e.field)
            && let Some(d) = c.to_digit(10)
            && *n < 100_000_000
        {
            *n = *n * 10 + d as i32;
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
