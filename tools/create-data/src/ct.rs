//! Create's connected textures (`ConnectedTextureBehaviour`, `AllCTTypes`) as OptiFine connected
//! textures (`optifine/ctm`), which the game's terrain mesher draws. Create's `_connected` sheets
//! are laid out for its own index functions (`AllCTTypes.getTextureIndex`); OptiFine's methods
//! take their tiles in other orders. At prepare time each sheet is cut into the tiles of the
//! method whose neighbour masks stand for the same contexts: for every OptiFine tile, the context
//! it stands for goes through Create's index function and that tile of Create's sheet is taken.
//! A properties file per block and sheet matches the block and the texture Create shifts
//! (`matchBlocks`, `matchTiles`, as Create's CT behaviours are per block and per sprite), on the
//! faces Create shifts. Block models stay Create's: whatever draws a block outside the terrain
//! (a copycat, a moving structure, a block display) shows its plain texture.
//!
//! Faces are oriented as Create's `ConnectedTextureBehaviour.buildContext` (a side's up is up,
//! its right as its texture reads from outside; the top's up is north, the bottom's south), which
//! are a cube's default UVs, as OptiFine's. Create's `isBeingBlocked` (a neighbour covered by the
//! same block in front doesn't join) is OptiFine's `innerSeams`.
//!
//! Left out: pillars (`RotatedPillarCTBehaviour` turns its axes by the pillar's axis and joins by
//! its `ConnectedPillarBlock` sides), the weathered iron window's position-picked sheets, copper
//! roofs (`ROOF`), metal scaffolding, chassis, the encased blocks' and crafters' state-dependent
//! sides, tanks (their sheets follow the multiblock's size and the block entity), the
//! glass panes' mirrored left and right on their north and west faces
//! (`GlassPaneCTBehaviour.reverseUVsHorizontally`), and casings joining the encased blocks of their
//! casing (`CasingConnectivity`): a casing joins only its own block. Item vaults join any vault
//! block of the same axis and size beside them, where Create joins only its own vault's blocks
//! (`ConnectivityHandler.isConnected`: two vaults side by side show their seam).

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

// OptiFine's neighbour mask bits (`horizon_optifine::ctm::connect`), in texture directions.
const U: u8 = 1;
const UR: u8 = 2;
const R: u8 = 4;
const DR: u8 = 8;
const D: u8 = 16;
const DL: u8 = 32;
const L: u8 = 64;
const UL: u8 = 128;

/// OptiFine's `ctm` template (`ctm_template.png`): the neighbours each of its 47 tiles joins.
const CTM: [u8; 47] = [
    0,
    R,
    R | L,
    L,
    R | D,
    D | L,
    U | R | D,
    R | D | L,
    U | UR | R | D | L,
    U | R | DR | D | L,
    U | R | D | DL | L | UL,
    U | UR | R | D | L | UL,
    D,
    R | DR | D,
    R | DR | D | DL | L,
    D | DL | L,
    U | R,
    U | L,
    U | R | L,
    U | D | L,
    U | R | D | L | UL,
    U | R | D | DL | L,
    U | R | DR | D | DL | L,
    U | UR | R | DR | D | L,
    U | D,
    U | UR | R | DR | D,
    U | UR | R | DR | D | DL | L | UL,
    U | D | DL | L | UL,
    U | R | DR | D,
    R | D | DL | L,
    U | UR | R | D,
    R | DR | D | L,
    U | UR | R | D | DL | L | UL,
    U | UR | R | DR | D | L | UL,
    U | UR | R | D | DL | L,
    U | R | DR | D | L | UL,
    U,
    U | UR | R,
    U | UR | R | L | UL,
    U | L | UL,
    U | UR | R | L,
    U | D | L | UL,
    U | R | L | UL,
    U | D | DL | L,
    U | R | DR | D | DL | L | UL,
    U | UR | R | DR | D | DL | L,
    U | R | D | L,
];

/// OptiFine's methods the sheets become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Ctm,
    Horizontal,
    Vertical,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Self::Ctm => "ctm",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    /// The neighbours each tile joins, by tile.
    fn masks(self) -> Vec<u8> {
        match self {
            Self::Ctm => CTM.to_vec(),
            // A row's left end (joined right), its middle, its right end, alone.
            Self::Horizontal => vec![R, L | R, L, 0],
            // A column's bottom (joined up), its middle, its top, alone.
            Self::Vertical => vec![U, U | D, D, 0],
        }
    }
}

impl CtType {
    /// The OptiFine method the sheet becomes: `RECTANGLE` reads only the sides, which the 47
    /// tiles tell apart too.
    fn method(self) -> Method {
        match self {
            Self::HorizontalKryppers => Method::Horizontal,
            Self::Vertical => Method::Vertical,
            Self::Omni | Self::Rectangle => Method::Ctm,
        }
    }
}

/// The context a mask stands for: a corner joins only with both sides beside it, as Create's
/// `buildContext` tests it.
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

/// The Create sheet tile each OptiFine tile of `ty`'s method takes.
fn tile_map(ty: CtType) -> Vec<u32> {
    ty.method()
        .masks()
        .into_iter()
        .map(|m| ty.index(context(m)))
        .collect()
}

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

/// Which faces a rule changes (`HorizontalCTBehaviour`: its shift on the sides, its
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
    /// The same state, the faces' neighbours found along the drawn texture's own directions
    /// (`ItemVaultCTBehaviour`: `state == other`, its `getUpDirection` and `getRightDirection`
    /// follow the vault model's turned faces).
    SameStateTexture,
}

/// `block`'s faces showing `original` take `sheet` (Create's `_connected` sheet
/// path, under `block/`).
struct Rule {
    original: String,
    sheet: String,
    ty: CtType,
    faces: Faces,
}

struct Plan {
    block: String,
    /// The block's states the rules apply to (`:large=true`), empty for all.
    state: String,
    join: Join,
    rules: Vec<Rule>,
}

impl Plan {
    /// The folder of its properties files.
    fn folder(&self) -> String {
        let state = self.state.replace([':', '='], "_");
        format!("{}{state}", self.block)
    }
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
        let (block, state) = block.split_once(':').unwrap_or((block, ""));
        out.push(Plan {
            block: block.to_owned(),
            state: if state.is_empty() {
                String::new()
            } else {
                format!(":{state}")
            },
            join,
            rules,
        });
    };
    // `ItemVaultCTBehaviour`: each face's sprite by the vault's axis (the model's turned faces),
    // a one-wide vault's (`large=false`) from the `_medium` sheets, a wider one's from the
    // `_large` (`AllSpriteShifts.vault`, `RECTANGLE`).
    for (large, size) in [("false", "medium"), ("true", "large")] {
        let rules = ["front", "side", "top", "bottom"]
            .into_iter()
            .map(|face| {
                let tex = format!("vault/vault_{face}_small");
                let sheet = format!("vault/vault_{face}_{size}");
                rule(&tex, &sheet, Rectangle, Faces::All)
            })
            .collect();
        plan(
            &format!("item_vault:large={large}"),
            Join::SameStateTexture,
            rules,
        );
    }
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

/// Create's sheet cut into the tiles of `ty`'s OptiFine method, in its order.
fn cut(sheet: &[u8], ty: CtType) -> Result<Vec<Vec<u8>>, String> {
    let src = decode(sheet)?;
    let n = ty.sheet_size();
    if src.w % n != 0 || src.w != src.h {
        return Err(format!("a {}x{} sheet for {ty:?}", src.w, src.h));
    }
    let t = src.w / n;
    Ok(tile_map(ty)
        .into_iter()
        .map(|from| {
            let (sx, sy) = ((from % n) * t, (from / n) * t);
            let mut px = Vec::with_capacity((t * t * 4) as usize);
            for y in 0..t {
                let s = (((sy + y) * src.w + sx) * 4) as usize;
                px.extend_from_slice(&src.px[s..s + (t * 4) as usize]);
            }
            encode(&Image { w: t, h: t, px })
        })
        .collect())
}

/// The properties file of `rule` for `plan`'s block, its tiles under `tiles/`.
fn properties(plan: &Plan, rule: &Rule, tiles: &str) -> String {
    let method = rule.ty.method();
    let count = method.masks().len();
    let connect = match plan.join {
        Join::SameStateTexture => "state\norient=texture",
        _ => "block",
    };
    let mut out = format!(
        "method={}\nmatchTiles={}\nmatchBlocks=create:{}{}\nconnect={connect}\ntiles={}\n",
        method.name(),
        rule.original,
        plan.block,
        plan.state,
        (0..count)
            .map(|i| format!("create:optifine/ctm/tiles/{tiles}/{i}"))
            .collect::<Vec<_>>()
            .join(" "),
    );
    match rule.faces {
        Faces::All => {}
        Faces::Sides => out.push_str("faces=sides\n"),
        Faces::Ends => out.push_str("faces=top bottom\n"),
    }
    // Create's `isBeingBlocked`: a neighbour whose face the same block covers doesn't join.
    if plan.join == Join::SameBlock {
        out.push_str("innerSeams=true\n");
    }
    out
}

/// The files connected textures add: one OptiFine `ctm` properties file per block and sheet,
/// and each sheet's tiles. Block models stay as Create's: only the terrain mesher swaps the
/// tiles in. `read` gives a file of the sources. Returns the files and what was left out, with
/// why.
pub fn files(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    let mut out = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut sheets: BTreeMap<String, bool> = BTreeMap::new();
    for plan in plans() {
        if read(&format!("assets/create/blockstates/{}.json", plan.block)).is_none() {
            skipped.push(format!("{}: no blockstate", plan.block));
            continue;
        }
        for (i, rule) in plan.rules.iter().enumerate() {
            let cut_ok = *sheets.entry(rule.sheet.clone()).or_insert_with(|| {
                let src = format!("assets/create/textures/block/{}.png", rule.sheet);
                if read(&format!("{src}.mcmeta")).is_some() {
                    skipped.push(format!("{}: animated", rule.sheet));
                    return false;
                }
                let Some(bytes) = read(&src) else {
                    skipped.push(format!("{}: not in the sources", rule.sheet));
                    return false;
                };
                match cut(&bytes, rule.ty) {
                    Ok(tiles) => {
                        for (n, png) in tiles.into_iter().enumerate() {
                            out.insert(
                                format!("assets/create/optifine/ctm/tiles/{}/{n}.png", rule.sheet),
                                png,
                            );
                        }
                        true
                    }
                    Err(e) => {
                        skipped.push(format!("{}: {e}", rule.sheet));
                        false
                    }
                }
            });
            if cut_ok {
                out.insert(
                    format!(
                        "assets/create/optifine/ctm/{}/{i}.properties",
                        plan.folder()
                    ),
                    properties(&plan, rule, &rule.sheet).into_bytes(),
                );
            }
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
        assert_eq!(CtType::Omni.index(ctx(U | R | D | L)), 3 + 8 * 3);
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
        assert_eq!(Rectangle.index(ctx(U | R | D | L)), 2 + 4);
    }

    #[test]
    fn optifine_tiles_take_the_matching_create_tiles() {
        // `ctm`: tile 0 alone, 26 joined all round, 46 all sides without corners, 12 joined
        // below only.
        let ctm = tile_map(CtType::Omni);
        assert_eq!(ctm.len(), 47);
        assert_eq!(ctm[0], 0);
        assert_eq!(ctm[26], 54);
        assert_eq!(ctm[46], 27);
        assert_eq!(ctm[12], 2);
        // `horizontal`: a row's left end (Create's "joined right", 2), middle, right end, alone.
        assert_eq!(tile_map(CtType::HorizontalKryppers), [2, 1, 3, 0]);
        // `vertical`: a column's bottom (Create's "joined up", 1), middle, top, alone.
        assert_eq!(tile_map(CtType::Vertical), [1, 3, 2, 0]);
        // A rectangle tile joined all four ways, corners or not, is its middle.
        let rect = tile_map(CtType::Rectangle);
        assert_eq!((rect[26], rect[46]), (6, 6));
    }

    #[test]
    fn cuts_a_sheet_into_its_tiles() {
        // A 2 x 2 sheet of 1-pixel tiles, each a grey of its index.
        let img = Image {
            w: 2,
            h: 2,
            px: (0..4u8).flat_map(|i| [i, i, i, 255]).collect(),
        };
        let tiles = cut(&encode(&img), CtType::HorizontalKryppers).unwrap();
        let greys: Vec<u8> = tiles.iter().map(|t| decode(t).unwrap().px[0]).collect();
        assert_eq!(greys, [2, 1, 3, 0]);
    }

    #[test]
    fn properties_name_the_block_texture_faces_and_seams() {
        let plans = plans();
        let plan = plans
            .iter()
            .find(|p| p.block == "horizontal_framed_glass")
            .unwrap();
        let text = properties(plan, &plan.rules[0], "x");
        assert!(text.contains("method=horizontal\n"));
        assert!(text.contains("matchTiles=create:block/palettes/framed_glass\n"));
        assert!(text.contains("matchBlocks=create:horizontal_framed_glass\n"));
        assert!(text.contains("faces=sides\n"));
        assert!(text.contains("innerSeams=true\n"));
        assert!(text.contains("tiles=create:optifine/ctm/tiles/x/0 "));
        let pane = plans
            .iter()
            .find(|p| p.block == "framed_glass_pane")
            .unwrap();
        let text = properties(pane, &pane.rules[0], "x");
        assert!(!text.contains("innerSeams") && !text.contains("faces="));
    }
}
