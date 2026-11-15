//! Vanilla block entities riding a moving structure, as `BlockEntityRenderHelper
//! .renderBlockEntities` draws them with the vanilla renderers: a sign's text
//! (`SignRenderer.renderSignText`, `StandingSignRenderer`/`HangingSignRenderer
//! .textTransformation`) and a banner (`BannerRenderer`: pole, flag, base colour and patterns).
//! The sign's board and a bed are their block models, drawn with the other blocks.

use mod_api::{BlockEntityDrawContext, ClientHost, Compound, ModelPart, Tag};
use std::cell::RefCell;

/// The block entity data keys a riding sign or banner is drawn from (`Contraption.updateTags`).
pub(super) const SHOWN_KEYS: [&str; 3] = ["front_text", "back_text", "patterns"];

/// Whether block `id` is one of those drawn here.
pub(super) fn draws(id: &str) -> bool {
    id.ends_with("_sign") || id.ends_with("_banner")
}

thread_local! {
    /// The banner's layers (`standing_banner#main`, `#flag`, `wall_banner#main`, `#flag`),
    /// baked once models are (`ModelLayers.STANDING_BANNER` and the rest).
    static BANNER: RefCell<Option<[ModelPart; 4]>> = const { RefCell::new(None) };
}

/// `BannerRenderer`'s layers, baked when models are.
pub(super) fn models_baked(host: &dyn ClientHost) {
    let layers = [
        "minecraft:standing_banner#main",
        "minecraft:standing_banner#flag",
        "minecraft:wall_banner#main",
        "minecraft:wall_banner#flag",
    ]
    .map(|l| host.bake_layer(l));
    let baked = match layers {
        [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
        _ => {
            crate::create_log!("[create] the banner's model layers are not there");
            None
        }
    };
    BANNER.with_borrow_mut(|b| *b = baked);
}

/// Draws the sign or banner `id` (with `props`) from its block entity data `nbt`, under the
/// pose of its block's corner.
pub(super) fn draw(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    id: &str,
    props: &[(&str, &str)],
    nbt: &Compound,
) {
    let prop = |k: &str| props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
    let wall = id.contains("_wall_");
    let angle = if wall {
        match prop("facing") {
            Some("west") => 90.0,
            Some("north") => 180.0,
            Some("east") => 270.0,
            _ => 0.0,
        }
    } else {
        prop("rotation")
            .and_then(|r| r.parse::<f32>().ok())
            .unwrap_or(0.0)
            * 22.5
    };
    if id.ends_with("_sign") {
        sign(host, draw, id.ends_with("_hanging_sign"), wall, angle, nbt);
    } else if id.ends_with("_banner") {
        let colour = id
            .strip_prefix("minecraft:")
            .and_then(|n| n.split("_wall_banner").next())
            .and_then(|n| n.strip_suffix("_banner").or(Some(n)))
            .map_or(0xF9FFFE, dye_colour);
        banner(draw, wall, angle, colour, nbt);
    }
}

/// `DyeColor.getTextColor`.
fn text_colour(name: &str) -> u32 {
    match name {
        "white" => 0xFFFFFF,
        "orange" => 0xFF681F,
        "magenta" => 0xFF00FF,
        "light_blue" => 0x9AC0CD,
        "yellow" => 0xFFFF00,
        "lime" => 0xBFFF00,
        "pink" => 0xFF69B4,
        "gray" => 0x808080,
        "light_gray" => 0xD3D3D3,
        "cyan" => 0x00FFFF,
        "purple" => 0xA020F0,
        "blue" => 0x0000FF,
        "brown" => 0x8B4513,
        "green" => 0x00FF00,
        "red" => 0xFF0000,
        _ => 0x000000,
    }
}

/// `DyeColor.getTextureDiffuseColor`.
fn dye_colour(name: &str) -> u32 {
    match name {
        "orange" => 0xF9801D,
        "magenta" => 0xC74EBD,
        "light_blue" => 0x3AB3DA,
        "yellow" => 0xFED83D,
        "lime" => 0x80C71F,
        "pink" => 0xF38BAA,
        "gray" => 0x474F52,
        "light_gray" => 0x9D9D97,
        "cyan" => 0x169C9C,
        "purple" => 0x8932B8,
        "blue" => 0x3C44AA,
        "brown" => 0x835432,
        "green" => 0x5E7C16,
        "red" => 0xB02E26,
        "black" => 0x1D1D21,
        _ => 0xF9FFFE,
    }
}

/// A component as NBT (`ComponentSerialization.CODEC`), written as JSON for the text reader.
fn json(tag: &Tag, out: &mut String) {
    match tag {
        Tag::String(s) => {
            out.push('"');
            for c in s.to_string_lossy().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Tag::Byte(b) => out.push_str(if *b != 0 { "true" } else { "false" }),
        Tag::Short(v) => out.push_str(&v.to_string()),
        Tag::Int(v) => out.push_str(&v.to_string()),
        Tag::Long(v) => out.push_str(&v.to_string()),
        Tag::Float(v) => out.push_str(&f32::from_bits(*v).to_string()),
        Tag::Double(v) => out.push_str(&f64::from_bits(*v).to_string()),
        Tag::List(l) => {
            out.push('[');
            for (i, t) in l.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json(t, out);
            }
            out.push(']');
        }
        Tag::Compound(c) => {
            out.push('{');
            for (i, (k, v)) in c.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json(&Tag::String(k.clone()), out);
                out.push(':');
                json(v, out);
            }
            out.push('}');
        }
        Tag::IntArray(v) => out.push_str(&format!("{v:?}")),
        _ => out.push_str("\"\""),
    }
}

/// `SignRenderer.renderSignText` for both sides: each line centred, `lineHeight` apart around
/// the middle, in the side's colour darkened (`getDarkColor`) or, glowing, in its colour full
/// bright with an outline (black text glowing: ringed with its dark colour).
fn sign(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    hanging: bool,
    wall: bool,
    angle: f32,
    nbt: &Compound,
) {
    let line_height = if hanging { 9.0 } else { 10.0 };
    let max_width = if hanging { 60.0 } else { 90.0 };
    let midpoint = (4.0 * line_height / 2.0f32).floor();
    for (front, key) in [(true, "front_text"), (false, "back_text")] {
        let Some(Tag::Compound(side)) = nbt.get(&key.into()) else {
            continue;
        };
        let Some(Tag::List(messages)) = side.get(&"messages".into()) else {
            continue;
        };
        let lines: Vec<mod_api::Text> = messages
            .iter()
            .take(4)
            .map(|m| {
                let mut s = String::new();
                json(m, &mut s);
                host.text_from_json(&s)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| mod_api::Text::literal(""))
            })
            .collect();
        if lines
            .iter()
            .all(|t| draw.text_width(t).unwrap_or(0.0) == 0.0)
        {
            continue;
        }
        let colour = match side.get(&"color".into()) {
            Some(Tag::String(c)) => text_colour(&c.to_string_lossy()),
            _ => 0,
        };
        let glowing = matches!(side.get(&"has_glowing_text".into()), Some(Tag::Byte(b)) if *b != 0);
        let dark = if colour == 0 && glowing {
            0xF0EBCC
        } else {
            let s = |shift: u32| ((((colour >> shift) & 255) as f32 * 0.4) as u32) << shift;
            s(16) | s(8) | s(0)
        };
        let (shown, outline) = if glowing {
            (colour, (colour == 0).then_some(dark))
        } else {
            (dark, None)
        };
        draw.push();
        if hanging {
            draw.translate([0.5, 0.9375, 0.5]);
            draw.rotate_about([0.0, 1.0, 0.0], -angle, [0.0; 3]);
            draw.translate([0.0, -0.3125, 0.0]);
            if !front {
                draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
            }
            draw.translate([0.0, -0.32, 0.073]);
            draw.scale([0.0140625, -0.0140625, 0.0140625]);
        } else {
            draw.translate([0.5; 3]);
            draw.rotate_about([0.0, 1.0, 0.0], -angle, [0.0; 3]);
            if wall {
                draw.translate([0.0, -0.3125, -0.4375]);
            }
            if !front {
                draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
            }
            draw.translate([0.0, 1.0 / 3.0, 0.046666667]);
            draw.scale([0.010416667, -0.010416667, 0.010416667]);
        }
        for (i, line) in lines.iter().enumerate() {
            let width = draw.text_width(line).unwrap_or(0.0).min(max_width);
            if width == 0.0 {
                continue;
            }
            let x = -((width.ceil() as i32) / 2) as f32;
            let y = i as f32 * line_height - midpoint;
            let _ = draw.text(line, [x, y], shown, glowing, outline);
        }
        draw.pop();
    }
}

/// `BannerRenderer.renderPatterns` under the banner's pose: the pole (none on a wall), then the
/// flag in the banner base, its colour and each pattern (`patterns`, up to 16) in its dye.
fn banner(
    draw: &mut dyn BlockEntityDrawContext,
    wall: bool,
    angle: f32,
    colour: u32,
    nbt: &Compound,
) {
    BANNER.with_borrow(|b| {
        let Some(layers) = b else { return };
        let (main, flag) = if wall {
            (&layers[2], &layers[3])
        } else {
            (&layers[0], &layers[1])
        };
        draw.push();
        draw.translate([0.5, 0.0, 0.5]);
        draw.rotate_about([0.0, 1.0, 0.0], -angle, [0.0; 3]);
        draw.scale([2.0 / 3.0, -2.0 / 3.0, -2.0 / 3.0]);
        const BASE: &str = "entity/banner/banner_base";
        draw.model_part(main, BASE, 0xFFFFFF);
        draw.model_part(flag, BASE, 0xFFFFFF);
        draw.model_part(flag, "entity/banner/base", colour);
        if let Some(Tag::List(patterns)) = nbt.get(&"patterns".into()) {
            for p in patterns.iter().take(16) {
                let Tag::Compound(p) = p else { continue };
                let (Some(Tag::String(pattern)), Some(Tag::String(c))) =
                    (p.get(&"pattern".into()), p.get(&"color".into()))
                else {
                    continue;
                };
                let pattern = pattern.to_string_lossy();
                let name = pattern.strip_prefix("minecraft:").unwrap_or(&pattern);
                let sprite = format!("entity/banner/{name}");
                draw.model_part(flag, &sprite, dye_colour(&c.to_string_lossy()));
            }
        }
        draw.pop();
    });
}
