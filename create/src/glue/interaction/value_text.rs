//! A value box's value on the face of the block looked at (`ScrollValueRenderer` with
//! `ValueBox.TextValueBox.renderContents`): the number a creative motor, a rotation speed
//! controller, a valve handle or a brass diode is set to, drawn by the block's own block entity (the kinetic
//! one) on the box of the face looked at, whether the box itself is hovered or not. The client
//! asks the server for the setting while it looks at such a block (the question a board asks,
//! `create:value_settings` with the position alone), every half second; the server sends the
//! block entity again with a count of the looks (`values::LOOKS`), as a kept block entity is drawn
//! anew only when its data changes, and once more when the client looks away.
//!
//! The icon boxes (`IconValueBox`: bearings', pistons', arms', tunnels' modes) are drawn as world
//! quads with the icon atlas (`hud::value_box_outline`). Left out: the ejector's number
//! (its block entity is the logistics one, drawn by another part; the same call would serve it) and the filter slots' items and counts (`FilteringRenderer`).

use super::super::net;
use super::board::block_at;
use super::values::{self, VALUE_SETTINGS};
use create_engine::Block as B;
use create_engine::kinetics::Dir;
use mod_api::text::TextStyle;
use mod_api::{BlockEntityDrawContext, ClientHost, CrosshairTarget, Text};
use std::cell::Cell;

thread_local! {
    /// The block looked at last tick, and when its setting was last asked for.
    static LOOKED: Cell<Option<[i32; 3]>> = const { Cell::new(None) };
    static ASKED: Cell<Option<([i32; 3], u32)>> = const { Cell::new(None) };
    /// The block whose value was drawn last: drawn again once more when it is no longer looked
    /// at, without it.
    static DRAWN: Cell<Option<[i32; 3]>> = const { Cell::new(None) };
}

/// The blocks whose value box shows a number, and how it reads (`withFormatter`).
fn text_of(block: B, value: i32, degrees: &str) -> Option<String> {
    match block {
        // `KineticScrollValueBehaviour`: the speed without its sign.
        B::CreativeMotor | B::RotationSpeedController => Some(value.abs().to_string()),
        // `ValveHandleScrollValueBehaviour`: the angle in degrees.
        b if create_engine::dye::is_valve_handle(b) => Some(format!("{}{degrees}", value.abs())),
        // `BrassDiodeBlockEntity.format`: ticks, seconds or minutes.
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => Some(if value < 60 {
            format!("{value}t")
        } else if value < 20 * 60 {
            format!("{}s", value / 20)
        } else {
            format!("{}m", value / 20 / 60)
        }),
        _ => None,
    }
}

/// The block and face the crosshair is on.
fn looked(host: &dyn ClientHost) -> Option<([i32; 3], Dir)> {
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    Some(([pos.x, pos.y, pos.z], Dir::from_offset(hit.face)?))
}

fn shown_block(host: &dyn ClientHost, pos: [i32; 3]) -> Option<B> {
    let id = block_at(host, pos);
    let block = super::super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()?;
    text_of(block, 0, "").map(|_| block)
}

/// Each client tick: asks for the setting of the block looked at, when it shows one.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let target = looked(host).map(|(p, _)| p);
    let before = LOOKED.replace(target);
    // A block whose value was drawn and is no longer looked at is sent again too, to be drawn
    // without it.
    if let Some(old) = before.filter(|b| Some(*b) != target && DRAWN.get() == Some(*b)) {
        net::send_to_server(host, VALUE_SETTINGS, &values::encode(&old));
    }
    // An option box's icon is drawn from its setting too (`hud::value_box_outline`).
    let icons = |p: [i32; 3]| {
        let id = block_at(host, p);
        super::super::client_table(host, |t| t.get(id).map(|(b, _)| b))
            .flatten()
            .is_some_and(|b| !super::values::option_icons(b).is_empty())
    };
    let Some(pos) = target.filter(|p| shown_block(host, *p).is_some() || icons(*p)) else {
        return;
    };
    let due = match ASKED.get() {
        Some((at, ticks)) if at == pos => {
            ASKED.set(Some((at, ticks + 1)));
            (ticks + 1) % 10 == 0
        }
        _ => {
            ASKED.set(Some((pos, 0)));
            true
        }
    };
    if due {
        net::send_to_server(host, VALUE_SETTINGS, &values::encode(&pos));
    }
}

/// Whether the block entity at `pos` must be drawn again this frame: it is looked at, or its
/// value was drawn and it no longer is.
pub(crate) fn animated(pos: [i32; 3]) -> bool {
    LOOKED.get() == Some(pos) || DRAWN.get() == Some(pos)
}

fn styled(text: String) -> Text {
    Text::literal(text).with_style(TextStyle::default())
}

/// `ValueBox.render` for the value of the block at `pos`, if it is looked at: on the box of the
/// face looked at, the number (`TextValueBox.renderContents`, `drawInBatch8xOutline` in 0xEDEDED
/// ringed with 0x333333, full bright).
pub(crate) fn draw(host: &dyn ClientHost, pos: [i32; 3], draw: &mut dyn BlockEntityDrawContext) {
    let Some((at, side)) = looked(host).filter(|(p, _)| *p == pos) else {
        if DRAWN.get() == Some(pos) {
            DRAWN.set(None);
        }
        return;
    };
    let Some(block) = shown_block(host, at) else {
        return;
    };
    let facing = super::super::client_table(host, |t| {
        t.get(block_at(host, pos)).and_then(|(_, i)| i.facing)
    })
    .flatten();
    let diode = matches!(block, B::PulseRepeater | B::PulseExtender | B::PulseTimer);
    let Some(value) = super::board::seen(pos) else {
        return;
    };
    let degrees = host
        .translate("create.generic.unit.degrees")
        .unwrap_or_else(|| "°".to_owned());
    let Some(text) = text_of(block, value, &degrees) else {
        return;
    };
    // Read before borrowing the table: `client_active` borrows it itself.
    let active = values::client_active(host, pos);
    let Some(slot) = super::super::client_table(host, |t| {
        values::value_box_slot(
            t,
            |p| block_at(host, p),
            crate::glue::logistics::client::ejector_charged,
            pos,
            side,
            active,
        )
    })
    .flatten() else {
        return;
    };
    DRAWN.set(Some(pos));
    let text = styled(text);
    let Ok(width) = draw.text_width(&text) else {
        return;
    };
    // `AngleHelper.horizontalAngle(side) + 180` about y, then 90 (up) or 270 (down) about x; a
    // diode's box (`BrassDiodeScrollSlot`) lies on it, turned by its facing.
    let horizontal = |d: Dir| -> f32 {
        match d {
            Dir::South => 0.0,
            Dir::East => 90.0,
            Dir::North => 180.0,
            Dir::West => 270.0,
            Dir::Up | Dir::Down => 0.0,
        }
    };
    let (horizontal, pitch) = if diode {
        (horizontal(facing.unwrap_or(Dir::North)), 90.0)
    } else {
        let pitch: f32 = match side {
            Dir::Up => 90.0,
            Dir::Down => 270.0,
            _ => 0.0,
        };
        (horizontal(side), pitch)
    };
    draw.push();
    draw.translate([slot[0] as f32, slot[1] as f32, slot[2] as f32]);
    draw.rotate_about([0.0, 1.0, 0.0], horizontal + 180.0, [0.0; 3]);
    draw.rotate_about([1.0, 0.0, 0.0], pitch, [0.0; 3]);
    // `ValueBoxTransform.getScale` (0.5), then `-getFontScale` (1/64).
    let s = 0.5 / 64.0;
    draw.scale([-s, -s, -s]);
    // `TextValueBox.renderContents`.
    draw.scale([3.0, 3.0, 1.0]);
    draw.translate([-4.0, -3.75, 5.0]);
    let line = 9.0;
    let single = width < 10.0;
    let mut number = line / width.max(1.0);
    if single {
        number /= 2.0;
    }
    let margin = (width - line) / 2.0;
    draw.scale([number, number, number]);
    draw.translate([
        if single { (width / 2.0).floor() } else { 0.0 },
        if single { -margin } else { margin },
        0.0,
    ]);
    // A diode's box draws plain text in its own colour (`getOverrideColor`).
    if diode {
        let _ = draw.text(&text, [0.0, 0.0], 0x592424, true, None);
    } else {
        let _ = draw.text(&text, [0.0, 0.0], 0xEDEDED, true, Some(0x333333));
    }
    draw.pop();
}
