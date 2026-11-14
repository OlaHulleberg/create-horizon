//! What the trains' screens (the station's, the schedule's) share: their wire encoding, Create's
//! translated text, its buttons (`IconButton`, `WideIconButton`) and word wrapping.

use mod_api::{ClientHost, DrawContext, SoundPlay, Text};

pub fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut n = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..n]);
}

pub fn put_pos(out: &mut Vec<u8>, p: [i32; 3]) {
    for v in p {
        put_i32(out, v);
    }
}

/// Reads what the `put_*` functions wrote.
pub struct Reader<'a>(pub &'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    pub fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.bytes(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.bytes(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
        String::from_utf8(self.bytes(usize::from(n))?.to_vec()).ok()
    }
    pub fn pos(&mut self) -> Option<[i32; 3]> {
        Some([self.i32()?, self.i32()?, self.i32()?])
    }
}

/// `key` in the client's language with `args` in its `%s` / `%1$s` places.
pub fn tr(host: &dyn ClientHost, key: &str, args: &[String]) -> String {
    let mut s = host.translate(key).unwrap_or_else(|| key.to_owned());
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

pub fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// A button's click (`SimpleSoundInstance.forUI(UI_BUTTON_CLICK)`, as Create's buttons play it).
pub fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

/// `AllGuiTextures.BUTTON*` in `widgets.png`: the button's background for its state.
fn button_src(active: bool, hovered: bool) -> [f32; 2] {
    match (active, hovered) {
        (false, _) => [90.0, 0.0],
        (true, true) => [18.0, 0.0],
        (true, false) => [0.0, 0.0],
    }
}

/// `IconButton` at `at` (18 px square) with `AllIcons` icon `icon` (its `[x, y]` in
/// `icons.png`).
pub fn icon_button(
    draw: &mut dyn DrawContext,
    at: [f32; 2],
    icon: [f32; 2],
    active: bool,
    hovered: bool,
) {
    let b = button_src(active, hovered);
    draw.sprite(
        "create:gui/widgets",
        [b[0], b[1], 18.0, 18.0],
        [at[0], at[1], 18.0, 18.0],
        None,
    );
    draw.sprite(
        "create:gui/icons",
        [icon[0], icon[1], 16.0, 16.0],
        [at[0] + 1.0, at[1] + 1.0, 16.0, 16.0],
        None,
    );
}

/// `WideIconButton` at `at` (26 by 18) with icon `src` (`[x, y, w, h]` of `sheet`).
pub fn wide_button(
    draw: &mut dyn DrawContext,
    at: [f32; 2],
    sheet: &str,
    src: [f32; 4],
    active: bool,
    hovered: bool,
) {
    let b = button_src(active, hovered);
    draw.sprite(
        "create:gui/widgets",
        [b[0], b[1], 18.0, 18.0],
        [at[0], at[1], 18.0, 18.0],
        None,
    );
    draw.sprite(
        "create:gui/widgets",
        [b[0] + 1.0, b[1], 17.0, 18.0],
        [at[0] + 9.0, at[1], 17.0, 18.0],
        None,
    );
    draw.sprite(sheet, src, [at[0] + 1.0, at[1] + 1.0, src[2], src[3]], None);
}

/// `Font.split`: `text` broken into lines no wider than `width`, at spaces where it can.
pub fn wrap(draw: &dyn DrawContext, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if draw.text_width(&candidate) <= width || line.is_empty() {
            line = candidate;
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `GuiGraphics.drawWordWrap`: lines 9 px apart; the number drawn.
pub fn word_wrap(
    draw: &mut dyn DrawContext,
    text: &str,
    at: [f32; 2],
    width: f32,
    argb: u32,
) -> usize {
    let lines = wrap(&*draw, text, width);
    for (i, l) in lines.iter().enumerate() {
        draw.text(l, [at[0], at[1] + 9.0 * i as f32], argb, false);
    }
    lines.len()
}

/// A tooltip of `lines` at the pointer.
pub fn tooltip(draw: &mut dyn DrawContext, lines: &[String], at: [f32; 2]) {
    let lines: Vec<Text> = lines.iter().map(|l| Text::literal(l.as_str())).collect();
    let _ = draw.gui_tooltip(&lines, at);
}

/// Milliseconds now, for a text cursor's blink.
pub fn millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Whether a typed character may go in a text box (`StringUtil.isAllowedChatCharacter`).
pub fn allowed(c: char) -> bool {
    c != '\u{a7}' && c >= ' ' && c != '\u{7f}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_numbers_cross_the_wire() {
        let mut out = Vec::new();
        put_pos(&mut out, [1, -2, 3]);
        put_str(&mut out, "Mine, Platform 2");
        put_f32(&mut out, 2.5);
        out.push(1);
        let mut r = Reader(&out);
        assert_eq!(r.pos(), Some([1, -2, 3]));
        assert_eq!(r.str().as_deref(), Some("Mine, Platform 2"));
        assert_eq!(r.f32(), Some(2.5));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.u8(), None);
    }
}
