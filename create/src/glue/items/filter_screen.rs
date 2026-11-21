//! The filters' screens (`FilterScreen`, `AttributeFilterScreen` over `AbstractFilterScreen`) on
//! their menus (`create:filter`, `create:attribute_filter`; the server's side is `filters`):
//! Create's panel, the deny / allow and data buttons or the three attribute modes, the attribute
//! selector, reset and confirm, the player's inventory under the panel and the held filter four
//! times its size beside it. The menu's slots are the game's: the ghost slots and the player's
//! inventory, clicked on the server.
//!
//! The menu opens with the filter's settings as its extra data; the buttons send
//! `FilterScreenPacket`'s options and `ClearMenuPacket` at the next tick, and the server sends
//! the reference item's attributes, read where the recipes are, when it changes.

use super::super::net;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Host, ItemStack, MenuPart, Screen, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, SoundPlay, Tag, Text,
};
use std::cell::RefCell;

/// Client to server: the buttons; server to client: the reference item's attributes.
pub(crate) const CHANNEL: &str = "create:filter_screen";
/// `AllMenuTypes.FILTER`, `ATTRIBUTE_FILTER`.
pub(crate) const FILTER_MENU: &str = "create:filter";
pub(crate) const ATTRIBUTE_MENU: &str = "create:attribute_filter";

/// A list filter's ghost slots.
pub(crate) const LIST_SLOTS: usize = 18;

/// `FilterScreenPacket.Option`'s ordinals.
pub(crate) const WHITELIST: u8 = 0;
pub(crate) const WHITELIST2: u8 = 1;
pub(crate) const BLACKLIST: u8 = 2;
pub(crate) const RESPECT_DATA: u8 = 3;
pub(crate) const IGNORE_DATA: u8 = 4;
const ADD_TAG: u8 = 6;
const ADD_INVERTED_TAG: u8 = 7;
/// `ClearMenuPacket`, past the options.
const CLEAR: u8 = 255;

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

/// The menu's extra data: the filter's settings (`FilterMenu.initAndReadInventory`,
/// `AttributeFilterMenu`'s).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Opened {
    pub deny: bool,
    pub respect: bool,
    /// The attribute mode (`AttributeFilterWhitelistMode`'s ordinal) and the attributes chosen.
    pub mode: u8,
    pub chosen: Vec<(AttrLine, bool)>,
}

/// `FilterScreenPacket` (an option, or an attribute added) and `ClearMenuPacket`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToServer {
    Option(u8),
    Add {
        inverted: bool,
        kind: String,
        value: Option<String>,
    },
    Clear,
}

/// The reference item's attributes (`ItemAttributeType.getAllAttributes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToClient {
    Options(Vec<AttrLine>),
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
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
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
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
        let deny = r.bool()?;
        let respect = r.bool()?;
        let mode = r.u8()?;
        let n = r.u8()?;
        let chosen = (0..n)
            .map(|_| Some((r.line()?, r.bool()?)))
            .collect::<Option<Vec<_>>>()?;
        Some(Opened {
            deny,
            respect,
            mode,
            chosen,
        })
    }
}

impl ToServer {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        match self {
            ToServer::Option(o) => w.u8(*o),
            ToServer::Add {
                inverted,
                kind,
                value,
            } => {
                w.u8(if *inverted { ADD_INVERTED_TAG } else { ADD_TAG });
                w.str(kind);
                w.opt(value.as_deref());
            }
            ToServer::Clear => w.u8(CLEAR),
        }
        w.0
    }

    pub(crate) fn decode(data: &[u8]) -> Option<ToServer> {
        let mut r = Reader(data);
        Some(match r.u8()? {
            CLEAR => ToServer::Clear,
            o @ (ADD_TAG | ADD_INVERTED_TAG) => ToServer::Add {
                inverted: o == ADD_INVERTED_TAG,
                kind: r.str()?,
                value: r.opt()?,
            },
            o => ToServer::Option(o),
        })
    }
}

impl ToClient {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        let ToClient::Options(lines) = self;
        w.u16(lines.len().min(usize::from(u16::MAX)) as u16);
        for l in lines.iter().take(usize::from(u16::MAX)) {
            w.line(l);
        }
        w.0
    }

    pub(crate) fn decode(data: &[u8]) -> Option<ToClient> {
        let mut r = Reader(data);
        let n = r.u16()?;
        let lines = (0..n).map(|_| r.line()).collect::<Option<Vec<_>>>()?;
        Some(ToClient::Options(lines))
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

/// The open screen's own state: the settings shown and the selector.
struct Open {
    attribute: bool,
    deny: bool,
    respect: bool,
    mode: u8,
    chosen: Vec<(AttrLine, bool)>,
    /// The reference item's attributes, as the server last sent them.
    options: Vec<AttrLine>,
    /// The selector's option.
    option: usize,
    /// Whether the reference slot holds an item.
    reference: bool,
    pressed: bool,
}

impl Open {
    fn options(&self) -> &[AttrLine] {
        if self.reference { &self.options } else { &[] }
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
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// What the screen hooks leave for the next tick: packets to send, and whether to close.
    static LATER: RefCell<(Vec<ToServer>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(crate) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn attribute_menu(menu: &str) -> Option<bool> {
    match menu {
        FILTER_MENU => Some(false),
        ATTRIBUTE_MENU => Some(true),
        _ => None,
    }
}

/// Runs `f` on the open screen's state, read from the menu's extra data when it opened.
fn with_open<R>(host: &dyn ClientHost, attribute: bool, f: impl FnOnce(&mut Open) -> R) -> R {
    OPEN.with_borrow_mut(|o| {
        let open = o.get_or_insert_with(|| {
            let opened = host
                .screen_extra_data()
                .and_then(|d| Opened::decode(&d))
                .unwrap_or_default();
            Open {
                attribute,
                deny: opened.deny,
                respect: opened.respect,
                mode: opened.mode,
                chosen: opened.chosen,
                options: Vec::new(),
                option: 0,
                reference: false,
                pressed: false,
            }
        });
        f(open)
    })
}

fn ours(menu: Option<&str>) -> Option<bool> {
    attribute_menu(menu?)
}

fn later(message: ToServer) {
    LATER.with_borrow_mut(|l| l.0.push(message));
}

fn main_hand(host: &dyn ClientHost) -> Option<ItemStack> {
    let player = host.local_player()?;
    host.living(player.entity())?.hand_item(host, Hand::Main)
}

/// A menu slot of the open menu, as the client has it.
fn menu_slot(host: &dyn ClientHost, slot: usize) -> Option<ItemStack> {
    let menu = host.local_player()?.container_menu(host)?;
    menu.slot(host, slot).flatten()
}

/// The reference item's attributes arrived.
pub(crate) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    if let Some(ToClient::Options(lines)) = ToClient::decode(data) {
        with_open(&*host, true, |o| {
            o.options = lines;
            o.option = 0;
        });
    }
    true
}

/// The buttons' packets, then the close they asked for.
pub(crate) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    for m in send {
        net::send_to_server(host, CHANNEL, &m.encode());
    }
    if close && let Some(menu) = host.local_player().and_then(|p| p.container_menu(&*host)) {
        menu.close(host);
    }
}

/// The filters' screens on their menus.
pub(crate) struct FilterScreen;

#[mod_api::sunrise_part]
impl MenuPart for FilterScreen {
    /// `AbstractFilterScreen.init`: the window (`setWindowSize`, moved by `setWindowOffset`,
    /// which a centred screen gets by growing twice the offset), the ghost slots and the
    /// player's inventory where the menu puts them.
    fn screen(&self, _host: &dyn Host, event: &mut mod_api::ScreenEvent<'_>) -> Option<Screen> {
        let attribute = attribute_menu(event.menu)?;
        OPEN.with_borrow_mut(|o| *o = None);
        let l = layout(attribute);
        let w = l.window();
        let slots: Vec<[f32; 2]> = if attribute {
            vec![REFERENCE_SLOT, SELECTED_SLOT]
        } else {
            (0..LIST_SLOTS).map(|i| list_slot([0.0, 0.0], i)).collect()
        };
        Some(Screen {
            width: w[0] - 2.0 * l.offset[0],
            height: w[1] - 2.0 * l.offset[1],
            slots: slots
                .into_iter()
                .map(|position| {
                    Some(mod_api::ScreenSlot {
                        position,
                        no_item_icon: None,
                    })
                })
                .collect(),
            inventory_x: l.inventory[0],
            inventory_y: l.inventory[1],
            own_labels: true,
            own_background: true,
            ..Default::default()
        })
    }

    fn screen_draw(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenDrawEvent<'_>) {
        let Some(attribute) = attribute_menu(event.menu) else {
            return;
        };
        let mouse = event.pointer.map(|p| p as f32);
        let origin = event.origin;
        let reference = attribute && menu_slot(host, 0).is_some();
        with_open(host, attribute, |o| {
            if o.reference != reference {
                o.reference = reference;
                o.option = 0;
            }
            draw(host, event.draw, o, origin, mouse);
        });
    }

    /// The buttons (`IconButton.onClick`): an active one takes the press.
    fn screen_click(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenClickEvent<'_>,
    ) -> Option<i32> {
        let attribute = attribute_menu(event.menu)?;
        if event.right {
            return None;
        }
        let at = event.at;
        with_open(host, attribute, |o| {
            let (b, _) = buttons(o, [0.0, 0.0])
                .into_iter()
                .find(|(b, r)| inside(*r, at) && active(o, *b))?;
            click_sound(host);
            press(o, b);
            Some(0)
        })
    }

    /// The buttons' tooltips (`handleTooltips`), the selector's, and the selected attributes
    /// over their slot (`AttributeFilterScreen.renderTooltip`).
    fn screen_tooltip(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenTooltipEvent<'_>) {
        let Some(attribute) = attribute_menu(event.menu) else {
            return;
        };
        let at = event.at;
        let lines = with_open(host, attribute, |o| {
            if attribute && event.slot == Some(1) && !event.lines.is_empty() {
                return Some(selected_attributes(host, o));
            }
            if event.slot.is_some() {
                return None;
            }
            button_tip(host, o, at).or_else(|| {
                let r = SELECTOR;
                let reference = menu_slot(host, 0)?;
                (attribute && inside(r, at)).then(|| {
                    selector_tip(
                        host,
                        host.hover_name(&reference).unwrap_or_default(),
                        o.options(),
                        o.option,
                    )
                })
            })
        });
        if let Some(lines) = lines {
            *event.lines = lines.into_iter().map(|t| (t, 0xFFFF_FFFF)).collect();
        }
    }

    /// The ghost slots take nothing from the cursor and give nothing up: the server's clicks
    /// copy into them, and the client's own prediction leaves them alone.
    fn menu_slot_rules(&self, _host: &dyn Host, event: &mut mod_api::MenuSlotRulesEvent<'_>) {
        if attribute_menu(event.menu).is_some() && event.container == mod_api::MenuContainer::Main {
            event.may_place = false;
            event.may_pickup = false;
        }
    }
}

/// What a button does on the client (`IconButton`'s callbacks).
fn press(o: &mut Open, b: Button) {
    match b {
        Button::Deny => {
            o.deny = true;
            later(ToServer::Option(BLACKLIST));
        }
        Button::Allow => {
            o.deny = false;
            later(ToServer::Option(WHITELIST));
        }
        Button::Respect => {
            o.respect = true;
            later(ToServer::Option(RESPECT_DATA));
        }
        Button::Ignore => {
            o.respect = false;
            later(ToServer::Option(IGNORE_DATA));
        }
        Button::Any => {
            o.mode = 0;
            later(ToServer::Option(WHITELIST));
        }
        Button::All => {
            o.mode = 1;
            later(ToServer::Option(WHITELIST2));
        }
        Button::None => {
            o.mode = 2;
            later(ToServer::Option(BLACKLIST));
        }
        Button::Add | Button::AddInverted => {
            if let Some(a) = o.options().get(o.option).cloned() {
                let inverted = b == Button::AddInverted;
                later(ToServer::Add {
                    inverted,
                    kind: a.kind.clone(),
                    value: a.value.clone(),
                });
                o.chosen.push((a, inverted));
            }
        }
        // `clearContents` and `ClearMenuPacket`: a list's slots, an attribute filter's
        // attributes.
        Button::Reset => {
            if o.attribute {
                o.chosen.clear();
            }
            later(ToServer::Clear);
        }
        Button::Confirm => LATER.with_borrow_mut(|l| l.1 = true),
    }
}

/// `selectedAttributes`: the attributes chosen, under a heading.
fn selected_attributes(host: &dyn ClientHost, o: &Open) -> Vec<Text> {
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
    lines
}

/// A button's name and, held with shift, its description.
fn button_tip(host: &dyn ClientHost, o: &Open, at: [f32; 2]) -> Option<Vec<Text>> {
    let (b, _) = buttons(o, [0.0, 0.0])
        .into_iter()
        .find(|(_, r)| inside(*r, at))?;
    let (name, desc) = tip(b)?;
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
        if host.modifiers().shift() {
            let desc = tr(host, desc);
            for line in cut(
                |t| host.text_width(&Text::literal(t)).map_or(0.0, |w| w as f32),
                &desc,
            ) {
                lines.push(styled(line, "gray", false));
            }
        }
    }
    Some(lines)
}

/// `AttributeFilterScreen.renderForeground`: the number of attributes chosen over their slot.
pub(crate) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if ours(event.screen.menu) != Some(true) {
        return;
    }
    let Some(menu) = host.local_player().and_then(|p| p.container_menu(host)) else {
        return;
    };
    let Some(Some([x, y, _, _])) = menu.slot_boxes(host).and_then(|b| b.get(1).copied()) else {
        return;
    };
    let Some(stack) = menu.slot(host, 1).flatten() else {
        return;
    };
    let count = OPEN.with_borrow(|o| o.as_ref().map_or(0, |o| o.chosen.len()));
    event
        .draw
        .gui_draw_item_with(&stack, [x, y], Some(&count.to_string()));
}

pub(crate) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if let Some(attribute) = ours(event.screen.menu) {
        with_open(host, attribute, |o| o.pressed = event.button == 0);
    }
    Flow::Continue
}

pub(crate) fn screen_mouse_released(
    _host: &dyn ClientHost,
    event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if ours(event.screen.menu).is_some() {
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                o.pressed = false;
            }
        });
    }
    Flow::Continue
}

/// The attribute selector (`ScrollInput.mouseScrolled`, inverted: the wheel up is the option
/// before).
pub(crate) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if ours(event.screen.menu) != Some(true) {
        return Flow::Continue;
    }
    let c = [event.screen.rect[0], event.screen.rect[1]];
    let r = [
        c[0] + SELECTOR[0],
        c[1] + SELECTOR[1],
        SELECTOR[2],
        SELECTOR[3],
    ];
    if !inside(r, event.screen.mouse) {
        return Flow::Continue;
    }
    let up = event.scroll[1] > 0.0;
    let stepped = with_open(host, true, |o| {
        let n = o.options().len();
        if n == 0 {
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

/// Packets still waiting go before the close: Escape and the inventory key wait for the tick.
pub(crate) fn screen_key_pressed(
    _host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if ours(event.screen.menu).is_none()
        || !matches!(event.key, "key.keyboard.escape" | "key.keyboard.e")
    {
        return Flow::Continue;
    }
    LATER.with_borrow_mut(|l| {
        if l.0.is_empty() {
            Flow::Continue
        } else {
            l.1 = true;
            Flow::Cancel
        }
    })
}

pub(crate) fn screen_closing(_host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if ours(event.screen.menu).is_some() {
        OPEN.with_borrow_mut(|o| *o = None);
    }
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
    #[cfg(test)]
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
        Button::Add | Button::AddInverted => o.reference && o.may_add(),
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

/// `TooltipHelper.cutTextComponent`: words wrapped at 200 pixels.
fn cut(width: impl Fn(&str) -> f32, text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_inclusive(' ') {
        if !line.is_empty() && width(&format!("{line}{word}")) > 200.0 {
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

/// `AbstractFilterScreen.renderBg` and the widgets, under the slots.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, c: [f32; 2], mouse: [f32; 2]) {
    let l = layout(o.attribute);
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
    if o.attribute {
        // The selector's label (`attributeSelectorLabel`).
        let label_at = [c[0] + 43.0, c[1] + 31.0];
        if !o.reference {
            draw.text(
                &tr(host, "create.gui.attribute_filter.add_reference_item"),
                label_at,
                0xFFF3_EBDE,
                true,
            );
        } else if let Some(a) = o.options().get(o.option) {
            let text = trim(draw, &format(host, a, false), 112.0);
            draw.text(&text, label_at, 0xFFF3_EBDE, true);
        }
    }
}

/// `SelectionScrollInput.updateTooltip`.
fn selector_tip(
    host: &dyn ClientHost,
    name: Text,
    options: &[AttrLine],
    state: usize,
) -> Vec<Text> {
    let mut lines = vec![
        styled("", "#5391E1", false)
            .append(name)
            .append(Text::literal("...")),
    ];
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
    fn the_menus_messages_cross_the_wire() {
        let o = Opened {
            deny: true,
            respect: false,
            mode: 2,
            chosen: vec![(line("create:renamed", None), true)],
        };
        assert_eq!(Opened::decode(&o.encode()), Some(o));
        for m in [
            ToServer::Option(RESPECT_DATA),
            ToServer::Add {
                inverted: true,
                kind: "create:in_tag".into(),
                value: Some("minecraft:logs".into()),
            },
            ToServer::Clear,
        ] {
            assert_eq!(ToServer::decode(&m.encode()), Some(m));
        }
        let lines = ToClient::Options(vec![
            line("create:added_by", Some("minecraft")),
            AttrLine {
                kind: "create:has_enchant".into(),
                value: Some("minecraft:sharpness".into()),
                key: "has_enchant".into(),
                arg: Some("enchantment.minecraft.sharpness".into()),
                arg_key: true,
            },
        ]);
        assert_eq!(ToClient::decode(&lines.encode()), Some(lines));
        assert_eq!(ToClient::decode(&[1]), None);
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
    fn the_cursor_picks_puts_down_and_never_takes_the_filter() {
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
        let w = LIST.window();
        let grown = [w[0] - 2.0 * LIST.offset[0], w[1] - 2.0 * LIST.offset[1]];
        assert_eq!(
            [
                ((400.0 - grown[0]) / 2.0_f32).floor(),
                ((300.0 - grown[1]) / 2.0_f32).floor()
            ],
            c
        );
        assert_eq!(LIST.inventory_panel(c), [82.0 + 11.0 + 19.0, 49.0 + 103.0]);
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
