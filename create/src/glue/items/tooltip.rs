//! Create's item tooltips (`Create.REGISTRATE`'s tooltip modifier, run by
//! `ClientEvents.addToItemTooltip`): every Create item's description (`ItemDescription`: its
//! summary and behaviours under Shift, its controls under Ctrl, "Hold [Shift] for Summary"
//! otherwise) after its name, and a kinetic block's stress impact or capacity
//! (`KineticStats`) at the end, numbers with goggles on.

use super::{Item, is};
use mod_api::text::TextStyle;
use mod_api::{ClientHost, EquipmentSlot, ItemTooltipEvent, Text};

/// `FontHelper.Palette.STANDARD_CREATE`: the primary and highlight colours.
const PRIMARY: &str = "#C9974C";
const HIGHLIGHT: &str = "#F1DD79";
/// `TooltipHelper.MAX_WIDTH_PER_LINE`.
const MAX_WIDTH: i32 = 200;

/// `CKinetics.mediumStressImpact`, `highStressImpact`, `mediumCapacity`, `highCapacity`.
const MEDIUM_IMPACT: f32 = 4.0;
const HIGH_IMPACT: f32 = 8.0;
const MEDIUM_CAPACITY: f32 = 256.0;
const HIGH_CAPACITY: f32 = 1024.0;

fn styled(text: impl Into<String>, color: &str) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    Text::literal(text).with_style(style)
}

fn line(parts: Vec<Text>) -> (Text, u32) {
    let mut text = Text::literal("");
    for p in parts {
        text = text.append(p);
    }
    (text, u32::MAX)
}

/// `ItemDescription.getTooltipTranslationKey`: the description id, or the one Create gives
/// a family of items (`useKey`, `referKey`).
fn description_key(host: &dyn ClientHost, item: mod_api::ItemKey) -> Option<String> {
    let id = item.id(host);
    let name = id.strip_prefix("create:")?;
    let shared = match name {
        "cardboard_helmet" | "cardboard_chestplate" | "cardboard_leggings" | "cardboard_boots" => {
            Some("item.create.cardboard_armor")
        }
        "red_sand_paper" => Some("item.create.sand_paper"),
        "mysterious_cuckoo_clock" => Some("block.create.cuckoo_clock"),
        n if n.ends_with("_postbox") => Some("block.create.package_postbox"),
        n if n.ends_with("_toolbox") => Some("block.create.toolbox"),
        n if n.ends_with("_seat") => Some("block.create.seat"),
        n if n.ends_with("_table_cloth") => Some("block.create.table_cloth"),
        _ => None,
    };
    if let Some(key) = shared {
        return Some(key.to_owned());
    }
    // A `BlockItem`'s description id is its block's.
    Some(match host.item_block(item) {
        Some(block) => {
            let block = block.id(host);
            format!("block.{}", block.replacen(':', ".", 1))
        }
        None => format!("item.create.{name}"),
    })
}

/// `TooltipHelper.cutTextComponent`: `s` broken into lines at most `MAX_WIDTH` wide (an
/// underscore takes no width), each starting with `indent` spaces; every underscore toggles
/// between the primary and the highlight colour, across lines too.
fn cut(host: &dyn ClientHost, s: &str, indent: usize) -> Vec<(Text, u32)> {
    let width = |w: &str| {
        host.text_width(&Text::literal(w.replace('_', "")))
            .unwrap_or(0)
    };
    // `BreakIterator.getLineInstance`: a break after each run of spaces and after a hyphen.
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        word.push(c);
        let next = chars.peek().copied();
        let ends = match next {
            None => true,
            Some(n) => (c == ' ' && n != ' ') || (c == '-' && n != ' '),
        };
        if ends {
            words.push(std::mem::take(&mut word));
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let (mut current, mut w) = (String::new(), 0);
    for word in words {
        let nw = width(&word);
        if w + nw > MAX_WIDTH {
            if w > 0 {
                lines.push(std::mem::take(&mut current));
                w = 0;
            } else {
                lines.push(word);
                continue;
            }
        }
        current.push_str(&word);
        w += nw;
    }
    if w > 0 {
        lines.push(current);
    }
    let mut highlighted = false;
    lines
        .into_iter()
        .map(|l| {
            let mut parts = vec![styled(" ".repeat(indent), PRIMARY)];
            for part in l.split('_') {
                parts.push(styled(part, if highlighted { HIGHLIGHT } else { PRIMARY }));
                highlighted = !highlighted;
            }
            highlighted = !highlighted;
            line(parts)
        })
        .collect()
}

/// `ItemDescription.create` and `getCurrentLines` for the key held now.
fn description(host: &dyn ClientHost, key: &str) -> Option<Vec<(Text, u32)>> {
    let tr = |k: String| host.translate(&k);
    let summary = tr(format!("{key}.summary"))?;
    let mut on_shift = cut(host, &summary, 0);
    let mut on_ctrl = Vec::new();
    let mut behaviours = Vec::new();
    for i in 1..100 {
        let Some(condition) = tr(format!("{key}.condition{i}")) else {
            break;
        };
        let behaviour = tr(format!("{key}.behaviour{i}")).unwrap_or_default();
        behaviours.push((condition, behaviour));
    }
    if !behaviours.is_empty() {
        on_shift.push((Text::literal(""), u32::MAX));
    }
    for (condition, behaviour) in behaviours {
        on_shift.push(line(vec![styled(condition, "gray")]));
        on_shift.extend(cut(host, &behaviour, 1));
    }
    for i in 1..100 {
        let Some(control) = tr(format!("{key}.control{i}")) else {
            break;
        };
        let action = tr(format!("{key}.action{i}")).unwrap_or_default();
        on_ctrl.push(line(vec![styled(control, "gray")]));
        on_ctrl.extend(cut(host, &action, 1));
    }
    let has_description = !on_shift.is_empty();
    let has_controls = !on_ctrl.is_empty();
    let hold = |k: &str, key_name: &str, held: bool| {
        let text = tr(format!("create.tooltip.{k}")).unwrap_or_default();
        let (before, after) = text.split_once("%1$s").unwrap_or((&text, ""));
        let name = tr(format!("create.tooltip.{key_name}")).unwrap_or_default();
        line(vec![
            styled(before, "dark_gray"),
            styled(name, if held { "white" } else { "gray" }),
            styled(after, "dark_gray"),
        ])
    };
    let modifiers = host.modifiers();
    let (shift, ctrl) = (modifiers.shift(), !modifiers.shift() && modifiers.control());
    let mut out = Vec::new();
    if has_description {
        out.push(hold(
            "holdForDescription",
            "keyShift",
            shift && has_description,
        ));
    }
    if has_controls {
        out.push(hold("holdForControls", "keyCtrl", ctrl && has_controls));
    }
    // Shift shows the description (or nothing more without one), Ctrl the controls.
    if shift && has_description {
        out.push((Text::literal(""), u32::MAX));
        out.append(&mut on_shift);
    } else if ctrl && has_controls {
        out.push((Text::literal(""), u32::MAX));
        out.append(&mut on_ctrl);
    }
    Some(out)
}

/// `TooltipHelper.makeProgressBar`.
fn bar(filled: usize) -> String {
    format!(
        " {}{} ",
        "\u{2588}".repeat(filled),
        "\u{2592}".repeat(3 - filled)
    )
}

/// `KineticStats.getKineticStats` for a block item whose block is Create's.
fn kinetic_stats(host: &dyn ClientHost, item: mod_api::ItemKey) -> Vec<(Text, u32)> {
    let Some(block) = host.item_block(item) else {
        return Vec::new();
    };
    let id = block.id(host);
    let Some(mut block) = id
        .strip_prefix("create:")
        .and_then(create_engine::Block::from_name)
    else {
        return Vec::new();
    };
    // Valve handles count as the copper one.
    if block.name().ends_with("valve_handle") {
        block = create_engine::Block::CopperValveHandle;
    }
    let goggles = host.local_player().is_some_and(|p| {
        let head = host
            .living(p.entity())
            .and_then(|l| l.equipment(host, EquipmentSlot::Head));
        is(head.as_ref(), Item::Goggles)
    });
    let tr = |k: &str| host.translate(&format!("create.{k}")).unwrap_or_default();
    let rpm = tr("generic.unit.rpm");
    let mut out = Vec::new();
    // (level, absolute colour) of `IRotate.StressImpact`: LOW yellow, MEDIUM gold, HIGH red.
    const COLOURS: [&str; 3] = ["yellow", "gold", "red"];
    const NAMES: [&str; 3] = ["low", "medium", "high"];
    let impact = block.impact();
    if impact > 0.0 {
        out.push(line(vec![styled(tr("tooltip.stressImpact"), "gray")]));
        let level = if impact >= HIGH_IMPACT {
            2
        } else if impact >= MEDIUM_IMPACT {
            1
        } else {
            0
        };
        let tail = if goggles {
            Text::literal(format!("{}x {rpm}", number(impact)))
        } else {
            Text::literal(tr(&format!("tooltip.stressImpact.{}", NAMES[level])))
        };
        out.push(line(vec![styled(bar(level + 1), COLOURS[level]), tail]));
    }
    let capacity = block.capacity();
    if capacity > 0.0 {
        out.push(line(vec![styled(tr("tooltip.capacityProvided"), "gray")]));
        let level = if capacity >= HIGH_CAPACITY {
            2
        } else if capacity >= MEDIUM_CAPACITY {
            1
        } else {
            0
        };
        // The bar takes the opposite impact's colour: much capacity is good.
        let tail = if goggles {
            Text::literal(format!("{}x {rpm}", number(capacity)))
        } else {
            Text::literal(tr(&format!("tooltip.capacityProvided.{}", NAMES[level])))
        };
        out.push(line(vec![styled(bar(level + 1), COLOURS[2 - level]), tail]));
        if goggles && let Some((speed, less)) = generated_rpm(block) {
            let amount = format!("{}{}", number(capacity * speed), tr("generic.unit.stress"));
            let amount = if less {
                tr("tooltip.up_to").replace("%1$s", &amount)
            } else {
                amount
            };
            out.push(line(vec![styled(format!(" -> {amount}"), "dark_gray")]));
        }
    }
    out
}

/// `BlockStressValues.RPM`: the speed a source generates, and whether it may generate less.
fn generated_rpm(block: create_engine::Block) -> Option<(f32, bool)> {
    use create_engine::Block as B;
    Some(match block {
        B::CreativeMotor => (256.0, true),
        B::WaterWheel => (8.0, false),
        B::LargeWaterWheel => (4.0, false),
        B::HandCrank | B::CopperValveHandle => (32.0, false),
        B::SteamEngine => (64.0, true),
        B::WindmillBearing => (16.0, true),
        _ => return None,
    })
}

/// `CreateLang.number`: up to two decimals, thousands grouped.
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

/// `ClientEvents.addToItemTooltip` for Create's items: the description after the name, the
/// kinetic stats after a blank line at the end.
pub(super) fn item_tooltip(host: &dyn ClientHost, event: &mut ItemTooltipEvent<'_>) {
    let item = event.stack.item();
    let Some(key) = description_key(host, item) else {
        return;
    };
    if let Some(lines) = description(host, &format!("{key}.tooltip")) {
        let at = 1.min(event.lines.len());
        event.lines.splice(at..at, lines);
    }
    let stats = kinetic_stats(host, item);
    if !stats.is_empty() {
        event.lines.push((Text::literal(""), u32::MAX));
        event.lines.extend(stats);
    }
}
