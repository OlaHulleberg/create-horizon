//! Create's GUI widgets as the handheld gadgets' screens (the wand of symmetry's, the
//! worldshaper's) draw and play them: `IconButton` over `AllGuiTextures.BUTTON*`, the `Indicator`,
//! `ScrollInput` and `SelectionScrollInput` (their tooltips and `SCROLL_VALUE` sound), drawn
//! on a mod's own screen (`ClientHost::mod_screen_open`).

use mod_api::text::TextStyle;
use mod_api::{ClientHost, DrawContext, SoundPlay, Text};

/// `create:textures/gui/widgets.png`, `icons.png` (`AllIcons`, 16 pixel cells) and
/// `curiosities.png` (`WAND_OF_SYMMETRY`, `TERRAINZAPPER`).
pub(super) const WIDGETS: &str = "create:gui/widgets";
pub(super) const ICONS: &str = "create:gui/icons";
pub(super) const CURIOSITIES: &str = "create:gui/curiosities";

/// `AllIcons.I_CONFIRM`'s cell.
pub(super) const I_CONFIRM: [f32; 2] = [0.0, 16.0];

/// `AbstractSimiScreen`'s window corner for a window of `size` moved by `offset_x`
/// (`setWindowOffset`), on a screen of `screen` GUI pixels.
pub(super) fn corner(screen: [f32; 2], size: [f32; 2], offset_x: i32) -> (f32, f32) {
    (
        ((screen[0] as i32 - size[0] as i32) / 2 + offset_x) as f32,
        ((screen[1] as i32 - size[1] as i32) / 2) as f32,
    )
}

pub(super) fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// An 18 pixel square at `at`.
pub(super) fn square(at: [f32; 2]) -> [f32; 4] {
    [at[0], at[1], 18.0, 18.0]
}

/// `CreateLang.translateDirect`: `create.<key>` in the client's language.
pub(super) fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(&format!("create.{key}"))
        .unwrap_or_else(|| key.to_owned())
}

/// `IconButton.doRender`: hovered, pressed (`BUTTON_DOWN` while the left button is held over
/// it), green, or plain; the icon at one pixel in.
pub(super) fn icon_button(
    draw: &mut dyn DrawContext,
    at: [f32; 2],
    icon: [f32; 2],
    green: bool,
    mouse: [f32; 2],
    pressed: bool,
) {
    let hovered = inside(square(at), mouse);
    let u = if hovered && pressed {
        36.0
    } else if hovered {
        18.0
    } else if green {
        72.0
    } else {
        0.0
    };
    draw.sprite(WIDGETS, [u, 0.0, 18.0, 18.0], square(at), None);
    draw.sprite(
        ICONS,
        [icon[0], icon[1], 16.0, 16.0],
        [at[0] + 1.0, at[1] + 1.0, 16.0, 16.0],
        None,
    );
}

/// `Indicator.render`: `INDICATOR_WHITE` when on, `INDICATOR` when off.
pub(super) fn indicator(draw: &mut dyn DrawContext, at: [f32; 2], on: bool) {
    let u = if on { 18.0 } else { 0.0 };
    draw.sprite(
        WIDGETS,
        [u, 18.0, 18.0, 6.0],
        [at[0], at[1], 18.0, 6.0],
        None,
    );
}

pub(super) fn styled(text: String, color: &str, italic: bool) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text).with_style(style)
}

/// `AbstractSimiWidget.HEADER_RGB`.
const HEADER: &str = "#5391E1";

/// `SelectionScrollInput.updateTooltip`: the title, the options round the chosen one (at most
/// sixteen, "> ..." for the rest), and "Scroll to Select".
pub(super) fn selection_tooltip(
    host: &dyn ClientHost,
    title: String,
    options: &[String],
    state: usize,
) -> Vec<Text> {
    let mut lines = vec![styled(title, HEADER, false)];
    let (lo, hi) = (0i32, options.len() as i32);
    let s = state as i32;
    let mut min = (hi - 16).min(s - 7).max(lo);
    let mut max = (lo + 16).max(s + 8).min(hi);
    if lo + 1 == min {
        min -= 1;
    }
    if min > lo {
        lines.push(styled("> ...".into(), "gray", false));
    }
    if hi - 1 == max {
        max += 1;
    }
    for (i, name) in options
        .iter()
        .enumerate()
        .take(max as usize)
        .skip(min as usize)
    {
        if i == state {
            lines.push(styled(format!("-> {name}"), "white", false));
        } else {
            lines.push(styled(format!("> {name}"), "gray", false));
        }
    }
    if max < hi {
        lines.push(styled("> ...".into(), "gray", false));
    }
    lines.push(styled(
        tr(host, "gui.scrollInput.scrollToSelect"),
        "dark_gray",
        true,
    ));
    lines
}

/// `ScrollInput.updateTooltip`: the title, "Scroll to Modify", "Shift to Scroll Faster".
pub(super) fn scroll_tooltip(host: &dyn ClientHost, title: String) -> Vec<Text> {
    vec![
        styled(title, HEADER, false),
        styled(
            tr(host, "gui.scrollInput.scrollToModify"),
            "dark_gray",
            true,
        ),
        styled(
            tr(host, "gui.scrollInput.shiftScrollsFaster"),
            "dark_gray",
            true,
        ),
    ]
}

/// `ScrollInput.mouseScrolled`: one step (five with shift, then snapped down to a multiple of
/// five), up the wheel being `+1` (`inverted` for a selection), kept in `min..max` (exclusive);
/// the new state, if it moved, with `SCROLL_VALUE` played at its pitch.
pub(super) fn scroll(
    host: &dyn ClientHost,
    state: i32,
    range: (i32, i32),
    wheel: f64,
    inverted: bool,
    shift: bool,
) -> Option<i32> {
    let (min, max) = range;
    let dir = if (wheel > 0.0) != inverted { 1 } else { -1 };
    let mut next = state + dir * if shift { 5 } else { 1 };
    if shift {
        next -= next % 5;
    }
    next = next.min(max - 1).max(min);
    if next == state {
        return None;
    }
    let pitch = 1.5 + 0.1 * (next - min) as f32 / (max - min).max(1) as f32;
    host.play_local_sound(&SoundPlay::ui("create:scroll_value").with_pitch(pitch), 0);
    Some(next)
}

/// A button's press (`AbstractWidget.playDownSound`).
pub(super) fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

/// Whether a screen event's GLFW `modifiers` hold shift (`AllKeys.shiftDown`).
pub(super) fn shift(modifiers: u32) -> bool {
    modifiers & 1 != 0
}

/// `Screen.renderBackground` in a world, under every one of Create's screens: the blur, then the
/// dark translucent menu background across the window.
pub(crate) fn dim(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let size = draw.size();
    host.apply_screen_blur();
    draw.draw_menu_background([0.0, 0.0, size[0], size[1]], true);
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_centre_with_their_offset() {
        // A 188-wide wand window, 20 left of centre, on a 427×240 screen.
        assert_eq!(
            super::corner([427.0, 240.0], [188.0, 101.0], -20),
            (99.0, 69.0)
        );
    }
}
