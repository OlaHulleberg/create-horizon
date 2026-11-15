//! The threshold switch's screen (`ThresholdSwitchScreen`): its upper and lower thresholds
//! (`ScrollInput`s stepping by an item, a stack or a bucket), whether items count in stacks,
//! the two torches showing when it powers, the inverting and confirm buttons, the block it
//! reads and the switch's item. A click on the switch (not on its filter slot) opens it from
//! the server with what the switch holds; while it is open the client asks again every ten
//! ticks for what the switch reads (Create's screen reads its client block entity each frame).
//! It sends its settings back 20 ticks after the last change, on inverting and when it closes
//! (`ConfigureThresholdSwitchPacket`), right after the key or click (`Subsystem::input_handled`:
//! the screen hooks are given a read-only client). Create's link from the block's item to the Ponder tag is left out with
//! Ponder.

use super::block_at;
use crate::glue::net;
use crate::glue::server::ServerLevel;
use create_engine::Block;
use create_engine::observe;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, DrawContext, Flow, PlayerRef, SceneBlock, SceneView, ScreenClosingEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, SoundPlay, Text,
};
use std::cell::RefCell;

/// Server to client: the screen opened (0) or what the switch reads now (1); client to server:
/// the settings (0) or a question for what it reads (1).
pub(super) const CHANNEL: &str = "create:threshold_switch";
/// The mod whose own screen it is.
const MOD: &str = "create";

const TEXTURE: &str = "create:gui/threshold_switch";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
/// `AllGuiTextures.THRESHOLD_SWITCH`, and `setWindowOffset(-20, 0)`.
const SIZE: [f32; 2] = [182.0, 103.0];
const OFFSET_X: i32 = -20;
/// `AbstractSimiWidget.HEADER_RGB`.
const HEADER: &str = "#5391E1";

/// `ThresholdSwitchBlockEntity.ThresholdType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Kind {
    #[default]
    Unsupported,
    Item,
    Fluid,
    /// A `ThresholdSwitchObservable`: a rope pulley.
    Custom,
}

/// What the switch holds and reads, as its screen shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Shown {
    pub pos: [i32; 3],
    pub off_below: i32,
    pub on_above: i32,
    pub inverted: bool,
    pub in_stacks: bool,
    /// `poweredAfterDelay`.
    pub powered: bool,
    pub kind: Kind,
    pub level: i32,
    pub min: i32,
    pub max: i32,
    /// The block it reads (`getDisplayItemForScreen`), empty for air.
    pub item: String,
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    fn i32(&mut self) -> Option<i32> {
        self.bytes(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn pos(&mut self) -> Option<[i32; 3]> {
        Some([self.i32()?, self.i32()?, self.i32()?])
    }
}

impl Shown {
    fn encode(&self, what: u8) -> Vec<u8> {
        let mut out = vec![what];
        for v in self.pos {
            put_i32(&mut out, v);
        }
        put_i32(&mut out, self.off_below);
        put_i32(&mut out, self.on_above);
        out.push(u8::from(self.inverted));
        out.push(u8::from(self.in_stacks));
        out.push(u8::from(self.powered));
        out.push(self.kind as u8);
        put_i32(&mut out, self.level);
        put_i32(&mut out, self.min);
        put_i32(&mut out, self.max);
        let item = &self.item.as_bytes()[..self.item.len().min(255)];
        out.push(item.len() as u8);
        out.extend_from_slice(item);
        out
    }

    fn decode(data: &[u8]) -> Option<(u8, Shown)> {
        let mut r = Reader(data);
        let what = r.u8()?;
        let pos = r.pos()?;
        let (off_below, on_above) = (r.i32()?, r.i32()?);
        let (inverted, in_stacks, powered) = (r.u8()? != 0, r.u8()? != 0, r.u8()? != 0);
        let kind = match r.u8()? {
            1 => Kind::Item,
            2 => Kind::Fluid,
            3 => Kind::Custom,
            _ => Kind::Unsupported,
        };
        let (level, min, max) = (r.i32()?, r.i32()?, r.i32()?);
        let n = usize::from(r.u8()?);
        let item = String::from_utf8(r.bytes(n)?.to_vec()).ok()?;
        Some((
            what,
            Shown {
                pos,
                off_below,
                on_above,
                inverted,
                in_stacks,
                powered,
                kind,
                level,
                min,
                max,
                item,
            },
        ))
    }
}

/// The settings the client sends: `ConfigureThresholdSwitchPacket`'s.
fn encode_settings(pos: [i32; 3], off: i32, on: i32, invert: bool, stacks: bool) -> Vec<u8> {
    let mut out = vec![0];
    for v in [pos[0], pos[1], pos[2], off, on] {
        put_i32(&mut out, v);
    }
    out.push(u8::from(invert));
    out.push(u8::from(stacks));
    out
}

// ---------------------------------------------------------------------------------------------
// Server

/// What the switch at `pos` holds and reads now; None when there is none.
fn shown(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<Shown> {
    let (block, info) = block_at(lv, pos)?;
    if block != Block::StockpileSwitch {
        return None;
    }
    let target = observe::target_pos(pos, info.target?, info.facing?);
    // `getTypeOfCurrentTarget`: an item handler first, then a fluid handler, then a
    // `ThresholdSwitchObservable`.
    let (seen, _) = super::detectors::switch_seen(lv, target);
    let kind = if seen.inventory.is_some() {
        Kind::Item
    } else if seen.tanks.is_some() {
        Kind::Fluid
    } else if seen.custom.is_some() {
        Kind::Custom
    } else {
        Kind::Unsupported
    };
    let item = lv
        .block_state(target)
        .and_then(|s| lv.state_name(s))
        .filter(|n| n != "minecraft:air")
        .unwrap_or_default();
    let dim = lv.dim;
    crate::glue::world::with_world(|w| {
        let s = w.dims[dim].detectors.switches.get(&pos)?;
        Some(Shown {
            pos,
            off_below: s.off_below,
            on_above: s.on_above,
            inverted: s.inverted,
            in_stacks: s.in_stacks,
            powered: s.powered,
            kind,
            level: s.level,
            min: s.min,
            max: s.max,
            item,
        })
    })
    .flatten()
}

/// `ThresholdSwitchBlock.useItemOn` -> `displayScreen`: the screen opens for `player`.
pub(super) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    if let Some(s) = shown(lv, pos) {
        net::send_to_player(lv.host, player, CHANNEL, &s.encode(0));
    }
}

/// The client's settings (`ConfigureThresholdSwitchPacket.applySettings`, from within 20
/// blocks of the switch) or its question for what the switch reads.
pub(super) fn client_message(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    channel: &str,
    data: &[u8],
) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let (Some(what), Some(pos)) = (r.u8(), r.pos()) else {
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
    if !near {
        return true;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return true;
    };
    match what {
        0 => {
            let (Some(off), Some(on), Some(invert), Some(stacks)) =
                (r.i32(), r.i32(), r.u8(), r.u8())
            else {
                return true;
            };
            super::detectors::configure(&mut lv, pos, off, on, invert != 0, stacks != 0);
        }
        _ => {
            if let Some(s) = shown(&mut lv, pos) {
                net::send_to_player(lv.host, player, CHANNEL, &s.encode(1));
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The inputs (`ScrollInput`s), as plain state

/// A `ScrollInput`: its state within `min..max` (the end left out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Input {
    state: i32,
    min: i32,
    max: i32,
}

impl Input {
    /// `clampState`.
    fn clamp(&mut self) {
        if self.state >= self.max {
            self.state = self.max - 1;
        }
        if self.state < self.min {
            self.state = self.min;
        }
    }
    fn set(&mut self, state: i32) {
        self.state = state;
        self.clamp();
    }
    fn range(&mut self, min: i32, max: i32) {
        self.min = min;
        self.max = max;
    }
}

/// `Mth.clamp` for ints: the minimum wins when they cross.
fn mth_clamp(v: i32, min: i32, max: i32) -> i32 {
    if v < min { min } else { v.min(max) }
}

/// The screen's inputs: the upper (`onAbove`) and lower (`offBelow`) thresholds and the item
/// measure (`inStacks`), and the ticks since the last change (`lastModification`, -1 for none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Inputs {
    on: Input,
    off: Input,
    stacks: bool,
    last_modification: i32,
}

impl Inputs {
    /// `init`, on the switch's settings and range.
    fn new(s: &Shown) -> Inputs {
        let mut i = Inputs {
            on: Input::default(),
            off: Input::default(),
            stacks: s.in_stacks,
            last_modification: -1,
        };
        let step = i.step(s.kind);
        i.off.range(s.min, s.max + 1 - step);
        i.off.set(s.off_below);
        i.on.range(s.min + step, s.max + 1);
        i.on.set(s.on_above);
        i.on_changed(s.kind);
        i.off_changed(s.kind);
        i.update(s);
        i
    }

    /// `getValueStep`.
    fn step(&self, kind: Kind) -> i32 {
        if kind == Kind::Fluid {
            1000
        } else if self.stacks {
            64
        } else {
            1
        }
    }

    /// `onAbove`'s callback: the lower threshold stays below it.
    fn on_changed(&mut self, kind: Kind) {
        self.last_modification = 0;
        let step = self.step(kind);
        if self.off.state / step == 0 && self.on.state / step == 0 {
            return;
        }
        if self.off.state / step >= self.on.state / step {
            self.off.set((self.on.state - step) / step * step);
            self.off_changed(kind);
        }
    }

    /// `offBelow`'s callback: the upper threshold stays above it.
    fn off_changed(&mut self, kind: Kind) {
        self.last_modification = 0;
        let step = self.step(kind);
        if self.on.state / step == 0 && self.off.state / step == 0 {
            return;
        }
        if self.on.state / step <= self.off.state / step {
            self.on.set((self.off.state + step) / step * step);
            self.on_changed(kind);
        }
    }

    /// `updateInputBoxes`: the ranges follow what the switch reads; the thresholds round to
    /// whole steps within them.
    fn update(&mut self, s: &Shown) {
        let step = self.step(s.kind);
        let (min, max) = (s.min + step, s.max);
        self.on.range(min, max + 1);
        let rounded = mth_clamp(self.on.state / step * step, min, max);
        if rounded != self.on.state {
            self.on.set(rounded);
            self.on_changed(s.kind);
        }
        let (min, max) = (s.min, s.max - step);
        self.off.range(min, max + 1);
        let rounded = mth_clamp(self.off.state / step * step, min, max);
        if rounded != self.off.state {
            self.off.set(rounded);
            self.off_changed(s.kind);
        }
    }

    /// `ScrollInput.mouseScrolled` on the upper (`upper`) or lower threshold: a step, ten with
    /// shift (then down to a multiple of 5). The state and range when it moved.
    fn scroll(
        &mut self,
        kind: Kind,
        upper: bool,
        up: bool,
        shift: bool,
    ) -> Option<(i32, i32, i32)> {
        let step = self.step(kind);
        let by = if shift { 10 * step } else { step };
        let input = if upper { &mut self.on } else { &mut self.off };
        let prior = input.state;
        input.state += if up { by } else { -by };
        if shift {
            input.state -= input.state % 5;
        }
        input.clamp();
        if input.state == prior {
            return None;
        }
        let moved = (input.state, input.min, input.max);
        if upper {
            self.on_changed(kind);
        } else {
            self.off_changed(kind);
        }
        Some(moved)
    }
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    shown: Shown,
    inputs: Inputs,
    /// The screen's size when last drawn.
    size: [f32; 2],
    ticks: u32,
}

#[derive(Default)]
struct Later {
    send: Vec<Vec<u8>>,
    close: bool,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<Later> = RefCell::new(Later::default());
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = Later::default());
}

/// `send`: the inputs' settings with `invert`.
fn settings(open: &Open, invert: bool) -> Vec<u8> {
    encode_settings(
        open.shown.pos,
        open.inputs.off.state,
        open.inputs.on.state,
        invert,
        open.inputs.stacks,
    )
}

/// The server opened the screen, or told what the switch reads.
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let Some((what, shown)) = Shown::decode(data) else {
        return true;
    };
    if what == 1 {
        // What its client block entity would now hold: the reading and output, not the
        // settings being edited.
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut().filter(|o| o.shown.pos == shown.pos) {
                o.shown.powered = shown.powered;
                o.shown.inverted = shown.inverted;
                o.shown.kind = shown.kind;
                o.shown.level = shown.level;
                o.shown.min = shown.min;
                o.shown.max = shown.max;
                o.shown.item = shown.item;
            }
        });
        return true;
    }
    if let Some(old) = OPEN.with_borrow_mut(Option::take) {
        let invert = old.shown.inverted;
        net::send_to_server(host, CHANNEL, &settings(&old, invert));
    }
    let inputs = Inputs::new(&shown);
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            shown,
            inputs,
            size: [0.0; 2],
            ticks: 0,
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

/// `tick`: the settings go 20 ticks after the last change; the ranges follow the reading,
/// which is asked for every ten ticks. Then what the hooks left: closing and sending.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let mut send = Vec::new();
    OPEN.with_borrow_mut(|o| {
        let Some(open) = o.as_mut() else {
            return;
        };
        open.ticks += 1;
        if open.inputs.last_modification >= 0 {
            open.inputs.last_modification += 1;
        }
        if open.inputs.last_modification >= 20 {
            open.inputs.last_modification = -1;
            send.push(settings(open, open.shown.inverted));
        }
        open.inputs.update(&open.shown);
        if open.ticks % 10 == 0 {
            let mut ask = vec![1];
            for v in open.shown.pos {
                put_i32(&mut ask, v);
            }
            send.push(ask);
        }
    });
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    flush(host);
}

/// What the hooks left: closing and sending, and the settings once the screen is gone.
pub(super) fn flush(host: &mut dyn ClientHost) {
    let later = LATER.with_borrow_mut(std::mem::take);
    if later.close {
        host.mod_screen_close(MOD);
    }
    for m in later.send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        let invert = open.shown.inverted;
        net::send_to_server(host, CHANNEL, &settings(&open, invert));
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

fn styled(text: String, color: &str, italic: bool) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text).with_style(style)
}

/// `AbstractSimiScreen.init`: the window's corner.
fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] as i32 - SIZE[0] as i32) / 2 + OFFSET_X) as f32,
        ((size[1] as i32 - SIZE[1] as i32) / 2) as f32,
    )
}

fn inside(r: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The screen's widgets, as `[x, y, w, h]` from its corner.
fn upper_input(left: f32, top: f32, items: bool) -> [f32; 4] {
    [
        left + 48.0,
        top + 23.0,
        if items { 48.0 } else { 103.0 },
        18.0,
    ]
}
fn lower_input(left: f32, top: f32, items: bool) -> [f32; 4] {
    [
        left + 48.0,
        top + 47.0,
        if items { 48.0 } else { 103.0 },
        18.0,
    ]
}
fn stacks_input(left: f32, top: f32) -> [f32; 4] {
    [left + 100.0, top + 23.0, 52.0, 42.0]
}
fn confirm_button(left: f32, top: f32) -> [f32; 4] {
    [left + SIZE[0] - 33.0, top + SIZE[1] - 24.0, 18.0, 18.0]
}
fn flip_button(left: f32, top: f32) -> [f32; 4] {
    [left + SIZE[0] - 62.0, top + SIZE[1] - 24.0, 18.0, 18.0]
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(open) = o.as_mut() {
            open.size = event.draw.size();
            draw(host, event.draw, mouse, open);
        }
    });
}

/// `format`: a number and its unit.
fn format(host: &dyn ClientHost, kind: Kind, value: i32, stacks: bool) -> String {
    // `ThresholdSwitchObservable.format`: a pulley's height.
    if kind == Kind::Custom {
        return tr(host, "create.gui.threshold_switch.pulley_y_level")
            .replace("%1$s", &value.to_string());
    }
    let unit = match kind {
        Kind::Item if stacks => "create.schedule.condition.threshold.stacks",
        Kind::Item => "create.schedule.condition.threshold.items",
        _ => "create.schedule.condition.threshold.buckets",
    };
    format!("{value} {}", tr(host, unit))
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

/// An `IconButton`: the button, hovered or not, and its icon at `icon` of the icon atlas.
fn icon_button(draw: &mut dyn DrawContext, r: [f32; 4], icon: [f32; 2], mouse: [f32; 2]) {
    let u = if inside(r, mouse) { 18.0 } else { 0.0 };
    draw.sprite(WIDGETS, [u, 0.0, 18.0, 18.0], r, None);
    draw.sprite(
        ICONS,
        [icon[0], icon[1], 16.0, 16.0],
        [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
        None,
    );
}

/// `renderWindow`, the buttons and the hovered widget's tooltip.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], open: &Open) {
    let (x, y) = corner(draw.size());
    let s = &open.shown;
    let inputs = &open.inputs;
    // `renderAdditional`'s switch item, five times its size, behind the window's right edge.
    if let Some(key) = item_key(host, "create:stockpile_switch") {
        draw.item(key, [x + SIZE[0] + 6.0, y + SIZE[1] - 56.0], 80.0);
    }
    draw.sprite(
        TEXTURE,
        [0.0, 0.0, SIZE[0], SIZE[1]],
        [x, y, SIZE[0], SIZE[1]],
        None,
    );
    let title = tr(host, "create.gui.threshold_switch.title");
    let tw = draw.text_width(&title);
    let tx = x + (SIZE[0] as i32 / 2) as f32 - (tw as i32 / 2) as f32;
    draw.text(&title, [tx, y + 4.0], 0xFF59_2424, false);

    let items = s.kind == Kind::Item;
    let v = if items { 105.0 } else { 129.0 };
    for dy in [21.0, 45.0] {
        draw.sprite(
            TEXTURE,
            [0.0, v, 115.0, 22.0],
            [x + 44.0, y + dy, 115.0, 22.0],
            None,
        );
    }
    let stacks = inputs.stacks;
    let step = inputs.step(s.kind);
    if items {
        let suffix = tr(
            host,
            if stacks {
                "create.schedule.condition.threshold.stacks"
            } else {
                "create.schedule.condition.threshold.items"
            },
        );
        draw.text(&suffix, [x + 105.0, y + 28.0], 0xFFFF_FFFF, true);
        draw.text(&suffix, [x + 105.0, y + 52.0], 0xFFFF_FFFF, true);
    }
    let value = |v: i32| match s.kind {
        Kind::Unsupported => String::new(),
        Kind::Item => (v / step).to_string(),
        _ => format(host, s.kind, v / step, stacks),
    };
    draw.text(
        &format!("\u{2265} {}", value(inputs.on.state)),
        [x + 53.0, y + 28.0],
        0xFFFF_FFFF,
        true,
    );
    draw.text(
        &format!("\u{2264} {}", value(inputs.off.state)),
        [x + 53.0, y + 52.0],
        0xFFFF_FFFF,
        true,
    );

    // The block it reads, or a barrier.
    let (item_x, item_y) = (x + 13.0, y + 80.0);
    let shown_item = (!s.item.is_empty())
        .then(|| item_key(host, &s.item))
        .flatten();
    if let Some(key) = shown_item.or_else(|| item_key(host, "minecraft:barrier")) {
        draw.item(key, [item_x, item_y], 16.0);
    }

    // The torch row it powers in, highlighted, and the two torches.
    let (torch_x, torch_y) = (x + 23.0, y + 24.0);
    let top_row = s.inverted ^ s.powered;
    draw.sprite(
        TEXTURE,
        [0.0, 153.0, 24.0, 24.0],
        [
            torch_x - 3.0,
            torch_y - 4.0 + if top_row { 0.0 } else { 24.0 },
            24.0,
            24.0,
        ],
        None,
    );
    for (row, power) in [(0.0, true), (26.0, false)] {
        let lit = s.inverted ^ power;
        let state = host.block_state_id(if lit {
            "minecraft:redstone_torch[lit=true]"
        } else {
            "minecraft:redstone_torch[lit=false]"
        });
        if let Some(state) = state {
            let top = torch_y + row - 4.0;
            let _ = draw.draw_scene(
                [torch_x - 4.0, top, torch_x + 20.0, top + 24.0],
                &mod_api::Scene::new(vec![SceneBlock::new([0, 0, 0], state, None)]),
                SceneView::new([0.5, 0.35, 0.5], 45.0, 22.5, 20.0),
            );
        }
    }

    // The buttons (`I_FLIP`, `I_CONFIRM`).
    icon_button(draw, flip_button(x, y), [128.0, 64.0], mouse);
    icon_button(draw, confirm_button(x, y), [0.0, 16.0], mouse);

    // Tooltips: the block it reads, the torches, then the hovered widget's.
    let mut lines: Vec<Text> = Vec::new();
    if inside([item_x, item_y, 16.0, 16.0], mouse) {
        let view = styled(
            tr(host, "create.display_link.view_compatible"),
            "dark_gray",
            false,
        );
        match shown_item {
            None => {
                lines.push(Text::literal(tr(
                    host,
                    "create.gui.threshold_switch.not_attached",
                )));
                lines.push(view);
            }
            Some(key) => {
                lines.push(Text::literal(host.hover_name(&mod_api::ItemStack::of(key))));
                if s.kind == Kind::Unsupported {
                    lines.push(styled(
                        tr(host, "create.gui.threshold_switch.incompatible"),
                        "gray",
                        false,
                    ));
                } else {
                    let now = format(host, s.kind, s.level / step, stacks);
                    lines.push(styled(
                        tr(host, "create.gui.threshold_switch.currently").replace("%1$s", &now),
                        "dark_aqua",
                        false,
                    ));
                    let max = format(host, s.kind, s.max / step, stacks);
                    let range = if s.min / step == 0 {
                        tr(host, "create.gui.threshold_switch.range_max").replace("%1$s", &max)
                    } else {
                        tr(host, "create.gui.threshold_switch.range")
                            .replace("%1$s", &(s.min / step).to_string())
                            .replace("%2$s", &max)
                    };
                    lines.push(styled(range, "gray", false));
                }
                lines.push(view);
            }
        }
    } else if let Some(power) = [(0.0, true), (26.0, false)]
        .into_iter()
        .find(|(row, _)| inside([torch_x, torch_y + row, 16.0, 16.0], mouse))
        .map(|(_, p)| p)
    {
        let key = if power ^ s.inverted {
            "create.gui.threshold_switch.power_on_when"
        } else {
            "create.gui.threshold_switch.power_off_when"
        };
        lines.push(styled(tr(host, key), HEADER, false));
    } else if s.kind != Kind::Unsupported && inside(upper_input(x, y, items), mouse) {
        lines = scroll_tooltip(host, "create.gui.threshold_switch.upper_threshold");
    } else if s.kind != Kind::Unsupported && inside(lower_input(x, y, items), mouse) {
        lines = scroll_tooltip(host, "create.gui.threshold_switch.lower_threshold");
    } else if items && inside(stacks_input(x, y), mouse) {
        lines.push(styled(
            tr(host, "create.schedule.condition.threshold.item_measure"),
            HEADER,
            false,
        ));
        for (k, key) in [
            "create.schedule.condition.threshold.items",
            "create.schedule.condition.threshold.stacks",
        ]
        .into_iter()
        .enumerate()
        {
            if (k == 1) == stacks {
                lines.push(styled(format!("-> {}", tr(host, key)), "white", false));
            } else {
                lines.push(styled(format!("> {}", tr(host, key)), "gray", false));
            }
        }
        lines.push(styled(
            tr(host, "create.gui.scrollInput.scrollToSelect"),
            "dark_gray",
            true,
        ));
    } else if inside(flip_button(x, y), mouse) {
        lines.push(Text::literal(tr(
            host,
            "create.gui.threshold_switch.invert_signal",
        )));
    }
    if !lines.is_empty() {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// `ScrollInput.updateTooltip`.
fn scroll_tooltip(host: &dyn ClientHost, title: &str) -> Vec<Text> {
    vec![
        styled(tr(host, title), HEADER, false),
        styled(
            tr(host, "create.gui.scrollInput.scrollToModify"),
            "dark_gray",
            true,
        ),
        styled(
            tr(host, "create.gui.scrollInput.shiftScrollsFaster"),
            "dark_gray",
            true,
        ),
    ]
}

/// `ScrollInput.mouseScrolled` on the hovered input, with `AllSoundEvents.SCROLL_VALUE`.
pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.scroll[1] == 0.0 {
        return Flow::Cancel;
    }
    let up = event.scroll[1] > 0.0;
    let shift = event.modifiers & 1 != 0;
    let mouse = event.screen.mouse;
    let moved = OPEN.with_borrow_mut(|o| {
        let open = o.as_mut()?;
        let (x, y) = corner(open.size);
        let kind = open.shown.kind;
        let items = kind == Kind::Item;
        if kind != Kind::Unsupported && inside(upper_input(x, y, items), mouse) {
            return open.inputs.scroll(kind, true, up, shift);
        }
        if kind != Kind::Unsupported && inside(lower_input(x, y, items), mouse) {
            return open.inputs.scroll(kind, false, up, shift);
        }
        if items && inside(stacks_input(x, y), mouse) {
            // A `SelectionScrollInput`: the wheel down moves on.
            let next = !up;
            if next != open.inputs.stacks {
                open.inputs.stacks = next;
                return Some((i32::from(next), 0, 2));
            }
        }
        None
    });
    if let Some((state, min, max)) = moved {
        let pitch = 1.5 + 0.1 * (state - min) as f32 / (max - min).max(1) as f32;
        host.play_local_sound(&SoundPlay::ui("create:scroll_value").with_pitch(pitch), 0);
    }
    Flow::Cancel
}

/// The buttons: inverting sends at once (`send(!isInverted)`); confirming closes the screen.
pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.button != 0 {
        return Flow::Cancel;
    }
    let mouse = event.screen.mouse;
    let Some(size) = OPEN.with_borrow(|o| o.as_ref().map(|o| o.size)) else {
        return Flow::Continue;
    };
    let (x, y) = corner(size);
    if inside(confirm_button(x, y), mouse) {
        host.play_local_sound(
            &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
            0,
        );
        if let Some(open) = OPEN.with_borrow_mut(Option::take) {
            let m = settings(&open, open.shown.inverted);
            LATER.with_borrow_mut(|l| {
                l.send.push(m);
                l.close = true;
            });
        }
    } else if inside(flip_button(x, y), mouse) {
        host.play_local_sound(
            &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
            0,
        );
        OPEN.with_borrow_mut(|o| {
            if let Some(open) = o.as_mut() {
                let m = settings(open, !open.shown.inverted);
                LATER.with_borrow_mut(|l| l.send.push(m));
            }
        });
    }
    Flow::Cancel
}

/// `removed`: the settings go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        let m = settings(&open, open.shown.inverted);
        LATER.with_borrow_mut(|l| l.send.push(m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(kind: Kind, min: i32, max: i32, off: i32, on: i32) -> Shown {
        Shown {
            pos: [1, 2, 3],
            off_below: off,
            on_above: on,
            kind,
            min,
            max,
            item: "minecraft:chest".into(),
            ..Shown::default()
        }
    }

    #[test]
    fn it_crosses_the_wire() {
        let s = shown(Kind::Fluid, 0, 8000, 1000, 4000);
        assert_eq!(Shown::decode(&s.encode(1)), Some((1, s)));
    }

    #[test]
    fn a_chest_s_thresholds_stay_in_its_range_and_apart() {
        // A chest of 27 slots: 0..=1728 items; Create's defaults (64, 128) stand.
        let s = shown(Kind::Item, 0, 1728, 64, 128);
        let mut i = Inputs::new(&s);
        assert_eq!((i.off.state, i.on.state), (64, 128));
        // The upper threshold scrolled under the lower one pushes it down.
        for _ in 0..64 {
            i.scroll(Kind::Item, true, false, false);
        }
        assert_eq!((i.off.state, i.on.state), (63, 64));
        assert!(i.last_modification == 0);
        // In stacks the thresholds round to whole stacks.
        i.stacks = true;
        i.update(&s);
        assert_eq!(i.on.state % 64, 0);
        assert_eq!(i.off.state % 64, 0);
        assert!(i.off.state < i.on.state);
    }

    #[test]
    fn a_tank_s_thresholds_step_by_buckets() {
        let s = shown(Kind::Fluid, 0, 8000, 1000, 4000);
        let mut i = Inputs::new(&s);
        assert_eq!((i.off.state, i.on.state), (1000, 4000));
        i.scroll(Kind::Fluid, true, true, false);
        assert_eq!(i.on.state, 5000);
        // Shift: ten buckets, within the tank.
        i.scroll(Kind::Fluid, true, true, true);
        assert_eq!(i.on.state, 8000);
        i.scroll(Kind::Fluid, false, false, true);
        assert_eq!(i.off.state, 0);
    }
}
