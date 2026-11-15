//! The filters' screens (`FilterScreen`, `AttributeFilterScreen` over `AbstractFilterScreen`,
//! their menus `FilterMenu`, `AttributeFilterMenu`, `GhostItemMenu`, `HeldItemGhostItemMenu`):
//! Create's panel, its ghost slots (eighteen for a list filter; a reference item and the
//! selected attributes for an attribute filter), the deny / allow and data buttons or the three
//! attribute modes, the attribute selector, reset and confirm, the player's inventory under the
//! panel and the held filter four times its size beside it.
//!
//! The server opens it with what the filter holds and a copy of the player's inventory (and, for
//! an attribute filter, the attributes each of those stacks has, read on the server where the
//! recipes are); the client sends what was chosen back when it closes (`FilterScreenPacket`'s
//! options and the menu's `saveData`), at its next tick. A ghost slot names the stack it copies:
//! an inventory slot of the copy sent, or the filter's own slot, so the server writes stacks it
//! read itself.
//!
//! Left out (no API for it):
//! - An item-opened mod menu has no screen layout (`player_open_mod_menu` without a block gives
//!   the client no `Screen`), so this is the mod's own screen: clicking an inventory slot puts a
//!   ghost copy of its stack on the cursor, and the player's real inventory never changes while
//!   it is open (Create carries the real stack).
//! - A slot's item tooltip is its name alone (`hover_name`): no host call gives an item's
//!   tooltip lines.
//! - The creative middle click on a ghost slot (a real full stack of it on the cursor): the
//!   cursor here carries ghosts only, as above (`API_GAPS.md`).

use super::super::net;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Host, ItemKey, ItemStack, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, SoundPlay, Tag, Text,
};
use std::cell::RefCell;

/// Server to client: open the screen; client to server: what was chosen.
pub(crate) const CHANNEL: &str = "create:filter_screen";
/// The mod whose own screen it is.
const MOD: &str = "create";

/// A list filter's ghost slots.
pub(crate) const LIST_SLOTS: usize = 18;

// ---------------------------------------------------------------------------------------------
// The wire

/// An attribute as the screen shows it (`ItemAttribute.format`): its type id and value as
/// saved, the lang key's suffix (`item_attributes.<key>`), and its argument, itself a lang key
/// when `arg_key` (an enchantment's name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AttrLine {
    pub kind: String,
    pub value: Option<String>,
    pub key: String,
    pub arg: Option<String>,
    pub arg_key: bool,
}

/// What the server opens the screen with.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Opened {
    /// An attribute filter's screen (else a list filter's).
    pub attribute: bool,
    /// The selected hotbar slot: the filter's own, which can't be picked.
    pub selected: u8,
    /// The player's inventory (slot 0..35, the stack's network NBT) and, for an attribute
    /// filter, each stack's attributes (`getAllAttributes`).
    pub inventory: Vec<(u8, Vec<u8>, Vec<AttrLine>)>,
    /// A list filter's slots (slot, network NBT).
    pub list: Vec<(u8, Vec<u8>)>,
    pub deny: bool,
    pub respect: bool,
    /// The attribute mode (`AttributeFilterWhitelistMode`'s ordinal) and the attributes chosen.
    pub mode: u8,
    pub chosen: Vec<(AttrLine, bool)>,
}

/// What a ghost slot copies: nothing, an inventory slot of the copy the server sent, or the
/// filter's own slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ghost {
    Inventory(u8),
    Own(u8),
}

impl Ghost {
    fn byte(g: Option<Ghost>) -> u8 {
        match g {
            None => 0,
            Some(Ghost::Inventory(i)) => 1 + i,
            Some(Ghost::Own(j)) => 64 + j,
        }
    }
    fn of(b: u8) -> Option<Ghost> {
        match b {
            1..=36 => Some(Ghost::Inventory(b - 1)),
            64..=127 => Some(Ghost::Own(b - 64)),
            _ => None,
        }
    }
}

/// What the client sends back when the screen closes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub attribute: bool,
    pub list: Vec<Option<Ghost>>,
    pub deny: bool,
    pub respect: bool,
    pub mode: u8,
    /// Type id, value, inverted.
    pub attributes: Vec<(String, Option<String>, bool)>,
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }
    fn bytes(&mut self, b: &[u8]) {
        let n = b.len().min(usize::from(u16::MAX));
        self.0.extend_from_slice(&(n as u16).to_le_bytes());
        self.0.extend_from_slice(&b[..n]);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn opt(&mut self, s: Option<&str>) {
        self.bool(s.is_some());
        if let Some(s) = s {
            self.str(s);
        }
    }
    fn line(&mut self, l: &AttrLine) {
        self.str(&l.kind);
        self.opt(l.value.as_deref());
        self.str(&l.key);
        self.opt(l.arg.as_deref());
        self.bool(l.arg_key);
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
        self.take(usize::from(n)).map(<[u8]>::to_vec)
    }
    fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
    fn opt(&mut self) -> Option<Option<String>> {
        if self.bool()? {
            self.str().map(Some)
        } else {
            Some(None)
        }
    }
    fn line(&mut self) -> Option<AttrLine> {
        Some(AttrLine {
            kind: self.str()?,
            value: self.opt()?,
            key: self.str()?,
            arg: self.opt()?,
            arg_key: self.bool()?,
        })
    }
}

impl Opened {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.bool(self.attribute);
        w.u8(self.selected);
        w.u8(self.inventory.len() as u8);
        for (slot, nbt, attrs) in &self.inventory {
            w.u8(*slot);
            w.bytes(nbt);
            w.u8(attrs.len().min(255) as u8);
            for a in attrs.iter().take(255) {
                w.line(a);
            }
        }
        w.u8(self.list.len() as u8);
        for (slot, nbt) in &self.list {
            w.u8(*slot);
            w.bytes(nbt);
        }
        w.bool(self.deny);
        w.bool(self.respect);
        w.u8(self.mode);
        w.u8(self.chosen.len().min(255) as u8);
        for (a, inverted) in self.chosen.iter().take(255) {
            w.line(a);
            w.bool(*inverted);
        }
        w.0
    }

    pub(crate) fn decode(data: &[u8]) -> Option<Opened> {
        let mut r = Reader(data);
        let attribute = r.bool()?;
        let selected = r.u8()?;
        let n = r.u8()?;
        let mut inventory = Vec::new();
        for _ in 0..n {
            let slot = r.u8()?;
            let nbt = r.bytes()?;
            let m = r.u8()?;
            let attrs = (0..m).map(|_| r.line()).collect::<Option<Vec<_>>>()?;
            inventory.push((slot, nbt, attrs));
        }
        let n = r.u8()?;
        let list = (0..n)
            .map(|_| Some((r.u8()?, r.bytes()?)))
            .collect::<Option<Vec<_>>>()?;
        let deny = r.bool()?;
        let respect = r.bool()?;
        let mode = r.u8()?;
        let n = r.u8()?;
        let chosen = (0..n)
            .map(|_| Some((r.line()?, r.bool()?)))
            .collect::<Option<Vec<_>>>()?;
        Some(Opened {
            attribute,
            selected,
            inventory,
            list,
            deny,
            respect,
            mode,
            chosen,
        })
    }
}

impl Chosen {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.bool(self.attribute);
        w.u8(self.list.len() as u8);
        for g in &self.list {
            w.u8(Ghost::byte(*g));
        }
        w.bool(self.deny);
        w.bool(self.respect);
        w.u8(self.mode);
        w.u8(self.attributes.len().min(255) as u8);
        for (kind, value, inverted) in self.attributes.iter().take(255) {
            w.str(kind);
            w.opt(value.as_deref());
            w.bool(*inverted);
        }
        w.0
    }

    pub(crate) fn decode(data: &[u8]) -> Option<Chosen> {
        let mut r = Reader(data);
        let attribute = r.bool()?;
        let n = r.u8()?;
        let list = (0..n)
            .map(|_| r.u8().map(Ghost::of))
            .collect::<Option<Vec<_>>>()?;
        let deny = r.bool()?;
        let respect = r.bool()?;
        let mode = r.u8()?;
        let n = r.u8()?;
        let attributes = (0..n)
            .map(|_| Some((r.str()?, r.opt()?, r.bool()?)))
            .collect::<Option<Vec<_>>>()?;
        Some(Chosen {
            attribute,
            list,
            deny,
            respect,
            mode,
            attributes,
        })
    }
}

/// A stack as the wire carries it (its network NBT), and back.
pub(crate) fn stack_bytes(host: &dyn Host, stack: &ItemStack) -> Option<Vec<u8>> {
    Tag::Compound(stack.to_nbt(host)?).to_network_bytes().ok()
}

pub(crate) fn stack_of(host: &dyn Host, bytes: &[u8]) -> Option<ItemStack> {
    match Tag::from_network_bytes(bytes, None).ok()?.0 {
        Tag::Compound(c) => ItemStack::from_nbt(host, &c),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// The ghost slots' rules

/// `GhostItemMenu.clicked` on a ghost slot: a 1-count copy of what is carried, or empty.
pub(crate) fn click_ghost<T: Clone>(slots: &mut [Option<T>], slot: usize, carried: Option<&T>) {
    if let Some(s) = slots.get_mut(slot) {
        *s = carried.cloned();
    }
}

/// `GhostItemMenu.quickMoveStack` from the inventory (repeats not allowed): into the first
/// empty slot, unless one like it comes first.
pub(crate) fn quick_move<T: Clone>(
    slots: &mut [Option<T>],
    stack: &T,
    same: impl Fn(&T, &T) -> bool,
) {
    for s in slots.iter_mut() {
        match s {
            Some(there) if same(there, stack) => break,
            Some(_) => {}
            None => {
                *s = Some(stack.clone());
                break;
            }
        }
    }
}

/// A click on an inventory slot (`clicked` below 36): its stack's ghost on the cursor, or the
/// cursor put down on the slot it came from or on an empty one. The filter's own slot
/// (`HeldItemGhostItemMenu.isInSlot`) is never picked.
pub(crate) fn click_inventory(
    cursor: Option<u8>,
    slot: u8,
    occupied: bool,
    selected: u8,
) -> Option<u8> {
    if slot == selected {
        return cursor;
    }
    if !occupied || cursor == Some(slot) {
        return None;
    }
    Some(slot)
}

// ---------------------------------------------------------------------------------------------
// The client

/// The open screen.
struct Open {
    attribute: bool,
    selected: u8,
    /// The filter's item: the screen closes when the main hand holds another.
    item: ItemKey,
    inventory: Vec<(u8, ItemStack)>,
    options: Vec<(u8, Vec<AttrLine>)>,
    /// The list's ghosts, with what each shows.
    list: Vec<Option<(Ghost, ItemStack)>>,
    deny: bool,
    respect: bool,
    mode: u8,
    chosen: Vec<(AttrLine, bool)>,
    /// The attribute filter's reference item (an inventory slot).
    reference: Option<u8>,
    /// The selector's option.
    option: usize,
    /// The ghost on the cursor (an inventory slot).
    cursor: Option<u8>,
    size: [f32; 2],
    pressed: bool,
}

impl Open {
    fn stack(&self, slot: u8) -> Option<&ItemStack> {
        self.inventory
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, st)| st)
    }

    fn options(&self) -> &[AttrLine] {
        self.reference
            .and_then(|r| self.options.iter().find(|(s, _)| *s == r))
            .map_or(&[][..], |(_, o)| o)
    }

    /// Whether the option chosen may be added (`referenceItemChanged`'s callback).
    fn may_add(&self) -> bool {
        let Some(o) = self.options().get(self.option) else {
            return false;
        };
        !self
            .chosen
            .iter()
            .any(|(c, _)| c.kind == o.kind && c.value == o.value)
    }

    fn chosen(&self) -> Chosen {
        Chosen {
            attribute: self.attribute,
            list: self
                .list
                .iter()
                .map(|g| g.as_ref().map(|(g, _)| *g))
                .collect(),
            deny: self.deny,
            respect: self.respect,
            mode: self.mode,
            attributes: self
                .chosen
                .iter()
                .map(|(a, inverted)| (a.kind.clone(), a.value.clone(), *inverted))
                .collect(),
        }
    }
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// What the screen hooks leave for the next tick: choices to send, and whether to close.
    static LATER: RefCell<(Vec<Chosen>, bool)> = const { RefCell::new((Vec::new(), false)) };
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
    let inventory: Vec<(u8, ItemStack)> = o
        .inventory
        .iter()
        .filter_map(|(slot, nbt, _)| Some((*slot, stack_of(h, nbt)?)))
        .collect();
    let mut list = vec![None; if o.attribute { 0 } else { LIST_SLOTS }];
    for (slot, nbt) in &o.list {
        if let (Some(cell), Some(stack)) = (list.get_mut(usize::from(*slot)), stack_of(h, nbt)) {
            *cell = Some((Ghost::Own(*slot), stack));
        }
    }
    let open = Open {
        attribute: o.attribute,
        selected: o.selected,
        item,
        options: o
            .inventory
            .into_iter()
            .map(|(slot, _, attrs)| (slot, attrs))
            .collect(),
        inventory,
        list,
        deny: o.deny,
        respect: o.respect,
        mode: o.mode,
        chosen: o.chosen,
        reference: None,
        option: 0,
        cursor: None,
        size: [0.0, 0.0],
        pressed: false,
    };
    OPEN.with_borrow_mut(|x| *x = Some(open));
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|x| *x = None);
    }
    true
}

pub(crate) fn tick(host: &mut dyn ClientHost) {
    // `containerTick`: the screen closes when the main hand no longer holds the filter.
    let item = OPEN.with_borrow(|o| o.as_ref().map(|o| o.item));
    if let Some(item) = item
        && host.mod_screen() == Some(MOD)
        && main_hand(&*host).map(|s| s.item()) != Some(item)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        LATER.with_borrow_mut(|l| {
            l.0.push(open.chosen());
            l.1 = true;
        });
    }
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for c in send {
        net::send_to_server(host, CHANNEL, &c.encode());
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &open.chosen().encode());
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

/// `translateDirect` with one argument.
fn tr1(host: &dyn ClientHost, key: &str, arg: &str) -> String {
    tr(host, key).replace("%1$s", arg).replace("%s", arg)
}

/// `ItemAttribute.format(inverted)`.
fn format(host: &dyn ClientHost, a: &AttrLine, inverted: bool) -> String {
    let key = format!(
        "create.item_attributes.{}{}",
        a.key,
        if inverted { ".inverted" } else { "" }
    );
    let arg = match &a.arg {
        Some(k) if a.arg_key => tr(host, k),
        Some(v) => v.clone(),
        None => String::new(),
    };
    tr1(host, &key, &arg)
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

// The layout, from `AllGuiTextures` and the screens' `init`.

/// `AllGuiTextures.FILTER` and `ATTRIBUTE_FILTER` in `filters.png`: `[u, v, w, h]`.
const FILTER_BG: [f32; 4] = [0.0, 0.0, 214.0, 99.0];
const ATTRIBUTE_BG: [f32; 4] = [0.0, 99.0, 241.0, 85.0];
/// `AllGuiTextures.PLAYER_INVENTORY`.
const INVENTORY_SIZE: [f32; 2] = [176.0, 108.0];

/// The screen's measures: background, window offset (`setWindowOffset`), player inventory
/// offset (`getPlayerInventoryXOffset` / `YOffset`).
struct Layout {
    bg: [f32; 4],
    offset: [f32; 2],
    inventory: [f32; 2],
}

const LIST: Layout = Layout {
    bg: FILTER_BG,
    offset: [-11.0, 5.0],
    inventory: [38.0, 121.0],
};
const ATTRIBUTE: Layout = Layout {
    bg: ATTRIBUTE_BG,
    offset: [-11.0, 7.0],
    inventory: [51.0, 107.0],
};

fn layout(attribute: bool) -> &'static Layout {
    if attribute { &ATTRIBUTE } else { &LIST }
}

impl Layout {
    /// `setWindowSize(max(bg, inventory) wide, bg + 4 + inventory high)`.
    fn window(&self) -> [f32; 2] {
        [
            self.bg[2].max(INVENTORY_SIZE[0]),
            self.bg[3] + 4.0 + INVENTORY_SIZE[1],
        ]
    }
    /// `leftPos`, `topPos`: the window centred, then moved by its offset.
    fn corner(&self, size: [f32; 2]) -> [f32; 2] {
        let w = self.window();
        [
            ((size[0] - w[0]) / 2.0).floor() + self.offset[0],
            ((size[1] - w[1]) / 2.0).floor() + self.offset[1],
        ]
    }
    /// Where the inventory panel is drawn (`getLeftOfCentered`, under the background).
    fn inventory_panel(&self, c: [f32; 2]) -> [f32; 2] {
        [
            c[0] - self.offset[0] + ((self.window()[0] - INVENTORY_SIZE[0]) / 2.0).floor(),
            c[1] + self.bg[3] + 4.0,
        ]
    }
    /// An inventory slot's top left (`addPlayerSlots`).
    fn inventory_slot(&self, c: [f32; 2], slot: u8) -> [f32; 2] {
        let [x, y] = [c[0] + self.inventory[0], c[1] + self.inventory[1]];
        let s = f32::from(slot);
        if slot < 9 {
            [x + s * 18.0, y + 58.0]
        } else {
            let i = s - 9.0;
            [x + (i % 9.0) * 18.0, y + (i / 9.0).floor() * 18.0]
        }
    }
    /// `[x, y, w, h]` of a button: reset and confirm (`background width - 62 / - 33`,
    /// `height - 24`).
    fn reset(&self, c: [f32; 2]) -> [f32; 4] {
        [
            c[0] + self.bg[2] - 62.0,
            c[1] + self.bg[3] - 24.0,
            18.0,
            18.0,
        ]
    }
    fn confirm(&self, c: [f32; 2]) -> [f32; 4] {
        [
            c[0] + self.bg[2] - 33.0,
            c[1] + self.bg[3] - 24.0,
            18.0,
            18.0,
        ]
    }
}

/// A list filter's ghost slot (`FilterMenu.addFilterSlots`).
fn list_slot(c: [f32; 2], i: usize) -> [f32; 2] {
    [
        c[0] + 23.0 + (i % 9) as f32 * 18.0,
        c[1] + 25.0 + (i / 9) as f32 * 18.0,
    ]
}

/// The attribute filter's two slots (`AttributeFilterMenu.addFilterSlots`).
const REFERENCE_SLOT: [f32; 2] = [16.0, 27.0];
const SELECTED_SLOT: [f32; 2] = [16.0, 62.0];
/// `attributeSelector`: `[x, y, w, h]`.
const SELECTOR: [f32; 4] = [39.0, 26.0, 137.0, 18.0];

/// The buttons: what they do and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Deny,
    Allow,
    Respect,
    Ignore,
    Any,
    All,
    None,
    Add,
    AddInverted,
    Reset,
    Confirm,
}

/// `AllIcons`' cell in `icons.png` (16 pixels a cell).
fn icon(b: Button) -> [f32; 2] {
    let (col, row) = match b {
        Button::Add => (0.0, 0.0),
        Button::Reset => (1.0, 0.0),
        Button::Deny => (8.0, 0.0),
        Button::Allow => (9.0, 0.0),
        Button::Any => (10.0, 0.0),
        Button::All => (11.0, 0.0),
        Button::None => (12.0, 0.0),
        Button::Respect => (13.0, 0.0),
        Button::Ignore => (14.0, 0.0),
        Button::Confirm => (0.0, 1.0),
        Button::AddInverted => (7.0, 4.0),
    };
    [col * 16.0, row * 16.0]
}

fn buttons(o: &Open, c: [f32; 2]) -> Vec<(Button, [f32; 4])> {
    let at = |x: f32, y: f32| [c[0] + x, c[1] + y, 18.0, 18.0];
    let l = layout(o.attribute);
    let mut b = if o.attribute {
        vec![
            (Button::Any, at(38.0, 61.0)),
            (Button::All, at(56.0, 61.0)),
            (Button::None, at(74.0, 61.0)),
            (Button::Add, at(182.0, 26.0)),
            (Button::AddInverted, at(200.0, 26.0)),
        ]
    } else {
        vec![
            (Button::Deny, at(18.0, 75.0)),
            (Button::Allow, at(36.0, 75.0)),
            (Button::Respect, at(60.0, 75.0)),
            (Button::Ignore, at(78.0, 75.0)),
        ]
    };
    b.push((Button::Reset, l.reset(c)));
    b.push((Button::Confirm, l.confirm(c)));
    b
}

/// `IconButton.green`: the option in force (`!isButtonEnabled`).
fn green(o: &Open, b: Button) -> bool {
    match b {
        Button::Deny => o.deny,
        Button::Allow => !o.deny,
        Button::Respect => o.respect,
        Button::Ignore => !o.respect,
        Button::Any => o.mode == 0,
        Button::All => o.mode == 1,
        Button::None => o.mode == 2,
        _ => false,
    }
}

/// `IconButton.active`: the add buttons only with an option that isn't chosen yet.
fn active(o: &Open, b: Button) -> bool {
    match b {
        Button::Add | Button::AddInverted => o.reference.is_some() && o.may_add(),
        _ => true,
    }
}

/// The button's name and, for the option buttons, its description (`getTooltipDescriptions`).
fn tip(b: Button) -> Option<(&'static str, Option<&'static str>)> {
    Some(match b {
        Button::Deny => (
            "create.gui.filter.deny_list",
            Some("create.gui.filter.deny_list.description"),
        ),
        Button::Allow => (
            "create.gui.filter.allow_list",
            Some("create.gui.filter.allow_list.description"),
        ),
        Button::Respect => (
            "create.gui.filter.respect_data",
            Some("create.gui.filter.respect_data.description"),
        ),
        Button::Ignore => (
            "create.gui.filter.ignore_data",
            Some("create.gui.filter.ignore_data.description"),
        ),
        Button::Any => (
            "create.gui.attribute_filter.allow_list_disjunctive",
            Some("create.gui.attribute_filter.allow_list_disjunctive.description"),
        ),
        Button::All => (
            "create.gui.attribute_filter.allow_list_conjunctive",
            Some("create.gui.attribute_filter.allow_list_conjunctive.description"),
        ),
        Button::None => (
            "create.gui.attribute_filter.deny_list",
            Some("create.gui.attribute_filter.deny_list.description"),
        ),
        Button::Add => ("create.gui.attribute_filter.add_attribute", None),
        Button::AddInverted => ("create.gui.attribute_filter.add_inverted_attribute", None),
        Button::Reset | Button::Confirm => return None,
    })
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// `AbstractContainerScreen.isHovering` of a 16-pixel slot.
fn over_slot(at: [f32; 2], p: [f32; 2]) -> bool {
    inside([at[0] - 1.0, at[1] - 1.0, 18.0, 18.0], p)
}

/// `TooltipHelper.cutTextComponent`: words wrapped at 200 pixels.
fn cut(draw: &dyn DrawContext, text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_inclusive(' ') {
        if !line.is_empty() && draw.text_width(&format!("{line}{word}")) > 200.0 {
            lines.push(std::mem::take(&mut line));
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Text cut to `width` pixels with "..." (`Label.setTextAndTrim`).
fn trim(draw: &dyn DrawContext, text: &str, width: f32) -> String {
    if draw.text_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if draw.text_width(&format!("{out}{ch}...")) > width {
            break;
        }
        out.push(ch);
    }
    out + "..."
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

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

/// `AbstractFilterScreen.renderBg`, the slots and widgets, the cursor and the tooltips.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    let l = layout(o.attribute);
    let size = draw.size();
    let c = l.corner(size);
    super::gadget_gui::dim(host, draw);
    // The player's inventory panel and its label.
    let p = l.inventory_panel(c);
    draw.sprite(
        "create:gui/player_inventory",
        [0.0, 0.0, INVENTORY_SIZE[0], INVENTORY_SIZE[1]],
        [p[0], p[1], INVENTORY_SIZE[0], INVENTORY_SIZE[1]],
        None,
    );
    draw.text(
        &tr(host, "container.inventory"),
        [p[0] + 8.0, p[1] + 6.0],
        0xFF40_4040,
        false,
    );
    // The background and its title, the filter's name.
    draw.sprite(
        "create:gui/filters",
        l.bg,
        [c[0], c[1], l.bg[2], l.bg[3]],
        None,
    );
    let title = tr(
        host,
        if o.attribute {
            "item.create.attribute_filter"
        } else {
            "item.create.filter"
        },
    );
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [
            c[0] + ((l.bg[2] - 8.0) / 2.0).floor() - (w / 2.0).floor(),
            c[1] + 4.0,
        ],
        if o.attribute {
            0xFF59_2424
        } else {
            0xFF30_3030
        },
        false,
    );
    // The held filter, four times its size, beside the window.
    if let Some(held) = main_hand(host) {
        draw.item_stack(&held, [c[0] + l.bg[2] + 8.0, c[1] + l.bg[3] - 52.0], 64.0);
    }
    // The buttons.
    for (b, r) in buttons(o, c) {
        let hovered = inside(r, mouse);
        let u = if !active(o, b) {
            90.0
        } else if hovered && o.pressed {
            36.0
        } else if hovered {
            18.0
        } else if green(o, b) {
            72.0
        } else {
            0.0
        };
        draw.sprite("create:gui/widgets", [u, 0.0, 18.0, 18.0], r, None);
        let i = icon(b);
        draw.sprite(
            "create:gui/icons",
            [i[0], i[1], 16.0, 16.0],
            [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
            None,
        );
    }
    let mut hover: Option<Vec<Text>> = None;
    // The player's inventory.
    for (slot, stack) in &o.inventory {
        let at = l.inventory_slot(c, *slot);
        let n = stack.count();
        let count = (n > 1).then(|| n.to_string());
        draw.gui_draw_item_with(stack, at, count.as_deref());
    }
    for slot in 0..36u8 {
        let at = l.inventory_slot(c, slot);
        if over_slot(at, mouse) {
            draw.fill([at[0], at[1], 16.0, 16.0], 0x80FF_FFFF);
            if o.cursor.is_none()
                && let Some(s) = o.stack(slot)
            {
                hover = Some(vec![Text::literal(host.hover_name(s))]);
            }
        }
    }
    if o.attribute {
        draw_attribute(host, draw, o, c, mouse, &mut hover);
    } else {
        for (i, g) in o.list.iter().enumerate() {
            let at = list_slot(c, i);
            if let Some((_, s)) = g {
                draw.item_stack(s, at, 16.0);
            }
            if over_slot(at, mouse) {
                draw.fill([at[0], at[1], 16.0, 16.0], 0x80FF_FFFF);
                if o.cursor.is_none()
                    && let Some((_, s)) = g
                {
                    hover = Some(vec![Text::literal(host.hover_name(s))]);
                }
            }
        }
    }
    // The buttons' tooltips (`handleTooltips`).
    let shift = host.modifiers().shift();
    for (b, r) in buttons(o, c) {
        if !inside(r, mouse) {
            continue;
        }
        let Some((name, desc)) = tip(b) else {
            continue;
        };
        let mut lines = vec![Text::literal(tr(host, name))];
        if let Some(desc) = desc {
            let key = tr(host, "create.tooltip.keyShift");
            let hold = tr(host, "create.tooltip.holdForDescription");
            let (a, rest) = hold.split_once("%1$s").unwrap_or((hold.as_str(), ""));
            let mut style = TextStyle::default();
            style.color = Some("dark_gray".to_owned());
            lines.push(
                Text::literal(a.to_owned())
                    .with_style(style)
                    .append(styled(key, "gray", false))
                    .append(styled(rest, "dark_gray", false)),
            );
            if shift {
                for line in cut(draw, &tr(host, desc)) {
                    lines.push(styled(line, "gray", false));
                }
            }
        }
        hover = Some(lines);
    }
    // The ghost on the cursor (`renderFloatingItem`).
    if let Some(s) = o.cursor.and_then(|slot| o.stack(slot)) {
        draw.item_stack(s, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
    }
    if let Some(lines) = hover {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// `AttributeFilterScreen`'s own parts: the reference slot, the selector and its label, and
/// the selected attributes' slot.
fn draw_attribute(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    o: &Open,
    c: [f32; 2],
    mouse: [f32; 2],
    hover: &mut Option<Vec<Text>>,
) {
    let reference = [c[0] + REFERENCE_SLOT[0], c[1] + REFERENCE_SLOT[1]];
    let reference_stack = o.reference.and_then(|r| o.stack(r));
    if let Some(s) = reference_stack {
        draw.item_stack(s, reference, 16.0);
    }
    if over_slot(reference, mouse) {
        draw.fill([reference[0], reference[1], 16.0, 16.0], 0x80FF_FFFF);
        if o.cursor.is_none()
            && let Some(s) = reference_stack
        {
            *hover = Some(vec![Text::literal(host.hover_name(s))]);
        }
    }
    // `renderForeground`: a name tag with the number of attributes chosen.
    let selected = [c[0] + SELECTED_SLOT[0], c[1] + SELECTED_SLOT[1]];
    if let Some(tag) = item_key(host, "minecraft:name_tag") {
        let count = o.chosen.len().to_string();
        draw.gui_draw_item_with(&ItemStack::of(tag), selected, Some(&count));
    }
    if over_slot(selected, mouse) {
        draw.fill([selected[0], selected[1], 16.0, 16.0], 0x80FF_FFFF);
        if o.cursor.is_none() {
            // `selectedAttributes`.
            let mut lines = vec![styled(
                tr(
                    host,
                    if o.chosen.is_empty() {
                        "create.gui.attribute_filter.no_selected_attributes"
                    } else {
                        "create.gui.attribute_filter.selected_attributes"
                    },
                ),
                "yellow",
                false,
            )];
            for (a, inverted) in &o.chosen {
                lines.push(styled(
                    format!("- {}", format(host, a, *inverted)),
                    "gray",
                    false,
                ));
            }
            *hover = Some(lines);
        }
    }
    // The selector's label (`attributeSelectorLabel`), and its tooltip.
    let options = o.options();
    let label_at = [c[0] + 43.0, c[1] + 31.0];
    match reference_stack {
        None => draw.text(
            &tr(host, "create.gui.attribute_filter.add_reference_item"),
            label_at,
            0xFFF3_EBDE,
            true,
        ),
        Some(s) => {
            if let Some(a) = options.get(o.option) {
                let text = trim(draw, &format(host, a, false), 112.0);
                draw.text(&text, label_at, 0xFFF3_EBDE, true);
            }
            let r = [
                c[0] + SELECTOR[0],
                c[1] + SELECTOR[1],
                SELECTOR[2],
                SELECTOR[3],
            ];
            if inside(r, mouse) {
                *hover = Some(selector_tip(host, &host.hover_name(s), options, o.option));
            }
        }
    }
}

/// `SelectionScrollInput.updateTooltip`.
fn selector_tip(
    host: &dyn ClientHost,
    name: &str,
    options: &[AttrLine],
    state: usize,
) -> Vec<Text> {
    let mut lines = vec![styled(format!("{name}..."), "#5391E1", false)];
    let (lo, hi) = window(options.len(), state);
    if lo > 0 {
        lines.push(styled("> ...", "gray", false));
    }
    for (i, a) in options.iter().enumerate().take(hi).skip(lo) {
        let text = format(host, a, false);
        lines.push(if i == state {
            styled(format!("-> {text}"), "white", false)
        } else {
            styled(format!("> {text}"), "gray", false)
        });
    }
    if hi < options.len() {
        lines.push(styled("> ...", "gray", false));
    }
    lines.push(styled(
        tr(host, "create.gui.scrollInput.scrollToSelect"),
        "dark_gray",
        true,
    ));
    lines
}

/// The options a `SelectionScrollInput`'s tooltip lists around `state` (of `n`).
fn window(n: usize, state: usize) -> (usize, usize) {
    let (n, state) = (n as i64, state as i64);
    let mut lo = (n - 16).min(state - 7).max(0);
    let mut hi = 16.max(state + 8).min(n);
    if lo == 1 {
        lo = 0;
    }
    if hi == n - 1 {
        hi = n;
    }
    (lo as usize, hi as usize)
}

/// What a click at `mouse` with `button` (`shift` held) does to the screen; true when it
/// closes it (confirm).
fn click(host: &dyn ClientHost, o: &mut Open, mouse: [f32; 2], button: i32, shift: bool) -> bool {
    let l = layout(o.attribute);
    let c = l.corner(o.size);
    if button == 0 {
        for (b, r) in buttons(o, c) {
            if !inside(r, mouse) || !active(o, b) {
                continue;
            }
            click_sound(host);
            match b {
                Button::Deny => o.deny = true,
                Button::Allow => o.deny = false,
                Button::Respect => o.respect = true,
                Button::Ignore => o.respect = false,
                Button::Any => o.mode = 0,
                Button::All => o.mode = 1,
                Button::None => o.mode = 2,
                Button::Add | Button::AddInverted => {
                    if let Some(a) = o.options().get(o.option).cloned() {
                        o.chosen.push((a, b == Button::AddInverted));
                    }
                }
                // `clearContents`: a list's slots, an attribute filter's attributes.
                Button::Reset => {
                    if o.attribute {
                        o.chosen.clear();
                    } else {
                        o.list.iter_mut().for_each(|s| *s = None);
                    }
                }
                Button::Confirm => return true,
            }
            return false;
        }
    }
    if button > 1 {
        return false;
    }
    // The player's inventory.
    for slot in 0..36u8 {
        if !over_slot(l.inventory_slot(c, slot), mouse) {
            continue;
        }
        let stack = o.stack(slot).cloned();
        if shift {
            // `quickMoveStack`: a copy into the list, or the reference item.
            if let Some(s) = stack
                && slot != o.selected
            {
                if o.attribute {
                    set_reference(o, Some(slot));
                } else {
                    let mut list: Vec<Option<(Ghost, ItemStack)>> = o.list.clone();
                    quick_move(&mut list, &(Ghost::Inventory(slot), s), |a, b| {
                        same_stack(&a.1, &b.1)
                    });
                    o.list = list;
                }
            }
        } else {
            o.cursor = click_inventory(o.cursor, slot, stack.is_some(), o.selected);
        }
        return false;
    }
    let carried = o
        .cursor
        .and_then(|slot| Some((Ghost::Inventory(slot), o.stack(slot)?.clone())));
    if o.attribute {
        let reference = [c[0] + REFERENCE_SLOT[0], c[1] + REFERENCE_SLOT[1]];
        if over_slot(reference, mouse) {
            // A shift-click takes the item out (`extractItem`), as a click with nothing does.
            let to = if shift { None } else { o.cursor };
            set_reference(o, to);
        }
        return false;
    }
    for i in 0..o.list.len() {
        if over_slot(list_slot(c, i), mouse) {
            let carried = if shift { None } else { carried.as_ref() };
            click_ghost(&mut o.list, i, carried);
            return false;
        }
    }
    // Outside the window: what is carried drops (the ghost goes).
    let w = l.window();
    if !inside([c[0], c[1], w[0], w[1]], mouse) {
        o.cursor = None;
    }
    false
}

/// `referenceItemChanged`: the selector starts again at its first option.
fn set_reference(o: &mut Open, to: Option<u8>) {
    o.reference = to;
    o.option = 0;
}

/// `ItemStack.isSameItemSameComponents`.
fn same_stack(a: &ItemStack, b: &ItemStack) -> bool {
    a.item() == b.item() && super::filters::components(a) == super::filters::components(b)
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
        o.as_mut().is_some_and(|o| {
            o.pressed = button == 0;
            click(host, o, mouse, button, shift)
        })
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| {
            l.0.push(open.chosen());
            l.1 = true;
        });
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
        }
    });
    Flow::Cancel
}

/// The attribute selector (`ScrollInput.mouseScrolled`, inverted: the wheel up is the option
/// before).
pub(crate) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let up = event.scroll[1] > 0.0;
    let stepped = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        let c = layout(o.attribute).corner(o.size);
        let r = [
            c[0] + SELECTOR[0],
            c[1] + SELECTOR[1],
            SELECTOR[2],
            SELECTOR[3],
        ];
        let n = o.options().len();
        if !o.attribute || o.reference.is_none() || n == 0 || !inside(r, mouse) {
            return None;
        }
        let before = o.option;
        o.option = if up {
            o.option.saturating_sub(1)
        } else {
            (o.option + 1).min(n - 1)
        };
        (o.option != before).then_some((o.option, n))
    });
    if let Some((state, n)) = stepped {
        let pitch = 1.5 + 0.1 * state as f32 / n as f32;
        host.play_local_sound(&SoundPlay::ui("create:scroll_value").with_pitch(pitch), 0);
    }
    Flow::Cancel
}

/// Keys: Escape closes (the game's own); the inventory key closes too, as a container's does.
pub(crate) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.key == "key.keyboard.e"
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        LATER.with_borrow_mut(|l| {
            l.0.push(open.chosen());
            l.1 = true;
        });
        return Flow::Cancel;
    }
    Flow::Continue
}

/// `removed` / `saveData`: what was chosen goes to the server at the next tick.
pub(crate) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(open.chosen()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: &str, value: Option<&str>) -> AttrLine {
        AttrLine {
            kind: kind.into(),
            value: value.map(Into::into),
            key: kind.trim_start_matches("create:").into(),
            arg: value.map(|v| format!("#{v}")),
            arg_key: false,
        }
    }

    #[test]
    fn the_screen_crosses_the_wire() {
        let o = Opened {
            attribute: true,
            selected: 3,
            inventory: vec![
                (0, vec![1, 2, 3], vec![line("create:placeable", None)]),
                (
                    35,
                    vec![9; 300],
                    vec![
                        line("create:added_by", Some("minecraft")),
                        AttrLine {
                            kind: "create:has_enchant".into(),
                            value: Some("minecraft:sharpness".into()),
                            key: "has_enchant".into(),
                            arg: Some("enchantment.minecraft.sharpness".into()),
                            arg_key: true,
                        },
                    ],
                ),
            ],
            list: vec![(17, vec![4, 5])],
            deny: true,
            respect: false,
            mode: 2,
            chosen: vec![(line("create:renamed", None), true)],
        };
        assert_eq!(Opened::decode(&o.encode()), Some(o));
        let c = Chosen {
            attribute: false,
            list: vec![
                None,
                Some(Ghost::Inventory(0)),
                Some(Ghost::Inventory(35)),
                Some(Ghost::Own(17)),
            ],
            deny: false,
            respect: true,
            mode: 1,
            attributes: vec![("create:book_copy".into(), Some("2".into()), false)],
        };
        assert_eq!(Chosen::decode(&c.encode()), Some(c));
        assert_eq!(Chosen::decode(&[1, 2]), None);
    }

    #[test]
    fn a_ghost_slot_takes_a_copy_or_empties() {
        let mut slots = vec![Some('a'), None];
        click_ghost(&mut slots, 1, Some(&'b'));
        assert_eq!(slots, vec![Some('a'), Some('b')]);
        click_ghost(&mut slots, 0, None);
        assert_eq!(slots, vec![None, Some('b')]);
    }

    #[test]
    fn a_shift_click_fills_the_first_empty_slot_unless_its_like_comes_first() {
        let mut slots = vec![Some('a'), None, Some('b'), None];
        quick_move(&mut slots, &'c', |a, b| a == b);
        assert_eq!(slots, vec![Some('a'), Some('c'), Some('b'), None]);
        // `a` is already there before an empty slot: nothing moves.
        quick_move(&mut slots, &'a', |a, b| a == b);
        assert_eq!(slots, vec![Some('a'), Some('c'), Some('b'), None]);
        // `b` is met before the remaining empty slot: nothing moves.
        quick_move(&mut slots, &'b', |a, b| a == b);
        assert_eq!(slots, vec![Some('a'), Some('c'), Some('b'), None]);
        // One like it after the first empty slot doesn't stop it (Create breaks only on a
        // match met first).
        let mut slots = vec![None, Some('d')];
        quick_move(&mut slots, &'d', |a, b| a == b);
        assert_eq!(slots, vec![Some('d'), Some('d')]);
    }

    #[test]
    fn the_cursor_picks_puts_down_and_never_takes_the_filter() {
        // The filter is in hotbar slot 2.
        assert_eq!(click_inventory(None, 5, true, 2), Some(5));
        assert_eq!(click_inventory(None, 2, true, 2), None);
        assert_eq!(click_inventory(Some(5), 2, true, 2), Some(5));
        assert_eq!(click_inventory(Some(5), 5, true, 2), None);
        assert_eq!(click_inventory(Some(5), 7, false, 2), None);
        assert_eq!(click_inventory(Some(5), 7, true, 2), Some(7));
        assert_eq!(click_inventory(None, 7, false, 2), None);
    }

    #[test]
    fn the_selectors_tooltip_lists_around_the_choice() {
        assert_eq!(window(5, 0), (0, 5));
        assert_eq!(window(40, 0), (0, 16));
        assert_eq!(window(40, 20), (13, 28));
        assert_eq!(window(40, 39), (24, 40));
    }

    #[test]
    fn the_windows_sit_as_creates() {
        // A 400 x 300 GUI: the list filter's window is 214 x 211, moved by (-11, 5).
        let c = LIST.corner([400.0, 300.0]);
        assert_eq!(c, [82.0, 49.0]);
        assert_eq!(LIST.inventory_panel(c), [82.0 + 11.0 + 19.0, 49.0 + 103.0]);
        assert_eq!(LIST.inventory_slot(c, 9), [82.0 + 38.0, 49.0 + 121.0]);
        assert_eq!(LIST.inventory_slot(c, 0), [82.0 + 38.0, 49.0 + 179.0]);
        assert_eq!(
            LIST.inventory_slot(c, 35),
            [82.0 + 38.0 + 144.0, 49.0 + 157.0]
        );
        assert_eq!(list_slot(c, 10), [82.0 + 41.0, 49.0 + 43.0]);
        assert_eq!(LIST.confirm(c), [82.0 + 181.0, 49.0 + 75.0, 18.0, 18.0]);
        // The attribute filter's: 241 x 197, moved by (-11, 7).
        let c = ATTRIBUTE.corner([400.0, 300.0]);
        assert_eq!(c, [68.0, 58.0]);
        assert_eq!(
            ATTRIBUTE.inventory_panel(c),
            [68.0 + 11.0 + 32.0, 58.0 + 89.0]
        );
        assert_eq!(ATTRIBUTE.reset(c), [68.0 + 179.0, 58.0 + 61.0, 18.0, 18.0]);
    }
}
