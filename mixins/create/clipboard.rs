// The clipboard's pages (`ClipboardBlockItem`, `ClipboardBlock`, `ClipboardEditPacket`,
// `ClipboardScreen`): the server opens the screen for the clipboard in hand or a placed one
// (`create:clipboard` messages) and writes back what the client sends when it closes; the client
// draws it as a mod screen (Horizon API 71), the engine's `clipboard::Editor` editing it. The
// pages are kept in `minecraft:custom_data` under Create's component name (Horizon adds no
// component types). Compiled against Horizon by `horizon.py check`; the template test swaps it for
// `tools/fixture/clipboard.rs`.
use super::Game;
use create_engine::clipboard::{self as cb, Content, Editor, Entry, Key, Kind};
use create_engine::world::dimension_index;
use horizon_client::debug_keys::KeyAction;
use horizon_client::hud::{HudBatch, rgba};
use horizon_client::items::ItemStack;
use horizon_client::text::Text;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::cell::RefCell;

const CUSTOM_DATA: &str = "minecraft:custom_data";
const CONTENT_KEY: &str = "create:clipboard_content";
/// The mod screen's name.
const SCREEN: &str = "create:clipboard";

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

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------------------------
// The pages on a stack

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.data().and_then(|d| d.other(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn key(k: &str) -> JavaString {
    JavaString::from(k)
}

/// `ClipboardContent.CODEC` out of the stack's custom data (`EMPTY` without one).
pub(super) fn content(stack: &ItemStack) -> Content {
    let Some(Tag::Compound(c)) = custom(stack).and_then(|c| c.get(&key(CONTENT_KEY))) else {
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
    if let Some(Tag::List(list)) = c.get(&key("pages")) {
        for page in list.iter() {
            let Tag::List(entries) = page else { continue };
            pages.push(
                entries
                    .iter()
                    .filter_map(|e| match e {
                        Tag::Compound(e) => Some(Entry {
                            checked: flag(e.get(&key("checked"))),
                            text: text(e.get(&key("text"))),
                            icon: match e.get(&key("icon")) {
                                Some(Tag::String(item)) => {
                                    Some((item.to_string_lossy(), int(e.get(&key("item_amount")))))
                                }
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
        kind: Kind::from_name(&text(c.get(&key("type")))),
        pages,
        read_only: flag(c.get(&key("read_only"))),
        previously_opened_page: int(c.get(&key("previously_opened_page"))),
    }
}

/// The stack with `content` (none takes the component off).
pub(super) fn with_content(stack: &ItemStack, content: Option<&Content>) -> ItemStack {
    let mut stack = stack.clone();
    let mut c = custom(&stack).cloned().unwrap_or_default();
    match content {
        Some(content) => {
            let mut out = Compound::new();
            out.insert(key("type"), Tag::String(content.kind.name().into()));
            let pages = content
                .pages
                .iter()
                .map(|page| {
                    Tag::List(
                        page.iter()
                            .map(|e| {
                                let mut t = Compound::new();
                                t.insert(key("checked"), Tag::Byte(i8::from(e.checked)));
                                t.insert(key("text"), Tag::String(e.text.as_str().into()));
                                if let Some((item, n)) = &e.icon {
                                    t.insert(key("icon"), Tag::String(item.as_str().into()));
                                    t.insert(key("item_amount"), Tag::Int(*n));
                                }
                                Tag::Compound(t)
                            })
                            .collect::<Vec<_>>()
                            .into(),
                    )
                })
                .collect::<Vec<_>>();
            out.insert(key("pages"), Tag::List(pages.into()));
            out.insert(key("read_only"), Tag::Byte(i8::from(content.read_only)));
            out.insert(
                key("previously_opened_page"),
                Tag::Int(content.previously_opened_page),
            );
            c.insert(key(CONTENT_KEY), Tag::Compound(out));
        }
        None => {
            c.remove(&key(CONTENT_KEY));
        }
    }
    let data = stack.data_mut();
    data.set_other(CUSTOM_DATA, (!c.is_empty()).then_some(Tag::Compound(c)));
    stack.tidy();
    stack
}

// ---------------------------------------------------------------------------------------------
// Server

/// Opens the screen on the acting player's client for `stack`: the clipboard in their hand
/// (`pos` none) or the one placed at `pos`.
pub(super) fn open(game: &mut Game<'_>, pos: Option<[i32; 3]>, stack: &ItemStack) {
    let Some(player) = game.native_acting_player().map(|p| p.uuid) else {
        return;
    };
    let message = cb::Open {
        slot: game.selected as u8,
        pos,
        content: content(stack),
    };
    super::create::send_to_player(game, player, cb::CHANNEL, &message.encode());
}

/// `ClipboardBlockItem.use` (in the air, or on a block when not sneaking, `useOn`): the screen
/// opens for the main hand's clipboard, which is marked as being edited.
pub(super) fn use_item(game: &mut Game<'_>, stack: &ItemStack, claimed: &mut bool) {
    if &*stack.item != "create:clipboard" || *claimed {
        return;
    }
    let held = game.inventory.get(game.selected).and_then(|s| s.as_ref());
    if held.is_none_or(|h| h != stack) {
        return;
    }
    *claimed = true;
    open(game, None, stack);
    let mut c = content(stack);
    c.kind = Kind::Editing;
    let edited = with_content(stack, Some(&c));
    if let Some(slot) = game.inventory.get_mut(game.selected) {
        *slot = Some(edited);
    }
}

/// `ClipboardBlockItem.useOn`: not sneaking, as `use`; sneaking places the clipboard.
pub(super) fn use_item_on(game: &mut Game<'_>, stack: &ItemStack, claimed: &mut bool) {
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return;
    }
    use_item(game, stack, claimed);
}

/// `ClipboardEditPacket.handle`: the pages a client sent onto the clipboard it edited, the
/// placed one when within 20 blocks of reach, else the one in that hotbar slot.
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != cb::CHANNEL {
        return;
    }
    let Some(edit) = cb::Edit::decode(data) else {
        return;
    };
    let Some(p) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
    else {
        return;
    };
    if let Some(pos) = edit.pos {
        let Some(dim) = dimension_index(&p.dimension) else {
            return;
        };
        let near = (0..3)
            .map(|k| (f64::from(pos[k]) + 0.5 - p.position[k]).powi(2))
            .sum::<f64>()
            < 25.0 * 25.0;
        if !near || p.dimension != game.dim.id() {
            return;
        }
        if let Some(stack) = super::create_decor::clipboard_stack(game, dim, pos) {
            let edited = with_content(&stack, edit.content.as_ref());
            super::create_decor::set_clipboard_stack(game, dim, pos, edited);
        }
        return;
    }
    let slot = usize::from(edit.slot);
    if slot >= 9 {
        return;
    }
    let acting = game.lan_acting;
    let Some(host) = game.lan.as_mut() else {
        return;
    };
    let Some(g) = host
        .hub
        .guests
        .iter_mut()
        .find(|g| g.placed && !g.dead && g.uuid == player && acting != Some(g.conn))
    else {
        return;
    };
    if let Some(Some(stack)) = g.inventory.get(slot)
        && &*stack.item == "create:clipboard"
    {
        let edited = with_content(stack, edit.content.as_ref());
        g.inventory[slot] = Some(edited);
    }
}

// ---------------------------------------------------------------------------------------------
// Client

/// The server opened the screen: the editor on the pages it sent.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != cb::CHANNEL {
        return;
    }
    let Some(open) = cb::Open::decode(data) else {
        return;
    };
    // `ScreenOpener.open`: the screen before closes first (and sends its pages).
    game.native_close_mod_screen();
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
    game.native_open_mod_screen(SCREEN, false);
}

fn advance<'a>(game: &'a Game<'_>) -> impl Fn(char) -> f32 + 'a {
    move |c: char| {
        let mut b = [0u8; 4];
        game.native_text_width(c.encode_utf8(&mut b))
    }
}

/// The window's corner (`setWindowSize(256, 256)`: `guiLeft`, `guiTop`).
fn corner(batch_or_size: [f32; 2]) -> (f32, f32) {
    (
        ((batch_or_size[0] as i32 - 256) / 2) as f32,
        ((batch_or_size[1] as i32 - 256) / 2) as f32,
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

fn text_argb(game: &Game<'_>, batch: &mut HudBatch, s: &str, at: [f32; 2], argb: u32) {
    let [a, r, g, b] = argb.to_be_bytes();
    batch.text_styled(
        &game.gui.atlas,
        &Text::literal(s),
        at[0],
        at[1],
        rgba(r, g, b, a),
        false,
    );
}

/// `ClipboardScreen.renderWindow`: the clipboard, the page number, each entry's checkbox (an
/// address's tag) and text, the entry being edited with its selection and blinking cursor, and
/// the buttons.
pub(super) fn draw(game: &mut Game<'_>, name: &str, batch: &mut HudBatch, cursor: [f32; 2]) {
    if name != SCREEN {
        return;
    }
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        game.native_close_mod_screen();
        return;
    };
    let size = [batch.width, batch.height];
    open.size = size;
    let (left, top) = corner(size);
    let (x, mut y) = (left, top - 8.0);
    {
        let advance = advance(game);
        if open.dragging && open.editor.editing.is_some() {
            let (text, _) = open.editor.display_text();
            let lines = cb::split(&text, cb::WRAP, &advance);
            let off = open.editor.editing_offset(&advance) as f32;
            let local = [cursor[0] - (left + 58.0), cursor[1] - (top + 42.0 + off)];
            let i = cb::index_at(&lines, &text, local[0], local[1], &advance);
            open.editor.drag(i);
        }
    }
    game.native_sprite(
        batch,
        cb::TEXTURE,
        [0.0, 0.0, 256.0, 256.0],
        [x, y, 256.0, 256.0],
    );
    let indicator = horizon_protocol::text::format_java(
        &game.translate("book.pageIndicator", "Page %1$s of %2$s"),
        &[
            (open.editor.page + 1).to_string(),
            open.editor.pages.len().to_string(),
        ],
    );
    text_argb(game, batch, &indicator, [x + 150.0, y + 9.0], 0x43ff_ffff);
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
            game.native_sprite(batch, cb::WIDGETS, sprite, [x + 44.0, y + 50.0, 8.0, 8.0]);
            shown = shown[1..].trim_start().to_owned();
        } else {
            let box_color = if e.checked { 0x668D_7F6B } else { 0xff8D_7F6B };
            text_argb(game, batch, "\u{25A1}", [x + 45.0, y + 51.0], box_color);
            if e.checked {
                text_argb(game, batch, "\u{2714}", [x + 45.0, y + 50.0], 0xff31_B25D);
            }
        }
        let chars: Vec<char> = shown.chars().collect();
        let lines = {
            let advance = advance(game);
            cb::split(&chars, cb::WRAP - icon_offset, &advance)
        };
        if lines.is_empty() {
            y += 12.0;
            continue;
        }
        if let Some((item, _)) = &e.icon {
            game.native_item(batch, item, [x + 54.0, y + 50.0], 16.0);
        }
        let color = match (e.checked, e.is_address()) {
            (true, true) => 0x668D_7F6B,
            (true, false) => 0xff31_B25D,
            _ => 0xff31_1A00,
        };
        for &(s, end) in &lines {
            if Some(i) != editing {
                let line: String = cb::stripped(&chars[s..end]).iter().collect();
                text_argb(
                    game,
                    batch,
                    &line,
                    [x + 58.0 + icon_offset, y + 50.0],
                    color,
                );
            }
            y += 9.0;
        }
        y += 3.0;
    }
    if editing.is_some() {
        draw_editing(game, &open, batch, left, top);
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
        game.native_sprite(batch, cb::WIDGETS, bg, r);
        game.native_sprite(batch, cb::ICONS, icon, [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0]);
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
            batch.sprite(&game.gui.atlas, &sprite, r[0], r[1]);
        }
    }
    OPEN.with_borrow_mut(|o| *o = Some(open));
}

/// The edited entry as `rebuildDisplayCache` lays it out: its lines, the selection's boxes and
/// the cursor (a bar, or `_` at the end), blinking every 6 ticks.
fn draw_editing(game: &Game<'_>, open: &Open, batch: &mut HudBatch, left: f32, top: f32) {
    let advance = advance(game);
    let e = &open.editor;
    let (text, offset) = e.display_text();
    let off = e.editing_offset(&advance) as f32;
    let (x0, y0) = (left + 58.0, top + 42.0 + off);
    let lines = cb::split(&text, cb::WRAP, &advance);
    let width = |s: &[char]| s.iter().map(|&c| advance(c)).sum::<f32>();
    for (k, &(s, end)) in lines.iter().enumerate() {
        let line: String = cb::stripped(&text[s..end]).iter().collect();
        text_argb(game, batch, &line, [x0, y0 + k as f32 * 9.0], 0xff31_1A00);
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
            game.native_fill(batch, [x, y0 + k as f32 * 9.0, w, 9.0], 0x8000_00ff);
        }
    }
    let ticks = (open.opened.elapsed().as_millis() / 50) as u64;
    if ticks / 6 % 2 != 0 {
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
        text_argb(game, batch, "_", [x0 + cx, y0 + cy], 0xff00_0000);
    } else {
        game.native_fill(batch, [x0 + cx, y0 + cy - 1.0, 1.0, 10.0], 0xff00_0000);
    }
}

/// `keyPressed` while the screen is open: page keys and the edited entry's keys are taken;
/// Escape closes it (Horizon's, not taken).
pub(super) fn input_key(
    game: &mut Game<'_>,
    key: &str,
    action: KeyAction,
    modifiers: u8,
    screen: bool,
    consumed: &mut bool,
) {
    if !screen || action == KeyAction::Release || game.native_mod_screen() != Some(SCREEN) {
        return;
    }
    let ctrl = modifiers & super::native_hooks::KEY_CONTROL != 0;
    let shift = modifiers & 0x1 != 0;
    let alt = modifiers & super::native_hooks::KEY_ALT != 0;
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
        _ => return,
    };
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        return;
    };
    let mut clipboard = if k == Key::Paste {
        game.native_clipboard_text()
    } else {
        String::new()
    };
    let used = {
        let advance = advance(game);
        let (text, _) = open.editor.display_text();
        let lines = cb::split(&text, cb::WRAP, &advance);
        open.editor
            .key(k, ctrl, shift, &mut clipboard, &lines, &advance)
    };
    if matches!(k, Key::Copy | Key::Cut) && used {
        game.native_set_clipboard_text(&clipboard);
    }
    *consumed |= used || open.editor.editing.is_some();
    OPEN.with_borrow_mut(|o| *o = Some(open));
}

/// `charTyped`: what `StringUtil.isAllowedChatCharacter` lets through, into the edited entry.
pub(super) fn text(game: &mut Game<'_>, name: &str, typed: &str) {
    if name != SCREEN {
        return;
    }
    let allowed: String = typed
        .chars()
        .filter(|&c| c != '\u{a7}' && c >= ' ' && c != '\u{7f}')
        .collect();
    let Some(mut open) = OPEN.with_borrow_mut(Option::take) else {
        return;
    };
    {
        let advance = advance(game);
        open.editor.insert(&allowed, &advance);
    }
    OPEN.with_borrow_mut(|o| *o = Some(open));
}

/// `mouseClicked` and the release: the buttons, a checkbox (with its sound), an entry to edit,
/// the cursor in the edited text (a double click selects the word), or editing stopped by a
/// click away from the text.
pub(super) fn mouse(game: &mut Game<'_>, name: &str, at: [f32; 2], button: u8, pressed: bool) {
    if name != SCREEN || button != 0 {
        return;
    }
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
        game.audio
            .play("minecraft:ui.button.click", None, 0.25, 1.0);
    } else if inside(close, at) {
        closing = true;
    } else if forward && inside(next, at) {
        open.editor.change_page(true);
        game.audio
            .play("minecraft:ui.button.click", None, 0.25, 1.0);
    } else if back && inside(prev, at) {
        open.editor.change_page(false);
        game.audio
            .play("minecraft:ui.button.click", None, 0.25, 1.0);
    } else {
        let adv = advance(game);
        let hovered = open
            .editor
            .hovered([at[0] - (left + 35.0), at[1] - (top + 41.0)], &adv);
        let toggled = open.editor.click_entry(hovered, &adv);
        let on_check = hovered.is_some_and(|(_, check)| check);
        drop(adv);
        match toggled {
            Some(true) => game.audio.play("create:clipboard_check", None, 1.0, 0.975),
            Some(false) => game.audio.play("create:clipboard_erase", None, 1.0, 1.0),
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
                let advance = advance(game);
                let (text, _) = open.editor.display_text();
                let lines = cb::split(&text, cb::WRAP, &advance);
                let off = open.editor.editing_offset(&advance) as f32;
                let i = cb::index_at(
                    &lines,
                    &text,
                    at[0] - (left + 58.0),
                    at[1] - (top + 42.0 + off),
                    &advance,
                );
                let shift = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
                let millis = open.opened.elapsed().as_millis() as u64;
                open.editor.click_text(i, millis, shift);
                open.dragging = true;
            }
        }
    }
    OPEN.with_borrow_mut(|o| *o = Some(open));
    if closing {
        game.native_close_mod_screen();
    }
}

/// `mouseScrolled`: a page on, wheeling down.
pub(super) fn scroll(name: &str, delta: f32) {
    if name != SCREEN || delta == 0.0 {
        return;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(open) = o.as_mut() {
            open.editor.change_page(delta < 0.0);
        }
    });
}

/// `removed`: the pages sent to the server (`ClipboardEditPacket`).
pub(super) fn removed(game: &mut Game<'_>, name: &str) {
    if name != SCREEN {
        return;
    }
    let Some(open) = OPEN.with_borrow_mut(Option::take) else {
        return;
    };
    let edit = cb::Edit {
        slot: open.slot,
        pos: open.pos,
        content: open.editor.close(),
    };
    let _ = game.native_net_send_to_server(cb::CHANNEL, &edit.encode());
}
