//! What the HUD shows of the block under the crosshair: a scroll value's tip over the hotbar
//! (`ValueSettingsClient.render`) and, with goggles on, its kinetic stats
//! (`GoggleOverlayRenderer`).

use super::super::items::{self, Item};
use super::board::{block_at, holds_wrench, seen, speed_at};
use create_engine::Block as B;
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

/// `ValueSettingsClient.render` for a scroll value (`KineticScrollValueBehaviour`,
/// `ScrollOptionBehaviour`, `ChassisScrollValueBehaviour`): its label over the hotbar, centred,
/// with the value and how to change it here (the wheel; holding use opens Create 6's value
/// board).
pub(super) fn value_tip(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let Some(pos) = aimed(host) else {
        return;
    };
    let Some(player) = host.local_player() else {
        return;
    };
    let wrench = holds_wrench(host, player);
    let Some((block, facing)) = super::super::client_table(host, |t| {
        t.get(block_at(host, pos)).map(|(b, i)| (b, i.facing))
    })
    .flatten() else {
        return;
    };
    let (label, value) = match block {
        // The setting, from the speed the motor gives along its axis.
        B::CreativeMotor => {
            let speed = speed_at(host, pos);
            let setting = facing.map_or(speed, |f| {
                create_engine::kinetics::convert_to_direction(speed, f)
            });
            ("Generated Speed in RPM", format!("{setting:.0}"))
        }
        // `ChassisScrollValueBehaviour` (with the wrench).
        B::LinearChassis | B::SecondaryLinearChassis | B::RadialChassis if wrench => (
            "Range of Sticky Sides",
            seen(pos)
                .unwrap_or(create_engine::contraption::CHASSIS_RANGE)
                .to_string(),
        ),
        // `BrassTunnelBlockEntity.selectionMode`.
        B::BrassTunnel => (
            "Distribution Method",
            create_engine::tunnel::Mode::ALL[seen(pos).unwrap_or(0).clamp(0, 4) as usize]
                .label()
                .to_owned(),
        ),
        _ => return,
    };
    let lines = [
        (label.to_owned(), 0xFB_DC7D),
        (value, 0xFF_FFFF),
        ("Scroll to change".to_owned(), 0xFF_FFFF),
    ];
    let [width, height] = draw.size();
    let mut y = height - 75.0 - lines.len() as f32 * 12.0;
    for (text, rgb) in &lines {
        let w = draw.text_width(text);
        draw.text(text, [width / 2.0 - w / 2.0, y], 0xFF00_0000 | rgb, true);
        y += 12.0;
    }
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
        lines.push(vec![(
            format!("     {}su at current speed", number(impact * speed.abs())),
            0x55FFFF,
        )]);
    }
    if capacity != 0.0 {
        lines.push(vec![("    Generator Stats:".into(), 0xFFFFFF)]);
        lines.push(vec![("    Kinetic Stress Capacity:".into(), 0xAAAAAA)]);
        lines.push(vec![(
            format!("     {}su at current speed", number(capacity * speed.abs())),
            0x55FFFF,
        )]);
    }
    for s in super::super::SUBSYSTEMS {
        s.goggle_tooltip(host, pos, block, &mut lines);
    }
    if lines.is_empty() {
        return;
    }
    let line_width = |draw: &dyn DrawContext, l: &[(String, u32)]| -> f32 {
        l.iter().map(|(t, _)| draw.text_width(t)).sum()
    };
    let [width, height] = draw.size();
    // RemovedGuiUtils.drawHoveringText at (width / 2 + 20, height / 2): vanilla's tooltip box.
    let w = lines
        .iter()
        .map(|l| line_width(draw, l))
        .fold(0.0f32, f32::max);
    let h = lines.len() as f32 * 10.0 - 2.0;
    let x = (width / 2.0 + 20.0).min(width - w - 20.0) + 4.0;
    let y = (height / 2.0).min(height - h - 20.0) + 4.0;
    draw.fill([x - 3.0, y - 4.0, w + 6.0, h + 8.0], 0xF010_0010);
    draw.fill([x - 3.0, y - 3.0, w + 6.0, 1.0], 0x5050_00FF);
    draw.fill([x - 3.0, y + h + 2.0, w + 6.0, 1.0], 0x5028_007F);
    for (i, line) in lines.iter().enumerate() {
        let mut dx = 0.0;
        for (text, rgb) in line {
            draw.text(text, [x + dx, y + i as f32 * 10.0], 0xFF00_0000 | rgb, true);
            dx += draw.text_width(text);
        }
    }
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
