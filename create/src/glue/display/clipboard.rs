//! The clipboard's pages (`ClipboardBlockItem`, `ClipboardEditPacket`, `ClipboardScreen`): the
//! server opens the screen for the clipboard in hand (`create:clipboard` messages) and writes
//! back what the client sends when it closes; the client draws it as the mod's own screen, the
//! engine's `clipboard::Editor` editing it. The pages are kept in `minecraft:custom_data` under
//! Create's component name.
//!
//! The screen hooks are given a read-only client: what they leave (closing the screen from its
//! close button, the pages to send when it closes) is done right after the key or click that
//! asked for it (`Subsystem::input_handled`), or at the next client tick.
//!
//! A placed clipboard (`ClipboardBlock`) is the decor port's block: it opens the screen with
//! [`open`] and writes back the pages a client sends for its position.

use crate::glue::net;
use create_engine::clipboard::{self as cb, Content, Editor, Entry, Key, Kind};
use mod_api::{
    ClientHost, Compound, DrawContext, Flow, HostSide, InteractionResult, ItemPart, ItemStack,
    PlayerRef, Registrar, RegistrationContext, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Tag, Text,
    UseItemEvent, UseOnEvent,
};
use std::cell::RefCell;

const CUSTOM_DATA: &str = "minecraft:custom_data";
const CONTENT_KEY: &str = "create:clipboard_content";
/// The mod whose own screen the clipboard is.
const MOD: &str = "create";

pub(super) fn register(r: &mut dyn Registrar, _cx: &RegistrationContext) {
    let key = r.item_key("create:clipboard");
    r.item_part(key, ClipboardItem);
}

// ---------------------------------------------------------------------------------------------
// The pages on a stack

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.components()?.as_nbt().get(&CUSTOM_DATA.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// `ClipboardContent.CODEC` out of the stack's custom data (`EMPTY` without one).
pub fn content(stack: &ItemStack) -> Content {
    let Some(Tag::Compound(c)) = custom(stack).and_then(|c| c.get(&CONTENT_KEY.into())) else {
        return Content::default();
    };
    let text = |t: Option<&Tag>| match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let int = |t: Option<&Tag>| match t {
        Some(Tag::Int(i)) => *i,
        _ => 0,
    };
    let flag = |t: Option<&Tag>| matches!(t, Some(Tag::Byte(b)) if *b != 0);
    let mut pages = Vec::new();
    if let Some(Tag::List(list)) = c.get(&"pages".into()) {
        for page in list {
            let Tag::List(entries) = page else { continue };
            pages.push(
                entries
                    .iter()
                    .filter_map(|e| match e {
                        Tag::Compound(e) => Some(Entry {
                            checked: flag(e.get(&"checked".into())),
                            text: text(e.get(&"text".into())),
                            icon: match e.get(&"icon".into()) {
                                Some(Tag::String(item)) => Some((
                                    item.to_string_lossy(),
                                    int(e.get(&"item_amount".into())),
                                )),
                                _ => None,
                            },
                        }),
                        _ => None,
                    })
                    .collect(),
            );
        }
    }
    Content {
        kind: Kind::from_name(&text(c.get(&"type".into()))),
        pages,
        read_only: flag(c.get(&"read_only".into())),
        previously_opened_page: int(c.get(&"previously_opened_page".into())),
    }
}

/// `ClipboardContent.copiedValues`: the block settings the clipboard copied.
pub(crate) fn copied_values(stack: &ItemStack) -> Option<Compound> {
    let Some(Tag::Compound(c)) = custom(stack).and_then(|c| c.get(&CONTENT_KEY.into())) else {
        return None;
    };
    match c.get(&"copied_values".into()) {
        Some(Tag::Compound(v)) => Some(v.clone()),
        _ => None,
    }
}

/// The clipboard with `values` copied (`setCopiedValues`), written on (`ClipboardType.WRITTEN`).
pub(crate) fn with_copied_values(
    _host: &dyn mod_api::Host,
    stack: &ItemStack,
    values: Compound,
) -> ItemStack {
    let mut content = content(stack);
    content.kind = Kind::Written;
    let mut out = with_content(stack, Some(&content));
    let mut patch = out
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    let mut c = custom(&out).cloned().unwrap_or_default();
    if let Some(Tag::Compound(mut inner)) = c.remove(&CONTENT_KEY.into()) {
        inner.insert("copied_values".into(), Tag::Compound(values));
        c.insert(CONTENT_KEY.into(), Tag::Compound(inner));
    }
    patch.insert(CUSTOM_DATA.into(), Tag::Compound(c));
    let _ = out.set_components(patch);
    out
}

/// The stack with `content` (none takes the component off).
pub fn with_content(stack: &ItemStack, content: Option<&Content>) -> ItemStack {
    let mut stack = stack.clone();
    let mut patch = stack
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    let mut c = custom(&stack).cloned().unwrap_or_default();
    // The settings it copied stay with its pages.
    let copied = copied_values(&stack);
    match content {
        Some(content) => {
            let mut out = Compound::new();
            if let Some(v) = copied {
                out.insert("copied_values".into(), Tag::Compound(v));
            }
            out.insert("type".into(), Tag::String(content.kind.name().into()));
            let pages = content
                .pages
                .iter()
                .map(|page| {
                    Tag::List(
                        page.iter()
                            .map(|e| {
                                let mut t = Compound::new();
                                t.insert("checked".into(), Tag::Byte(i8::from(e.checked)));
                                t.insert("text".into(), Tag::String(e.text.as_str().into()));
                                if let Some((item, n)) = &e.icon {
                                    t.insert("icon".into(), Tag::String(item.as_str().into()));
                                    t.insert("item_amount".into(), Tag::Int(*n));
                                }
                                Tag::Compound(t)
                            })
                            .collect(),
                    )
                })
                .collect();
            out.insert("pages".into(), Tag::List(pages));
            out.insert("read_only".into(), Tag::Byte(i8::from(content.read_only)));
            out.insert(
                "previously_opened_page".into(),
                Tag::Int(content.previously_opened_page),
            );
            c.insert(CONTENT_KEY.into(), Tag::Compound(out));
        }
        None => {
            c.remove(&CONTENT_KEY.into());
        }
    }
    if c.is_empty() {
        patch.remove(&CUSTOM_DATA.into());
    } else {
        patch.insert(CUSTOM_DATA.into(), Tag::Compound(c));
    }
    if let Err(e) = stack.set_components(patch) {
        crate::create_log!("[create] the clipboard's pages could not be written: {e:?}");
    }
    stack
}

// ---------------------------------------------------------------------------------------------
// Server

/// Opens the screen on `player`'s client for `stack`: the clipboard in their hand (`pos` none)
/// or the one placed at `pos`.
pub fn open(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    pos: Option<[i32; 3]>,
    stack: &ItemStack,
) {
    let slot = player.selected_slot(&*host).unwrap_or(0) as u8;
    let message = cb::Open {
        slot,
        pos,
        content: content(stack),
    };
    net::send_to_player(host, player, cb::CHANNEL, &message.encode());
}

/// `ClipboardBlockItem.use` (in the air, or on a block when not sneaking, `useOn`): the screen
/// opens for the main hand's clipboard, which is marked as being edited.
fn use_clipboard(
    host: HostSide,
    uuid: u128,
    hand: mod_api::Hand,
    stack: &ItemStack,
) -> Option<InteractionResult> {
    if hand != mod_api::Hand::Main {
        return None;
    }
    let HostSide::Server(host) = host else {
        return Some(InteractionResult::Success);
    };
    let player = host.player(uuid)?;
    open(host, player, None, stack);
    let mut c = content(stack);
    c.kind = Kind::Editing;
    let edited = with_content(stack, Some(&c));
    if let Some(slot) = player.selected_slot(&*host)
        && let Err(e) = player.set_inventory_slot(host, slot as u32, Some(edited))
    {
        crate::create_log!("[create] the clipboard could not be marked as edited: {e:?}");
    }
    Some(InteractionResult::SuccessServer)
}

struct ClipboardItem;

#[mod_api::horizon_part]
impl ItemPart for ClipboardItem {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        use_clipboard(host, event.player, event.hand, event.stack)
    }

    /// `ClipboardBlockItem.useOn`: not sneaking, as `use`; sneaking places the clipboard.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let sneaking = match &host {
            HostSide::Server(h) => h
                .player(event.player)
                .and_then(|p| p.is_secondary_use_active(&**h)),
            HostSide::Client(h) => h
                .local_player()
                .and_then(|p| p.is_secondary_use_active(&**h)),
        };
        if sneaking != Some(false) {
            return None;
        }
        use_clipboard(host, event.player, event.hand, event.stack)
    }
}

/// `ClipboardEditPacket.handle`: the pages a client sent onto the clipboard in that hotbar slot
/// (a placed one's are the decor port's).
pub(super) fn client_message(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    channel: &str,
    data: &[u8],
) -> bool {
    if channel != cb::CHANNEL {
        return false;
    }
    let Some(edit) = cb::Edit::decode(data) else {
        return true;
    };
    // A placed clipboard's (`targetedBlock`): written onto the block, from within reach.
    if let Some(pos) = edit.pos {
        let near = player.entity().position(&*host).is_some_and(|at| {
            (0..3)
                .map(|i| (at[i] - (f64::from(pos[i]) + 0.5)).powi(2))
                .sum::<f64>()
                < 24.0 * 24.0
        });
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return true;
        };
        let Some(mut lv) = crate::glue::server::ServerLevel::new(host, &dimension) else {
            return true;
        };
        if !near || !lv.is_loaded(pos) {
            return true;
        }
        let Some(stack) = crate::glue::decor::placed_clipboard(lv.dim, pos)
            .or_else(|| crate::glue::decor::clipboard_stack(&*lv.host))
        else {
            return true;
        };
        let name = lv.block_state(pos).and_then(|s| lv.state_name(s));
        if name.as_deref() != Some("create:clipboard") {
            return true;
        }
        let edited = with_content(&stack, edit.content.as_ref());
        crate::glue::decor::set_placed_clipboard(&mut lv, pos, edited, edit.content.is_some());
        return true;
    }
    let slot = u32::from(edit.slot);
    if slot >= 9 {
        return true;
    }
    let stack = player
        .inventory_slots(&*host)
        .unwrap_or_default()
        .into_iter()
        .find(|(i, _)| *i == slot)
        .map(|(_, s)| s);
    if let Some(stack) = stack
        && stack.item().id(&*host) == "create:clipboard"
    {
        let edited = with_content(&stack, edit.content.as_ref());
        if let Err(e) = player.set_inventory_slot(host, slot, Some(edited)) {
            crate::create_log!("[create] the clipboard's pages could not be kept: {e:?}");
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Client

/// The screen open on this client: the editor, the clipboard it edits (hotbar slot, or block),
/// whether the left button is held over the text, and when it opened (for the cursor's blink).
struct Open {
    editor: Editor,
    slot: u8,
    pos: Option<[i32; 3]>,
    dragging: bool,
    opened: std::time::Instant,
    /// The screen's size in GUI pixels when last drawn, for clicks.
    size: [f32; 2],
}

/// What the client does at its next tick, the screen hooks having only a read-only client.
#[derive(Default)]
struct Later {
    close: bool,
    send: Vec<cb::Edit>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<Later> = RefCell::new(Later::default());
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = Later::default());
}

/// The server opened the screen: the editor on the pages it sent.
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != cb::CHANNEL {
        return false;
    }
    let Some(open) = cb::Open::decode(data) else {
        return true;
    };
    // `ScreenOpener.open`: the screen before closes first (and sends its pages).
    if let Some(old) = OPEN.with_borrow_mut(Option::take) {
        send_edit(host, old);
        host.mod_screen_close(MOD);
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            editor: Editor::open(open.content),
            slot: open.slot,
            pos: open.pos,
            dragging: false,
            opened: std::time::Instant::now(),
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

/// `removed`: the pages sent to the server (`ClipboardEditPacket`).
fn send_edit(host: &mut dyn ClientHost, open: Open) {
    let edit = cb::Edit {
        slot: open.slot,
        pos: open.pos,
        content: open.editor.close(),
    };
    net::send_to_server(host, cb::CHANNEL, &edit.encode());
}

/// The waiting close and pages, at the client's tick.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let later = LATER.with_borrow_mut(std::mem::take);
    if later.close {
        host.mod_screen_close(MOD);
    }
    for edit in later.send {
        net::send_to_server(host, cb::CHANNEL, &edit.encode());
    }
    // The screen went without a closing (another mod's replaced it): its pages go too.
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        send_edit(host, open);
    }
}

/// Whether the clipboard's screen is the one open.
fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// The width of character `c` in GUI pixels.
fn advance(host: &dyn ClientHost) -> impl Fn(char) -> f32 + '_ {
    move |c: char| {
        let mut b = [0u8; 4];
        host.text_width(&Text::literal(c.encode_utf8(&mut b) as &str))
            .unwrap_or(0) as f32
    }
}

/// The window's corner (`setWindowSize(256, 256)`: `guiLeft`, `guiTop`).
fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] as i32 - 256) / 2) as f32,
        ((size[1] as i32 - 256) / 2) as f32,
    )
}

/// The buttons: erase checked, close, forward, back, as `[x, y, w, h]` from the window.
fn buttons(left: f32, top: f32) -> [[f32; 4]; 4] {
    let (x, y) = (left, top - 8.0);
    [
        [x + 234.0, y + 153.0, 18.0, 18.0],
        [x + 234.0, y + 175.0, 18.0, 18.0],
        [x + 176.0, y + 229.0, 23.0, 13.0],
        [x + 53.0, y + 229.0, 23.0, 13.0],
    ]
}

fn inside(r: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// A UI sound (`SimpleSoundInstance.forUI`).
fn ui_sound(host: &dyn ClientHost, id: &str, volume: f32, pitch: f32) {
    host.play_local_sound(&SoundPlay::ui(id).with_volume(volume).with_pitch(pitch), 0);
}

/// `book.pageIndicator` in the client's language: `Page %1$s of %2$s`.
fn page_indicator(host: &dyn ClientHost, page: usize, pages: usize) -> String {
    host.translate("book.pageIndicator")
        .unwrap_or_else(|| "Page %1$s of %2$s".to_owned())
        .replace("%1$s", &page.to_string())
        .replace("%2$s", &pages.to_string())
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if ours(host) {
        draw(host, event.draw, event.screen.mouse);
    }
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.button != 0 {
        return Flow::Continue;
    }
    mouse(host, event.screen.mouse, true, event.modifiers & 1 != 0);
    Flow::Cancel
}

pub(super) fn screen_mouse_released(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.button != 0 {
        return Flow::Continue;
    }
    mouse(host, event.screen.mouse, false, false);
    Flow::Cancel
}

/// `mouseScrolled`: a page on, wheeling down.
pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let down = event.scroll[1] < 0.0;
    OPEN.with_borrow_mut(|o| {
        if let Some(open) = o.as_mut() {
            open.editor.change_page(down);
        }
    });
    Flow::Cancel
}

pub(super) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if key(host, event.key, event.modifiers) {
        Flow::Cancel
    } else {
        Flow::Continue
    }
}

/// `charTyped`: what `StringUtil.isAllowedChatCharacter` lets through, into the edited entry.
pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    let mut b = [0u8; 4];
    let typed: &str = c.encode_utf8(&mut b);
    OPEN.with_borrow_mut(|o| {
        if let Some(open) = o.as_mut() {
            open.editor.insert(typed, &advance(host));
        }
    });
    Flow::Cancel
}

/// `removed`: the pages go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        let edit = cb::Edit {
            slot: open.slot,
            pos: open.pos,
            content: open.editor.close(),
        };
        LATER.with_borrow_mut(|l| l.send.push(edit));
    }
}

fn text_argb(draw: &mut dyn DrawContext, s: &str, at: [f32; 2], argb: u32) {
    draw.text(s, at, argb, false);
}

/// `ClipboardScreen.renderWindow`: the clipboard, the page number, each entry's checkbox (an
/// address's tag) and text, the entry being edited with its selection and blinking cursor, and
/// the buttons.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, cursor: [f32; 2]) {
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        return;
    };
    let size = draw.size();
    open.size = size;
    let (left, top) = corner(size);
    let (x, mut y) = (left, top - 8.0);
    let adv = advance(host);
    if open.dragging && open.editor.editing.is_some() {
        let (text, _) = open.editor.display_text();
        let lines = cb::split(&text, cb::WRAP, &adv);
        let off = open.editor.editing_offset(&adv) as f32;
        let local = [cursor[0] - (left + 58.0), cursor[1] - (top + 42.0 + off)];
        let i = cb::index_at(&lines, &text, local[0], local[1], &adv);
        open.editor.drag(i);
    }
    draw.sprite(
        cb::TEXTURE,
        [0.0, 0.0, 256.0, 256.0],
        [x, y, 256.0, 256.0],
        None,
    );
    let indicator = page_indicator(host, open.editor.page + 1, open.editor.pages.len());
    text_argb(draw, &indicator, [x + 150.0, y + 9.0], 0x43FF_FFFF);
    let editing = open.editor.editing;
    let entries = open.editor.entries().to_vec();
    for (i, e) in entries.iter().enumerate() {
        let icon_offset = if e.icon.is_some() { 16.0 } else { 0.0 };
        let mut shown = e.text.clone();
        if e.is_address() {
            let sprite = if e.checked {
                cb::ADDRESS_INACTIVE
            } else {
                cb::ADDRESS
            };
            draw.sprite(cb::WIDGETS, sprite, [x + 44.0, y + 50.0, 8.0, 8.0], None);
            shown = shown
                .chars()
                .skip(1)
                .collect::<String>()
                .trim_start()
                .to_owned();
        } else {
            let box_color = if e.checked { 0x668D_7F6B } else { 0xFF8D_7F6B };
            text_argb(draw, "\u{25A1}", [x + 45.0, y + 51.0], box_color);
            if e.checked {
                text_argb(draw, "\u{2714}", [x + 45.0, y + 50.0], 0xFF31_B25D);
            }
        }
        let chars: Vec<char> = shown.chars().collect();
        let lines = cb::split(&chars, cb::WRAP - icon_offset, &adv);
        if lines.is_empty() {
            y += 12.0;
            continue;
        }
        if let Some((item, _)) = &e.icon
            && let Some(key) = mod_api::Identifier::parse(item)
                .ok()
                .and_then(|id| host.item_key(&id))
        {
            draw.item(key, [x + 54.0, y + 50.0], 16.0);
        }
        let color = match (e.checked, e.is_address()) {
            (true, true) => 0x668D_7F6B,
            (true, false) => 0xFF31_B25D,
            _ => 0xFF31_1A00,
        };
        for &(s, end) in &lines {
            if Some(i) != editing {
                let line: String = cb::stripped(&chars[s..end]).iter().collect();
                text_argb(draw, &line, [x + 58.0 + icon_offset, y + 50.0], color);
            }
            y += 9.0;
        }
        y += 3.0;
    }
    if editing.is_some() {
        draw_editing(host, &open, draw, left, top);
    }
    // IconButtons and the book's page buttons.
    let (forward, back) = open.editor.page_buttons();
    let [clear, close, next, prev] = buttons(left, top);
    for (r, icon) in [(clear, cb::ICON_CLEAR_CHECKED), (close, cb::ICON_CLOSE)] {
        let bg = if inside(r, cursor) {
            cb::BUTTON_HOVER
        } else {
            cb::BUTTON
        };
        draw.sprite(cb::WIDGETS, bg, r, None);
        draw.sprite(cb::ICONS, icon, [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0], None);
    }
    for (show, r, name) in [
        (forward, next, "page_forward"),
        (back, prev, "page_backward"),
    ] {
        if show {
            let sprite = if inside(r, cursor) {
                format!("gui/sprites/widget/{name}_highlighted")
            } else {
                format!("gui/sprites/widget/{name}")
            };
            draw.sprite(&sprite, [0.0, 0.0, r[2], r[3]], r, None);
        }
    }
    OPEN.with_borrow_mut(|o| *o = Some(open));
}

/// The edited entry as `rebuildDisplayCache` lays it out: its lines, the selection's boxes and
/// the cursor (a bar, or `_` at the end), blinking every 6 ticks.
fn draw_editing(
    host: &dyn ClientHost,
    open: &Open,
    draw: &mut dyn DrawContext,
    left: f32,
    top: f32,
) {
    let advance = advance(host);
    let e = &open.editor;
    let (text, offset) = e.display_text();
    let off = e.editing_offset(&advance) as f32;
    let (x0, y0) = (left + 58.0, top + 42.0 + off);
    let lines = cb::split(&text, cb::WRAP, &advance);
    let width = |s: &[char]| s.iter().map(|&c| advance(c)).sum::<f32>();
    for (k, &(s, end)) in lines.iter().enumerate() {
        let line: String = cb::stripped(&text[s..end]).iter().collect();
        text_argb(draw, &line, [x0, y0 + k as f32 * 9.0], 0xFF31_1A00);
    }
    let cursor = e.cursor.saturating_sub(offset).min(text.len());
    let selection = e.selection.saturating_sub(offset).min(text.len());
    if cursor != selection {
        let (lo, hi) = (cursor.min(selection), cursor.max(selection));
        let (a, b) = (cb::line_of(&lines, lo), cb::line_of(&lines, hi));
        for k in a..=b {
            let Some(&(s, end)) = lines.get(k) else {
                continue;
            };
            let from = if k == a { lo } else { s };
            let to = if k == b { hi } else { end };
            let x = x0 + width(&text[s..from.max(s)]);
            let w = width(&text[from.max(s)..to.min(end).max(from.max(s))]);
            // `renderHighlight`: inverted blue over the text.
            draw.fill([x, y0 + k as f32 * 9.0, w, 9.0], 0x8000_00FF);
        }
    }
    let ticks = (open.opened.elapsed().as_millis() / 50) as u64;
    if !(ticks / 6).is_multiple_of(2) {
        return;
    }
    let at_end = cursor == text.len();
    let (cx, cy) = if lines.is_empty() {
        (0.0, 0.0)
    } else if at_end && text.last() == Some(&'\n') {
        (0.0, lines.len() as f32 * 9.0)
    } else {
        let k = cb::line_of(&lines, cursor);
        (width(&text[lines[k].0..cursor]), k as f32 * 9.0)
    };
    if at_end {
        text_argb(draw, "_", [x0 + cx, y0 + cy], 0xFF00_0000);
    } else {
        draw.fill([x0 + cx, y0 + cy - 1.0, 1.0, 10.0], 0xFF00_0000);
    }
}

/// `keyPressed` while the screen is open: page keys and the edited entry's keys are taken;
/// Escape closes it (the game's, not taken). True when taken.
fn key(host: &dyn ClientHost, key: &str, modifiers: u32) -> bool {
    let ctrl = modifiers & 0x2 != 0;
    let shift = modifiers & 0x1 != 0;
    let alt = modifiers & 0x4 != 0;
    let edit = ctrl && !shift && !alt;
    let k = match key {
        "key.keyboard.page.up" => Key::PageUp,
        "key.keyboard.page.down" => Key::PageDown,
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => Key::Enter,
        "key.keyboard.backspace" => Key::Backspace,
        "key.keyboard.delete" => Key::Delete,
        "key.keyboard.left" => Key::Left,
        "key.keyboard.right" => Key::Right,
        "key.keyboard.up" => Key::Up,
        "key.keyboard.down" => Key::Down,
        "key.keyboard.home" => Key::Home,
        "key.keyboard.end" => Key::End,
        "key.keyboard.a" if edit => Key::SelectAll,
        "key.keyboard.c" if edit => Key::Copy,
        "key.keyboard.v" if edit => Key::Paste,
        "key.keyboard.x" if edit => Key::Cut,
        _ => return false,
    };
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        return false;
    };
    let mut clipboard = if k == Key::Paste {
        host.clipboard_text().unwrap_or_default()
    } else {
        String::new()
    };
    let used = {
        let advance = advance(host);
        let (text, _) = open.editor.display_text();
        let lines = cb::split(&text, cb::WRAP, &advance);
        open.editor
            .key(k, ctrl, shift, &mut clipboard, &lines, &advance)
    };
    if matches!(k, Key::Copy | Key::Cut) && used {
        host.set_clipboard_text(&clipboard);
    }
    let taken = used || open.editor.editing.is_some();
    OPEN.with_borrow_mut(|o| *o = Some(open));
    taken
}

/// `mouseClicked` and the release: the buttons, a checkbox (with its sound), an entry to edit,
/// the cursor in the edited text (a double click selects the word), or editing stopped by a
/// click away from the text.
fn mouse(host: &dyn ClientHost, at: [f32; 2], pressed: bool, shift: bool) {
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        return;
    };
    if !pressed {
        open.dragging = false;
        OPEN.with_borrow_mut(|o| *o = Some(open));
        return;
    }
    let (left, top) = corner(open.size);
    let [clear, close, next, prev] = buttons(left, top);
    let (forward, back) = open.editor.page_buttons();
    let mut closing = false;
    if inside(clear, at) {
        let e = &mut open.editor;
        e.editing = None;
        let page = e.page;
        e.pages[page].retain(|en| !en.checked);
        if e.pages[page].is_empty() {
            e.pages[page].push(Entry::default());
        }
        ui_sound(host, "minecraft:ui.button.click", 0.25, 1.0);
    } else if inside(close, at) {
        closing = true;
    } else if forward && inside(next, at) {
        open.editor.change_page(true);
        ui_sound(host, "minecraft:ui.button.click", 0.25, 1.0);
    } else if back && inside(prev, at) {
        open.editor.change_page(false);
        ui_sound(host, "minecraft:ui.button.click", 0.25, 1.0);
    } else {
        let adv = advance(host);
        let hovered = open
            .editor
            .hovered([at[0] - (left + 35.0), at[1] - (top + 41.0)], &adv);
        let toggled = open.editor.click_entry(hovered, &adv);
        let on_check = hovered.is_some_and(|(_, check)| check);
        match toggled {
            Some(true) => ui_sound(host, "create:clipboard_check", 1.0, 0.975),
            Some(false) => ui_sound(host, "create:clipboard_erase", 1.0, 1.0),
            None => {}
        }
        if !on_check && open.editor.editing.is_some() {
            let away = at[0] < left + 50.0
                || at[0] > left + 220.0
                || at[1] < top + 30.0
                || at[1] > top + 230.0;
            if away {
                open.editor.editing = None;
            } else {
                let (text, _) = open.editor.display_text();
                let lines = cb::split(&text, cb::WRAP, &adv);
                let off = open.editor.editing_offset(&adv) as f32;
                let i = cb::index_at(
                    &lines,
                    &text,
                    at[0] - (left + 58.0),
                    at[1] - (top + 42.0 + off),
                    &adv,
                );
                let millis = open.opened.elapsed().as_millis() as u64;
                open.editor.click_text(i, millis, shift);
                open.dragging = true;
            }
        }
    }
    OPEN.with_borrow_mut(|o| *o = Some(open));
    if closing {
        LATER.with_borrow_mut(|l| l.close = true);
    }
}
