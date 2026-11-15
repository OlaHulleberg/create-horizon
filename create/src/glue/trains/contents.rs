//! What the block entities on a carriage or a cart's contraption show while they ride
//! (`ContraptionEntityRenderer` drawing the contraption's block entities): the blocks are block
//! displays (`visuals`), so what a renderer draws over its block is item and text displays riding
//! with them, posed as the renderer poses it in the block: a depot's or ejector's stacks
//! (`DepotRenderer`), a chute's stack, a belt (its halves and pulleys, undyed, and its items:
//! `BeltRenderer`), a basin's items
//! (`BasinRenderer`), a crafter's grid (`MechanicalCrafterRenderer`), a nixie tube's characters
//! (`NixieTubeRenderer`), a display board's lines (`FlapDisplayRenderer`), a sign's lines
//! (`SignRenderer`) and a banner (its item, `BannerRenderer`'s model with its patterns). Each
//! comes from the block entity's data as the structure took it
//! (`contraptions::actors::shown_entities`). The items are found by the renderers' own code
//! drawing into a [`Recorder`], which keeps each item and where it was drawn.
//!
//! What the API cannot do, left out (API_GAPS.md): fluids (tanks, basins, spouts, drains), a
//! dyed belt's colour and a cased belt's casing (models with a sprite swapped), and a wall
//! banner's own model (drawn as its item, the standing banner).
//!
//! `create_debug station <x> <y> <z> <name N | assemble | exit | disassemble | go N>`
//! (`station_screen::debug_command`) does what a station's screen does, for tests.

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
            Block::Depot
            | Block::WeightedEjector
            | Block::Chute
            | Block::SmartChute
            | Block::Belt => {
                let mut r = Recorder::new(host);
                if block == Block::Belt {
                    crate::glue::logistics::client::draw_riding_segment(
                        &mut r, block, &info, [0; 3],
                    );
                }
                crate::glue::logistics::client::draw_riding_items(&mut r, host, block, entity);
                r.out
            }
            Block::Basin | Block::MechanicalCrafter => {
                let mut r = Recorder::new(host);
                crate::glue::processing::client::draw_riding_items(
                    &mut r,
                    host,
                    block,
                    info.facing,
                    prop("pointing"),
                    entity,
                );
                r.out
            }
            Block::DisplayBoard => board(info.facing, entity),
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
    if name.ends_with("_banner") {
        return banner(name, &props, entity);
    }
    Vec::new()
}

/// A draw context that draws nothing and keeps each item and block drawn into it, as an item or
/// block display, at the pose it was drawn at (from the block's corner). Its parts stand still,
/// at rest.
struct Recorder<'a> {
    host: &'a dyn mod_api::Host,
    stack: Vec<Mat4>,
    out: Vec<Extra>,
}

impl<'a> Recorder<'a> {
    fn new(host: &'a dyn mod_api::Host) -> Self {
        Recorder {
            host,
            stack: vec![Mat4::IDENTITY],
            out: Vec::new(),
        }
    }
    fn top(&mut self) -> &mut Mat4 {
        self.stack.last_mut().expect("the block's own pose")
    }
    fn apply(&mut self, m: Mat4) {
        let t = self.top();
        *t *= m;
    }
}

/// `degrees` about the unit `axis`.
fn rotation(axis: [f32; 3], degrees: f32) -> Mat4 {
    let q = create_engine::math::Quat::from_axis_angle(
        Vec3::new(axis[0], axis[1], axis[2]),
        degrees.to_radians(),
    );
    let c = |v: Vec3| {
        let r = q * v;
        [r.x, r.y, r.z, 0.0]
    };
    Mat4::from_cols_array_2d(&[
        c(Vec3::new(1.0, 0.0, 0.0)),
        c(Vec3::new(0.0, 1.0, 0.0)),
        c(Vec3::new(0.0, 0.0, 1.0)),
        [0.0, 0.0, 0.0, 1.0],
    ])
}

impl mod_api::BlockEntityDrawContext for Recorder<'_> {
    fn push(&mut self) {
        let t = *self.top();
        self.stack.push(t);
    }
    fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }
    fn translate(&mut self, by: [f32; 3]) {
        self.apply(Mat4::from_translation(Vec3::new(by[0], by[1], by[2])));
    }
    fn rotate_about(&mut self, axis: [f32; 3], degrees: f32, pivot: [f32; 3]) {
        let p = Vec3::new(pivot[0], pivot[1], pivot[2]);
        self.apply(
            Mat4::from_translation(p)
                * rotation(axis, degrees)
                * Mat4::from_translation(Vec3::new(-p.x, -p.y, -p.z)),
        );
    }
    fn scale(&mut self, by: [f32; 3]) {
        self.apply(Mat4::from_scale(Vec3::new(by[0], by[1], by[2])));
    }
    fn keep(&mut self) {}
    fn partial(&self) -> f32 {
        0.0
    }
    fn game_time(&self) -> i64 {
        0
    }
    fn animation_time(&self) -> f64 {
        0.0
    }
    fn light(&self) -> u32 {
        0xF0F0
    }
    fn set_light(&mut self, _light: u32) {}
    fn quad(&mut self, _: &str, _: [[f32; 3]; 4], _: [f32; 4], _: u32) -> bool {
        false
    }
    fn cube(
        &mut self,
        _: [f32; 3],
        _: [f32; 3],
        _: [Option<mod_api::CubeFace<'_>>; 6],
        _: u32,
    ) -> bool {
        false
    }
    fn block(&mut self, block: mod_api::BlockKey, properties: &[(&str, &str)]) -> bool {
        let mut state = Compound::from([(
            "Name".into(),
            Tag::String(block.id(self.host).as_str().into()),
        )]);
        if !properties.is_empty() {
            let props = properties
                .iter()
                .map(|(k, v)| ((*k).into(), Tag::String((*v).into())))
                .collect();
            state.insert("Properties".into(), Tag::Compound(props));
        }
        let pose = *self.top();
        self.out.push(Extra {
            kind: "minecraft:block_display",
            nbt: Compound::from([("block_state".into(), Tag::Compound(state))]),
            pose,
        });
        true
    }
    fn item(
        &mut self,
        stack: &mod_api::ItemStack,
        display: mod_api::ItemDisplayContext,
        _owner_yaw: Option<f32>,
        _seed: i32,
    ) -> bool {
        let Some(mut nbt) = stack.to_nbt(self.host) else {
            return false;
        };
        nbt.insert("count".into(), Tag::Int(1));
        let context = match display {
            mod_api::ItemDisplayContext::Fixed => "fixed",
            mod_api::ItemDisplayContext::Ground => "ground",
            mod_api::ItemDisplayContext::Gui => "gui",
            _ => "none",
        };
        let pose = *self.top();
        self.out.push(Extra {
            kind: "minecraft:item_display",
            nbt: Compound::from([
                ("item".into(), Tag::Compound(nbt)),
                ("item_display".into(), Tag::String(context.into())),
            ]),
            pose,
        });
        true
    }
    fn spin(&mut self, axis: [f32; 3], pivot: [f32; 3], _: f32, phase: f32) {
        self.push();
        self.rotate_about(axis, phase, pivot);
    }
    fn still(&mut self) {
        self.pop();
    }
    fn model(&mut self, _: mod_api::ModelRef, _: u32) -> Result<(), mod_api::ModelError> {
        Ok(())
    }
    fn terrain_model(
        &mut self,
        _: mod_api::ModelRef,
        _: mod_api::TerrainModelOptions<'_>,
    ) -> Result<(), mod_api::TerrainModelError> {
        Ok(())
    }
    fn text(
        &mut self,
        _: &mod_api::Text,
        _: [f32; 2],
        _: u32,
        _: bool,
        _: Option<u32>,
    ) -> Result<f32, mod_api::TextFormError> {
        Ok(0.0)
    }
    fn text_width(&self, _: &mod_api::Text) -> Result<f32, mod_api::TextFormError> {
        Ok(0.0)
    }
    fn model_part(&mut self, _: &mod_api::ModelPart, _: &str, _: u32) -> bool {
        false
    }
    fn fluid_box(
        &mut self,
        _: &mod_api::FluidStack,
        _: [f32; 6],
        _: mod_api::FluidBoxOptions,
    ) -> Result<(), mod_api::FluidBoxError> {
        Ok(())
    }
}

/// `FlapDisplayRenderer` for a controlling board: each line's flaps as they stood when the
/// structure took it, settled (a riding board has no speed).
fn board(facing: Option<create_engine::kinetics::Dir>, entity: &Compound) -> Vec<Extra> {
    use create_engine::display;
    let Some(facing) = facing else {
        return Vec::new();
    };
    let Some(b) = carried(entity).and_then(crate::glue::display::carried::board_of) else {
        return Vec::new();
    };
    let lines: Vec<(String, Vec<display::ShownSection>)> = b
        .lines
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            let sections = l
                .sections
                .into_iter()
                .map(|mut s| {
                    s.transition = false;
                    display::ShownSection::new(s, 1)
                })
                .collect();
            (l.key, sections)
        })
        .collect();
    let options = vec![Vec::new(); display::Cycle::ALL.len()];
    // Laid out with no width: each flap's character is centred where the renderer centres it.
    display::board_texts(
        facing,
        b.x_size,
        &lines,
        &b.colour,
        &b.glowing,
        &options,
        false,
        0,
        |_| 0.0,
    )
    .into_iter()
    .filter(|t| !t.text.trim().is_empty())
    .map(|t| {
        text(
            Tag::String(t.text.as_str().into()),
            t.color,
            t.full_bright,
            text_pose(t.pose, 1, 0.0),
        )
    })
    .collect()
}

/// `BannerRenderer`: the banner's item (the same model, with its patterns), stood as the block
/// stands, turned by its rotation; a wall banner hung on its wall, without its pole.
fn banner(name: &str, props: &[(&str, &str)], entity: &Compound) -> Vec<Extra> {
    let wall = name.contains("_wall_banner");
    let item = name.replace("_wall_banner", "_banner");
    let angle = if wall {
        match prop_of(props, "facing") {
            Some("west") => 90.0f32,
            Some("north") => 180.0,
            Some("east") => 270.0,
            _ => 0.0,
        }
    } else {
        prop_of(props, "rotation")
            .and_then(|r| r.parse::<f32>().ok())
            .unwrap_or(0.0)
            * 22.5
    };
    let mut stack = Compound::from([
        ("id".into(), Tag::String(item.as_str().into())),
        ("count".into(), Tag::Int(1)),
    ]);
    if let Some(p @ Tag::List(_)) = entity.get(&"patterns".into()) {
        stack.insert(
            "components".into(),
            Tag::Compound(Compound::from([(
                "minecraft:banner_patterns".into(),
                p.clone(),
            )])),
        );
    }
    let t = |x: f32, y: f32, z: f32| Mat4::from_translation(Vec3::new(x, y, z));
    // The item's model as authored is the standing banner from the block's corner; a wall
    // banner's sits 5 px lower and 7 px back (`BannerRenderer`'s wall offset).
    let wall_offset = if wall {
        t(0.0, -0.3125, -0.4375)
    } else {
        Mat4::IDENTITY
    };
    let pose = t(0.5, 0.0, 0.5)
        * Mat4::from_rotation_y((-angle).to_radians())
        * wall_offset
        * t(0.0, 0.5, 0.0);
    vec![Extra {
        kind: "minecraft:item_display",
        nbt: Compound::from([
            ("item".into(), Tag::Compound(stack)),
            ("item_display".into(), Tag::String("none".into())),
        ]),
        pose,
    }]
}

fn carried(entity: &Compound) -> Option<&Compound> {
    match entity.get(&crate::glue::contraptions::actors::SHOWN_CARRIED.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
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
