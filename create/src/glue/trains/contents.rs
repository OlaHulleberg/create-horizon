//! What the block entities on a carriage or a cart's contraption show while they ride
//! (`ContraptionEntityRenderer` drawing the contraption's block entities): the blocks are block
//! displays (`visuals`), so what a renderer draws over its block is item and text displays riding
//! with them, posed as the renderer poses it in the block: a depot's or ejector's stacks
//! (`DepotRenderer`), a nixie tube's characters (`NixieTubeRenderer`) and a sign's lines
//! (`SignRenderer`). Each comes from the block entity's data as the structure took it
//! (`contraptions::actors::shown_entities`).

use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::math::{Mat4, Vec3};
use mod_api::{Compound, Tag};

/// A display riding over a block: its entity type, its own data (`item`, `text` and the rest),
/// and its pose from the block's corner.
pub struct Extra {
    pub kind: &'static str,
    pub nbt: Compound,
    pub pose: Mat4,
}

/// The displays the carriage block numbered `car` (`car_id`) shows from its block entity data
/// `entity`.
pub fn extras(lv: &ServerLevel<'_>, car: u32, entity: &Compound) -> Vec<Extra> {
    let host = &*lv.host;
    let Some(text) = super::car_state(car).and_then(|s| host.block_state_string(s)) else {
        return Vec::new();
    };
    let (name, props) = states::split(&text);
    let prop = |k: &str| prop_of(&props, k);
    if let Some((block, info)) = crate::glue::contraptions::create_info(car) {
        return match block {
            Block::Depot | Block::WeightedEjector => depot(lv, entity),
            b if create_engine::display::is_nixie(b) => {
                let Some(facing) = info.facing else {
                    return Vec::new();
                };
                let Some(face) = prop("double_face").and_then(create_engine::display::Face::read)
                else {
                    return Vec::new();
                };
                nixie(b, face, facing, entity)
            }
            _ => Vec::new(),
        };
    }
    if name.ends_with("_sign") {
        return sign(name, &props, entity);
    }
    Vec::new()
}

fn carried(entity: &Compound) -> Option<&Compound> {
    match entity.get(&crate::glue::contraptions::actors::SHOWN_CARRIED.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// An item display of one of `id`.
fn item(id: &str, pose: Mat4) -> Extra {
    let stack = Compound::from([
        ("id".into(), Tag::String(id.into())),
        ("count".into(), Tag::Int(1)),
    ]);
    Extra {
        kind: "minecraft:item_display",
        nbt: Compound::from([
            ("item".into(), Tag::Compound(stack)),
            ("item_display".into(), Tag::String("fixed".into())),
        ]),
        pose,
    }
}

/// `DepotRenderer.renderItemsOf` at rest: the held stack at its offset, turned by its angle, and
/// each output 0.35 out at 45° steps.
fn depot(lv: &ServerLevel<'_>, entity: &Compound) -> Vec<Extra> {
    use create_engine::depot;
    let Some(Tag::List(items)) = entity.get(&"Items".into()) else {
        return Vec::new();
    };
    let angle = match carried(entity)
        .and_then(|c| c.get(&crate::glue::logistics::carried::DEPOT_ANGLE.into()))
    {
        Some(Tag::Int(a)) => *a,
        _ => 0,
    };
    let held = depot::Held::new(angle, false);
    let mut out = Vec::new();
    for t in items {
        let Tag::Compound(c) = t else { continue };
        let slot = match c.get(&"Slot".into()) {
            Some(Tag::Byte(b)) => *b as u8,
            _ => 0,
        };
        let Some(Tag::String(id)) = c.get(&"id".into()) else {
            continue;
        };
        let id = id.to_string_lossy();
        let count = match c.get(&"count".into()) {
            Some(Tag::Int(n)) => (*n).clamp(1, 255) as u32,
            _ => 1,
        };
        let solid = crate::glue::logistics::items::is_block_item(&*lv.host, &id);
        let package = create_engine::package::is_package(create_engine::arm::item_id(&id));
        let (poses, phase, offset) = if slot == 0 {
            let poses = depot::stack_poses(count, solid, package, &mut depot::JavaRandom::new(0));
            (poses, angle as f32, held.offset())
        } else {
            let (poses, phase) = depot::output_place(usize::from(slot) - 1, count, solid, package);
            (poses, phase, Vec3::new(0.0, 0.0, 0.0))
        };
        let pivot = Vec3::new(0.5, 0.0, 0.5);
        let turn = Mat4::from_translation(offset)
            * Mat4::from_translation(pivot)
            * Mat4::from_rotation_y(phase.to_radians())
            * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
        for p in poses {
            out.push(item(&id, turn * depot::top_centre() * p));
        }
    }
    out
}

/// A text display's pose for text laid out as the game's `Font` lays it under `pose` (font pixels,
/// y down), its lines' tops from `top` and each centred on x 0: the display draws its lines
/// centred, the first `height` pixels above its origin, at a 40th of a block
/// (`TextDisplayRenderer`).
fn text_pose(pose: Mat4, lines: usize, top: f32) -> Mat4 {
    let height = lines as f32 * 10.0 - 1.0;
    pose * Mat4::from_translation(Vec3::new(-1.0, height + top, 0.0))
        * Mat4::from_scale(Vec3::new(40.0, -40.0, 40.0))
}

/// A text display showing `text` (a component) in 0xRRGGBB `color`, full bright or not.
fn text(text: Tag, color: u32, bright: bool, pose: Mat4) -> Extra {
    let mut root = match text {
        Tag::Compound(c) => c,
        other => Compound::from([("text".into(), other)]),
    };
    root.insert(
        "color".into(),
        Tag::String(format!("#{:06x}", color & 0xFF_FFFF).as_str().into()),
    );
    let mut nbt = Compound::from([
        ("text".into(), Tag::Compound(root)),
        ("background".into(), Tag::Int(0)),
    ]);
    if bright {
        nbt.insert(
            "brightness".into(),
            Tag::Compound(Compound::from([
                ("sky".into(), Tag::Int(15)),
                ("block".into(), Tag::Int(15)),
            ])),
        );
    }
    Extra {
        kind: "minecraft:text_display",
        nbt,
        pose,
    }
}

/// `NixieTubeRenderer`: each tube's character, its shadow, and both again on the back, glowing.
fn nixie(
    block: Block,
    face: create_engine::display::Face,
    facing: create_engine::kinetics::Dir,
    entity: &Compound,
) -> Vec<Extra> {
    let Some(Tag::List(shown)) =
        carried(entity).and_then(|c| c.get(&crate::glue::display::carried::SHOWN.into()))
    else {
        return Vec::new();
    };
    let ch = |i: usize| match shown.get(i) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let Some(i) = create_engine::dye::NIXIE_TUBES
        .iter()
        .position(|b| *b == block)
    else {
        return Vec::new();
    };
    let colour = create_engine::dye::DyeColor::ALL[i];
    // Laid out with no width: each character is centred where the renderer centres it.
    create_engine::display::nixie_texts(face, facing, colour, &ch(0), &ch(1), |_| 0.0)
        .into_iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| {
            text(
                Tag::String(t.text.as_str().into()),
                t.color,
                true,
                text_pose(t.pose, 1, 0.0),
            )
        })
        .collect()
}

/// `SignRenderer.renderSignText` for both sides of a standing, wall or hanging sign: its four
/// lines centred, in the side's dark colour (or its colour, glowing).
fn sign(name: &str, props: &[(&str, &str)], entity: &Compound) -> Vec<Extra> {
    let prop = |k: &str| prop_of(props, k);
    let hanging = name.ends_with("_hanging_sign");
    let wall = name.contains("_wall_");
    let angle = if wall {
        match prop("facing") {
            Some("west") => 90.0f32,
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
    let t = |x: f32, y: f32, z: f32| Mat4::from_translation(Vec3::new(x, y, z));
    let half = Mat4::from_rotation_y(std::f32::consts::PI);
    let mut out = Vec::new();
    for (front, key) in [(true, "front_text"), (false, "back_text")] {
        let Some(Tag::Compound(side)) = entity.get(&key.into()) else {
            continue;
        };
        let Some(Tag::List(messages)) = side.get(&"messages".into()) else {
            continue;
        };
        let empty = |m: &Tag| matches!(m, Tag::String(s) if s.to_string_lossy().is_empty());
        if messages.iter().all(empty) {
            continue;
        }
        let mut extra = Vec::new();
        for (i, m) in messages.iter().take(4).enumerate() {
            if i > 0 {
                extra.push(Tag::Compound(Compound::from([(
                    "text".into(),
                    Tag::String("\n".into()),
                )])));
            }
            extra.push(match m {
                Tag::Compound(c) => Tag::Compound(c.clone()),
                other => Tag::Compound(Compound::from([("text".into(), other.clone())])),
            });
        }
        let line = Compound::from([
            ("text".into(), Tag::String("".into())),
            ("extra".into(), Tag::List(extra)),
        ]);
        let colour = match side.get(&"color".into()) {
            Some(Tag::String(c)) => text_colour(&c.to_string_lossy()),
            _ => 0,
        };
        let glowing = matches!(side.get(&"has_glowing_text".into()), Some(Tag::Byte(b)) if *b != 0);
        let shown = if glowing {
            colour
        } else {
            let s = |shift: u32| ((((colour >> shift) & 255) as f32 * 0.4) as u32) << shift;
            s(16) | s(8) | s(0)
        };
        let back = if front { Mat4::IDENTITY } else { half };
        let pose = if hanging {
            let s = 0.0140625;
            t(0.5, 0.9375, 0.5)
                * Mat4::from_rotation_y((-angle).to_radians())
                * t(0.0, -0.3125, 0.0)
                * back
                * t(0.0, -0.32, 0.073)
                * Mat4::from_scale(Vec3::new(s, -s, s))
        } else {
            let s = 0.010416667;
            let wall = if wall {
                t(0.0, -0.3125, -0.4375)
            } else {
                Mat4::IDENTITY
            };
            t(0.5, 0.5, 0.5)
                * Mat4::from_rotation_y((-angle).to_radians())
                * wall
                * back
                * t(0.0, 1.0 / 3.0, 0.046666667)
                * Mat4::from_scale(Vec3::new(s, -s, s))
        };
        let line_height: f32 = if hanging { 9.0 } else { 10.0 };
        let top = -(4.0 * line_height / 2.0).floor();
        out.push(text(
            Tag::Compound(line),
            shown,
            glowing,
            text_pose(pose, 4, top),
        ));
    }
    out
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

fn prop_of<'a>(props: &[(&str, &'a str)], k: &str) -> Option<&'a str> {
    props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
}
