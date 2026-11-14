//! The clipboard's pages and its editing screen (`ClipboardContent`, `ClipboardEntry`,
//! `ClipboardScreen` with vanilla's `TextFieldHelper` and the screen's `DisplayCache`): up to 51
//! pages of entries, each a checkbox and a line of text wrapped to 150 pixels, edited one entry at
//! a time. Widths come from the caller's font (`width` of one character), so the layout is the
//! game's. Indices count characters.

/// `ClipboardScreen.changePage`: pages 0 to 50.
pub const LAST_PAGE: usize = 50;
/// `ClipboardScreen.validateTextForEntry`: a page's entries stay under this height.
pub const PAGE_HEIGHT: i32 = 185;
/// The width text wraps to (`font.split(.., 150)`), and beside an item icon.
pub const WRAP: f32 = 150.0;
pub const WRAP_ICON: f32 = 130.0;
/// `Font.lineHeight`.
pub const LINE: i32 = 9;
/// The mod message channel: the server opens the screen, the client sends the pages back.
pub const CHANNEL: &str = "create:clipboard";
/// `AllGuiTextures.CLIPBOARD` and the address marks, and the two `AllIcons` buttons.
pub const TEXTURE: &str = "create:gui/clipboard";
pub const WIDGETS: &str = "create:gui/widgets";
pub const ICONS: &str = "create:gui/icons";
pub const ADDRESS: [f32; 4] = [116.0, 7.0, 8.0, 8.0];
pub const ADDRESS_INACTIVE: [f32; 4] = [125.0, 7.0, 8.0, 8.0];
pub const BUTTON: [f32; 4] = [0.0, 0.0, 18.0, 18.0];
pub const BUTTON_HOVER: [f32; 4] = [18.0, 0.0, 18.0, 18.0];
pub const ICON_CLEAR_CHECKED: [f32; 4] = [64.0, 112.0, 16.0, 16.0];
pub const ICON_CLOSE: [f32; 4] = [64.0, 0.0, 16.0, 16.0];

/// `ClipboardEntry`: a checkbox, its text, and an item drawn before it (with an amount).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub checked: bool,
    pub text: String,
    pub icon: Option<(String, i32)>,
}

impl Entry {
    fn empty() -> Entry {
        Entry::default()
    }
    /// An address entry (`#` and more than blanks): drawn without its `#`, beside a tag.
    pub fn is_address(&self) -> bool {
        address(&self.text)
    }
}

fn address(text: &str) -> bool {
    text.strip_prefix('#').is_some_and(|r| !r.trim().is_empty())
}

/// `ClipboardType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Empty,
    Written,
    Editing,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Empty => "empty",
            Kind::Written => "written",
            Kind::Editing => "editing",
        }
    }
    pub fn from_name(name: &str) -> Kind {
        match name {
            "written" => Kind::Written,
            "editing" => Kind::Editing,
            _ => Kind::Empty,
        }
    }
}

/// `ClipboardContent` (its copied values are not kept: the clipboard's settings copying is not
/// ported).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub kind: Kind,
    pub pages: Vec<Vec<Entry>>,
    pub read_only: bool,
    pub previously_opened_page: i32,
}

/// `StringSplitter.splitLines(text, width, Style.EMPTY, ..)` by character widths: each line's
/// start and end (a line ends at a `\n`, or once a character overflows at the last space so far,
/// or before that character; the `\n` or space at the end belongs to no line).
pub fn split(text: &[char], width: f32, advance: &dyn Fn(char) -> f32) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let (mut used, mut visible, mut space) = (0.0f32, false, None);
        let mut cut = None;
        for (i, &c) in text.iter().enumerate().skip(start) {
            if c == '\n' {
                cut = Some(i);
                break;
            }
            if c == ' ' {
                space = Some(i);
            }
            let a = advance(c);
            used += a;
            if !visible || used <= width {
                visible |= a != 0.0;
            } else {
                cut = Some(space.unwrap_or(i));
                break;
            }
        }
        let Some(at) = cut else {
            out.push((start, text.len()));
            break;
        };
        out.push((start, at));
        start = if matches!(text[at], '\n' | ' ') {
            at + 1
        } else {
            at
        };
    }
    out
}

/// `StringSplitter.getWordPosition(text, dir, from, true)`.
pub fn word_position(text: &[char], dir: i32, from: usize) -> usize {
    let mut i = from.min(text.len());
    for _ in 0..dir.unsigned_abs() {
        if dir < 0 {
            while i > 0 && text[i - 1] == ' ' {
                i -= 1;
            }
            while i > 0 && text[i - 1] != ' ' {
                i -= 1;
            }
        } else {
            i = text[i..]
                .iter()
                .position(|&c| c == ' ')
                .map_or(text.len(), |p| i + p);
            while i < text.len() && text[i] == ' ' {
                i += 1;
            }
        }
    }
    i
}

/// The open screen (`ClipboardScreen`): the pages, the page shown, the entry being edited and
/// its `TextFieldHelper` (cursor and selection), and the last click for double clicks.
#[derive(Clone, Debug)]
pub struct Editor {
    pub content: Content,
    pub pages: Vec<Vec<Entry>>,
    pub page: usize,
    pub editing: Option<usize>,
    pub cursor: usize,
    pub selection: usize,
    pub readonly: bool,
    last_click: Option<(usize, u64)>,
}

/// What a key does (vanilla key codes `ClipboardScreen.keyPressed` reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    PageUp,
    PageDown,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    SelectAll,
    Copy,
    Paste,
    Cut,
}

impl Editor {
    /// `ClipboardScreen(..)` and `reopenWith`: on the page last opened, an empty page starting
    /// an entry to type into; read-only clipboards edit nothing.
    pub fn open(content: Content) -> Editor {
        let mut pages = content.pages.clone();
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        let page = (content.previously_opened_page.max(0) as usize).min(pages.len() - 1);
        let start_empty = pages[page].is_empty();
        if start_empty {
            pages[page].push(Entry::empty());
        }
        let readonly = content.read_only;
        Editor {
            editing: (start_empty && !readonly).then_some(0),
            content,
            pages,
            page,
            cursor: 0,
            selection: 0,
            readonly,
            last_click: None,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.pages[self.page]
    }

    fn text(&self) -> Vec<char> {
        self.editing
            .and_then(|i| self.pages[self.page].get(i))
            .map(|e| e.text.chars().collect())
            .unwrap_or_default()
    }

    fn set_text(&mut self, text: &[char]) {
        if let Some(e) = self.editing.and_then(|i| self.pages[self.page].get_mut(i)) {
            e.text = text.iter().collect();
        }
    }

    /// An entry's height on the page: `max(12, lines * 9 + 3)`, wrapped to `width`.
    fn height(text: &str, width: f32, advance: &dyn Fn(char) -> f32) -> i32 {
        let chars: Vec<char> = text.chars().collect();
        (split(&chars, width, advance).len() as i32 * LINE + 3).max(12)
    }

    /// `validateTextForEntry`: with the edited entry's text `text`, the page stays under 185.
    fn fits(&self, text: &str, advance: &dyn Fn(char) -> f32) -> bool {
        let total: i32 = (0..self.entries().len())
            .map(|i| {
                let t = if Some(i) == self.editing {
                    text
                } else {
                    &self.entries()[i].text
                };
                Self::height(t, WRAP, advance)
            })
            .sum();
        total < PAGE_HEIGHT
    }

    /// `yOffsetOfEditingEntry`: the height of the entries above the edited one.
    pub fn editing_offset(&self, advance: &dyn Fn(char) -> f32) -> i32 {
        let end = self.editing.unwrap_or(0);
        self.entries()[..end.min(self.entries().len())]
            .iter()
            .map(|e| Self::height(&e.text, WRAP, advance))
            .sum()
    }

    fn cursor_to_end(&mut self) {
        let n = self.text().len();
        self.cursor = n;
        self.selection = n;
    }

    fn set_cursor(&mut self, pos: usize, extend: bool) {
        self.cursor = pos.min(self.text().len());
        if !extend {
            self.selection = self.cursor;
        }
    }

    /// `TextFieldHelper.insertText`: in place of the selection, kept only while the page fits.
    pub fn insert(&mut self, input: &str, advance: &dyn Fn(char) -> f32) {
        if self.editing.is_none() {
            return;
        }
        let mut text = self.text();
        let (lo, hi) = (
            self.cursor.min(self.selection),
            self.cursor.max(self.selection),
        );
        let (lo, hi) = (lo.min(text.len()), hi.min(text.len()));
        text.drain(lo..hi);
        let at = lo;
        let added: Vec<char> = input.chars().collect();
        let mut next = text.clone();
        next.splice(at..at, added.iter().copied());
        let candidate: String = next.iter().collect();
        if self.fits(&candidate, advance) {
            self.set_text(&next);
            self.cursor = at + added.len();
        } else {
            self.set_text(&text);
            self.cursor = at;
        }
        self.selection = self.cursor;
    }

    /// `TextFieldHelper.removeCharsFromCursor`: the selection, else `n` characters (back when
    /// negative).
    fn remove(&mut self, n: i32) {
        let mut text = self.text();
        let (lo, hi) = if self.cursor != self.selection {
            (
                self.cursor.min(self.selection),
                self.cursor.max(self.selection),
            )
        } else {
            let to = (self.cursor as i64 + i64::from(n)).clamp(0, text.len() as i64) as usize;
            (self.cursor.min(to), self.cursor.max(to))
        };
        let hi = hi.min(text.len());
        text.drain(lo.min(hi)..hi);
        self.set_text(&text);
        self.cursor = lo.min(text.len());
        self.selection = self.cursor;
    }

    fn selected(&self) -> String {
        let text = self.text();
        let (lo, hi) = (
            self.cursor.min(self.selection),
            self.cursor.max(self.selection),
        );
        text[lo.min(text.len())..hi.min(text.len())]
            .iter()
            .collect()
    }

    /// `keyPressed` while the screen is open: the page keys, and the edited entry's keys
    /// (`keyPressedWhileEditing`). `ctrl` and `shift` are held; `clipboard` is the system
    /// clipboard (read for a paste, set by a copy or a cut). True when the key was used.
    pub fn key(
        &mut self,
        key: Key,
        ctrl: bool,
        shift: bool,
        clipboard: &mut String,
        lines: &[(usize, usize)],
        advance: &dyn Fn(char) -> f32,
    ) -> bool {
        match key {
            Key::PageUp => {
                self.change_page(false);
                return true;
            }
            Key::PageDown => {
                self.change_page(true);
                return true;
            }
            _ => {}
        }
        let Some(index) = self.editing else {
            return false;
        };
        match key {
            Key::SelectAll => {
                self.selection = 0;
                self.cursor = self.text().len();
            }
            Key::Copy => *clipboard = self.selected(),
            Key::Cut => {
                *clipboard = self.selected();
                self.remove(0);
            }
            Key::Paste => {
                let text = clipboard.clone();
                self.insert(&text, advance);
            }
            Key::Enter if shift => self.insert("\n", advance),
            Key::Enter if !ctrl => {
                // A new entry after this one (or the empty one there), unless the page is full.
                let next_empty = self
                    .entries()
                    .get(index + 1)
                    .is_some_and(|e| e.text.is_empty());
                if !next_empty {
                    self.pages[self.page].insert(index + 1, Entry::empty());
                }
                self.editing = Some(index + 1);
                self.cursor_to_end();
                if !self.fits(" ", advance) {
                    self.pages[self.page].remove(index + 1);
                    self.editing = Some(index);
                    self.cursor_to_end();
                }
            }
            Key::Enter => self.editing = None,
            Key::Backspace => {
                if self.entries()[index].text.is_empty() && self.entries().len() > 1 {
                    self.pages[self.page].remove(index);
                    self.editing = Some(index.saturating_sub(1));
                    self.cursor_to_end();
                } else if ctrl {
                    let from = self.cursor;
                    let to = word_position(&self.text(), -1, from);
                    if to != from {
                        self.selection = self.cursor;
                        self.remove(to as i32 - from as i32);
                    }
                } else {
                    self.remove(-1);
                }
            }
            Key::Delete => {
                if ctrl {
                    let from = self.cursor;
                    let to = word_position(&self.text(), 1, from);
                    if to != from {
                        self.selection = self.cursor;
                        self.remove(to as i32 - from as i32);
                    }
                } else {
                    self.remove(1);
                }
            }
            Key::Right | Key::Left => {
                let dir = if key == Key::Right { 1 } else { -1 };
                let to = if ctrl {
                    word_position(&self.text(), dir, self.cursor)
                } else {
                    (self.cursor as i64 + i64::from(dir)).clamp(0, self.text().len() as i64)
                        as usize
                };
                self.set_cursor(to, shift);
            }
            Key::Up | Key::Down => {
                let to = change_line(
                    lines,
                    &self.text(),
                    self.cursor,
                    if key == Key::Up { -1 } else { 1 },
                );
                self.set_cursor(to, shift);
            }
            Key::Home => {
                let line = line_of(lines, self.cursor);
                self.set_cursor(lines.get(line).map_or(0, |l| l.0), shift);
            }
            Key::End => {
                let line = line_of(lines, self.cursor);
                let end = lines.get(line).map_or(self.text().len(), |&(s, e)| {
                    s + stripped(&self.text()[s..e]).len()
                });
                self.set_cursor(end, shift);
            }
            Key::PageUp | Key::PageDown => {}
        }
        true
    }

    /// `changePage`: pages 0 to 50, a new page added past the last (not when read-only), an
    /// empty page started for typing, and a blank page left behind going back taken out.
    pub fn change_page(&mut self, next: bool) {
        let before = self.page;
        let page = if next {
            (self.page + 1).min(LAST_PAGE)
        } else {
            self.page.saturating_sub(1)
        };
        if page == before {
            return;
        }
        self.editing = None;
        if self.pages.len() <= page {
            if self.readonly {
                return;
            }
            self.pages.push(Vec::new());
        }
        self.page = page;
        if self.pages[page].is_empty() {
            self.pages[page].push(Entry::empty());
            if !self.readonly {
                self.editing = Some(0);
                self.cursor_to_end();
            }
        }
        if !next
            && self
                .pages
                .get(page + 1)
                .is_some_and(|p| p.iter().all(|e| e.text.trim().is_empty()))
        {
            self.pages.remove(page + 1);
        }
    }

    /// Whether the forward and back page buttons show.
    pub fn page_buttons(&self) -> (bool, bool) {
        (
            self.page < LAST_PAGE && (!self.readonly || self.page + 1 < self.pages.len()),
            self.page > 0,
        )
    }

    /// `tick`'s hover: the entry under `at` (relative to the window's corner less 35, 41), and
    /// whether on its checkbox; past the last entry, the next one's index.
    pub fn hovered(&self, at: [f32; 2], advance: &dyn Fn(char) -> f32) -> Option<(usize, bool)> {
        let (mx, my) = (at[0] as i32, at[1] as i32);
        if !(mx > 0 && mx < 183 && my > 0 && my < 190) {
            return None;
        }
        let mut total = 0;
        for (i, e) in self.entries().iter().enumerate() {
            let width = if e.icon.is_some() { WRAP_ICON } else { WRAP };
            total += Self::height(&e.text, width, advance);
            if total > my {
                return Some((i, mx < 20));
            }
        }
        Some((self.entries().len(), mx < 20))
    }

    /// `mouseClicked` with the left button over `hovered` (as `hovered` gives it): a checkbox
    /// toggles (Some(true) checked, Some(false) cleared, for its sound); another entry starts
    /// being edited, a new one past the last if the page has room.
    pub fn click_entry(
        &mut self,
        hovered: Option<(usize, bool)>,
        advance: &dyn Fn(char) -> f32,
    ) -> Option<bool> {
        let (entry, check) = hovered?;
        if check {
            self.editing = None;
            let e = self.pages[self.page].get_mut(entry)?;
            e.checked ^= true;
            return Some(e.checked);
        }
        if Some(entry) != self.editing && !self.readonly {
            self.editing = Some(entry);
            if entry >= self.entries().len() {
                self.pages[self.page].push(Entry::empty());
                if !self.fits(" ", advance) {
                    self.pages[self.page].remove(entry);
                    self.editing = None;
                    return None;
                }
            }
            self.cursor_to_end();
        }
        None
    }

    /// The text click of `mouseClicked` at character `index` of the edited entry (`time` in
    /// milliseconds): a second click there within 250 ms selects the word, a third everything;
    /// else the cursor moves (extending the selection with shift).
    pub fn click_text(&mut self, index: usize, time: u64, shift: bool) {
        let text = self.text();
        if self
            .last_click
            .is_some_and(|(i, t)| i == index && time.saturating_sub(t) < 250)
        {
            if self.cursor == self.selection {
                self.selection = word_position(&text, -1, index).min(index);
                self.cursor = word_position_no_skip(&text, index);
            } else {
                self.selection = 0;
                self.cursor = text.len();
            }
        } else {
            self.set_cursor(index, shift);
        }
        self.last_click = Some((index, time));
    }

    /// `mouseDragged`: the cursor to `index`, the selection kept.
    pub fn drag(&mut self, index: usize) {
        self.set_cursor(index, true);
    }

    /// `ClipboardScreen.removed`: blank entries and empty pages go, the page shown is
    /// remembered, and the content sent is written (none at all with no page left).
    pub fn close(mut self) -> Option<Content> {
        let shown = self.page;
        let mut opened = None;
        let mut pages = Vec::new();
        for (i, mut page) in std::mem::take(&mut self.pages).into_iter().enumerate() {
            page.retain(|e| !e.text.trim().is_empty());
            if page.is_empty() {
                continue;
            }
            if i == shown {
                opened = Some(pages.len() as i32);
            }
            pages.push(page);
        }
        if pages.is_empty() {
            return None;
        }
        // The page shown, where it is now; kept as it was when that page went.
        Some(Content {
            kind: Kind::Written,
            pages,
            read_only: self.content.read_only,
            previously_opened_page: opened.unwrap_or(self.content.previously_opened_page),
        })
    }

    /// The edited entry's text without an address's `#` and leading blanks, and how many
    /// characters that took off (`rebuildDisplayCache`).
    pub fn display_text(&self) -> (Vec<char>, usize) {
        let text = self.text();
        let s: String = text.iter().collect();
        if address(&s) {
            let rest = s[1..].trim_start();
            let rest: Vec<char> = rest.chars().collect();
            let offset = text.len() - rest.len();
            return (rest, offset);
        }
        (text, 0)
    }
}

/// `StringSplitter.getWordPosition(text, 1, from, false)`: the end of the word at `from`.
fn word_position_no_skip(text: &[char], from: usize) -> usize {
    let from = from.min(text.len());
    text[from..]
        .iter()
        .position(|&c| c == ' ')
        .map_or(text.len(), |p| from + p)
}

/// A line's text without the spaces and line break it ends with (`StringUtils.stripEnd`).
pub fn stripped(line: &[char]) -> &[char] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], ' ' | '\n') {
        end -= 1;
    }
    &line[..end]
}

/// `DisplayCache.findLineFromPos`: the line holding character `pos` (by line starts).
pub fn line_of(lines: &[(usize, usize)], pos: usize) -> usize {
    match lines.binary_search_by(|l| l.0.cmp(&pos)) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

/// `DisplayCache.changeLine`: the same column `dy` lines away, at most that line's length.
pub fn change_line(lines: &[(usize, usize)], text: &[char], pos: usize, dy: i32) -> usize {
    if lines.is_empty() {
        return pos;
    }
    let i = line_of(lines, pos);
    let j = i as i64 + i64::from(dy);
    if j < 0 || j as usize >= lines.len() {
        return pos;
    }
    let (start, end) = lines[j as usize];
    let column = pos - lines[i].0;
    start + column.min(stripped(&text[start..end]).len())
}

/// `DisplayCache.getIndexAtPosition`: the character at `x`, `y` pixels into the edited text.
pub fn index_at(
    lines: &[(usize, usize)],
    text: &[char],
    x: f32,
    y: f32,
    advance: &dyn Fn(char) -> f32,
) -> usize {
    let line = (y as i32).div_euclid(LINE);
    if line < 0 {
        return 0;
    }
    let Some(&(start, end)) = lines.get(line as usize) else {
        return text.len();
    };
    // `plainIndexAtWidth`: the last character whose middle is left of `x`.
    let mut w = 0.0;
    for (k, &c) in stripped(&text[start..end]).iter().enumerate() {
        let a = advance(c);
        if w + a / 2.0 > x {
            return start + k;
        }
        w += a;
    }
    start + stripped(&text[start..end]).len()
}

// ---------------------------------------------------------------------------------------------
// The wire: content as bytes (strings as length and UTF-8)

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u32).to_le_bytes());
    out.extend(s.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.bytes(n)?.to_vec()).ok()
    }
}

impl Content {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.kind as u8);
        out.push(u8::from(self.read_only));
        out.extend(self.previously_opened_page.to_le_bytes());
        out.extend((self.pages.len() as u32).to_le_bytes());
        for page in &self.pages {
            out.extend((page.len() as u32).to_le_bytes());
            for e in page {
                out.push(u8::from(e.checked));
                put_str(out, &e.text);
                match &e.icon {
                    Some((item, n)) => {
                        out.push(1);
                        put_str(out, item);
                        out.extend(n.to_le_bytes());
                    }
                    None => out.push(0),
                }
            }
        }
    }

    fn decode_from(r: &mut Reader<'_>) -> Option<Content> {
        let kind = match r.u8()? {
            1 => Kind::Written,
            2 => Kind::Editing,
            _ => Kind::Empty,
        };
        let read_only = r.u8()? != 0;
        let previously_opened_page = r.u32()? as i32;
        let n = r.u32()? as usize;
        let mut pages = Vec::with_capacity(n.min(LAST_PAGE + 1));
        for _ in 0..n.min(LAST_PAGE + 1) {
            let m = r.u32()? as usize;
            let mut page = Vec::new();
            for _ in 0..m.min(256) {
                let checked = r.u8()? != 0;
                let text = r.str()?;
                let icon = match r.u8()? {
                    1 => Some((r.str()?, r.u32()? as i32)),
                    _ => None,
                };
                page.push(Entry {
                    checked,
                    text,
                    icon,
                });
            }
            pages.push(page);
        }
        Some(Content {
            kind,
            pages,
            read_only,
            previously_opened_page,
        })
    }
}

/// Server to client: open the screen for hotbar slot `slot`, or the clipboard block at `pos`.
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub slot: u8,
    pub pos: Option<[i32; 3]>,
    pub content: Content,
}

/// Client to server (`ClipboardEditPacket`): the pages for hotbar slot `slot` or the block at
/// `pos`; none takes them all off.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub slot: u8,
    pub pos: Option<[i32; 3]>,
    pub content: Option<Content>,
}

fn put_target(out: &mut Vec<u8>, slot: u8, pos: Option<[i32; 3]>) {
    out.push(slot);
    match pos {
        Some(p) => {
            out.push(1);
            for v in p {
                out.extend(v.to_le_bytes());
            }
        }
        None => out.push(0),
    }
}

fn read_target(r: &mut Reader<'_>) -> Option<(u8, Option<[i32; 3]>)> {
    let slot = r.u8()?;
    let pos = match r.u8()? {
        1 => Some([r.u32()? as i32, r.u32()? as i32, r.u32()? as i32]),
        _ => None,
    };
    Some((slot, pos))
}

impl Open {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0];
        put_target(&mut out, self.slot, self.pos);
        self.content.encode(&mut out);
        out
    }
    pub fn decode(data: &[u8]) -> Option<Open> {
        let mut r = Reader(data);
        (r.u8()? == 0).then_some(())?;
        let (slot, pos) = read_target(&mut r)?;
        let content = Content::decode_from(&mut r)?;
        Some(Open { slot, pos, content })
    }
}

impl Edit {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![1];
        put_target(&mut out, self.slot, self.pos);
        match &self.content {
            Some(c) => {
                out.push(1);
                c.encode(&mut out);
            }
            None => out.push(0),
        }
        out
    }
    pub fn decode(data: &[u8]) -> Option<Edit> {
        let mut r = Reader(data);
        (r.u8()? == 1).then_some(())?;
        let (slot, pos) = read_target(&mut r)?;
        let content = match r.u8()? {
            1 => Some(Content::decode_from(&mut r)?),
            _ => None,
        };
        Some(Edit { slot, pos, content })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six pixels a character, as most of the default font.
    fn six(_: char) -> f32 {
        6.0
    }

    fn typed(e: &mut Editor, s: &str) {
        e.insert(s, &six);
    }

    #[test]
    fn a_new_clipboard_starts_typing_on_its_first_page() {
        let mut e = Editor::open(Content::default());
        assert_eq!((e.page, e.editing), (0, Some(0)));
        typed(&mut e, "buy iron");
        assert_eq!(e.entries()[0].text, "buy iron");
        let mut clip = String::new();
        // Enter starts the next entry; backspace on an empty one takes it out again.
        assert!(e.key(Key::Enter, false, false, &mut clip, &[], &six));
        assert_eq!((e.entries().len(), e.editing), (2, Some(1)));
        typed(&mut e, "and gold");
        let c = e.close().expect("written");
        assert_eq!(c.kind, Kind::Written);
        assert_eq!(
            c.pages[0]
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            ["buy iron", "and gold"]
        );
    }

    #[test]
    fn editing_follows_text_field_helper() {
        let mut e = Editor::open(Content::default());
        let mut clip = String::new();
        typed(&mut e, "one two three");
        e.key(Key::Backspace, true, false, &mut clip, &[], &six);
        assert_eq!(e.entries()[0].text, "one two ");
        e.key(Key::Left, true, false, &mut clip, &[], &six);
        assert_eq!(e.cursor, 4);
        e.key(Key::Right, false, true, &mut clip, &[], &six);
        e.key(Key::Right, false, true, &mut clip, &[], &six);
        e.key(Key::Copy, false, false, &mut clip, &[], &six);
        assert_eq!(clip, "tw");
        e.key(Key::Cut, false, false, &mut clip, &[], &six);
        assert_eq!(e.entries()[0].text, "one o ");
        e.key(
            Key::End,
            false,
            false,
            &mut clip,
            &split(&e.text(), WRAP, &six),
            &six,
        );
        e.key(Key::Paste, false, false, &mut clip, &[], &six);
        // End stops before the line's trailing space (`findLineEnd`: the stripped contents).
        assert_eq!(e.entries()[0].text, "one otw ");
        e.key(Key::SelectAll, false, false, &mut clip, &[], &six);
        typed(&mut e, "x");
        assert_eq!(e.entries()[0].text, "x");
    }

    #[test]
    fn a_page_holds_what_fits_under_185_pixels() {
        let mut e = Editor::open(Content::default());
        let mut clip = String::new();
        // 15 one-line entries of 12: 180; the 16th would make 192.
        for i in 0..20 {
            typed(&mut e, &format!("item {i}"));
            e.key(Key::Enter, false, false, &mut clip, &[], &six);
        }
        assert_eq!(e.entries().len(), 15);
        // A line wraps at 150 pixels: 25 characters of 6.
        let chars: Vec<char> = "aaaa bbbb cccc dddd eeee ffff".chars().collect();
        assert_eq!(split(&chars, WRAP, &six), vec![(0, 24), (25, 29)]);
    }

    #[test]
    fn checkboxes_toggle_and_pages_turn() {
        let mut e = Editor::open(Content::default());
        typed(&mut e, "milk");
        assert_eq!(e.click_entry(Some((0, true)), &six), Some(true));
        assert!(e.entries()[0].checked && e.editing.is_none());
        // A hover past the last entry starts a new one.
        assert_eq!(e.hovered([30.0, 13.0], &six), Some((1, false)));
        e.click_entry(Some((1, false)), &six);
        assert_eq!((e.entries().len(), e.editing), (2, Some(1)));
        e.change_page(true);
        assert_eq!((e.page, e.pages.len(), e.editing), (1, 2, Some(0)));
        // Back again: the blank page behind is taken out.
        e.change_page(false);
        assert_eq!((e.page, e.pages.len()), (0, 1));
        let c = e.close().unwrap();
        assert_eq!(c.pages[0].len(), 1);
        assert!(c.pages[0][0].checked);
    }

    #[test]
    fn read_only_clipboards_edit_nothing_and_closing_blank_clears_them() {
        let content = Content {
            kind: Kind::Written,
            pages: vec![vec![Entry {
                text: "do not touch".into(),
                ..Entry::default()
            }]],
            read_only: true,
            previously_opened_page: 3,
        };
        let mut e = Editor::open(content);
        assert_eq!((e.page, e.editing), (0, None));
        e.click_entry(Some((0, false)), &six);
        assert_eq!(e.editing, None);
        e.change_page(true);
        assert_eq!(e.page, 0);
        assert!(Editor::open(Content::default()).close().is_none());
    }

    #[test]
    fn addresses_and_lines_and_clicks() {
        assert!(address("# Depot") && !address("#  ") && !address("Depot"));
        let text: Vec<char> = "ab cd".chars().collect();
        let lines = split(&text, 12.0, &six);
        assert_eq!(lines, vec![(0, 2), (3, 5)]);
        assert_eq!(change_line(&lines, &text, 1, 1), 4);
        assert_eq!(index_at(&lines, &text, 7.0, 10.0, &six), 4);
        assert_eq!(word_position(&text, 1, 0), 3);
        assert_eq!(word_position(&text, -1, 5), 3);
        let mut e = Editor::open(Content::default());
        typed(&mut e, "ab cd");
        e.click_text(4, 1000, false);
        e.click_text(4, 1100, false);
        assert_eq!((e.selection, e.cursor), (3, 5));
    }

    #[test]
    fn content_round_trips_on_the_wire() {
        let content = Content {
            kind: Kind::Written,
            pages: vec![vec![
                Entry {
                    checked: true,
                    text: "Ünïcode ✔".into(),
                    icon: Some(("minecraft:iron_ingot".into(), 3)),
                },
                Entry::default(),
            ]],
            read_only: false,
            previously_opened_page: 0,
        };
        let open = Open {
            slot: 4,
            pos: Some([1, -2, 3]),
            content: content.clone(),
        };
        assert_eq!(Open::decode(&open.encode()), Some(open));
        let edit = Edit {
            slot: 2,
            pos: None,
            content: Some(content),
        };
        assert_eq!(Edit::decode(&edit.encode()), Some(edit.clone()));
        assert_eq!(Open::decode(&edit.encode()), None);
    }
}
