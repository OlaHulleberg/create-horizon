//! Create's connected textures (`ConnectedTextureBehaviour`, `AllCTTypes`) as Fusion connecting
//! textures, which the game draws. Create's `_connected` sheets are laid out for its own index
//! functions (`AllCTTypes.getTextureIndex`); Fusion's layouts are other grids. At prepare time
//! each sheet is cut into the Fusion layout whose masks stand for the same neighbours: for every
//! Fusion tile, the context it stands for goes through Create's index function and that tile of
//! Create's sheet is copied. The blocks' models are then given child models of type
//! `fusion:connecting` whose texture keys name the new sheets, joined by Create's `connectsTo`.
//!
//! Faces are oriented as Create's `ConnectedTextureBehaviour.buildContext` (a side's up is up,
//! its right as its texture reads from outside; the top's up is north, the bottom's south), which
//! are a cube's default UVs, as Fusion's (`Axes::cube`).
//!
//! Left out: pillars (`RotatedPillarCTBehaviour` turns its axes by the pillar's axis and joins by
//! its `ConnectedPillarBlock` sides), the weathered iron window's position-picked sheets, copper
//! roofs (`ROOF`), metal scaffolding, chassis, the encased blocks' and crafters' state-dependent
//! sides, tanks and vaults (their sheets follow the multiblock's size and the block entity), and
//! casings joining the encased blocks of their casing (`CasingConnectivity`): a casing joins only
//! its own block.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Create's CT types the port's blocks use, with their sheet sizes (`AllCTTypes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtType {
    HorizontalKryppers,
    Vertical,
    Omni,
    Rectangle,
}

/// `CTContext`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ctx {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
}

impl CtType {
    pub fn sheet_size(self) -> u32 {
        match self {
            Self::HorizontalKryppers | Self::Vertical => 2,
            Self::Omni => 8,
            Self::Rectangle => 4,
        }
    }

    /// `getTextureIndex`.
    pub fn index(self, c: Ctx) -> u32 {
        let b = u32::from;
        match self {
            Self::HorizontalKryppers => match (c.left, c.right) {
                (false, false) => 0,
                (_, false) => 3,
                (false, _) => 2,
                _ => 1,
            },
            Self::Vertical => b(c.up) + 2 * b(c.down),
            Self::Rectangle => {
                let x = match (c.left, c.right) {
                    (true, true) => 2,
                    (true, false) => 3,
                    (false, true) => 1,
                    _ => 0,
                };
                let y = match (c.up, c.down) {
                    (true, true) => 1,
                    (true, false) => 2,
                    (false, true) => 0,
                    _ => 3,
                };
                x + y * 4
            }
            Self::Omni => omni(c),
        }
    }

    /// The Fusion layout the sheet becomes.
    fn layout(self) -> Layout {
        match self {
            Self::HorizontalKryppers => Layout::Horizontal,
            Self::Vertical => Layout::Vertical,
            Self::Omni => Layout::Full,
            Self::Rectangle => Layout::Simple,
        }
    }
}

/// `OMNIDIRECTIONAL.getTextureIndex`.
fn omni(c: Ctx) -> u32 {
    let (mut x, mut y): (i32, i32) = (0, 0);
    let borders = [c.up, c.down, c.left, c.right]
        .iter()
        .filter(|j| !**j)
        .count();
    if c.up {
        x += 1;
    }
    if c.down {
        x += 2;
    }
    if c.left {
        y += 1;
    }
    if c.right {
        y += 2;
    }
    if borders == 0 {
        if c.top_right {
            x += 1;
        }
        if c.top_left {
            x += 2;
        }
        if c.bottom_right {
            y += 2;
        }
        if c.bottom_left {
            y += 1;
        }
    }
    let i = i32::from;
    if borders == 1 {
        if !c.right && (c.top_left || c.bottom_left) {
            y = 4;
            x = -1 + i(c.bottom_left) + i(c.top_left) * 2;
        }
        if !c.left && (c.top_right || c.bottom_right) {
            y = 5;
            x = -1 + i(c.bottom_right) + i(c.top_right) * 2;
        }
        if !c.down && (c.top_left || c.top_right) {
            y = 6;
            x = -1 + i(c.top_left) + i(c.top_right) * 2;
        }
        if !c.up && (c.bottom_left || c.bottom_right) {
            y = 7;
            x = -1 + i(c.bottom_left) + i(c.bottom_right) * 2;
        }
    }
    if borders == 2
        && ((c.up && c.left && c.top_left)
            || (c.down && c.left && c.bottom_left)
            || (c.up && c.right && c.top_right)
            || (c.down && c.right && c.bottom_right))
    {
        x += 3;
    }
    (x + 8 * y) as u32
}

// Fusion's neighbour mask bits (`horizon_optifine::ctm::connect`).
const U: u8 = 1;
const UR: u8 = 2;
const R: u8 = 4;
const DR: u8 = 8;
const D: u8 = 16;
const DL: u8 = 32;
const L: u8 = 64;
const UL: u8 = 128;
const SIDES: u8 = U | R | D | L;
const CORNERS: u8 = UR | DR | DL | UL;

/// Fusion's `full` template: each tile's mask, row by row of its 8 x 6 grid. `None` is its one
/// cell no mask draws.
const FULL: [Option<u8>; 48] = [
    Some(0),
    Some(R),
    Some(R | L),
    Some(L),
    Some(R | D),
    Some(D | L),
    Some(U | R | D),
    Some(R | D | L),
    Some(D),
    Some(R | DR | D),
    Some(R | DR | D | DL | L),
    Some(D | DL | L),
    Some(U | R),
    Some(U | L),
    Some(U | R | L),
    Some(U | D | L),
    Some(U | D),
    Some(U | UR | R | DR | D),
    Some(SIDES | CORNERS),
    Some(U | D | DL | L | UL),
    Some(U | UR | R | D),
    Some(R | DR | D | L),
    Some(U | R | DR | D),
    Some(R | D | DL | L),
    Some(U),
    Some(U | UR | R),
    Some(U | UR | R | L | UL),
    Some(U | L | UL),
    Some(U | R | L | UL),
    Some(U | D | DL | L),
    Some(U | UR | R | L),
    Some(U | D | L | UL),
    Some(SIDES | UR | DL),
    Some(SIDES),
    Some(SIDES | UR | DR),
    Some(SIDES | DR | DL),
    Some(SIDES | DR),
    Some(SIDES | DL),
    Some(SIDES | UL | UR | DL),
    Some(SIDES | UL | UR | DR),
    Some(SIDES | UL | DR),
    None,
    Some(SIDES | UL | UR),
    Some(SIDES | UL | DL),
    Some(SIDES | UR),
    Some(SIDES | UL),
    Some(SIDES | UL | DL | DR),
    Some(SIDES | UR | DL | DR),
];

/// Fusion's `simple` template (4 x 4): the sides each tile joins.
const SIMPLE: [u8; 16] = [
    0,
    SIDES,
    D,
    L,
    R | L,
    U | D,
    R,
    U,
    U | R | D,
    R | D | L,
    R | D,
    D | L,
    U | R | L,
    U | D | L,
    U | R,
    U | L,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Full,
    Simple,
    Horizontal,
    Vertical,
}

impl Layout {
    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Simple => "simple",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    /// Columns, rows.
    fn grid(self) -> (u32, u32) {
        match self {
            Self::Full => (8, 6),
            Self::Simple => (4, 4),
            Self::Horizontal => (4, 1),
            Self::Vertical => (1, 4),
        }
    }

    /// The neighbour mask each tile stands for (`None`: never drawn).
    fn masks(self) -> Vec<Option<u8>> {
        // Fusion's straight layouts: alone, joined only toward `b` (a row's left end, a
        // column's top), both ways, only toward `a`.
        let straight = |a: u8, b: u8| vec![Some(0), Some(b), Some(a | b), Some(a)];
        match self {
            Self::Full => FULL.to_vec(),
            Self::Simple => SIMPLE.iter().map(|m| Some(*m)).collect(),
            Self::Horizontal => straight(L, R),
            Self::Vertical => straight(U, D),
        }
    }
}

/// The context a Fusion mask stands for: a corner joins only with both sides beside it, as
/// Create's `buildContext` tests it.
fn context(mask: u8) -> Ctx {
    let has = |b: u8| mask & b != 0;
    let (up, down, left, right) = (has(U), has(D), has(L), has(R));
    Ctx {
        up,
        down,
        left,
        right,
        top_left: up && left && has(UL),
        top_right: up && right && has(UR),
        bottom_left: down && left && has(DL),
        bottom_right: down && right && has(DR),
    }
}

/// The Create sheet tile each Fusion tile of `ty`'s layout takes.
fn tile_map(ty: CtType) -> Vec<Option<u32>> {
    ty.layout()
        .masks()
        .into_iter()
        .map(|m| m.map(|m| ty.index(context(m))))
        .collect()
}

/// An RGBA image.
struct Image {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3]
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2]
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => return Err(format!("unexpected colour type {other:?}")),
    };
    Ok(Image { w, h, px })
}

fn encode(img: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.w, img.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().expect("an image in memory");
    w.write_image_data(&img.px).expect("an image in memory");
    drop(w);
    out
}

/// Create's sheet re-cut into `ty`'s Fusion layout, and that layout's name.
fn retile(sheet: &[u8], ty: CtType) -> Result<(Vec<u8>, &'static str), String> {
    let src = decode(sheet)?;
    let n = ty.sheet_size();
    if src.w % n != 0 || src.w != src.h {
        return Err(format!("a {}x{} sheet for {ty:?}", src.w, src.h));
    }
    let t = src.w / n;
    let layout = ty.layout();
    let (cols, rows) = layout.grid();
    let mut out = Image {
        w: cols * t,
        h: rows * t,
        px: vec![0; (cols * t * rows * t * 4) as usize],
    };
    for (i, from) in tile_map(ty).into_iter().enumerate() {
        let Some(from) = from else { continue };
        let (sx, sy) = ((from % n) * t, (from / n) * t);
        let (dx, dy) = ((i as u32 % cols) * t, (i as u32 / cols) * t);
        for y in 0..t {
            let s = (((sy + y) * src.w + sx) * 4) as usize;
            let d = (((dy + y) * out.w + dx) * 4) as usize;
            out.px[d..d + (t * 4) as usize].copy_from_slice(&src.px[s..s + (t * 4) as usize]);
        }
    }
    Ok((encode(&out), layout.name()))
}

/// Which faces of a model a rule changes (`HorizontalCTBehaviour`: its shift on the sides, its
/// top shift on the top and bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Faces {
    All,
    Sides,
    Ends,
}

/// How a block joins (`connectsTo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// The same block, unless the block in front of it covers it (`isBeingBlocked`).
    SameBlock,
    /// The same block (`GlassPaneCTBehaviour`).
    SameBlockAlways,
}

/// Faces of `block`'s models showing `original` take `sheet` (Create's `_connected` sheet
/// path, under `block/`).
struct Rule {
    original: String,
    sheet: String,
    ty: CtType,
    faces: Faces,
}

struct Plan {
    block: String,
    join: Join,
    rules: Vec<Rule>,
}

fn rule(original: &str, sheet: &str, ty: CtType, faces: Faces) -> Rule {
    Rule {
        original: format!("create:block/{original}"),
        sheet: sheet.to_owned(),
        ty,
        faces,
    }
}

const WOODS: [&str; 11] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "crimson", "warped",
    "cherry", "bamboo",
];

const STONES: [&str; 14] = [
    "granite",
    "diorite",
    "andesite",
    "calcite",
    "dripstone",
    "deepslate",
    "tuff",
    "asurine",
    "crimsite",
    "limestone",
    "ochrum",
    "scorchia",
    "scoria",
    "veridium",
];

/// Every block given connected textures, as Create registers them.
fn plans() -> Vec<Plan> {
    use CtType::*;
    let mut out = Vec::new();
    let mut plan = |block: &str, join: Join, rules: Vec<Rule>| {
        out.push(Plan {
            block: block.to_owned(),
            join,
            rules,
        });
    };
    for casing in [
        "andesite_casing",
        "brass_casing",
        "copper_casing",
        "shadow_steel_casing",
        "refined_radiance_casing",
    ] {
        let sheet = format!("{casing}_connected");
        plan(
            casing,
            Join::SameBlock,
            vec![rule(casing, &sheet, Omni, Faces::All)],
        );
    }
    plan(
        "railway_casing",
        Join::SameBlock,
        vec![
            rule(
                "railway_casing_side",
                "railway_casing_side_connected",
                Omni,
                Faces::Sides,
            ),
            rule(
                "railway_casing",
                "railway_casing_connected",
                Omni,
                Faces::Ends,
            ),
        ],
    );
    let glass = "palettes/framed_glass";
    let framed = |faces| rule(glass, "palettes/framed_glass_connected", Omni, faces);
    let horizontal = |faces| {
        rule(
            glass,
            "palettes/horizontal_framed_glass_connected",
            HorizontalKryppers,
            faces,
        )
    };
    let vertical = |faces| {
        rule(
            glass,
            "palettes/vertical_framed_glass_connected",
            Vertical,
            faces,
        )
    };
    plan("framed_glass", Join::SameBlock, vec![framed(Faces::All)]);
    plan(
        "horizontal_framed_glass",
        Join::SameBlock,
        vec![horizontal(Faces::Sides), framed(Faces::Ends)],
    );
    plan(
        "vertical_framed_glass",
        Join::SameBlock,
        vec![vertical(Faces::Sides)],
    );
    plan(
        "framed_glass_pane",
        Join::SameBlockAlways,
        vec![framed(Faces::All)],
    );
    plan(
        "horizontal_framed_glass_pane",
        Join::SameBlockAlways,
        vec![horizontal(Faces::All)],
    );
    plan(
        "vertical_framed_glass_pane",
        Join::SameBlockAlways,
        vec![vertical(Faces::All)],
    );
    let mut window = |name: &str, ty: CtType| {
        let tex = format!("palettes/{name}");
        let sheet = format!("palettes/{name}_connected");
        plan(
            name,
            Join::SameBlock,
            vec![rule(&tex, &sheet, ty, Faces::Sides)],
        );
        plan(
            &format!("{name}_pane"),
            Join::SameBlockAlways,
            vec![rule(&tex, &sheet, ty, Faces::All)],
        );
    };
    for wood in WOODS {
        window(&format!("{wood}_window"), Vertical);
    }
    window("ornate_iron_window", Vertical);
    window("industrial_iron_window", Rectangle);
    for stone in STONES {
        let layered = format!("palettes/stone_types/layered/{stone}_cut_layered");
        let cap = format!("palettes/stone_types/cap/{stone}_cut_cap");
        plan(
            &format!("layered_{stone}"),
            Join::SameBlock,
            vec![
                rule(
                    &layered,
                    &format!("{layered}_connected"),
                    HorizontalKryppers,
                    Faces::Sides,
                ),
                rule(&cap, &format!("{cap}_connected"), Omni, Faces::Ends),
            ],
        );
    }
    out
}

fn connection(join: Join, block: &str) -> Value {
    match join {
        Join::SameBlockAlways => json!({ "type": "is_same_block" }),
        Join::SameBlock => json!({
            "type": "and",
            "predicates": [
                { "type": "is_same_block" },
                { "type": "not", "predicate": { "type": "match_block_in_front", "block": format!("create:{block}") } }
            ]
        }),
    }
}

/// `create:block/x` (or `create:x`, `block/x`) as its file in the jar.
fn model_path(name: &str) -> Option<String> {
    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
    (ns == "create").then(|| format!("assets/create/models/{path}.json"))
}

fn namespaced(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

/// A Create model's texture keys over its Create parents (a child's win), and the first parent
/// outside Create.
fn textures_of(
    read: &dyn Fn(&str) -> Option<Value>,
    model: &str,
) -> (Map<String, Value>, Option<String>) {
    let mut keys = Map::new();
    let mut name = model.to_owned();
    for _ in 0..16 {
        let Some(doc) = model_path(&name).and_then(|p| read(&p)) else {
            return (keys, Some(namespaced(&name)));
        };
        if let Some(t) = doc.get("textures").and_then(Value::as_object) {
            for (k, v) in t {
                keys.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        match doc.get("parent").and_then(Value::as_str) {
            Some(p) => name = p.to_owned(),
            None => return (keys, None),
        }
    }
    (keys, None)
}

/// A key's texture, `#` references followed.
fn resolve(keys: &Map<String, Value>, value: &str) -> Option<String> {
    let mut v = value.to_owned();
    for _ in 0..16 {
        match v.strip_prefix('#') {
            Some(k) => v = keys.get(k)?.as_str()?.to_owned(),
            None => return Some(namespaced(&v)),
        }
    }
    None
}

fn key_faces(key: &str) -> Option<Faces> {
    match key {
        "north" | "south" | "east" | "west" | "side" => Some(Faces::Sides),
        "up" | "down" | "end" | "top" | "bottom" => Some(Faces::Ends),
        _ => None,
    }
}

/// The connecting child of `model` for `plan`, `None` when no face of it shows a rule's texture.
fn connecting_model(
    read: &dyn Fn(&str) -> Option<Value>,
    plan: &Plan,
    model: &str,
) -> Option<Value> {
    let (mut keys, outside) = textures_of(read, model);
    let mut parent = model.to_owned();
    // `cube_all` names every face `all`: as `cube` each face has a key of its own.
    if outside.as_deref() == Some("minecraft:block/cube_all")
        && plan.rules.iter().any(|r| r.faces != Faces::All)
    {
        let all = keys.get("all").cloned()?;
        for face in ["down", "up", "north", "south", "west", "east"] {
            keys.insert(face.into(), all.clone());
        }
        keys.entry("particle").or_insert(all);
        keys.remove("all");
        parent = "minecraft:block/cube".into();
    }
    let mut textures = Map::new();
    let mut first_original = None;
    for (key, value) in &keys {
        if key == "particle" {
            continue;
        }
        let Some(texture) = value.as_str().and_then(|v| resolve(&keys, v)) else {
            continue;
        };
        let class = key_faces(key);
        let Some(rule) = plan.rules.iter().find(|r| {
            r.original == texture
                && (r.faces == Faces::All || class.is_none() || class == Some(r.faces))
        }) else {
            continue;
        };
        first_original.get_or_insert_with(|| rule.original.clone());
        textures.insert(key.clone(), json!(sheet_name(&rule.sheet)));
    }
    if textures.is_empty() {
        return None;
    }
    if parent != model {
        for (key, value) in &keys {
            textures.entry(key.clone()).or_insert(value.clone());
        }
    }
    // The particle stays the plain texture: the model's own, else the texture its first
    // connected face replaces (a vanilla parent's `#side` or `#all`).
    let particle = keys
        .get("particle")
        .and_then(Value::as_str)
        .and_then(|v| resolve(&keys, v))
        .or(first_original);
    if let Some(particle) = particle {
        textures.insert("particle".into(), json!(particle));
    }
    Some(json!({
        "parent": parent,
        "loader": "fusion:model",
        "type": "connecting",
        "textures": textures,
        "connections": connection(plan.join, &plan.block),
    }))
}

/// The sprite of a re-cut sheet.
fn sheet_name(sheet: &str) -> String {
    format!("create:block/ct/{sheet}")
}

/// Each `model` string of a blockstate, mutably.
fn models_mut(v: &mut Value, f: &mut dyn FnMut(&mut Value)) {
    match v {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if k == "model" && child.is_string() {
                    f(child);
                } else {
                    models_mut(child, f);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| models_mut(i, f)),
        _ => {}
    }
}

/// The files connected textures add or replace: re-cut sheets with their `.mcmeta`, connecting
/// models, and the blockstates that name them. `read` gives a file of the assembled mod (ours
/// over Create's). Returns the files and the sheets left out, with why.
pub fn files(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    let json = |p: &str| read(p).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let mut out = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut sheets: BTreeMap<String, bool> = BTreeMap::new();
    for plan in plans() {
        // The sheets first: a plan whose sheet can't be cut is left as it is.
        let ok = plan.rules.iter().all(|r| {
            *sheets.entry(r.sheet.clone()).or_insert_with(|| {
                let src = format!("assets/create/textures/block/{}.png", r.sheet);
                if read(&format!("{src}.mcmeta")).is_some() {
                    skipped.push(format!("{}: animated", r.sheet));
                    return false;
                }
                let Some(bytes) = read(&src) else {
                    skipped.push(format!("{}: not in the sources", r.sheet));
                    return false;
                };
                match retile(&bytes, r.ty) {
                    Ok((png, layout)) => {
                        let dst = format!("assets/create/textures/block/ct/{}.png", r.sheet);
                        let meta = json!({ "fusion": { "type": "connecting", "layout": layout } });
                        out.insert(format!("{dst}.mcmeta"), meta.to_string().into_bytes());
                        out.insert(dst, png);
                        true
                    }
                    Err(e) => {
                        skipped.push(format!("{}: {e}", r.sheet));
                        false
                    }
                }
            })
        });
        if !ok {
            continue;
        }
        let state_path = format!("assets/create/blockstates/{}.json", plan.block);
        let Some(mut state) = json(&state_path) else {
            skipped.push(format!("{}: no blockstate", plan.block));
            continue;
        };
        let mut changed = false;
        let read_json = |p: &str| json(p);
        models_mut(&mut state, &mut |m| {
            let name = m.as_str().unwrap_or_default().to_owned();
            let Some(doc) = connecting_model(&read_json, &plan, &name) else {
                return;
            };
            let Some(path) = name
                .strip_prefix("create:block/")
                .or_else(|| name.strip_prefix("create:"))
            else {
                return;
            };
            let new = format!("block/ct/{}/{path}", plan.block);
            out.insert(
                format!("assets/create/models/{new}.json"),
                serde_json::to_vec_pretty(&doc).unwrap(),
            );
            *m = json!(format!("create:{new}"));
            changed = true;
        });
        if changed {
            out.insert(state_path, serde_json::to_vec_pretty(&state).unwrap());
        } else {
            skipped.push(format!("{}: no face shows its texture", plan.block));
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(mask: u8) -> Ctx {
        context(mask)
    }

    #[test]
    fn omni_reads_create_s_sheet() {
        // Alone: Create's top-left tile; all four sides and no corner: (3, 3); joined all round
        // with every corner: (6, 6), the sheet's plain middle.
        assert_eq!(CtType::Omni.index(ctx(0)), 0);
        assert_eq!(CtType::Omni.index(ctx(SIDES)), 3 + 8 * 3);
        assert_eq!(CtType::Omni.index(ctx(0xff)), 6 + 8 * 6);
        // Joined up only: column 1 of row 0; down only: column 2; left only: row 1.
        assert_eq!(CtType::Omni.index(ctx(U)), 1);
        assert_eq!(CtType::Omni.index(ctx(D)), 2);
        assert_eq!(CtType::Omni.index(ctx(L)), 8);
        // Up and left with the corner between: (1 + 3, 1).
        assert_eq!(CtType::Omni.index(ctx(U | L | UL)), 4 + 8);
        // Three sides (not right) and the top-left corner: row 4, column 1.
        assert_eq!(CtType::Omni.index(ctx(U | D | L | UL)), 1 + 8 * 4);
    }

    #[test]
    fn a_corner_joins_only_beside_two_joined_sides() {
        assert_eq!(ctx(UL), Ctx::default());
        assert!(!ctx(U | UL).top_left);
        assert!(ctx(U | L | UL).top_left);
    }

    #[test]
    fn straight_types() {
        use CtType::*;
        let side = |l, r| Ctx {
            left: l,
            right: r,
            ..Ctx::default()
        };
        assert_eq!(HorizontalKryppers.index(side(false, false)), 0);
        assert_eq!(HorizontalKryppers.index(side(true, true)), 1);
        assert_eq!(HorizontalKryppers.index(side(false, true)), 2);
        assert_eq!(HorizontalKryppers.index(side(true, false)), 3);
        let col = |u, d| Ctx {
            up: u,
            down: d,
            ..Ctx::default()
        };
        assert_eq!(Vertical.index(col(true, false)), 1);
        assert_eq!(Vertical.index(col(false, true)), 2);
        // A lone rectangle tile is (0, 3); joined all four ways (2, 1).
        assert_eq!(Rectangle.index(Ctx::default()), 12);
        assert_eq!(Rectangle.index(ctx(SIDES)), 2 + 4);
    }

    #[test]
    fn fusion_tiles_take_the_matching_create_tiles() {
        // Fusion's full layout: tile 0 alone, 18 joined all round, 33 all sides no corners.
        let full = tile_map(CtType::Omni);
        assert_eq!(full.len(), 48);
        assert_eq!(full[0], Some(0));
        assert_eq!(full[18], Some(54));
        assert_eq!(full[33], Some(27));
        assert_eq!(full[41], None);
        // Horizontal: alone, a row's left end (joins right), middle, right end.
        assert_eq!(
            tile_map(CtType::HorizontalKryppers),
            [Some(0), Some(2), Some(1), Some(3)]
        );
        // Vertical: alone, a column's top (joins down), middle, bottom (joins up).
        assert_eq!(
            tile_map(CtType::Vertical),
            [Some(0), Some(2), Some(3), Some(1)]
        );
        // Simple tile 1 joins all four sides: the rectangle's middle.
        assert_eq!(tile_map(CtType::Rectangle)[1], Some(6));
    }

    #[test]
    fn retiles_a_sheet_into_its_layout() {
        // A 2 x 2 sheet of 1-pixel tiles, each a grey of its index.
        let img = Image {
            w: 2,
            h: 2,
            px: (0..4u8).flat_map(|i| [i, i, i, 255]).collect(),
        };
        let (png, layout) = retile(&encode(&img), CtType::HorizontalKryppers).unwrap();
        assert_eq!(layout, "horizontal");
        let out = decode(&png).unwrap();
        assert_eq!((out.w, out.h), (4, 1));
        let greys: Vec<u8> = out.px.chunks(4).map(|p| p[0]).collect();
        assert_eq!(greys, [0, 2, 1, 3]);
    }

    #[test]
    fn a_cube_all_block_gets_per_face_sheets() {
        let files: BTreeMap<String, Value> = [(
            "assets/create/models/block/horizontal_framed_glass.json".to_owned(),
            json!({"parent": "minecraft:block/cube_all", "textures": {"all": "create:block/palettes/framed_glass"}}),
        )]
        .into();
        let read = |p: &str| files.get(p).cloned();
        let plans = plans();
        let plan = plans
            .iter()
            .find(|p| p.block == "horizontal_framed_glass")
            .unwrap();
        let doc = connecting_model(&read, plan, "create:block/horizontal_framed_glass").unwrap();
        assert_eq!(doc["parent"], "minecraft:block/cube");
        assert_eq!(
            doc["textures"]["north"],
            "create:block/ct/palettes/horizontal_framed_glass_connected"
        );
        assert_eq!(
            doc["textures"]["up"],
            "create:block/ct/palettes/framed_glass_connected"
        );
        assert_eq!(
            doc["textures"]["particle"],
            "create:block/palettes/framed_glass"
        );
    }
}
