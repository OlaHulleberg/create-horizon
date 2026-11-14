//! The schedule's screen (`ScheduleScreen`, `ScheduleMenu`): Create's panel with the
//! schedule's cards (each entry's instruction, its condition groups side by side, the remove,
//! duplicate and move buttons, the new card and new condition buttons, the pointer at the saved
//! progress), scrolled with the wheel and each card's conditions with Shift and the wheel or
//! the arrows; the loop, reset-progress and skip buttons and confirm. Clicking an instruction or
//! a condition opens the editor (`startEditing`): its type chosen with the wheel, its own line
//! of inputs (`initConfigurationWidgets` through `ModularGuiLineBuilder`: scroll inputs,
//! selections and text fields on their data areas), its item slots over the player's inventory,
//! and confirm or delete. The destination's text field suggests the stations' names
//! (`DestinationSuggestions`).
//!
//! The server opens it with the held schedule, a copy of the player's inventory and the
//! stations' names; the client sends the schedule back when it closes (`ScheduleEditPacket`),
//! at its next tick.
//!
//! Left out (no API for it):
//! - An item-opened mod menu has no screen layout (as for the filters'), so this is the mod's
//!   own screen: clicking an inventory slot puts a ghost copy of its stack on the cursor for
//!   the editor's slots, and the player's inventory never changes while it is open.
//! - `DrawContext` has no scissor: a card scrolled past the panel's top or bottom is cut by
//!   drawing the panel's frame over it again, and a condition group that doesn't fit across a
//!   card is left out until scrolled to, where Create cuts it at the card's edge.

use super::super::net;
use super::filter_screen::stack_of;
use super::schedule::{self, CONDITIONS, Entry, INSTRUCTIONS, Input, Schedule};
use mod_api::client::widgets::{Outcome, TextField, Widgets};
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, Compound, DrawContext, Flow, Hand, Host, ItemKey, ItemStack,
    ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, SoundPlay, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Server to client: open the screen; client to server: the schedule written.
pub(crate) const CHANNEL: &str = "create:schedule_screen";
/// The mod whose own screen it is.
const MOD: &str = "create";

// ---------------------------------------------------------------------------------------------
// The wire

/// What the server opens the screen with.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Opened {
    /// The selected hotbar slot.
    pub selected: u8,
    /// The player's inventory: slot (0..35) and the stack's network NBT.
    pub inventory: Vec<(u8, Vec<u8>)>,
    /// The held schedule's network NBT, empty for none.
    pub schedule: Vec<u8>,
    /// The stations' names, nearest first.
    pub stations: Vec<String>,
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let (b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*b)
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = u32::from_le_bytes(self.0.get(..4)?.try_into().ok()?) as usize;
        let b = self.0.get(4..4 + n)?.to_vec();
        self.0 = &self.0[4 + n..];
        Some(b)
    }
}

impl Opened {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.selected, self.inventory.len().min(255) as u8];
        for (slot, nbt) in self.inventory.iter().take(255) {
            out.push(*slot);
            put(&mut out, nbt);
        }
        put(&mut out, &self.schedule);
        out.extend_from_slice(&(self.stations.len() as u32).to_le_bytes());
        for s in &self.stations {
            put(&mut out, s.as_bytes());
        }
        out
    }

    pub(crate) fn decode(data: &[u8]) -> Option<Opened> {
        let mut r = Reader(data);
        let selected = r.u8()?;
        let n = r.u8()?;
        let mut inventory = Vec::new();
        for _ in 0..n {
            inventory.push((r.u8()?, r.bytes()?));
        }
        let schedule = r.bytes()?;
        let count = u32::from_le_bytes(r.0.get(..4)?.try_into().ok()?);
        r.0 = &r.0[4..];
        let mut stations = Vec::new();
        for _ in 0..count {
            stations.push(String::from_utf8(r.bytes()?).ok()?);
        }
        Some(Opened {
            selected,
            inventory,
            schedule,
            stations,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The editor's line of inputs (`initConfigurationWidgets`)

/// What an input of the line is.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// `addScrollInput`: a number from `min` up to (not with) `max`, stepping by `step` (by
    /// `shift_step` with Shift, rounded down to it), its label the number and `suffix`.
    Number {
        min: i32,
        max: i32,
        step: i32,
        shift: i32,
        shift_step: i32,
        suffix: &'static str,
    },
    /// `addSelectionScrollInput`: one of `options` (lang keys), the wheel up the one before.
    Choice { options: Vec<&'static str> },
    /// `addTextInput` / `addIntegerTextInput`.
    Text { integer: bool, wildcards: bool },
}

/// An input: its `x` and width on the line, its data key, its kind, the scroll input's title
/// (a lang key) and whether its label shows.
#[derive(Clone, Debug, PartialEq)]
struct Field {
    x: f32,
    w: f32,
    key: &'static str,
    kind: Kind,
    title: Option<&'static str>,
    label: bool,
}

fn number(
    x: f32,
    w: f32,
    key: &'static str,
    (min, max): (i32, i32),
    shift: i32,
    title: &'static str,
) -> Field {
    Field {
        x,
        w,
        key,
        kind: Kind::Number {
            min,
            max,
            step: 1,
            shift,
            shift_step: shift,
            suffix: "",
        },
        title: Some(title),
        label: true,
    }
}

fn choice(
    x: f32,
    w: f32,
    key: &'static str,
    options: Vec<&'static str>,
    title: Option<&'static str>,
) -> Field {
    Field {
        x,
        w,
        key,
        kind: Kind::Choice { options },
        title,
        label: true,
    }
}

fn text(x: f32, w: f32, integer: bool, wildcards: bool, key: &'static str) -> Field {
    Field {
        x,
        w,
        key,
        kind: Kind::Text { integer, wildcards },
        title: None,
        label: false,
    }
}

const ROTATIONS: [&str; 10] = [
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

const OPERATORS: [&str; 3] = [
    "create.schedule.condition.threshold.greater",
    "create.schedule.condition.threshold.less",
    "create.schedule.condition.threshold.equal",
];

/// `CargoThresholdCondition.Ops.formatted`.
const OPERATOR_SIGNS: [&str; 3] = [">", "<", "="];

/// Each type's `initConfigurationWidgets`.
fn fields(id: &str) -> Vec<Field> {
    match id {
        "destination" | "package_retrieval" => vec![text(0.0, 121.0, false, true, "Text")],
        "rename" => vec![text(0.0, 121.0, false, false, "Text")],
        "throttle" => vec![Field {
            x: 0.0,
            w: 50.0,
            key: "Value",
            kind: Kind::Number {
                min: 5,
                max: 101,
                step: 5,
                shift: 25,
                shift_step: 5,
                suffix: "%",
            },
            title: Some("create.schedule.instruction.throttle_edit_box"),
            label: true,
        }],
        "delay" | "idle" => vec![
            number(0.0, 31.0, "Value", (0, 121), 15, "create.generic.duration"),
            choice(
                36.0,
                85.0,
                "TimeUnit",
                vec![
                    "create.generic.unit.ticks",
                    "create.generic.unit.seconds",
                    "create.generic.unit.minutes",
                ],
                Some("create.generic.timeUnit"),
            ),
        ],
        "time_of_day" => vec![
            number(0.0, 16.0, "Hour", (0, 24), 6, "create.generic.daytime.hour"),
            Field {
                label: false,
                ..number(
                    18.0,
                    16.0,
                    "Minute",
                    (0, 60),
                    15,
                    "create.generic.daytime.minute",
                )
            },
            choice(
                52.0,
                62.0,
                "Rotation",
                ROTATIONS.to_vec(),
                Some("create.schedule.condition.time_of_day.rotation"),
            ),
        ],
        "item_threshold" | "fluid_threshold" => vec![
            choice(
                0.0,
                24.0,
                "Operator",
                OPERATORS.to_vec(),
                Some("create.schedule.condition.threshold.train_holds"),
            ),
            text(29.0, 41.0, true, false, "Threshold"),
            if id == "item_threshold" {
                choice(
                    71.0,
                    50.0,
                    "Measure",
                    vec![
                        "create.schedule.condition.threshold.items",
                        "create.schedule.condition.threshold.stacks",
                    ],
                    Some("create.schedule.condition.threshold.item_measure"),
                )
            } else {
                choice(
                    71.0,
                    50.0,
                    "Measure",
                    vec!["create.schedule.condition.threshold.buckets"],
                    None,
                )
            },
        ],
        "redstone_link" => vec![choice(
            20.0,
            101.0,
            "Inverted",
            vec![
                "create.schedule.condition.redstone_link.powered",
                "create.schedule.condition.redstone_link.unpowered",
            ],
            Some("create.schedule.condition.redstone_link.frequency_state"),
        )],
        "player_count" => vec![
            number(
                0.0,
                31.0,
                "Count",
                (0, 21),
                5,
                "create.schedule.condition.player_count.players",
            ),
            choice(
                36.0,
                85.0,
                "Exact",
                vec![
                    "create.schedule.condition.player_count.exactly",
                    "create.schedule.condition.player_count.or_above",
                ],
                Some("create.schedule.condition.player_count.condition"),
            ),
        ],
        _ => Vec::new(),
    }
}

/// `customArea`s: the data areas drawn instead of the inputs' own.
fn custom_areas(id: &str) -> &'static [(f32, f32)] {
    match id {
        "time_of_day" => &[(0.0, 52.0), (52.0, 69.0)],
        _ => &[],
    }
}

/// `ScrollInput.clampState` and the step (`mouseScrolled`, the wheel's way already turned for a
/// choice).
fn scrolled(kind: &Kind, state: i32, up: bool, shift: bool) -> i32 {
    match kind {
        Kind::Number {
            min,
            max,
            step,
            shift: shift_by,
            shift_step,
            ..
        } => {
            let by = if shift { *shift_by } else { *step };
            let mut s = state + if up { by } else { -by };
            if shift && *shift_step > 0 {
                s -= s.rem_euclid(*shift_step);
            }
            s.clamp(*min, max - 1)
        }
        Kind::Choice { options } => {
            // `SelectionScrollInput` is inverted: the wheel up is the option before.
            let s = state + if up { -1 } else { 1 };
            s.clamp(0, options.len() as i32 - 1)
        }
        Kind::Text { .. } => state,
    }
}

/// `EditBox.setFilter` of the line's text fields: an integer's digits (and a sign), at most
/// three `*`.
fn text_allowed(kind: &Kind, s: &str) -> bool {
    match kind {
        Kind::Text { integer: true, .. } => s.is_empty() || s.parse::<i32>().is_ok(),
        Kind::Text {
            wildcards: true, ..
        } => s.matches('*').count() <= 3,
        _ => true,
    }
}

// ---------------------------------------------------------------------------------------------
// The screen's state

/// What the editor edits and what its confirm or delete does (`onEditorClose`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// An entry's instruction.
    Instruction(usize),
    /// A new entry (`add_entry`): its instruction, then a delay.
    NewEntry,
    /// A condition: entry, group, row.
    Condition(usize, usize, usize),
    /// A condition added to a group.
    Append(usize, usize),
    /// A new group (`alternative_condition`).
    Group(usize),
}

struct Editing {
    target: Target,
    input: Input,
    /// Deleting is offered (`allowDeletion`).
    delete: bool,
    /// The item slots (`ghostInventory`).
    ghosts: [Option<ItemStack>; 2],
    /// The line's text fields, by field index.
    widgets: Widgets,
    texts: Vec<(usize, mod_api::client::widgets::WidgetId)>,
    /// The destination's suggestions (`DestinationSuggestions`): the names offered and the one
    /// chosen.
    suggestions: Vec<String>,
    chosen: usize,
}

impl Editing {
    fn instruction(&self) -> bool {
        matches!(self.target, Target::Instruction(_) | Target::NewEntry)
    }
}

struct Open {
    item: ItemKey,
    inventory: Vec<(u8, ItemStack)>,
    schedule: Schedule,
    stations: Vec<String>,
    /// The list's scroll (`scroll`) and where it chases to, and each card's condition scroll
    /// (`horizontalScrolls`), in groups.
    scroll: f32,
    scroll_to: f32,
    across: Vec<(f32, f32)>,
    editing: Option<Editing>,
    /// The inventory slot whose ghost is on the cursor.
    cursor: Option<u8>,
    size: [f32; 2],
    pressed: bool,
    /// Text widths seen when drawing, for clicks between draws.
    widths: HashMap<String, f32>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Schedules to send at the next client tick, and whether to close the screen then.
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(crate) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn main_hand(host: &dyn ClientHost) -> Option<ItemStack> {
    let player = host.local_player()?;
    host.living(player.entity())?.hand_item(host, Hand::Main)
}

/// The server opened the screen.
pub(crate) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let Some(o) = Opened::decode(data) else {
        return true;
    };
    let Some(item) = main_hand(&*host).map(|s| s.item()) else {
        return true;
    };
    let h: &dyn Host = &*host;
    let inventory = o
        .inventory
        .iter()
        .filter_map(|(slot, nbt)| Some((*slot, stack_of(h, nbt)?)))
        .collect();
    // `menu.contentHolder`'s schedule, or a new one.
    let schedule = schedule::compound_of(&o.schedule)
        .filter(|c| !c.is_empty())
        .map(|c| Schedule::from_nbt(&c))
        .unwrap_or_default();
    let across = vec![(0.0, 0.0); schedule.entries.len()];
    let open = Open {
        item,
        inventory,
        schedule,
        stations: o.stations,
        scroll: 0.0,
        scroll_to: 0.0,
        across,
        editing: None,
        cursor: None,
        size: [0.0, 0.0],
        pressed: false,
        widths: HashMap::new(),
    };
    OPEN.with_borrow_mut(|x| *x = Some(open));
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|x| *x = None);
    }
    true
}

/// The schedule as `removed` sends it.
fn sent(o: &Open) -> Vec<u8> {
    schedule::bytes(&o.schedule.to_nbt())
}

fn close_later(o: Open) {
    LATER.with_borrow_mut(|l| {
        l.0.push(sent(&o));
        l.1 = true;
    });
}

pub(crate) fn tick(host: &mut dyn ClientHost) {
    // The screen closes when the main hand no longer holds the schedule.
    let item = OPEN.with_borrow(|o| o.as_ref().map(|o| o.item));
    if let Some(item) = item
        && host.mod_screen() == Some(MOD)
        && main_hand(&*host).map(|s| s.item()) != Some(item)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        close_later(open);
    }
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for s in send {
        net::send_to_server(host, CHANNEL, &s);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &sent(&open));
    }
    // `containerTick`: the chasers, and the saved progress kept within the entries.
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        o.scroll = chase(o.scroll, o.scroll_to, 0.7);
        for (v, t) in &mut o.across {
            *v = chase(*v, *t, 0.5);
        }
        let n = o.schedule.entries.len() as i32;
        o.schedule.progress = if n == 0 {
            0
        } else {
            o.schedule.progress.clamp(0, n - 1)
        };
    });
}

/// `LerpedFloat`'s `Chaser.EXP`.
fn chase(v: f32, target: f32, speed: f32) -> f32 {
    let next = v + (target - v) * speed;
    if (target - next).abs() < 0.01 {
        target
    } else {
        next
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

/// A translation with its arguments (`%1$s`, `%2$s`... or `%s` in turn).
fn trf(host: &dyn ClientHost, key: &str, args: &[&str]) -> String {
    let mut s = tr(host, key);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("%{}$s", i + 1), a);
    }
    for a in args {
        if let Some(at) = s.find("%s") {
            s.replace_range(at..at + 2, a);
        }
    }
    s
}

fn styled(text: impl Into<String>, color: &str, italic: bool) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text.into()).with_style(style)
}

fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

fn item_key(host: &dyn Host, id: &str) -> Option<ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

fn stack(host: &dyn Host, id: &str) -> Option<ItemStack> {
    item_key(host, id).map(ItemStack::of)
}

fn stack_of_nbt(host: &dyn Host, c: &Compound) -> Option<ItemStack> {
    ItemStack::from_nbt(host, c)
}

// ---------------------------------------------------------------------------------------------
// What each type shows (`getSummary`, `getTitleAs`, `getSecondLineIcon`, `getSecondLineTooltip`)

/// A summary's icon.
#[derive(Clone)]
enum Icon {
    Item(ItemStack),
    /// `TimeOfDayCondition.renderSpecialIcon`: the clock at the condition's time (its summary's
    /// item is a structure void, which is not drawn).
    Clock(u32),
}

/// `TimedWaitCondition.formatTime`.
fn format_time(host: &dyn ClientHost, i: &Input, compact: bool) -> String {
    let unit = i.int("TimeUnit").rem_euclid(3) as usize;
    if compact {
        format!("{}{}", i.int("Value"), ["t", "s", "min"][unit])
    } else {
        let key = [
            "create.generic.unit.ticks",
            "create.generic.unit.seconds",
            "create.generic.unit.minutes",
        ][unit];
        format!("{} {}", i.int("Value"), tr(host, key))
    }
}

fn two(t: i32) -> String {
    format!("{t:02}")
}

/// `TimeOfDayCondition.getDigitalDisplay`.
fn digital(host: &dyn ClientHost, hour: i32, minute: i32, double: bool) -> String {
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let hr12 = if double { two(h12) } else { h12.to_string() };
    let hr24 = if double { two(hour) } else { hour.to_string() };
    let ampm = tr(
        host,
        if hour > 11 {
            "create.generic.daytime.pm"
        } else {
            "create.generic.daytime.am"
        },
    );
    trf(
        host,
        "create.schedule.condition.time_of_day.digital_format",
        &[&hr12, &hr24, &two(minute), &ampm],
    )
}

/// The clock frame of a time of day (`renderSpecialIcon`).
fn clock_frame(i: &Input) -> u32 {
    let hr = (i.int("Hour") + 12) % 24;
    let progress = (hr as f32 * 60.0 + i.int("Minute") as f32) / (24.0 * 60.0);
    ((progress * 64.0) as i32).clamp(0, 63) as u32
}

/// `CargoThresholdCondition.getThreshold`.
fn threshold(i: &Input) -> i32 {
    i.text("Threshold").parse().unwrap_or(0)
}

fn operator(i: &Input) -> usize {
    i.int("Operator").rem_euclid(3) as usize
}

/// `getSummary`: the icon and the text.
fn summary(host: &dyn ClientHost, i: &Input) -> (Option<Icon>, String) {
    let item = |id: &str| stack(host, id).map(Icon::Item);
    let key = |k: &str| tr(host, &format!("create.schedule.{k}"));
    match i.id.as_str() {
        "destination" => (item("create:track_station"), i.text("Text")),
        "rename" => (item("minecraft:name_tag"), i.text("Text")),
        "throttle" => (item("create:controls"), format!("{}%", i.int("Value"))),
        "package_delivery" => (
            item("create:white_postbox"),
            key("instruction.package_delivery"),
        ),
        "package_retrieval" => (
            item("create:cardboard_package_12x12"),
            key("instruction.package_retrieval"),
        ),
        "delay" | "idle" => (
            None,
            trf(
                host,
                &format!("create.schedule.condition.{}_short", i.id),
                &[&format_time(host, i, true)],
            ),
        ),
        "time_of_day" => (
            Some(Icon::Clock(clock_frame(i))),
            digital(host, i.int("Hour"), i.int("Minute"), false),
        ),
        "item_threshold" | "fluid_threshold" => {
            let fluid = i.id == "fluid_threshold";
            let unit = if fluid {
                "b"
            } else if i.int("Measure") == 1 {
                "\u{25A4}"
            } else {
                ""
            };
            (
                i.item(0)
                    .and_then(|c| stack_of_nbt(host, &c))
                    .map(Icon::Item),
                format!("{} {}{unit}", OPERATOR_SIGNS[operator(i)], threshold(i)),
            )
        }
        "redstone_link" => (
            item("create:redstone_link"),
            key(if i.int("Inverted") == 1 {
                "condition.redstone_link_off"
            } else {
                "condition.redstone_link_on"
            }),
        ),
        "player_count" => {
            let n = i.int("Count");
            (
                item("create:yellow_seat"),
                trf(
                    host,
                    if n == 1 {
                        "create.schedule.condition.player_count.summary"
                    } else {
                        "create.schedule.condition.player_count.summary_plural"
                    },
                    &[&n.to_string()],
                ),
            )
        }
        other => (None, key(&format!("condition.{other}"))),
    }
}

/// `getSecondLineIcon`.
fn second_icon(host: &dyn ClientHost, i: &Input) -> Option<ItemStack> {
    let id = match i.id.as_str() {
        "destination" => "create:track_station",
        "rename" => "minecraft:name_tag",
        "throttle" => "create:controls",
        "package_delivery" => "create:white_postbox",
        "package_retrieval" => "create:cardboard_package_12x12",
        "delay" | "idle" => "minecraft:repeater",
        _ => return None,
    };
    stack(host, id)
}

/// `getSecondLineTooltip(slot)`.
fn second_tooltip(host: &dyn ClientHost, i: &Input, slot: usize) -> Option<Vec<Text>> {
    let k = |key: &str| tr(host, &format!("create.{key}"));
    let lines = |head: &str, rest: &[(&str, &str)]| {
        let mut v = vec![Text::literal(k(head))];
        for (key, colour) in rest {
            v.push(styled(k(key), colour, false));
        }
        v
    };
    Some(match i.id.as_str() {
        "destination" => lines(
            "schedule.instruction.filter_edit_box",
            &[
                ("schedule.instruction.filter_edit_box_1", "gray"),
                ("schedule.instruction.filter_edit_box_2", "dark_gray"),
                ("schedule.instruction.filter_edit_box_3", "dark_gray"),
            ],
        ),
        "package_retrieval" => lines(
            "schedule.instruction.address_filter_edit_box",
            &[
                ("schedule.instruction.address_filter_edit_box_1", "gray"),
                (
                    "schedule.instruction.address_filter_edit_box_2",
                    "dark_gray",
                ),
                (
                    "schedule.instruction.address_filter_edit_box_3",
                    "dark_gray",
                ),
            ],
        ),
        "rename" => lines(
            "schedule.instruction.name_edit_box",
            &[
                ("schedule.instruction.name_edit_box_1", "gray"),
                ("schedule.instruction.name_edit_box_2", "dark_gray"),
            ],
        ),
        "throttle" => lines(
            "schedule.instruction.throttle_edit_box",
            &[("schedule.instruction.throttle_edit_box_1", "gray")],
        ),
        "delay" | "idle" => vec![Text::literal(k("generic.duration"))],
        "item_threshold" | "fluid_threshold" => lines(
            "schedule.condition.threshold.place_item",
            &[
                ("schedule.condition.threshold.place_item_2", "gray"),
                ("schedule.condition.threshold.place_item_3", "gray"),
            ],
        ),
        "redstone_link" => vec![styled(
            k(if slot == 0 {
                "logistics.firstFrequency"
            } else {
                "logistics.secondFrequency"
            }),
            "red",
            false,
        )],
        _ => return None,
    })
}

/// `getTitleAs(type)`.
fn title_as(host: &dyn ClientHost, i: &Input, instruction: bool) -> Vec<Text> {
    let kind = if instruction {
        "instruction"
    } else {
        "condition"
    };
    let name = || tr(host, &format!("create.schedule.{kind}.{}", i.id));
    let quoted = || format!("\"{}\"", i.text("Text"));
    let k = |key: &str| tr(host, &format!("create.schedule.{key}"));
    match i.id.as_str() {
        "destination" | "rename" => vec![
            styled(k(&format!("{kind}.{}.summary", i.id)), "gold", false),
            Text::literal(quoted()),
        ],
        "package_retrieval" => vec![
            styled(k("instruction.package_retrieval.summary"), "gold", false),
            Text::literal(quoted()),
            styled(k("instruction.package_retrieval.summary_1"), "gray", false),
            styled(k("instruction.package_retrieval.summary_2"), "gray", false),
        ],
        "package_delivery" => vec![
            styled(k("instruction.package_delivery.summary"), "gold", false),
            styled(k("instruction.package_delivery.summary_1"), "gray", false),
            styled(k("instruction.package_delivery.summary_2"), "gray", false),
        ],
        "throttle" => {
            let t = tr(host, "create.schedule.instruction.throttle.summary");
            let (a, b) = t.split_once("%1$s").unwrap_or((t.as_str(), ""));
            vec![
                styled(a, "gold", false)
                    .append(styled(format!("{}%", i.int("Value")), "white", false))
                    .append(styled(b, "gold", false)),
            ]
        }
        "delay" | "idle" => vec![
            Text::literal(name()),
            styled(
                trf(
                    host,
                    "create.schedule.condition.for_x_time",
                    &[&format_time(host, i, false)],
                ),
                "dark_aqua",
                false,
            ),
        ],
        "time_of_day" => {
            let rotation = ROTATIONS
                .get(i.int("Rotation").clamp(0, 9) as usize)
                .map_or(String::new(), |r| tr(host, r));
            vec![
                Text::literal(trf(
                    host,
                    "create.schedule.condition.time_of_day.scheduled",
                    &[""],
                )),
                styled(
                    digital(host, i.int("Hour"), i.int("Minute"), false),
                    "dark_aqua",
                    false,
                )
                .append(styled(" -> ", "dark_gray", false))
                .append(styled(rotation, "gray", false)),
            ]
        }
        "item_threshold" | "fluid_threshold" => {
            let op = tr(host, OPERATORS[operator(i)]);
            let unit = if i.id == "fluid_threshold" {
                "buckets"
            } else if i.int("Measure") == 1 {
                "stacks"
            } else {
                "items"
            };
            let what = match i.item(0).and_then(|c| stack_of_nbt(host, &c)) {
                None => k("condition.threshold.anything"),
                Some(s) if s.item().id(host).ends_with("filter") => {
                    k("condition.threshold.matching_content")
                }
                Some(s) => host.hover_name(&s),
            };
            vec![
                Text::literal(trf(
                    host,
                    "create.schedule.condition.threshold.train_holds",
                    &[&op],
                )),
                styled(
                    trf(
                        host,
                        "create.schedule.condition.threshold.x_units_of_item",
                        &[
                            &threshold(i).to_string(),
                            &k(&format!("condition.threshold.{unit}")),
                            &what,
                        ],
                    ),
                    "dark_aqua",
                    false,
                ),
            ]
        }
        "redstone_link" => {
            let name = |slot: usize| {
                i.item(slot)
                    .and_then(|c| stack_of_nbt(host, &c))
                    .map_or_else(|| tr(host, "block.minecraft.air"), |s| host.hover_name(&s))
            };
            vec![
                Text::literal(k(if i.int("Inverted") == 1 {
                    "condition.redstone_link.frequency_unpowered"
                } else {
                    "condition.redstone_link.frequency_powered"
                })),
                styled(" #1 ", "gray", false).append(styled(name(0), "dark_aqua", false)),
                styled(" #2 ", "gray", false).append(styled(name(1), "dark_aqua", false)),
            ]
        }
        "player_count" => {
            let n = i.int("Count");
            let count = trf(
                host,
                if n == 1 {
                    "create.schedule.condition.player_count.summary"
                } else {
                    "create.schedule.condition.player_count.summary_plural"
                },
                &[&n.to_string()],
            );
            vec![Text::literal(trf(
                host,
                "create.schedule.condition.player_count.seated",
                &[&count],
            ))]
        }
        _ => vec![Text::literal(name())],
    }
}

// ---------------------------------------------------------------------------------------------
// The layout (`AllGuiTextures`, `ScheduleScreen`), from the window's corner

const TEX: &str = "create:gui/schedule";
const TEX_2: &str = "create:gui/schedule_2";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
const DATA: &str = "create:gui/display_link";
const INVENTORY: &str = "create:gui/player_inventory";

/// `AllGuiTextures.SCHEDULE`.
const BG: [f32; 2] = [256.0, 226.0];
const CARD_HEADER: f32 = 22.0;
const CARD_WIDTH: f32 = 195.0;

// `[u, v, w, h]` in schedule.png.
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
const SCROLL_LEFT: [f32; 4] = [161.0, 247.0, 4.0, 8.0];
const SCROLL_RIGHT: [f32; 4] = [166.0, 247.0, 4.0, 8.0];
const STRIP_DARK: [f32; 4] = [5.0, 235.0, 3.0, 1.0];
const STRIP_LIGHT: [f32; 4] = [5.0, 237.0, 3.0, 1.0];
const STRIP_WAIT: [f32; 4] = [1.0, 239.0, 11.0, 16.0];
const STRIP_TRAVEL: [f32; 4] = [12.0, 239.0, 11.0, 16.0];
const STRIP_DOTTED: [f32; 4] = [23.0, 239.0, 11.0, 16.0];
const STRIP_END: [f32; 4] = [34.0, 239.0, 11.0, 16.0];
const STRIP_ACTION: [f32; 4] = [209.0, 239.0, 11.0, 16.0];
const POINTER: [f32; 4] = [185.0, 239.0, 21.0, 16.0];
const POINTER_OFFSCREEN: [f32; 4] = [171.0, 239.0, 13.0, 16.0];
// schedule_2.png.
const EDITOR: [f32; 4] = [0.0, 0.0, 256.0, 89.0];
const EDITOR_SLOT: [f32; 4] = [55.0, 47.0, 32.0, 18.0];
const EDITOR_INACTIVE_SLOT: [f32; 4] = [0.0, 91.0, 18.0, 18.0];
// display_link.png.
const DATA_START: [f32; 4] = [0.0, 163.0, 2.0, 18.0];
const DATA_SPEECH: [f32; 4] = [8.0, 163.0, 5.0, 18.0];
const DATA_AREA: [f32; 4] = [3.0, 163.0, 1.0, 18.0];
const DATA_END: [f32; 4] = [5.0, 163.0, 2.0, 18.0];

/// `AllIcons` cells in icons.png.
const I_CONFIRM: [f32; 2] = [0.0, 16.0];
const I_TRASH: [f32; 2] = [16.0, 0.0];
const I_REFRESH: [f32; 2] = [48.0, 16.0];
const I_PRIORITY_VERY_HIGH: [f32; 2] = [112.0, 0.0];
const I_PRIORITY_LOW: [f32; 2] = [80.0, 0.0];

/// The panel's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Confirm,
    Cyclic,
    Reset,
    Skip,
    EditorConfirm,
    EditorDelete,
}

/// `setWindowSize` centred: the window's corner.
fn corner(size: [f32; 2]) -> [f32; 2] {
    [
        ((size[0] - BG[0]) / 2.0).floor(),
        ((size[1] - BG[1]) / 2.0).floor(),
    ]
}

fn at(c: [f32; 2], x: f32, y: f32) -> [f32; 2] {
    [c[0] + x, c[1] + y]
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// The buttons shown, each with its place and icon.
fn buttons(o: &Open, c: [f32; 2]) -> Vec<(Button, [f32; 4], [f32; 2])> {
    let b = |x: f32, y: f32| [c[0] + x, c[1] + y, 18.0, 18.0];
    match &o.editing {
        None => vec![
            (Button::Confirm, b(BG[0] - 42.0, BG[1] - 30.0), I_CONFIRM),
            (Button::Cyclic, b(21.0, 196.0), I_REFRESH),
            (Button::Reset, b(45.0, 196.0), I_PRIORITY_VERY_HIGH),
            (Button::Skip, b(63.0, 196.0), I_PRIORITY_LOW),
        ],
        Some(e) => {
            let mut v = vec![(
                Button::EditorConfirm,
                b(56.0 + 168.0, 65.0 + 22.0),
                I_CONFIRM,
            )];
            if e.delete {
                v.push((Button::EditorDelete, b(56.0 - 45.0, 65.0 + 22.0), I_TRASH));
            }
            v
        }
    }
}

fn button_active(o: &Open, b: Button) -> bool {
    match b {
        Button::Reset => o.schedule.progress > 0 && !o.schedule.entries.is_empty(),
        Button::Skip => o.schedule.entries.len() > 1,
        _ => true,
    }
}

/// The inventory slot's place (`addPlayerSlots(46, 140)`).
fn inventory_slot(c: [f32; 2], slot: u8) -> [f32; 2] {
    if slot < 9 {
        at(c, 46.0 + 18.0 * f32::from(slot), 140.0 + 58.0)
    } else {
        let i = slot - 9;
        at(
            c,
            46.0 + 18.0 * f32::from(i % 9),
            140.0 + 18.0 * f32::from(i / 9),
        )
    }
}

/// The editor's item slot (`54 + 20 * i, 88`).
fn ghost_slot(c: [f32; 2], i: usize) -> [f32; 2] {
    at(c, 54.0 + 20.0 * i as f32, 88.0)
}

fn over_slot(p: [f32; 2], mouse: [f32; 2]) -> bool {
    inside([p[0] - 1.0, p[1] - 1.0, 18.0, 18.0], mouse)
}

/// The line's place: its inputs' `x` from here, its labels at `y`.
const LINE: [f32; 2] = [77.0, 92.0];

/// An input's area on the line (`ScrollInput(x + this.x, y - 4, width, 18)`).
fn field_rect(c: [f32; 2], f: &Field) -> [f32; 4] {
    [c[0] + LINE[0] + f.x, c[1] + LINE[1] - 4.0, f.w, 18.0]
}

/// `getFieldSize`: the text's width, an item's 20 and 16, at least `min`.
fn field_size(min: f32, text_w: f32, item: bool) -> f32 {
    (text_w + if item { 20.0 } else { 0.0 } + 16.0).max(min)
}

/// `CARD_HEADER + (supportsConditions ? 24 + maxRows * 18 : 4)`.
fn card_height(e: &Entry) -> f32 {
    let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
    CARD_HEADER
        + if e.instruction.supports_conditions() {
            24.0 + rows * 18.0
        } else {
            4.0
        }
}

impl Open {
    fn width(&self, s: &str) -> f32 {
        self.widths
            .get(s)
            .copied()
            .unwrap_or(s.chars().count() as f32 * 6.0)
    }

    fn summary_size(&self, host: &dyn ClientHost, i: &Input, min: f32) -> f32 {
        let (icon, text) = summary(host, i);
        field_size(min, self.width(&text), icon.is_some())
    }

    /// `getConditionColumnWidth`.
    fn column_width(&self, host: &dyn ClientHost, g: &[Input]) -> f32 {
        g.iter()
            .map(|c| self.summary_size(host, c, 32.0))
            .fold(0.0, f32::max)
    }

    /// `isConditionAreaScrollable`.
    fn scrollable(&self, host: &dyn ClientHost, e: &Entry) -> bool {
        let x = 26.0
            + e.conditions
                .iter()
                .map(|g| self.column_width(host, g) + 10.0)
                .sum::<f32>();
        x + 16.0 > CARD_WIDTH - 26.0
    }

    /// `getConditionScroll`: the offset of card `i`'s groups.
    fn condition_scroll(&self, host: &dyn ClientHost, e: &Entry, i: usize) -> f32 {
        let mut index = self.across.get(i).map_or(0.0, |a| a.0);
        let mut offset = 0.0;
        for g in &e.conditions {
            let part = index.min(1.0);
            offset += (self.column_width(host, g) + 10.0) * part;
            index -= part;
        }
        offset
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing

pub(crate) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.size = size;
            draw(host, event.draw, o, mouse);
        }
    });
}

/// `UIRenderHelper.drawStretched`.
fn stretched(draw: &mut dyn DrawContext, tex: &str, src: [f32; 4], dst: [f32; 4]) {
    if dst[2] > 0.0 && dst[3] > 0.0 {
        draw.sprite(tex, src, dst, None);
    }
}

fn blit(draw: &mut dyn DrawContext, tex: &str, src: [f32; 4], p: [f32; 2]) {
    draw.sprite(tex, src, [p[0], p[1], src[2], src[3]], None);
}

/// A vertical gradient as rows of fills (`fillGradient`).
fn gradient(draw: &mut dyn DrawContext, r: [f32; 4], top: u32, bottom: u32) {
    let rows = r[3].max(1.0) as usize;
    for i in 0..rows {
        let t = i as f32 / (rows.max(2) - 1) as f32;
        let lerp = |s: u32| {
            let a = ((top >> s) & 0xFF) as f32;
            let b = ((bottom >> s) & 0xFF) as f32;
            ((a + (b - a) * t).round() as u32) << s
        };
        let argb = lerp(24) | lerp(16) | lerp(8) | lerp(0);
        draw.fill([r[0], r[1] + i as f32, r[2], 1.0], argb);
    }
}

/// `renderInput`: a summary's field at `p`, `clean` without its left cap; its width.
fn draw_input(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &mut Open,
    i: &Input,
    p: [f32; 2],
    clean: bool,
    min: f32,
) -> f32 {
    let (icon, text) = summary(host, i);
    let w = draw.text_width(&text);
    o.widths.insert(text.clone(), w);
    let size = field_size(min, w, icon.is_some()).min(150.0);
    stretched(draw, TEX, CONDITION_MIDDLE, [p[0], p[1], size, 16.0]);
    if clean {
        blit(draw, TEX, CONDITION_LEFT_CLEAN, p);
    } else {
        blit(draw, TEX, CONDITION_LEFT, [p[0] - 3.0, p[1]]);
    }
    blit(draw, TEX, CONDITION_RIGHT, [p[0] + size - 2.0, p[1]]);
    match &icon {
        Some(Icon::Item(s)) => {
            blit(draw, TEX, CONDITION_ITEM, [p[0] + 3.0, p[1]]);
            draw.item_stack(s, [p[0] + 4.0, p[1]], 16.0);
        }
        Some(Icon::Clock(_)) => blit(draw, TEX, CONDITION_ITEM, [p[0] + 3.0, p[1]]),
        None => {}
    }
    let shown = trim(draw, &text, 120.0);
    let x = if icon.is_some() { 28.0 } else { 8.0 };
    draw.text(&shown, [p[0] + x, p[1] + 4.0], 0xFFF2_F2EE, true);
    size
}

/// `renderSpecialIcon` of a time of day: the clock showing its time.
fn draw_clock(host: &dyn ClientHost, draw: &mut dyn DrawContext, frame: u32, p: [f32; 2]) {
    if let Some(clock) = item_key(host, "minecraft:clock") {
        draw.item_model(clock, &format!("minecraft:item/clock_{frame:02}"), p, 16.0);
    }
}

/// `font.substrByWidth`.
fn trim(draw: &dyn DrawContext, text: &str, width: f32) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        out.push(ch);
        if draw.text_width(&out) > width {
            out.pop();
            break;
        }
    }
    out
}

fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &mut Open, mouse: [f32; 2]) {
    let size = draw.size();
    let c = corner(size);
    // `renderBackground` in a world: the blur, then the dark translucent menu background.
    host.apply_screen_blur();
    draw.draw_menu_background([0.0, 0.0, size[0], size[1]], true);
    // `renderBg`: the panel, its title, the schedule.
    blit(draw, TEX, [0.0, 0.0, BG[0], BG[1]], c);
    let center = c[0] + ((BG[0] - 8.0) / 2.0).floor();
    let title = tr(host, "item.create.schedule");
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [center - (w / 2.0).floor(), c[1] + 4.0],
        0xFF50_5050,
        false,
    );
    draw_schedule(host, draw, o, c);
    let mut hover: Option<Vec<Text>> = None;
    if o.editing.is_none() {
        draw_buttons(host, draw, o, c, mouse, &mut hover);
    }
    // The held schedule, three times its size, beside the panel.
    if let Some(held) = main_hand(host) {
        draw.item_stack(&held, at(c, BG[0], BG[1] - 56.0), 48.0);
    }
    if o.editing.is_some() {
        draw_editor(host, draw, o, c, mouse, &mut hover);
    } else if hover.is_none() {
        hover = action(host, o, c, mouse, None).1;
    }
    // The ghost on the cursor.
    if let Some(s) = o.cursor.and_then(|slot| stack_at(o, slot)) {
        draw.item_stack(&s, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
    }
    if let Some(lines) = hover {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// The buttons shown (`IconButton.renderWidget`) and the hovered one's tooltip.
fn draw_buttons(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &Open,
    c: [f32; 2],
    mouse: [f32; 2],
    hover: &mut Option<Vec<Text>>,
) {
    for (b, r, icon) in buttons(o, c) {
        let hovered = inside(r, mouse);
        let green = b == Button::Cyclic && o.schedule.cyclic;
        let u = if !button_active(o, b) {
            90.0
        } else if hovered && o.pressed {
            36.0
        } else if hovered {
            18.0
        } else if green {
            72.0
        } else {
            0.0
        };
        draw.sprite(WIDGETS, [u, 0.0, 18.0, 18.0], r, None);
        draw.sprite(
            ICONS,
            [icon[0], icon[1], 16.0, 16.0],
            [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
            None,
        );
        if hovered && let Some(lines) = button_tooltip(host, o, b) {
            *hover = Some(lines);
        }
    }
}

fn stack_at(o: &Open, slot: u8) -> Option<ItemStack> {
    o.inventory
        .iter()
        .find(|(s, _)| *s == slot)
        .map(|(_, st)| st.clone())
}

fn button_tooltip(host: &dyn ClientHost, o: &Open, b: Button) -> Option<Vec<Text>> {
    Some(match b {
        Button::Cyclic => vec![
            Text::literal(tr(host, "create.schedule.loop")),
            if o.schedule.cyclic {
                styled(
                    tr(host, "create.gui.schematicannon.optionEnabled"),
                    "dark_green",
                    false,
                )
            } else {
                styled(
                    tr(host, "create.gui.schematicannon.optionDisabled"),
                    "red",
                    false,
                )
            },
            styled(tr(host, "create.schedule.loop1"), "gray", false),
            styled(tr(host, "create.schedule.loop2"), "gray", false),
        ],
        Button::Reset => vec![Text::literal(tr(host, "create.schedule.reset"))],
        Button::Skip => vec![Text::literal(tr(host, "create.schedule.skip"))],
        _ => return None,
    })
}

/// `renderSchedule`.
fn draw_schedule(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &mut Open, c: [f32; 2]) {
    stretched(
        draw,
        TEX,
        STRIP_DARK,
        [c[0] + 33.0, c[1] + 16.0, 3.0, 173.0],
    );
    let scroll = -o.scroll;
    let entries = o.schedule.entries.clone();
    let mut y = 25.0;
    for i in 0..=entries.len() {
        if o.schedule.progress == i as i32 && !entries.is_empty() {
            let expected = scroll + y + 4.0;
            let actual = expected.clamp(18.0, 170.0);
            let tex = if expected == actual {
                POINTER
            } else {
                POINTER_OFFSCREEN
            };
            blit(draw, TEX, tex, at(c, 0.0, actual));
        }
        if i == 0 || entries.is_empty() {
            stretched(
                draw,
                TEX,
                STRIP_LIGHT,
                [c[0] + 33.0, c[1] + 16.0 + scroll, 3.0, 10.0],
            );
        }
        if i == entries.len() {
            if i > 0 {
                y += 9.0;
            }
            blit(draw, TEX, STRIP_END, at(c, 29.0, y + scroll));
            blit(draw, TEX, CARD_NEW, at(c, 43.0, y + scroll));
            break;
        }
        let e = &entries[i];
        let card_y = y;
        let h = draw_card(host, draw, o, e, i, at(c, 25.0, card_y + scroll));
        y += h;
        if i + 1 < entries.len() {
            blit(draw, TEX, STRIP_DOTTED, at(c, 29.0, y - 3.0 + scroll));
            y += 10.0;
        }
        if !e.instruction.supports_conditions() {
            continue;
        }
        draw_conditions(host, draw, o, e, i, at(c, 25.0, card_y + scroll), h);
        if o.scrollable(host, e) {
            let center = ((h - 8.0 + CARD_HEADER) / 2.0).floor();
            let target = o.across.get(i).map_or(0.0, |a| a.1);
            if target != 0.0 {
                blit(
                    draw,
                    TEX,
                    SCROLL_LEFT,
                    at(c, 40.0, card_y + scroll + center),
                );
            }
            if target != e.conditions.len() as f32 - 1.0 {
                blit(
                    draw,
                    TEX,
                    SCROLL_RIGHT,
                    at(c, 203.0, card_y + scroll + center),
                );
            }
        }
    }
    // The scissor's stand-in: the panel's frame over what scrolled past it.
    blit(draw, TEX, [0.0, 0.0, BG[0], 16.0], c);
    blit(
        draw,
        TEX,
        [0.0, 189.0, BG[0], BG[1] - 189.0],
        at(c, 0.0, 189.0),
    );
    gradient(
        draw,
        [c[0] + 16.0, c[1] + 16.0, 220.0, 10.0],
        0x7700_0000,
        0,
    );
    gradient(
        draw,
        [c[0] + 16.0, c[1] + 179.0, 220.0, 10.0],
        0,
        0x7700_0000,
    );
}

/// `renderScheduleEntry`: the card of entry `i` at `p`; its height.
fn draw_card(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &mut Open,
    e: &Entry,
    i: usize,
    p: [f32; 2],
) -> f32 {
    let w = CARD_WIDTH;
    let h = card_height(e);
    let cond = e.instruction.supports_conditions();
    let r = |x: f32, y: f32, rw: f32, rh: f32| [p[0] + x, p[1] + y, rw, rh];
    stretched(draw, TEX, CARD_LIGHT, r(0.0, 1.0, w, h - 2.0));
    stretched(draw, TEX, CARD_LIGHT, r(1.0, 0.0, w - 2.0, h));
    stretched(draw, TEX, CARD_DARK, r(1.0, 1.0, w - 2.0, h - 2.0));
    stretched(draw, TEX, CARD_MEDIUM, r(2.0, 2.0, w - 4.0, h - 4.0));
    stretched(
        draw,
        TEX,
        if cond { CARD_LIGHT } else { CARD_MEDIUM },
        r(2.0, 2.0, w - 4.0, CARD_HEADER),
    );
    blit(draw, TEX, CARD_REMOVE, [p[0] + w - 14.0, p[1] + 2.0]);
    blit(
        draw,
        TEX,
        CARD_DUPLICATE,
        [p[0] + w - 14.0, p[1] + h - 14.0],
    );
    if i > 0 {
        blit(
            draw,
            TEX,
            CARD_MOVE_UP,
            [p[0] + w, p[1] + CARD_HEADER - 14.0],
        );
    }
    if i + 1 < o.schedule.entries.len() {
        blit(draw, TEX, CARD_MOVE_DOWN, [p[0] + w, p[1] + CARD_HEADER]);
    }
    stretched(draw, TEX, STRIP_LIGHT, r(8.0, 0.0, 3.0, h + 10.0));
    blit(
        draw,
        TEX,
        if cond { STRIP_TRAVEL } else { STRIP_ACTION },
        [p[0] + 4.0, p[1] + 6.0],
    );
    if cond {
        blit(draw, TEX, STRIP_WAIT, [p[0] + 4.0, p[1] + 28.0]);
    }
    draw_input(
        host,
        draw,
        o,
        &e.instruction,
        [p[0] + 26.0, p[1] + 5.0],
        false,
        100.0,
    );
    h
}

/// `renderScheduleConditions`: the card's groups side by side, scrolled, within the card.
fn draw_conditions(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &mut Open,
    e: &Entry,
    i: usize,
    p: [f32; 2],
    h: f32,
) {
    let shift = o.condition_scroll(host, e, i);
    let mut x = 26.0;
    // Only what lies within the card's condition area (`enableScissor(43, .., 204, ..)`).
    let fits = |from: f32, to: f32| from - shift >= 18.0 - 0.5 && to - shift <= 179.0 + 0.5;
    for g in &e.conditions {
        let w = o.column_width(host, g);
        if fits(x - 3.0, x + w) {
            for (row, cond) in g.iter().enumerate() {
                let q = [p[0] + x - shift, p[1] + 29.0 + row as f32 * 18.0];
                draw_input(host, draw, o, cond, q, row != 0, w);
                if let (Some(Icon::Clock(f)), _) = summary(host, cond) {
                    draw_clock(host, draw, f, [q[0] + 4.0, q[1]]);
                }
            }
            blit(
                draw,
                TEX,
                CONDITION_APPEND,
                [
                    p[0] + x - shift + ((w - 10.0) / 2.0).floor(),
                    p[1] + 29.0 + g.len() as f32 * 18.0,
                ],
            );
        }
        x += w + 10.0;
    }
    if fits(x - 3.0, x + 16.0) {
        blit(
            draw,
            TEX,
            CONDITION_NEW,
            [p[0] + x - 3.0 - shift, p[1] + 29.0],
        );
    }
    if x + 16.0 > CARD_WIDTH - 26.0 {
        // The shades at the area's sides (`fillGradient` turned).
        let top = p[1] + CARD_HEADER + 2.0;
        let rows = h - 4.0 - CARD_HEADER;
        for k in 0..10 {
            let a = ((0x44 as f32) * (1.0 - k as f32 / 9.0)) as u32;
            draw.fill([p[0] + 18.0 + k as f32, top, 1.0, rows], a << 24);
            draw.fill(
                [p[0] + CARD_WIDTH - 17.0 - k as f32, top, 1.0, rows],
                a << 24,
            );
        }
    }
}

/// The editor (`renderBg` while editing, the line's widgets, the slots and the inventory).
fn draw_editor(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &mut Open,
    c: [f32; 2],
    mouse: [f32; 2],
    hover: &mut Option<Vec<Text>>,
) {
    let size = draw.size();
    gradient(draw, [0.0, 0.0, size[0], size[1]], 0xC010_1010, 0xD010_1010);
    blit(draw, TEX_2, EDITOR, at(c, -2.0, 40.0));
    blit(
        draw,
        INVENTORY,
        [0.0, 0.0, 176.0, 108.0],
        at(c, 38.0, 122.0),
    );
    draw.text(
        &tr(host, "container.inventory"),
        at(c, 46.0, 128.0),
        0xFF50_5050,
        false,
    );
    let Some(e) = o.editing.as_mut() else {
        return;
    };
    let center = c[0] + ((BG[0] - 8.0) / 2.0).floor();
    let title = tr(
        host,
        if e.instruction() {
            "create.schedule.instruction.editor"
        } else {
            "create.schedule.condition.editor"
        },
    );
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [center - (w / 2.0).floor(), c[1] + 44.0],
        0xFF50_5050,
        false,
    );
    let input = e.input.clone();
    let slots = input.slots();
    for i in 0..slots {
        blit(
            draw,
            TEX_2,
            EDITOR_SLOT,
            at(c, 53.0 + 20.0 * i as f32, 87.0),
        );
    }
    if slots == 0 {
        let (icon, _) = summary(host, &input);
        match icon {
            Some(Icon::Clock(f)) => draw_clock(host, draw, f, at(c, 54.0, 88.0)),
            Some(Icon::Item(s)) => {
                draw.item_stack(&s, at(c, 54.0, 88.0), 16.0);
            }
            None => match second_icon(host, &input) {
                Some(s) => {
                    draw.item_stack(&s, at(c, 54.0, 88.0), 16.0);
                }
                None => blit(draw, TEX_2, EDITOR_INACTIVE_SLOT, at(c, 53.0, 87.0)),
            },
        }
    }
    // The line's data areas (`renderWidgetBG`).
    let line = fields(&input.id);
    let areas: Vec<(f32, f32)> = if custom_areas(&input.id).is_empty() {
        line.iter().map(|f| (f.x, f.w)).collect()
    } else {
        custom_areas(&input.id).to_vec()
    };
    for (k, (x, w)) in areas.iter().enumerate() {
        let x = c[0] + LINE[0] + x;
        let y = c[1] + 87.0;
        stretched(draw, DATA, DATA_AREA, [x, y, *w, 18.0]);
        if k == 0 {
            blit(draw, DATA, DATA_SPEECH, [x - 3.0, y]);
        } else {
            blit(draw, DATA, DATA_START, [x, y]);
        }
        blit(draw, DATA, DATA_END, [x + w - 2.0, y]);
    }
    draw_buttons(host, draw, o, c, mouse, hover);
    let Some(e) = o.editing.as_mut() else {
        return;
    };
    // The type's selector and its label.
    let (types, type_key): (&[&str], &str) = if e.instruction() {
        (&INSTRUCTIONS, "instruction")
    } else {
        (&CONDITIONS, "condition")
    };
    let name = tr(host, &format!("create.schedule.{type_key}.{}", input.id));
    draw.text(&name, at(c, 59.0, 69.0), 0xFFFF_FFFF, true);
    let selector = [c[0] + 56.0, c[1] + 65.0, 143.0, 16.0];
    if inside(selector, mouse) {
        let state = types.iter().position(|t| *t == input.id).unwrap_or(0);
        let options: Vec<String> = types
            .iter()
            .map(|t| tr(host, &format!("create.schedule.{type_key}.{t}")))
            .collect();
        *hover = Some(choice_tooltip(
            host,
            Some(&tr(host, &format!("create.schedule.{type_key}_type"))),
            &options,
            state,
        ));
    }
    // The inputs' labels and their tooltips.
    for f in &line {
        let r = field_rect(c, f);
        match &f.kind {
            Kind::Number { suffix, .. } => {
                let v = input.int(f.key);
                if f.label {
                    let label = if input.id == "time_of_day" && f.key == "Hour" {
                        digital(host, v, input.int("Minute"), true)
                    } else {
                        format!("{v}{suffix}")
                    };
                    draw.text(&label, [r[0] + 5.0, c[1] + LINE[1]], 0xFFFF_FFFF, true);
                }
                if inside(r, mouse) {
                    let mut lines = Vec::new();
                    if let Some(t) = f.title {
                        lines.push(styled(tr(host, t), "#5391E1", false));
                    }
                    lines.push(styled(
                        tr(host, "create.gui.scrollInput.scrollToModify"),
                        "dark_gray",
                        true,
                    ));
                    *hover = Some(lines);
                }
            }
            Kind::Choice { options } => {
                let state = input.int(f.key).clamp(0, options.len() as i32 - 1) as usize;
                let label = if f.key == "Operator" {
                    format!(" {}", OPERATOR_SIGNS[state.min(2)])
                } else {
                    tr(host, options[state])
                };
                draw.text(&label, [r[0] + 5.0, c[1] + LINE[1]], 0xFFFF_FFFF, true);
                if inside(r, mouse) {
                    let names: Vec<String> = options.iter().map(|k| tr(host, k)).collect();
                    let title = f.title.map(|t| {
                        if f.key == "Operator" {
                            trf(host, t, &[""])
                        } else {
                            tr(host, t)
                        }
                    });
                    *hover = Some(choice_tooltip(host, title.as_deref(), &names, state));
                }
            }
            Kind::Text { .. } => {}
        }
    }
    let _ = e.widgets.draw(draw, host, mouse);
    // The destination's suggestions (`DestinationSuggestions`, its list under the panel's top).
    if !e.suggestions.is_empty()
        && let Some((_, id)) = e.texts.first()
        && let Some(field) = e.widgets.text_field(*id)
    {
        let x0 = field.rect[0];
        let width = e
            .suggestions
            .iter()
            .map(|s| draw.text_width(s))
            .fold(0.0, f32::max);
        let x = x0.clamp(0.0, (x0 + field.rect[2] - width).max(0.0));
        let y = c[1] + 33.0 + 72.0;
        let shown = e.suggestions.len().min(7);
        draw.fill([x - 1.0, y, width + 2.0, 12.0 * shown as f32], 0xEE30_3030);
        let first = e
            .chosen
            .saturating_sub(shown - 1)
            .min(e.suggestions.len() - shown);
        for (k, s) in e.suggestions.iter().enumerate().skip(first).take(shown) {
            let colour = if k == e.chosen {
                0xFFFF_FF00
            } else {
                0xFFAA_AAAA
            };
            draw.text(
                s,
                [x + 1.0, y + 2.0 + 12.0 * (k - first) as f32],
                colour,
                true,
            );
        }
    }
    // The item slots and the inventory.
    for i in 0..slots {
        let p = ghost_slot(c, i);
        if let Some(s) = &e.ghosts[i] {
            draw.item_stack(s, p, 16.0);
        }
        if over_slot(p, mouse) {
            draw.fill([p[0], p[1], 16.0, 16.0], 0x80FF_FFFF);
        }
    }
    for (slot, s) in &o.inventory {
        let p = inventory_slot(c, *slot);
        let n = s.count();
        let count = (n > 1).then(|| n.to_string());
        draw.gui_draw_item_with(s, p, count.as_deref());
    }
    for slot in 0..36u8 {
        let p = inventory_slot(c, slot);
        if over_slot(p, mouse) {
            draw.fill([p[0], p[1], 16.0, 16.0], 0x80FF_FFFF);
            if o.cursor.is_none()
                && let Some(s) = stack_at(o, slot)
            {
                *hover = Some(vec![Text::literal(host.hover_name(&s))]);
            }
        }
    }
    // `renderForeground`: an empty slot's tooltip.
    let Some(e) = o.editing.as_ref() else {
        return;
    };
    let line_area = [c[0] + 53.0, c[1] + 87.0, 120.0, 18.0];
    if inside(line_area, mouse) {
        for i in 0..slots.max(1) {
            let p = ghost_slot(c, i);
            if e.ghosts.get(i).is_some_and(Option::is_some) || !over_slot(p, mouse) {
                continue;
            }
            if let Some(lines) = second_tooltip(host, &e.input, i) {
                *hover = Some(lines);
            }
        }
    }
}

/// `SelectionScrollInput.updateTooltip`: the title, the options around the chosen one, and the
/// hint.
fn choice_tooltip(
    host: &dyn ClientHost,
    title: Option<&str>,
    options: &[String],
    state: usize,
) -> Vec<Text> {
    let mut lines = Vec::new();
    if let Some(t) = title {
        lines.push(styled(t, "#5391E1", false));
    }
    let n = options.len() as i32;
    let s = state as i32;
    let min = (n - 16).min(s - 7).max(0);
    let max = (16).max(s + 8).min(n);
    if min > 0 {
        lines.push(styled("> ...", "gray", false));
    }
    for (k, o) in options
        .iter()
        .enumerate()
        .take(max as usize)
        .skip(min as usize)
    {
        if k == state {
            lines.push(styled(format!("-> {o}"), "white", false));
        } else {
            lines.push(styled(format!("> {o}"), "gray", false));
        }
    }
    if max < n {
        lines.push(styled("> ...", "gray", false));
    }
    lines.push(styled(
        tr(host, "create.gui.scrollInput.scrollToSelect"),
        "dark_gray",
        true,
    ));
    lines
}

// ---------------------------------------------------------------------------------------------
// Clicks (`action`, `mouseClicked`)

/// What a click on the schedule's cards does.
enum Act {
    Edit(Target),
    RemoveEntry(usize),
    Duplicate(usize),
    MoveUp(usize),
    MoveDown(usize),
    ScrollLeft(usize),
    ScrollRight(usize),
    RemoveCondition(usize, usize, usize),
}

/// `ScheduleScreen.action`: what lies under `mouse`, its tooltip, and what a click with
/// `button` does there.
fn action(
    host: &dyn ClientHost,
    o: &Open,
    c: [f32; 2],
    mouse: [f32; 2],
    button: Option<i32>,
) -> (Option<Act>, Option<Vec<Text>>) {
    if o.editing.is_some() {
        return (None, None);
    }
    let x0 = (mouse[0] - c[0] - 25.0).floor();
    let mut y = (mouse[1] - c[1] - 25.0).floor();
    if !(0.0..205.0).contains(&x0) || !(0.0..173.0).contains(&y) {
        return (None, None);
    }
    y += o.scroll;
    let click = |a: Act, want: i32| (button == Some(want)).then_some(a);
    let edit = tr(host, "create.gui.schedule.lmb_edit");
    let remove = tr(host, "create.gui.schedule.rmb_remove");
    let one = |key: &str| Some(vec![Text::literal(tr(host, key))]);
    for (i, e) in o.schedule.entries.iter().enumerate() {
        let mut x = x0;
        let h = card_height(e);
        if y >= h + 5.0 {
            y -= h + 10.0;
            if y < 0.0 {
                return (None, None);
            }
            continue;
        }
        let size = o.summary_size(host, &e.instruction, 100.0);
        if x > 25.0 && x <= 25.0 + size && y > 4.0 && y <= 20.0 {
            let mut lines = title_as(host, &e.instruction, true);
            lines.push(Text::literal(""));
            lines.push(styled(edit, "dark_gray", true));
            return (click(Act::Edit(Target::Instruction(i)), 0), Some(lines));
        }
        if x > 180.0 && x <= 192.0 {
            if y > 0.0 && y <= 14.0 {
                return (
                    click(Act::RemoveEntry(i), 0),
                    one("create.gui.schedule.remove_entry"),
                );
            }
            if y > h - 14.0 {
                return (
                    click(Act::Duplicate(i), 0),
                    one("create.gui.schedule.duplicate"),
                );
            }
        }
        if x > 194.0 {
            if y > 7.0 && y <= 20.0 && i > 0 {
                return (click(Act::MoveUp(i), 0), one("create.gui.schedule.move_up"));
            }
            if y > 20.0 && y <= 33.0 && i + 1 < o.schedule.entries.len() {
                return (
                    click(Act::MoveDown(i), 0),
                    one("create.gui.schedule.move_down"),
                );
            }
        }
        let center = ((h - 8.0 + CARD_HEADER) / 2.0).floor();
        if y > center - 1.0 && y <= center + 7.0 && o.scrollable(host, e) {
            let target = o.across.get(i).map_or(0.0, |a| a.1);
            if x > 12.0 && x <= 19.0 && target != 0.0 {
                return (click(Act::ScrollLeft(i), 0), None);
            }
            if x > 177.0 && x <= 184.0 && target != e.conditions.len() as f32 - 1.0 {
                return (click(Act::ScrollRight(i), 0), None);
            }
        }
        x -= 18.0;
        y -= 28.0;
        if x < 0.0 || y < 0.0 || x > 160.0 {
            return (None, None);
        }
        x += o.condition_scroll(host, e, i) - 8.0;
        for (j, g) in e.conditions.iter().enumerate() {
            if x < 0.0 {
                return (None, None);
            }
            let w = o.column_width(host, g);
            if x >= w {
                x -= w + 10.0;
                continue;
            }
            let row = (y / 18.0).floor() as usize;
            if row < g.len() {
                let can_remove = g.len() > 1 || e.conditions.len() > 1;
                let mut lines = vec![styled(
                    tr(host, "create.schedule.condition_type"),
                    "gray",
                    false,
                )];
                lines.extend(title_as(host, &g[row], false));
                lines.push(Text::literal(""));
                lines.push(styled(edit.clone(), "dark_gray", true));
                if can_remove {
                    lines.push(styled(remove.clone(), "dark_gray", true));
                }
                let act = match button {
                    Some(1) if can_remove => Some(Act::RemoveCondition(i, j, row)),
                    Some(0) => Some(Act::Edit(Target::Condition(i, j, row))),
                    _ => None,
                };
                return (act, Some(lines));
            }
            let n = g.len() as f32;
            if y > 18.0 * n && y <= 18.0 * n + 10.0 && x >= w / 2.0 - 5.0 && x < w / 2.0 + 5.0 {
                return (
                    click(Act::Edit(Target::Append(i, j)), 0),
                    one("create.gui.schedule.add_condition"),
                );
            }
            return (None, None);
        }
        if x < 0.0 || x > 15.0 || y > 20.0 {
            return (None, None);
        }
        return (
            click(Act::Edit(Target::Group(i)), 0),
            one("create.gui.schedule.alternative_condition"),
        );
    }
    let x = x0;
    if !(18.0..=33.0).contains(&x) || y > 14.0 {
        return (None, None);
    }
    (
        click(Act::Edit(Target::NewEntry), 0),
        one("create.gui.schedule.add_entry"),
    )
}

/// `startEditing`: the input edited and its line (`updateEditorSubwidgets`).
fn start_editing(o: &mut Open, target: Target) {
    let (input, delete) = match target {
        Target::Instruction(i) => (o.schedule.entries[i].instruction.clone(), false),
        Target::NewEntry => (Input::new("destination"), true),
        Target::Condition(i, j, r) => {
            let e = &o.schedule.entries[i];
            let can_remove = e.conditions[j].len() > 1 || e.conditions.len() > 1;
            (e.conditions[j][r].clone(), can_remove)
        }
        Target::Append(..) | Target::Group(_) => (Input::new("delay"), true),
    };
    o.editing = Some(Editing {
        target,
        input,
        delete,
        ghosts: [None, None],
        widgets: Widgets::new(),
        texts: Vec::new(),
        suggestions: Vec::new(),
        chosen: 0,
    });
    o.cursor = None;
    let c = corner(o.size);
    if let Some(e) = o.editing.as_mut() {
        rebuild_line(e, c);
    }
}

/// The line's text fields for the input edited (`initConfigurationWidgets` and `load`).
fn rebuild_line(e: &mut Editing, c: [f32; 2]) {
    e.widgets = Widgets::new();
    e.texts.clear();
    e.suggestions.clear();
    e.chosen = 0;
    for (k, f) in fields(&e.input.id).iter().enumerate() {
        if let Kind::Text { .. } = f.kind {
            // `new EditBox(font, x + this.x + 5, y, width - 9, 8)`, borderless, white.
            let mut field = TextField::new(
                [c[0] + LINE[0] + f.x + 5.0, c[1] + LINE[1], f.w - 9.0, 8.0],
                32,
            );
            field.bordered = false;
            field.text_color = 0xFFFF_FFFF;
            field.set_value(&e.input.text(f.key));
            let id = e.widgets.add_text_field(field);
            e.texts.push((k, id));
        }
    }
}

/// The ghosts the editor's slots start with (`getItem`), as stacks.
fn load_ghosts(host: &dyn Host, e: &mut Editing) {
    for i in 0..2 {
        e.ghosts[i] = (i < e.input.slots())
            .then(|| e.input.item(i))
            .flatten()
            .and_then(|c| stack_of_nbt(host, &c));
    }
}

/// `stopEditing` (after `onEditorClose`): the slots and the line saved into the input.
fn save_editing(host: &dyn Host, e: &mut Editing) {
    for i in 0..e.input.slots() {
        let nbt = e.ghosts[i].as_ref().and_then(|s| {
            let mut one = s.clone();
            let _ = one.set_count(1);
            one.to_nbt(host)
        });
        e.input.set_item(i, nbt);
    }
    let line = fields(&e.input.id);
    for (k, id) in &e.texts {
        if let (Some(f), Some(field)) = (line.get(*k), e.widgets.text_field(*id)) {
            let v = field.value().to_owned();
            e.input.set_text(f.key, &v);
        }
    }
}

/// `onEditorClose(confirmed)` for the target, then `stopEditing`.
fn finish(host: &dyn Host, o: &mut Open, confirmed: bool) {
    let Some(mut e) = o.editing.take() else {
        return;
    };
    o.cursor = None;
    save_editing(host, &mut e);
    let s = &mut o.schedule;
    match e.target {
        Target::Instruction(i) => {
            if confirmed && let Some(entry) = s.entries.get_mut(i) {
                entry.instruction = e.input;
            }
        }
        Target::NewEntry => {
            if confirmed {
                s.entries.push(Entry {
                    instruction: e.input,
                    conditions: vec![vec![Input::new("delay")]],
                });
            }
        }
        Target::Condition(i, j, r) => {
            if let Some(entry) = s.entries.get_mut(i)
                && let Some(g) = entry.conditions.get_mut(j)
                && r < g.len()
            {
                g.remove(r);
                if confirmed {
                    g.insert(r, e.input);
                } else if g.is_empty() {
                    entry.conditions.remove(j);
                }
            }
        }
        Target::Append(i, j) => {
            if confirmed && let Some(g) = s.entries.get_mut(i).and_then(|e| e.conditions.get_mut(j))
            {
                g.push(e.input);
            }
        }
        Target::Group(i) => {
            if confirmed && let Some(entry) = s.entries.get_mut(i) {
                entry.conditions.push(vec![e.input]);
            }
        }
    }
    // `init`: a horizontal scroll per entry, from the start.
    o.across = vec![(0.0, 0.0); o.schedule.entries.len()];
}

fn act(host: &dyn ClientHost, o: &mut Open, a: Act) {
    let s = &mut o.schedule;
    match a {
        Act::Edit(t) => {
            start_editing(o, t);
            if let Some(e) = o.editing.as_mut() {
                load_ghosts(host, e);
            }
            update_suggestions(o);
            return;
        }
        Act::RemoveEntry(i) => {
            s.entries.remove(i);
        }
        Act::Duplicate(i) => {
            let copy = s.entries[i].clone();
            s.entries.insert(i, copy);
        }
        Act::MoveUp(i) => s.entries.swap(i, i - 1),
        Act::MoveDown(i) => s.entries.swap(i, i + 1),
        Act::ScrollLeft(i) => {
            if let Some(a) = o.across.get_mut(i) {
                a.1 -= 1.0;
            }
            return;
        }
        Act::ScrollRight(i) => {
            if let Some(a) = o.across.get_mut(i) {
                a.1 += 1.0;
            }
            return;
        }
        Act::RemoveCondition(i, j, r) => {
            let e = &mut s.entries[i];
            e.conditions[j].remove(r);
            if e.conditions[j].is_empty() {
                e.conditions.remove(j);
            }
            return;
        }
    }
    o.across = vec![(0.0, 0.0); o.schedule.entries.len()];
}

/// `DestinationSuggestions.updateCommandInfo`: the stations starting with what is typed before
/// the cursor, other than it, nearest first; the chosen one's rest shown grey in the field.
fn update_suggestions(o: &mut Open) {
    let stations = o.stations.clone();
    let Some(e) = o.editing.as_mut() else {
        return;
    };
    e.suggestions.clear();
    if e.input.id != "destination" {
        return;
    }
    let Some((_, id)) = e.texts.first().copied() else {
        return;
    };
    let focused = e.widgets.focused() == Some(id);
    let Some(field) = e.widgets.text_field_mut(id) else {
        return;
    };
    field.suggestion = None;
    if !focused {
        return;
    }
    let typed: String = field.value().chars().take(field.edit.cursor).collect();
    let lower = typed.to_lowercase();
    e.suggestions = stations
        .into_iter()
        .filter(|s| *s != typed && s.to_lowercase().starts_with(&lower))
        .collect();
    e.chosen = e.chosen.min(e.suggestions.len().saturating_sub(1));
    if let Some(s) = e.suggestions.get(e.chosen)
        && s.starts_with(field.value())
    {
        field.suggestion = Some(s[field.value().len()..].to_owned());
    }
}

/// A click on the editor (not its buttons): the line's fields, the item slots and the
/// inventory.
fn click_editor(
    host: &dyn ClientHost,
    o: &mut Open,
    c: [f32; 2],
    mouse: [f32; 2],
    button: i32,
    shift: bool,
) {
    let modifiers = host.modifiers().bits;
    // A suggestion clicked (`SuggestionsList.mouseClicked`).
    if let Some(e) = o.editing.as_mut()
        && !e.suggestions.is_empty()
        && let Some((_, id)) = e.texts.first().copied()
        && let Some(field) = e.widgets.text_field(id)
    {
        let x0 = field.rect[0];
        let y = c[1] + 105.0;
        let shown = e.suggestions.len().min(7);
        let first = e
            .chosen
            .saturating_sub(shown - 1)
            .min(e.suggestions.len() - shown);
        let row = ((mouse[1] - y) / 12.0).floor();
        if mouse[0] >= x0 - 1.0 && row >= 0.0 && (row as usize) < shown {
            let pick = e.suggestions[first + row as usize].clone();
            if let Some(f) = e.widgets.text_field_mut(id) {
                f.set_value(&pick);
            }
            update_suggestions(o);
            return;
        }
    }
    if let Some(e) = o.editing.as_mut()
        && e.widgets
            .mouse_pressed(host, mouse, button, modifiers)
            .handled()
    {
        update_suggestions(o);
        return;
    }
    let slots = o.editing.as_ref().map_or(0, |e| e.input.slots());
    for i in 0..slots {
        if over_slot(ghost_slot(c, i), mouse) {
            let carried = o.cursor.and_then(|s| stack_at(o, s));
            if let Some(e) = o.editing.as_mut() {
                // `GhostItemMenu.clicked`: a 1-count copy of what is carried, or empty.
                e.ghosts[i] = carried.map(|mut s| {
                    let _ = s.set_count(1);
                    s
                });
            }
            return;
        }
    }
    for slot in 0..36u8 {
        if !over_slot(inventory_slot(c, slot), mouse) {
            continue;
        }
        if shift {
            // `quickMoveStack`: a copy into the first empty slot.
            if let Some(s) = stack_at(o, slot)
                && let Some(e) = o.editing.as_mut()
                && let Some(g) = e.ghosts.iter_mut().take(slots).find(|g| g.is_none())
            {
                let mut one = s;
                let _ = one.set_count(1);
                *g = Some(one);
            }
            return;
        }
        o.cursor = match o.cursor {
            Some(_) => None,
            None => stack_at(o, slot).is_some().then_some(slot),
        };
        return;
    }
}

pub(crate) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let shift = host.modifiers().shift();
    let mouse = event.screen.mouse;
    let button = event.button;
    let close = OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return false;
        };
        o.pressed = button == 0;
        let c = corner(o.size);
        if button == 0 {
            for (b, r, _) in buttons(o, c) {
                if !inside(r, mouse) || !button_active(o, b) {
                    continue;
                }
                click_sound(host);
                match b {
                    Button::Confirm => return true,
                    Button::Cyclic => o.schedule.cyclic = !o.schedule.cyclic,
                    Button::Reset => o.schedule.progress = 0,
                    Button::Skip => {
                        let n = o.schedule.entries.len() as i32;
                        o.schedule.progress = (o.schedule.progress + 1) % n.max(1);
                    }
                    Button::EditorConfirm => finish(host, o, true),
                    Button::EditorDelete => finish(host, o, false),
                }
                return false;
            }
        }
        if o.editing.is_some() {
            click_editor(host, o, c, mouse, button, shift);
            return false;
        }
        if let (Some(a), _) = action(host, o, c, mouse, Some(button)) {
            act(host, o, a);
        }
        false
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        close_later(open);
    }
    Flow::Cancel
}

pub(crate) fn screen_mouse_released(
    host: &dyn ClientHost,
    _event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.pressed = false;
            if let Some(e) = o.editing.as_mut() {
                e.widgets.mouse_released(0);
            }
        }
    });
    Flow::Cancel
}

/// The wheel: on the editor's inputs and type, the inputs; else Shift turns a card's
/// conditions, and the list scrolls (`mouseScrolled`).
pub(crate) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let delta = event.scroll[1];
    let up = delta > 0.0;
    let shift = host.modifiers().shift();
    let sound = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        let c = corner(o.size);
        if o.editing.is_some() {
            return scroll_editor(o, c, mouse, up, shift);
        }
        if shift {
            let mut y = mouse[1] - c[1] - 25.0 + o.scroll;
            for i in 0..o.schedule.entries.len() {
                let e = &o.schedule.entries[i];
                let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
                let h = CARD_HEADER + 24.0 + rows * 18.0;
                if y >= h {
                    y -= h + 9.0;
                    if y < 0.0 {
                        break;
                    }
                    continue;
                }
                if !o.scrollable(host, e)
                    || y < 24.0
                    || mouse[0] < c[0] + 25.0
                    || mouse[0] > c[0] + 205.0
                {
                    break;
                }
                let last = e.conditions.len() as f32 - 1.0;
                let a = &mut o.across[i];
                if up && a.1 != 0.0 {
                    a.1 -= 1.0;
                } else if !up && a.1 != last {
                    a.1 += 1.0;
                }
                return None;
            }
        }
        let mut max = 40.0 - 173.0;
        for e in &o.schedule.entries {
            let rows = e.conditions.iter().map(Vec::len).max().unwrap_or(0) as f32;
            max += CARD_HEADER + 24.0 + rows * 18.0 + 10.0;
        }
        o.scroll_to = if max > 0.0 {
            (o.scroll_to - delta.signum() as f32 * 12.0)
                .clamp(0.0, max)
                .floor()
        } else {
            0.0
        };
        None
    });
    if let Some(pitch) = sound {
        host.play_local_sound(&SoundPlay::ui("create:scroll_value").with_pitch(pitch), 0);
    }
    Flow::Cancel
}

/// The editor's wheel: the type's selector (`SelectionScrollInput`) or one of the line's scroll
/// inputs; the scroll sound's pitch when it changed.
fn scroll_editor(o: &mut Open, c: [f32; 2], mouse: [f32; 2], up: bool, shift: bool) -> Option<f32> {
    let e = o.editing.as_mut()?;
    let selector = [c[0] + 56.0, c[1] + 65.0, 143.0, 16.0];
    if inside(selector, mouse) {
        let types: &[&str] = if e.instruction() {
            &INSTRUCTIONS
        } else {
            &CONDITIONS
        };
        let state = types.iter().position(|t| *t == e.input.id).unwrap_or(0) as i32;
        let kind = Kind::Choice {
            options: types.to_vec(),
        };
        let next = scrolled(&kind, state, up, shift);
        if next == state {
            return None;
        }
        // A new input of the type chosen (`calling`), unless it is the same type.
        let id = types[next as usize];
        if e.input.id != id {
            e.input = Input::new(id);
            e.ghosts = [None, None];
            rebuild_line(e, c);
        }
        let _ = up;
        return Some(1.5 + 0.1 * next as f32 / (types.len() - 1).max(1) as f32);
    }
    for f in fields(&e.input.id) {
        if !inside(field_rect(c, &f), mouse) || matches!(f.kind, Kind::Text { .. }) {
            continue;
        }
        let state = e.input.int(f.key);
        let next = scrolled(&f.kind, state, up, shift);
        if next == state {
            return None;
        }
        e.input.set_int(f.key, next);
        let (min, max) = match &f.kind {
            Kind::Number { min, max, .. } => (*min, *max),
            Kind::Choice { options } => (0, options.len() as i32),
            Kind::Text { .. } => (0, 1),
        };
        return Some(1.5 + 0.1 * (next - min) as f32 / (max - min).max(1) as f32);
    }
    None
}

/// Keys: while editing, the line's text field takes them, Enter confirms the editor and the
/// suggestions take Tab and the arrows; the inventory key closes only outside the editor.
pub(crate) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let key = event.key;
    if key == "key.keyboard.escape" {
        return Flow::Continue;
    }
    let modifiers = host.modifiers().bits;
    let close = OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return false;
        };
        let Some(e) = o.editing.as_mut() else {
            return key == "key.keyboard.e";
        };
        // `DestinationSuggestions.keyPressed`.
        if !e.suggestions.is_empty() {
            let n = e.suggestions.len();
            match key {
                "key.keyboard.tab" => {
                    let pick = e.suggestions[e.chosen].clone();
                    if let Some((_, id)) = e.texts.first().copied()
                        && let Some(f) = e.widgets.text_field_mut(id)
                    {
                        f.set_value(&pick);
                    }
                    e.chosen = (e.chosen + 1) % n;
                    update_suggestions(o);
                    return false;
                }
                "key.keyboard.up" => {
                    e.chosen = (e.chosen + n - 1) % n;
                    update_suggestions(o);
                    return false;
                }
                "key.keyboard.down" => {
                    e.chosen = (e.chosen + 1) % n;
                    update_suggestions(o);
                    return false;
                }
                _ => {}
            }
        }
        let before = text_values(e);
        match e.widgets.key_pressed(host, key, modifiers) {
            Outcome::Submitted(_) => {
                finish(host, o, true);
                return false;
            }
            Outcome::Changed(_) => keep_allowed(e, &before),
            _ => {}
        }
        update_suggestions(o);
        false
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        close_later(open);
    }
    Flow::Cancel
}

pub(crate) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let Some(e) = o.editing.as_mut() else {
            return;
        };
        let before = text_values(e);
        if let Outcome::Changed(_) = e.widgets.char_typed(host, c) {
            keep_allowed(e, &before);
        }
        update_suggestions(o);
    });
    Flow::Cancel
}

fn text_values(e: &Editing) -> Vec<String> {
    e.texts
        .iter()
        .map(|(_, id)| {
            e.widgets
                .text_field(*id)
                .map_or_else(String::new, |f| f.value().to_owned())
        })
        .collect()
}

/// `EditBox.setFilter`: a change the field's filter refuses is undone.
fn keep_allowed(e: &mut Editing, before: &[String]) {
    let line = fields(&e.input.id);
    for (n, (k, id)) in e.texts.iter().enumerate() {
        let Some(f) = line.get(*k) else {
            continue;
        };
        if let Some(field) = e.widgets.text_field_mut(*id)
            && !text_allowed(&f.kind, field.value())
            && let Some(old) = before.get(n)
        {
            field.set_value(old);
        }
    }
}

/// `removed`: the schedule goes to the server at the next tick.
pub(crate) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(sent(&open)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_crosses_the_wire() {
        let o = Opened {
            selected: 4,
            inventory: vec![(0, vec![1, 2, 3]), (35, Vec::new())],
            schedule: vec![10, 0, 0, 5],
            stations: vec!["Depot".into(), "Mine, Platform 1".into()],
        };
        assert_eq!(Opened::decode(&o.encode()), Some(o.clone()));
        assert_eq!(Opened::decode(&o.encode()[..5]), None);
    }

    #[test]
    fn scroll_inputs_step_and_clamp_as_creates() {
        let throttle = &fields("throttle")[0].kind;
        // 5 a notch, 25 with Shift, between 5 and 100.
        assert_eq!(scrolled(throttle, 100, true, false), 100);
        assert_eq!(scrolled(throttle, 100, false, false), 95);
        assert_eq!(scrolled(throttle, 95, false, true), 70);
        assert_eq!(scrolled(throttle, 10, false, true), 5);
        let delay = &fields("delay")[0].kind;
        // Shift steps 15 and rounds down to it.
        assert_eq!(scrolled(delay, 5, true, true), 15);
        assert_eq!(scrolled(delay, 119, true, true), 120);
        // A choice turns the other way: the wheel up is the option before.
        let unit = &fields("delay")[1].kind;
        assert_eq!(scrolled(unit, 1, true, false), 0);
        assert_eq!(scrolled(unit, 1, false, false), 2);
        assert_eq!(scrolled(unit, 2, false, false), 2);
    }

    #[test]
    fn text_fields_filter_as_creates() {
        let dest = &fields("destination")[0].kind;
        assert!(text_allowed(dest, "A*B*C*"));
        assert!(!text_allowed(dest, "*A*B*C*"));
        let threshold = &fields("item_threshold")[1].kind;
        assert!(text_allowed(threshold, ""));
        assert!(text_allowed(threshold, "-12"));
        assert!(!text_allowed(threshold, "1a"));
    }

    #[test]
    fn cards_are_as_tall_as_their_groups() {
        let mut e = Entry {
            instruction: Input::new("destination"),
            conditions: vec![vec![Input::new("delay")], vec![Input::new("idle"); 3]],
        };
        assert_eq!(card_height(&e), 22.0 + 24.0 + 3.0 * 18.0);
        e.instruction = Input::new("rename");
        assert_eq!(card_height(&e), 26.0);
    }
}
