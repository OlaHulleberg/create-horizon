//! What the HUD shows of the block under the crosshair: a scroll value's tip over the hotbar
//! (`ValueSettingsClient.render`) and, with goggles on, its kinetic stats
//! (`GoggleOverlayRenderer`).

use super::super::items::{self, Item};
use super::board::{self, block_at, speed_at};
use mod_api::{ClientHost, CrosshairTarget, DrawContext, EquipmentSlot};

/// The block under the crosshair, when it is Create's.
fn aimed(host: &dyn ClientHost) -> Option<[i32; 3]> {
    let Some(CrosshairTarget::Block { pos, state, .. }) = host.crosshair_target() else {
        return None;
    };
    // Hot: one lookup for any other block.
    super::super::states::id(state)?;
    Some([pos.x, pos.y, pos.z])
}

/// `ValueSettingsClient.render` for the value box under the crosshair (`ScrollValueRenderer`
/// and `showHoverTip`): its label and "Click and hold to edit", centred over the hotbar, fading
/// in over five ticks once looked at for six.
pub(super) fn value_tip(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let now = host
        .level(host.dimension())
        .and_then(|l| l.game_time(host))
        .unwrap_or(0);
    let Some((pos, title)) = board::hovered_box(host) else {
        TIP.set(None);
        return;
    };
    let since = match TIP.get() {
        Some((at, t)) if at == pos => t,
        _ => {
            TIP.set(Some((pos, now)));
            now
        }
    };
    // `hoverWarmup`: six ticks before it shows, then five to fade in.
    let shown = now - since - 6;
    if shown < 0 {
        return;
    }
    let alpha = ((shown as f32 + 1.0) / 5.0).min(1.0);
    let a = ((alpha * 255.0) as u32).max(5) << 24;
    let hint = host
        .translate("create.gui.value_settings.hold_to_edit")
        .unwrap_or_else(|| "Click and hold to edit".into());
    let lines = [(title.to_owned(), 0xFB_DC7D), (hint, 0xFF_FFFF)];
    let [width, height] = draw.size();
    let mut y = height - 75.0 - lines.len() as f32 * 12.0;
    for (text, rgb) in &lines {
        let w = draw.text_width(text);
        draw.text(text, [(width / 2.0 - w / 2.0).floor(), y], a | rgb, true);
        y += 12.0;
    }
}

std::thread_local! {
    /// The value box looked at, and since when (game time).
    static TIP: std::cell::Cell<Option<([i32; 3], i64)>> = const { std::cell::Cell::new(None) };
}

/// `LangNumberFormat`: grouped thousands, at most two decimals (en_us).
fn number(v: f32) -> String {
    let text = format!("{v:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac}")
    }
}

/// `GoggleOverlayRenderer` for kinetic blocks: with goggles on the head, the block under the
/// crosshair tells its stress (`KineticBlockEntity.addToGoggleTooltip`: impact × speed;
/// `GeneratingKineticBlockEntity`: capacity × speed), then what each subsystem adds
/// (`Subsystem::goggle_tooltip`), in a tooltip right of the crosshair.
pub(super) fn goggle_overlay(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let Some(player) = host.local_player() else {
        return;
    };
    // Hot: one compare on the head slot.
    let head = host
        .living(player.entity())
        .and_then(|l| l.equipment(host, EquipmentSlot::Head));
    if !items::is(head.as_ref(), Item::Goggles) {
        return;
    }
    let Some(pos) = aimed(host) else {
        HOVER.set(None);
        return;
    };
    let Some((block, impact, capacity)) = super::super::client_table(host, |t| {
        let (block, info) = t.get(block_at(host, pos))?;
        let [part, _] = create_engine::parts::parts(block, info, &Default::default());
        Some(part.map_or((block, 0.0, 0.0), |p| (block, p.impact, p.capacity)))
    })
    .flatten() else {
        return;
    };
    let speed = speed_at(host, pos);
    // Create's tooltip: a header, then each stat indented by four spaces and its value by one
    // more; a line may change colour along the way.
    let mut lines: Vec<Vec<(String, u32)>> = Vec::new();
    if impact != 0.0 {
        lines.push(vec![("    Kinetic Stats:".into(), 0xFFFFFF)]);
        lines.push(vec![("    Kinetic Stress Impact:".into(), 0xAAAAAA)]);
        lines.push(vec![
            (format!("     {}su", number(impact * speed.abs())), 0x55FFFF),
            (" at current speed".into(), 0x555555),
        ]);
    }
    if capacity != 0.0 {
        lines.push(vec![("    Generator Stats:".into(), 0xFFFFFF)]);
        lines.push(vec![("    Kinetic Stress Capacity:".into(), 0xAAAAAA)]);
        lines.push(vec![
            (
                format!("     {}su", number(capacity * speed.abs())),
                0x55FFFF,
            ),
            (" at current speed".into(), 0x555555),
        ]);
    }
    for s in super::super::SUBSYSTEMS {
        s.goggle_tooltip(host, pos, block, &mut lines);
    }
    if lines.is_empty() {
        HOVER.set(None);
        return;
    }
    // `hoverTicks`: how long this block has been looked at, for the fade in.
    let now = host
        .level(host.dimension())
        .and_then(|l| l.game_time(host))
        .unwrap_or(0);
    let since = match HOVER.get() {
        Some((at, start)) if at == pos => start,
        _ => {
            HOVER.set(Some((pos, now)));
            now
        }
    };
    let fade = ((now - since) as f32 / 24.0).clamp(0.0, 1.0);
    let line_width = |draw: &dyn DrawContext, l: &[(String, u32)]| -> f32 {
        l.iter().map(|(t, _)| draw.text_width(t)).sum()
    };
    let [width, height] = draw.size();
    let w = lines
        .iter()
        .map(|l| line_width(draw, l))
        .fold(0.0f32, f32::max)
        .ceil();
    // The title line, then a gap of two, then ten a line.
    let h = 8.0
        + if lines.len() > 1 {
            2.0 + (lines.len() - 1) as f32 * 10.0
        } else {
            0.0
        };
    // At (width / 2 + overlayOffsetX 20, height / 2 + overlayOffsetY 0), kept on screen, sliding in
    // from 8 px right while it fades.
    let slide = (1.0 - fade).powi(3) * 8.0;
    let pos_x = (width / 2.0 + 20.0).min(width - w - 20.0).floor() + slide;
    let pos_y = (height / 2.0).min(height - h - 20.0).floor();
    // RemovedGuiUtils.drawHoveringText at the point given: the box 12 right and 12 up of it, in
    // vanilla's tooltip colours (the background at three quarters of its alpha).
    let (x, y) = (pos_x + 12.0, pos_y - 12.0);
    let alpha = |argb: u32| {
        let a = ((argb >> 24) as f32 * fade).round() as u32;
        (a << 24) | (argb & 0xFF_FFFF)
    };
    let background = alpha(0xB410_0010);
    let (top, bottom) = (alpha(0x5050_00FF), alpha(0x5028_007F));
    draw.fill([x - 3.0, y - 4.0, w + 6.0, 1.0], background);
    draw.fill([x - 3.0, y + h + 3.0, w + 6.0, 1.0], background);
    draw.fill([x - 3.0, y - 3.0, w + 6.0, h + 6.0], background);
    draw.fill([x - 4.0, y - 3.0, 1.0, h + 6.0], background);
    draw.fill([x + w + 3.0, y - 3.0, 1.0, h + 6.0], background);
    // The sides' borders fade from the top colour to the bottom one.
    let rows = (h + 4.0) as u32;
    for row in 0..rows {
        let c = mix_argb(top, bottom, row as f32 / rows.max(1) as f32);
        let at = y - 2.0 + row as f32;
        draw.fill([x - 3.0, at, 1.0, 1.0], c);
        draw.fill([x + w + 2.0, at, 1.0, 1.0], c);
    }
    draw.fill([x - 3.0, y - 3.0, w + 6.0, 1.0], top);
    draw.fill([x - 3.0, y + h + 2.0, w + 6.0, 1.0], bottom);
    // The goggles over the box's corner (z 450 over the tooltip's 400).
    if let Some(goggles) = items::key(Item::Goggles) {
        draw.item(goggles, [pos_x + 10.0, pos_y - 16.0], 16.0);
    }
    let text_alpha = ((255.0 * fade) as u32).max(5) << 24;
    for (i, line) in lines.iter().enumerate() {
        let mut dx = 0.0;
        let ty = y + i as f32 * 10.0 + if i > 0 { 2.0 } else { 0.0 };
        for (text, rgb) in line {
            draw.text(text, [x + dx, ty], text_alpha | rgb, true);
            dx += draw.text_width(text);
        }
    }
}

std::thread_local! {
    /// The block the goggles' overlay shows, and the game time it was first shown.
    static HOVER: std::cell::Cell<Option<([i32; 3], i64)>> = const { std::cell::Cell::new(None) };
}

/// Each channel of `a` moved `t` of the way to `b`'s (`fillGradient`'s vertical blend).
fn mix_argb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32 & 0xFF) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::number;

    #[test]
    fn numbers_group_thousands_and_keep_two_decimals() {
        assert_eq!(number(4096.0), "4,096");
        assert_eq!(number(12.5), "12.5");
        assert_eq!(number(0.126), "0.13");
        assert_eq!(number(1234567.0), "1,234,567");
    }
}
