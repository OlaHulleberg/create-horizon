//! What the HUD shows of the block under the crosshair: a scroll value's tip over the hotbar
//! (`ValueSettingsClient.render`) and, with goggles on, its kinetic stats
//! (`GoggleOverlayRenderer`).

use super::super::items::{self, Item};
use super::board::{self, block_at};
use create_engine::kinetics::Dir;
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
    // A clipboard in hand tells its actions instead (`ClipboardValueSettingsHandler.clientTick`
    // shows its tip after the box's); a factory gauge's panel shows its own label, stock and tips
    // (`FactoryPanelBehaviour`); another value box its title in Create's yellow, the rest white.
    let tinted = |pos: [i32; 3], tip: Vec<String>| {
        let lines = tip
            .into_iter()
            .enumerate()
            .map(|(i, t)| (t, if i == 0 { 0xFB_DC7D } else { 0xFF_FFFF }))
            .collect::<Vec<(String, u32)>>();
        (pos, lines)
    };
    let hovered = if let Some((pos, tip)) =
        super::super::redstone::clipboard_values::hover_tip(host)
    {
        Some(tinted(pos, tip))
    } else {
        match crate::glue::stock::gauge_client::hover_lines(host) {
            Some(lines) => match host.crosshair_target() {
                Some(CrosshairTarget::Block { pos, .. }) => Some(([pos.x, pos.y, pos.z], lines)),
                _ => None,
            },
            None => board::hovered_box(host).map(|(pos, tip)| tinted(pos, tip)),
        }
    };
    let Some((pos, lines)) = hovered else {
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
/// (`Subsystem::goggle_tooltip`), in a tooltip right of the crosshair; an overstressed network's
/// warning shows with or without them.
pub(super) fn goggle_overlay(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let Some(player) = host.local_player() else {
        return;
    };
    let Some(pos) = aimed(host) else {
        HOVER.set(None);
        return;
    };
    // A value box looked at (its outline highlighted, not passive) hides the overlay.
    if board::hovered_box(host).is_some() {
        HOVER.set(None);
        return;
    }
    let head = host
        .living(player.entity())
        .and_then(|l| l.equipment(host, EquipmentSlot::Head));
    let goggles = items::is(head.as_ref(), Item::Goggles);
    let Some((block, impact, capacity)) = super::super::client_table(host, |t| {
        let (block, info) = t.get(block_at(host, pos))?;
        // A powered shaft's capacity is its engine's share (`calculateAddedStressCapacity`).
        let mut settings = create_engine::parts::Settings::default();
        if block == create_engine::Block::PoweredShaft
            && let Some(d) = host
                .level(host.dimension())
                .and_then(|l| host.block_entity(l, pos))
                .and_then(|e| e.data(host))
        {
            settings.steam = super::super::fluids::engine::shown_steam(&d);
        }
        let [part, _] = create_engine::parts::parts(block, info, &settings);
        Some(part.map_or((block, 0.0, 0.0), |p| (block, p.impact, p.capacity)))
    })
    .flatten() else {
        return;
    };
    // `getTheoreticalSpeed`: what the network turns it at, overstressed or not.
    let (speed, overstressed) = host.level(host.dimension()).map_or((0.0, false), |l| {
        super::super::kinetics::shown_theory(host, l, pos)
    });
    // Create's tooltip: a header, then each stat indented by four spaces and its value by one
    // more; a line may change colour along the way.
    let mut lines: Vec<Vec<(String, u32)>> = Vec::new();
    let at_speed = |su: f32| {
        vec![
            (format!("     {}su", number(su)), 0x55FFFF),
            (" at current speed".to_owned(), 0x555555),
        ]
    };
    if goggles {
        if impact != 0.0 {
            lines.push(vec![("    Kinetic Stats:".into(), 0xFFFFFF)]);
            lines.push(vec![("    Kinetic Stress Impact:".into(), 0xAAAAAA)]);
            lines.push(at_speed(impact * speed.abs()));
        }
        if capacity != 0.0 {
            // `GeneratingKineticBlockEntity.addToGoggleTooltip`: the capacity at the speed it
            // generates (`stressBase *= generated / speed`), none while its network is still.
            let generated = host
                .level(host.dimension())
                .and_then(|l| host.block_entity(l, pos))
                .and_then(|e| e.data(host))
                .and_then(|d| super::super::sources::generated_of(&d))
                .unwrap_or(speed);
            let total = if speed == 0.0 {
                0.0
            } else {
                capacity * generated.abs()
            };
            lines.push(vec![("    Generator Stats:".into(), 0xFFFFFF)]);
            lines.push(vec![("    Kinetic Stress Capacity:".into(), 0xAAAAAA)]);
            lines.push(at_speed(total));
        }
        for s in super::super::SUBSYSTEMS {
            s.goggle_tooltip(host, pos, block, &mut lines);
        }
    }
    // `KineticBlockEntity.addToTooltip` (shown with or without goggles): an overstressed
    // network's warning, after a blank line.
    if overstressed {
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        lines.push(vec![("    Overstressed".into(), 0xFFAA00)]);
        lines.extend(cut_hint(
            draw,
            "It appears that this contraption is _overstressed_. Add more sources or _slow_ \
             _down_ the components with a high _stress_ _impact_.",
        ));
    } else if let Some(required) = create_engine::gauge::minimum_speed(block) {
        // `notFastEnough`: turning (`getSpeed`), but slower than its speed level asks.
        let turning = host.level(host.dimension()).map_or(0.0, |l| {
            super::super::kinetics::shown_speeds(host, l, pos)[0]
        });
        if turning != 0.0 && turning.abs() < required {
            if !lines.is_empty() {
                lines.push(Vec::new());
            }
            lines.push(vec![("    Speed Requirement:".into(), 0xFFAA00)]);
            let name = host
                .translate(&format!("block.create.{}", block.name()))
                .unwrap_or_else(|| block.name().to_owned());
            lines.extend(cut_hint(
                draw,
                &format!("It appears that this _{name}_ is _not_ rotating with _enough_ _speed_."),
            ));
        }
    }
    // `MechanicalBearingBlockEntity.addToTooltip`: a bearing that hasn't taken the structure in
    // front of it says how to (after the overstressed warning, which comes instead).
    if !overstressed && bearing_hint(host, pos, block) {
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        let title = translated(host, "create.hint.empty_bearing.title", "Update Bearing");
        lines.push(vec![(format!("    {title}"), 0xFFAA00)]);
        lines.extend(cut_hint(
            draw,
            &translated(
                host,
                "create.hint.empty_bearing",
                "_Right-click_ the bearing with an _empty_ _hand_ to _attach_ the structure you \
                 just built in front of it.",
            ),
        ));
    }
    // `IDisplayAssemblyExceptions.addExceptionToTooltip`: why its last assembly failed.
    if let Some(text) = assembly_exception(host, pos) {
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        let title = translated(
            host,
            "create.gui.assembly.exception",
            "This Contraption was unable to assemble:",
        );
        lines.push(vec![(format!("    {title}"), 0xFFAA00)]);
        for l in text.split('\n') {
            lines.extend(cut_hint(draw, l));
        }
    }
    // With goggles, a piston's pole tells the length of the poles it is in.
    if goggles && block == create_engine::Block::PistonExtensionPole {
        let Some(poles) = pole_length(host, pos) else {
            HOVER.set(None);
            return;
        };
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        let label = translated(host, "create.gui.goggles.pole_length", "Pole Length:");
        lines.push(vec![(format!("    {label} {poles}"), 0xFFFFFF)]);
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
    draw.fill_gradient([x - 3.0, y - 2.0, 1.0, h + 4.0], top, bottom);
    draw.fill_gradient([x + w + 2.0, y - 2.0, 1.0, h + 4.0], top, bottom);
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

/// `key`'s text in the client's language, or `english`.
fn translated(host: &dyn ClientHost, key: &str, english: &str) -> String {
    host.translate(key).unwrap_or_else(|| english.to_owned())
}

/// The data of the block entity at `pos` on this client.
fn data_at(host: &dyn ClientHost, pos: [i32; 3]) -> Option<mod_api::Compound> {
    let level = host.level(host.dimension())?;
    host.block_entity(level, pos)?.data(host)
}

/// Whether the bearing at `pos` hints at taking its structure (`addToTooltip`): not while the
/// player sneaks, a mechanical bearing only while it turns, not while running, and only with a
/// block in front of it that placing would not replace.
fn bearing_hint(host: &dyn ClientHost, pos: [i32; 3], block: create_engine::Block) -> bool {
    use create_engine::Block as B;
    if !matches!(block, B::MechanicalBearing | B::WindmillBearing) {
        return false;
    }
    let sneaking = host
        .local_player()
        .is_some_and(|p| items::sneaking(host, p));
    if sneaking {
        return false;
    }
    let Some(data) = data_at(host, pos) else {
        return false;
    };
    let speed = match data.get(&super::super::kinetics::SPEEDS.into()) {
        Some(mod_api::Tag::List(l)) => match l.first() {
            Some(mod_api::Tag::Float(bits)) => f32::from_bits(*bits),
            _ => 0.0,
        },
        _ => 0.0,
    };
    if block == B::MechanicalBearing && speed == 0.0 {
        return false;
    }
    let running = matches!(
        super::super::kinetics::synced(&data, super::super::contraptions::draw_key()),
        Some(mod_api::Tag::Compound(_))
    );
    if running {
        return false;
    }
    let Some(facing) = super::super::client_table(host, |t| {
        t.get(block_at(host, pos)).and_then(|(_, i)| i.facing)
    })
    .flatten() else {
        return false;
    };
    let o = facing.offset();
    let front = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let Some(level) = host.level(host.dimension()) else {
        return false;
    };
    level
        .block_state(host, front)
        .and_then(|s| mod_api::HostBlockStates::block_states(host).replaceable(s))
        == Some(false)
}

/// `GoggleOverlayRenderer`'s piston poles: the poles in line with the one at `pos` (on its
/// axis), when a mechanical piston is at either end; None without one.
fn pole_length(host: &dyn ClientHost, pos: [i32; 3]) -> Option<u32> {
    use create_engine::Block as B;
    let at = |p: [i32; 3]| {
        super::super::client_table(host, |t| {
            t.get(block_at(host, p)).map(|(b, i)| (b, i.facing))
        })
        .flatten()
    };
    let axis = at(pos)?.1?.axis();
    let mut poles = 1;
    let mut piston = false;
    for d in create_engine::kinetics::Dir::ALL
        .into_iter()
        .filter(|d| d.axis() == axis)
    {
        let o = d.offset();
        let mut n = 1;
        loop {
            let p = [0, 1, 2].map(|i| pos[i] + o[i] * n);
            match at(p) {
                Some((B::PistonExtensionPole, Some(f))) if f.axis() == axis => n += 1,
                found => {
                    piston |= matches!(
                        found,
                        Some((B::MechanicalPiston | B::StickyMechanicalPiston, _))
                    );
                    break;
                }
            }
        }
        poles += n as u32 - 1;
    }
    piston.then_some(poles)
}

/// The message of the last assembly the block at `pos` failed (`AssemblyException`), its
/// arguments filled in (an unmovable block's name in the client's language).
fn assembly_exception(host: &dyn ClientHost, pos: [i32; 3]) -> Option<String> {
    use mod_api::Tag;
    let data = data_at(host, pos)?;
    let Some(Tag::Compound(c)) =
        super::super::kinetics::synced(&data, super::super::contraptions::LAST_EXCEPTION)
    else {
        return None;
    };
    let text = |k: &str| match c.get(&k.into()) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    };
    let key = text("Key")?;
    let mut args: Vec<String> = match c.get(&"Args".into()) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|t| match t {
                Tag::String(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if let Some(block) = text("Block") {
        let name = block
            .split_once(':')
            .and_then(|(ns, path)| host.translate(&format!("block.{ns}.{path}")))
            .unwrap_or(block);
        args.push(name);
    }
    let mut message = host.translate(&format!("create.gui.assembly.exception.{key}"))?;
    for (i, a) in args.iter().enumerate() {
        message = message.replace(&format!("%{}$s", i + 1), a);
    }
    Some(message)
}

std::thread_local! {
    /// The block the goggles' overlay shows, and the game time it was first shown.
    static HOVER: std::cell::Cell<Option<([i32; 3], i64)>> = const { std::cell::Cell::new(None) };
}

/// `TooltipHelper.cutTextComponent` with `GRAY_AND_WHITE`, as goggle lines: words wrapped at
/// 200 pixels, the parts between underscores white, the rest grey.
fn cut_hint(draw: &dyn DrawContext, text: &str) -> Vec<Vec<(String, u32)>> {
    const MAX_WIDTH_PER_LINE: f32 = 200.0;
    let mut lines: Vec<String> = Vec::new();
    let (mut line, mut width) = (String::new(), 0.0);
    for word in text.split_inclusive(' ') {
        let w = draw.text_width(&word.replace('_', ""));
        if width + w > MAX_WIDTH_PER_LINE {
            if width > 0.0 {
                lines.push(std::mem::take(&mut line));
                width = 0.0;
            } else {
                lines.push(word.to_owned());
                continue;
            }
        }
        line.push_str(word);
        width += w;
    }
    if width > 0.0 {
        lines.push(line);
    }
    let mut highlighted = false;
    lines
        .into_iter()
        .map(|l| {
            let mut out = vec![("    ".to_owned(), 0xAAAAAA)];
            for part in l.split('_') {
                out.push((
                    part.to_owned(),
                    if highlighted { 0xFFFFFF } else { 0xAAAAAA },
                ));
                highlighted = !highlighted;
            }
            highlighted = !highlighted;
            out
        })
        .collect()
}

/// `ValueBox.render` of the highlighted box (`passive(false)`): Create's `VALUE_BOX_HOVER_4PX`
/// frame round the box (the icon atlas through the glue's world pipeline, else white lines),
/// and an option box's icon (`IconValueBox.renderContents`) on the face looked at. A number's
/// box is the block entity's to draw (`value_text`).
pub(super) fn value_box_outline(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    use mod_api::{RenderLevelStage, WorldDepth, WorldLine};
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let mut panel = Vec::new();
    if crate::glue::stock::gauge_client::hover_outline(host, &mut panel) {
        let _ = event.draw.lines(&panel);
        return;
    }
    let icons = crate::glue::pipelines::resolve(host, crate::glue::pipelines::Textured::Icons);
    let mut quads = Vec::new();
    if let Some((pos, side, slot)) = super::board::hovered_slot(host) {
        let frame = BoxFrame::of(host, pos, side, slot);
        match (icons, frame) {
            (Some(_), Some(f)) => {
                // `scale(-2.01, -2.01, 2.01)`, `translate(-8 / 16, -8 / 16, -0.5 / 16)`.
                let at =
                    |x: f64, y: f64| f.world([-2.01 * (x - 0.5), -2.01 * (y - 0.5), -2.01 / 32.0]);
                quads.push(icon_quad(at, VALUE_BOX_HOVER_4PX));
            }
            _ => {
                let n = side.offset();
                let axis = (0..3).find(|i| n[*i] != 0).unwrap_or(1);
                let (u, v) = match axis {
                    0 => (1, 2),
                    1 => (0, 2),
                    _ => (0, 1),
                };
                let centre: [f64; 3] =
                    [0, 1, 2].map(|i| f64::from(pos[i]) + slot[i] + f64::from(n[i]) * 0.002);
                let corner = |a: f64, b: f64| {
                    let mut c = centre;
                    c[u] += a * 0.25;
                    c[v] += b * 0.25;
                    c
                };
                let square = [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ];
                let lines: Vec<WorldLine> = (0..4)
                    .map(|i| {
                        WorldLine::new(
                            square[i],
                            square[(i + 1) % 4],
                            0xFFFF_FFFF,
                            2.0,
                            WorldDepth::Tested,
                        )
                    })
                    .collect();
                let _ = event.draw.lines(&lines);
            }
        }
    }
    // The option's icon on the box of the face looked at (`ScrollValueRenderer` draws the box
    // of the face looked at, hovered or not).
    if let Some(icons) = icons
        && let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target()
        && let Some(side) = Dir::from_offset(hit.face)
    {
        let pos = [pos.x, pos.y, pos.z];
        let block = super::super::client_table(host, |t| {
            t.get(super::board::block_at(host, pos)).map(|(b, _)| b)
        })
        .flatten();
        let cells = block.map_or(&[][..], super::values::option_icons);
        let cell = super::board::seen(pos).and_then(|v| cells.get(usize::try_from(v).ok()?));
        // Read before borrowing the table: `client_active` borrows it itself.
        let active = super::values::client_active(host, pos);
        let slot = super::super::client_table(host, |t| {
            super::values::value_box_slot(
                t,
                |p| super::board::block_at(host, p),
                crate::glue::logistics::client::ejector_charged,
                pos,
                side,
                active,
            )
        })
        .flatten();
        if let (Some(cell), Some(slot)) = (cell, slot)
            && let Some(f) = BoxFrame::of(host, pos, side, slot)
        {
            // `fontScale` (-1/64), then `scale(32)`, `translate(-0.5, -0.5, 5 / 32)`.
            let s = -1.0 / 64.0 * 32.0;
            let at = |x: f64, y: f64| f.world([s * (x - 0.5), s * (y - 0.5), s * 5.0 / 32.0]);
            quads.push(icon_quad(at, *cell));
        }
        if !quads.is_empty() {
            let _ = event.draw.pipeline_quads(icons, &quads);
        }
    } else if let Some(icons) = icons
        && !quads.is_empty()
    {
        let _ = event.draw.pipeline_quads(icons, &quads);
    }
}

/// `AllIcons.VALUE_BOX_HOVER_4PX`'s cell in the icon atlas.
const VALUE_BOX_HOVER_4PX: [f32; 2] = [48.0, 128.0];

/// `AllIcons.render`: the icon at `cell` on the unit square `at` maps, white, full bright.
fn icon_quad(at: impl Fn(f64, f64) -> [f64; 3], cell: [f32; 2]) -> mod_api::PipelineQuad {
    let (u0, v0) = (cell[0] / 256.0, cell[1] / 256.0);
    let (u1, v1) = ((cell[0] + 16.0) / 256.0, (cell[1] + 16.0) / 256.0);
    mod_api::PipelineQuad::new(
        [at(0.0, 0.0), at(0.0, 1.0), at(1.0, 1.0), at(1.0, 0.0)],
        [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
        0xFFFF_FFFF,
    )
}

/// A value box's frame (`ValueBoxTransform.Sided.transform`): the box's centre, turned
/// `yaw` about y and `pitch` about x, at the transform's scale (0.5).
struct BoxFrame {
    centre: [f64; 3],
    yaw: f64,
    pitch: f64,
}

impl BoxFrame {
    fn of(host: &dyn ClientHost, pos: [i32; 3], side: Dir, slot: [f64; 3]) -> Option<BoxFrame> {
        let (block, facing) = super::super::client_table(host, |t| {
            t.get(super::board::block_at(host, pos))
                .map(|(b, i)| (b, i.facing))
        })
        .flatten()?;
        let horizontal = |d: Dir| -> f64 {
            match d {
                Dir::South => 0.0,
                Dir::East => 90.0,
                Dir::North => 180.0,
                Dir::West => 270.0,
                Dir::Up | Dir::Down => 0.0,
            }
        };
        let diode = matches!(
            block,
            create_engine::Block::PulseRepeater
                | create_engine::Block::PulseExtender
                | create_engine::Block::PulseTimer
        );
        let (yaw, pitch) = if diode {
            (horizontal(facing.unwrap_or(Dir::North)), 90.0)
        } else {
            let pitch = match side {
                Dir::Up => 90.0,
                Dir::Down => 270.0,
                _ => 0.0,
            };
            (horizontal(side), pitch)
        };
        Some(BoxFrame {
            centre: [0, 1, 2].map(|i| f64::from(pos[i]) + slot[i]),
            yaw: yaw + 180.0,
            pitch,
        })
    }

    /// A point of the box's own space (before the transform's scale) in the world.
    fn world(&self, local: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = local.map(|v| v * 0.5);
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        let (y1, z1) = (y * cp - z * sp, y * sp + z * cp);
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (x2, z2) = (x * cy + z1 * sy, -x * sy + z1 * cy);
        [
            self.centre[0] + x2,
            self.centre[1] + y1,
            self.centre[2] + z2,
        ]
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
