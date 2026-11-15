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
//! are a cube's default UVs, as OptiFine's (`orient=none`). Where Create turns a face's directions
//! with the block (pillars, chassis, vaults, encased cogs' sides), so that its sheet reads as its
//! turned texture does, the rule reads the quad's own texture directions (`orient=texture`).
//! Create's `isBeingBlocked` (a neighbour covered by the same block in front doesn't join) is
//! OptiFine's `innerSeams`. Create's joins that ask more than the block are approximated by the
//! same block state (`connect=state`: encased blocks, vaults, trapdoors), and its random sheet
//! (the weathered iron window) by a random rule picking a sheet's tile, which the rule of that
//! sheet matches again to join it.
//!
//! Left out: copper roofs (`ROOF`: joined by collision heights, stairs mapped by shape), the
//! crafters' and brass tunnels' sides (joined by their block entities), fluid tanks' joins their
//! state doesn't tell (the tank's size and block entity: `tank_files` joins the rest), a pillar's `ConnectedPillarBlock` sides (the port keeps them unset:
//! same pillars join), a vault joining only its own vault (two vaults side by side join), the
//! glass panes' mirrored left and right on their north and west faces
//! (`GlassPaneCTBehaviour.reverseUVsHorizontally`), and casings joining the encased blocks of their
//! casing (`CasingConnectivity`): a casing joins only its own block, an encased block its own
//! state.

use std::collections::BTreeMap;

/// Create's CT types the port's blocks use, with their sheet sizes (`AllCTTypes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtType {
    HorizontalKryppers,
    /// `HORIZONTAL`.
    Horizontal,
    /// A `VERTICAL` or `HORIZONTAL` sheet whose slit runs along its texture's width, read in
    /// the texture's own directions: its second tile is joined on the texture's left, its third
    /// on its right (the encased cogs' sides, which Create reads in turned directions).
    HorizontalByTexture,
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
            Self::HorizontalKryppers
            | Self::Horizontal
            | Self::HorizontalByTexture
            | Self::Vertical => 2,
            Self::Omni => 8,
            Self::Rectangle => 4,
        }
    }

    /// The folder name of a sheet's tiles cut for the type.
    fn name(self) -> &'static str {
        match self {
            Self::HorizontalKryppers => "horizontal_kryppers",
            Self::Horizontal => "horizontal",
            Self::HorizontalByTexture => "horizontal_by_texture",
            Self::Vertical => "vertical",
            Self::Omni => "omni",
            Self::Rectangle => "rectangle",
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
            Self::Horizontal => b(c.right) + 2 * b(c.left),
            Self::HorizontalByTexture => b(c.left) + 2 * b(c.right),
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
            Self::HorizontalKryppers | Self::Horizontal | Self::HorizontalByTexture => {
                Method::Horizontal
            }
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
/// top shift on the top and bottom), by their direction in the world.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Faces {
    All,
    Sides,
    Ends,
    /// OptiFine's face names, as `faces` takes them.
    Some(String),
}

/// How a block joins (`connectsTo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// The same block.
    Block,
    /// The same block state: the same block in the same form (`state == other`, or the axis,
    /// shafts and sides Create compares).
    State,
}

/// `block`'s faces showing `original` take `sheet` (Create's `_connected` sheet
/// path, under `block/`).
struct Rule {
    original: String,
    sheet: String,
    ty: CtType,
    faces: Faces,
    /// Create's `isBeingBlocked`: a neighbour whose face the same block covers doesn't join.
    seams: bool,
    /// The face's directions are its texture's (`orient=texture`), where Create turns them with
    /// the block (`getUpDirection`, `getRightDirection` and the reversed UVs of pillars,
    /// chassis, vaults and encased cogs), not a cube's.
    by_texture: bool,
    /// Create picks one of these sheets per face at random (`WeatheredIronWindowCTBehaviour`);
    /// `sheet` is then unused.
    random: Vec<String>,
}

impl Rule {
    fn seams(mut self, seams: bool) -> Self {
        self.seams = seams;
        self
    }

    fn by_texture(mut self) -> Self {
        self.by_texture = true;
        self
    }
}

struct Plan {
    /// The folder of its properties files, under `optifine/ctm/`.
    name: String,
    /// The block states it matches (`matchBlocks` entries, without `create:`).
    blocks: Vec<String>,
    join: Join,
    rules: Vec<Rule>,
}

fn rule(original: &str, sheet: &str, ty: CtType, faces: Faces) -> Rule {
    Rule {
        original: format!("create:block/{original}"),
        sheet: sheet.to_owned(),
        ty,
        faces,
        seams: true,
        by_texture: false,
        random: Vec::new(),
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

/// OptiFine's names of the faces whose direction is on `axis` (`true`) or not.
fn faces_of(axis: char, along: bool) -> Faces {
    let names: &[&str] = match (axis, along) {
        ('x', true) => &["east", "west"],
        ('y', true) => &["top", "bottom"],
        ('z', true) => &["north", "south"],
        ('x', false) => &["north", "south", "top", "bottom"],
        ('y', false) => &["sides"],
        _ => &["east", "west", "top", "bottom"],
    };
    Faces::Some(names.join(" "))
}

/// The face on `axis` toward its positive (`true`) or negative end.
fn end_face(axis: char, positive: bool) -> &'static str {
    match (axis, positive) {
        ('x', true) => "east",
        ('x', false) => "west",
        ('y', true) => "top",
        ('y', false) => "bottom",
        ('z', true) => "south",
        _ => "north",
    }
}

/// Every block given connected textures, as Create registers them.
fn plans() -> Vec<Plan> {
    use CtType::*;
    let mut out = Vec::new();
    let mut plan = |name: &str, blocks: &[&str], join: Join, rules: Vec<Rule>| {
        out.push(Plan {
            name: name.to_owned(),
            blocks: blocks.iter().map(|b| (*b).to_owned()).collect(),
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
            &[casing],
            Join::Block,
            vec![rule(casing, &sheet, Omni, Faces::All)],
        );
    }
    plan(
        "railway_casing",
        &["railway_casing"],
        Join::Block,
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
    // `EncasedCTBehaviour` and `CasingConnectivity`: an encased block's casing joins on the
    // faces its entry calls valid, those of another block of the same casing whose faces are
    // valid too. Here a block joins the same block in the same state (the same axis and
    // shafts), on the faces that state makes valid.
    for (casing, kinds) in [("andesite_casing", "andesite"), ("brass_casing", "brass")] {
        let sheet = format!("{casing}_connected");
        let shaft = format!("{kinds}_encased_shaft");
        for axis in ['x', 'y', 'z'] {
            // `encasedShaft`: the faces off the shaft's axis.
            let state = format!("{shaft}:axis={axis}");
            plan(
                &format!("{shaft}_{axis}"),
                &[&state],
                Join::State,
                vec![rule(casing, &sheet, Omni, faces_of(axis, false))],
            );
            // `encasedCogwheelBase`: the faces on the cog's axis without a shaft.
            for cog in [
                format!("{kinds}_encased_cogwheel"),
                format!("{kinds}_encased_large_cogwheel"),
            ] {
                for (positive, prop) in [(true, "top_shaft"), (false, "bottom_shaft")] {
                    let state = format!("{cog}:axis={axis}:{prop}=false");
                    plan(
                        &format!("{cog}_{axis}_{prop}"),
                        &[&state],
                        Join::State,
                        vec![
                            rule(
                                casing,
                                &sheet,
                                Omni,
                                Faces::Some(end_face(axis, positive).to_owned()),
                            )
                            // `EncasedCogCTBehaviour.reverseUVs`: a horizontal cog's positive faces
                            // read their turned texture.
                            .by_texture(),
                        ],
                    );
                }
            }
            // `EncasedCogCTBehaviour`: a small cog's sides show the cog through their slit,
            // joined along it to the same cog on the same axis. Create turns the sheet by the
            // face (`VERTICAL` on some, `HORIZONTAL` on others, and reversed UVs) so that its
            // slit runs along the texture's width: there it is one row.
            let cog = format!("{kinds}_encased_cogwheel");
            let side = format!("{kinds}_encased_cogwheel_side");
            let state = format!("{cog}:axis={axis}");
            plan(
                &format!("{cog}_{axis}_sides"),
                &[&state],
                Join::State,
                vec![
                    rule(
                        &side,
                        &format!("{side}_connected"),
                        HorizontalByTexture,
                        faces_of(axis, false),
                    )
                    .seams(false)
                    .by_texture(),
                ],
            );
        }
    }
    // The gearbox: the faces on its axis.
    for axis in ['x', 'y', 'z'] {
        let state = format!("gearbox:axis={axis}");
        plan(
            &format!("gearbox_{axis}"),
            &[&state],
            Join::State,
            vec![rule(
                "andesite_casing",
                "andesite_casing_connected",
                Omni,
                faces_of(axis, true),
            )],
        );
    }
    // The encased fluid pipe: each face without a pipe through it.
    for face in ["north", "south", "east", "west", "up", "down"] {
        let state = format!("encased_fluid_pipe:{face}=false");
        let name = match face {
            "up" => "top",
            "down" => "bottom",
            f => f,
        };
        plan(
            &format!("encased_fluid_pipe_{face}"),
            &[&state],
            Join::State,
            vec![rule(
                "copper_casing",
                "copper_casing_connected",
                Omni,
                Faces::Some(name.to_owned()),
            )],
        );
    }
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
    plan(
        "framed_glass",
        &["framed_glass"],
        Join::Block,
        vec![framed(Faces::All)],
    );
    plan(
        "horizontal_framed_glass",
        &["horizontal_framed_glass"],
        Join::Block,
        vec![horizontal(Faces::Sides), framed(Faces::Ends)],
    );
    plan(
        "vertical_framed_glass",
        &["vertical_framed_glass"],
        Join::Block,
        vec![vertical(Faces::Sides)],
    );
    plan(
        "framed_glass_pane",
        &["framed_glass_pane"],
        Join::Block,
        vec![framed(Faces::All).seams(false)],
    );
    plan(
        "horizontal_framed_glass_pane",
        &["horizontal_framed_glass_pane"],
        Join::Block,
        vec![horizontal(Faces::All).seams(false)],
    );
    plan(
        "vertical_framed_glass_pane",
        &["vertical_framed_glass_pane"],
        Join::Block,
        vec![vertical(Faces::All).seams(false)],
    );
    // `TrapdoorCTBehaviour`: framed glass, joined to the same trapdoor lying or standing the
    // same way (`TrainTrapdoorBlock.isConnected`).
    plan(
        "framed_glass_trapdoor",
        &["framed_glass_trapdoor"],
        Join::State,
        vec![framed(Faces::All).seams(false)],
    );
    let mut window = |name: &str, ty: CtType| {
        let tex = format!("palettes/{name}");
        let sheet = format!("palettes/{name}_connected");
        plan(
            name,
            &[name],
            Join::Block,
            vec![rule(&tex, &sheet, ty, Faces::Sides)],
        );
        let pane = format!("{name}_pane");
        plan(
            &pane,
            &[&pane],
            Join::Block,
            vec![rule(&tex, &sheet, ty, Faces::All).seams(false)],
        );
    };
    for wood in WOODS {
        window(&format!("{wood}_window"), Vertical);
    }
    window("ornate_iron_window", Vertical);
    window("industrial_iron_window", Rectangle);
    // `WeatheredIronWindowCTBehaviour`: each side takes one of four sheets at random.
    let weathered = |faces| {
        let mut r = rule("palettes/weathered_iron_window", "", Rectangle, faces);
        r.random = (1..=4)
            .map(|k| format!("palettes/weathered_iron_window_{k}_connected"))
            .collect();
        r
    };
    plan(
        "weathered_iron_window",
        &["weathered_iron_window"],
        Join::Block,
        vec![weathered(Faces::Sides)],
    );
    plan(
        "weathered_iron_window_pane",
        &["weathered_iron_window_pane"],
        Join::Block,
        vec![weathered(Faces::All).seams(false)],
    );
    for stone in STONES {
        let layered = format!("palettes/stone_types/layered/{stone}_cut_layered");
        let pillar = format!("palettes/stone_types/pillar/{stone}_cut_pillar");
        let cap = format!("palettes/stone_types/cap/{stone}_cut_cap");
        let block = format!("layered_{stone}");
        plan(
            &block,
            &[&block],
            Join::Block,
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
        // `RotatedPillarCTBehaviour`: the pillar's sides and caps turned with its axis, as its
        // turned `cube_column` model is. Create's `ConnectedPillarBlock` sides (joined only to a
        // column of the same length) are not kept by the port: same pillars join.
        let block = format!("{stone}_pillar");
        plan(
            &block,
            &[&block],
            Join::Block,
            vec![
                rule(
                    &pillar,
                    &format!("{pillar}_connected"),
                    Rectangle,
                    Faces::All,
                )
                .seams(false)
                .by_texture(),
                rule(&cap, &format!("{cap}_connected"), Omni, Faces::All).by_texture(),
            ],
        );
    }
    // `MetalScaffoldingCTBehaviour`: a horizontal scaffolding's (`bottom`) outside and inside
    // in rows, its casing top joined like the casing; only bottom scaffolding joins (the same
    // state: horizontal scaffolding are all at the template's distance 7, as they never fall).
    for metal in ["andesite", "brass", "copper"] {
        let block = format!("{metal}_scaffolding");
        let state = format!("{block}:bottom=true");
        let side = format!("scaffold/{metal}_scaffold");
        let inside = format!("scaffold/{metal}_scaffold_inside");
        let casing = format!("{metal}_casing");
        plan(
            &block,
            &[&state],
            Join::State,
            vec![
                rule(
                    &side,
                    &format!("{side}_connected"),
                    Horizontal,
                    Faces::Sides,
                )
                .seams(false),
                rule(
                    &inside,
                    &format!("{inside}_connected"),
                    Horizontal,
                    Faces::Sides,
                )
                .seams(false),
                rule(&casing, &format!("{casing}_connected"), Omni, Faces::Ends),
            ],
        );
    }
    // `ChassisCTBehaviour`: the sides joined to the same chassis, the ends (sticky or not) too
    // unless covered, turned with the chassis' axis.
    for (block, side) in [
        ("linear_chassis", "linear_chassis_side"),
        ("secondary_linear_chassis", "secondary_linear_chassis_side"),
    ] {
        let end = |t: &str| rule(t, &format!("{t}_connected"), Omni, Faces::All).by_texture();
        plan(
            block,
            &[block],
            Join::Block,
            vec![
                rule(side, &format!("{side}_connected"), Omni, Faces::All)
                    .seams(false)
                    .by_texture(),
                end("linear_chassis_end"),
                end("linear_chassis_end_sticky"),
            ],
        );
    }
    // `GirderCTBehaviour`: a lone pole's sides, joined to the poles above and below.
    plan(
        "metal_girder",
        &["metal_girder:x=false:z=false"],
        Join::Block,
        vec![
            rule(
                "girder_pole_side",
                "girder_pole_side_connected",
                Vertical,
                Faces::Sides,
            )
            .seams(false)
            .by_texture(),
        ],
    );
    // `ItemVaultCTBehaviour`: each face's sheet of the vault's size (a large vault's when it is
    // 3 wide, else the medium one's), joined to the same vault block in the same state (Create
    // also asks that it is the same vault).
    for (large, size) in [(false, "medium"), (true, "large")] {
        let state = format!("item_vault:large={large}");
        let rules = ["front", "side", "top", "bottom"]
            .iter()
            .map(|face| {
                let original = format!("vault/vault_{face}_small");
                rule(
                    &original,
                    &format!("vault/vault_{face}_{size}"),
                    Rectangle,
                    Faces::All,
                )
                .seams(false)
                .by_texture()
            })
            .collect();
        plan(&format!("item_vault_{size}"), &[&state], Join::State, rules);
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

/// The folder of `sheet`'s tiles cut for `ty`.
fn tiles_dir(sheet: &str, ty: CtType) -> String {
    format!("{sheet}/{}", ty.name())
}

/// A tile's name in properties files.
fn tile(dir: &str, i: usize) -> String {
    format!("create:optifine/ctm/tiles/{dir}/{i}")
}

/// The properties file of `rule` for `plan`, matching `tiles` (a sprite or a tile picked by
/// another rule) and drawing the tiles of `dir`.
fn properties(plan: &Plan, rule: &Rule, matched: &str, dir: &str) -> String {
    let method = rule.ty.method();
    let count = method.masks().len();
    let mut out = format!(
        "method={}\nmatchTiles={}\nmatchBlocks={}\nconnect={}\ntiles={}\n",
        method.name(),
        matched,
        plan.blocks
            .iter()
            .map(|b| format!("create:{b}"))
            .collect::<Vec<_>>()
            .join(" "),
        match plan.join {
            Join::Block => "block",
            Join::State => "state",
        },
        (0..count)
            .map(|i| tile(dir, i))
            .collect::<Vec<_>>()
            .join(" "),
    );
    push_common(&mut out, rule);
    out
}

fn push_common(out: &mut String, rule: &Rule) {
    match &rule.faces {
        Faces::All => {}
        Faces::Sides => out.push_str("faces=sides\n"),
        Faces::Ends => out.push_str("faces=top bottom\n"),
        Faces::Some(names) => out.push_str(&format!("faces={names}\n")),
    }
    if rule.seams {
        out.push_str("innerSeams=true\n");
    }
    out.push_str(if rule.by_texture {
        "orient=texture\n"
    } else {
        "orient=none\n"
    });
}

/// `FluidTankCTBehaviour` (`HorizontalCTBehaviour` of `RECTANGLE` sheets) for the fluid and
/// creative tanks, as far as a tank block's state tells which of its neighbours are in its tank:
/// up and down by `top` and `bottom`, the sides of a two-wide tank with windows by its corner
/// `shape`. Each face of each state takes the tile of its context, by a `fixed` rule matching the
/// state and the face; a block whose neighbours its state doesn't tell (a plain shape, or a
/// window that may be one block wide or the middle of a three-wide side) joins only up and down.
/// The block's own sprites name the sheets (`FluidTankModel`: the sides, the top and the
/// inside's floor and ceiling).
fn tank_files(
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    out: &mut BTreeMap<String, Vec<u8>>,
    skipped: &mut Vec<String>,
) {
    type V = [i32; 3];
    const N: V = [0, 0, -1];
    const S: V = [0, 0, 1];
    const E: V = [1, 0, 0];
    const W: V = [-1, 0, 0];
    const U: V = [0, 1, 0];
    // (block, side, top, inside) sprites; each has a `_connected` sheet.
    let tanks = [
        (
            "fluid_tank",
            "fluid_tank",
            "fluid_tank_top",
            "fluid_tank_inner",
        ),
        (
            "creative_fluid_tank",
            "creative_fluid_tank",
            "creative_casing",
            "creative_casing",
        ),
    ];
    // Each face and its texture's up and right as read from outside (`buildContext`).
    let faces: [(&str, V, V); 6] = [
        ("north", U, W),
        ("south", U, E),
        ("east", U, N),
        ("west", U, S),
        ("up", N, E),
        ("down", S, E),
    ];
    let shapes = [
        "plain",
        "window",
        "window_nw",
        "window_ne",
        "window_sw",
        "window_se",
    ];
    let mut cut_sheets: BTreeMap<&str, bool> = BTreeMap::new();
    for (block, side, top, inner) in tanks {
        if read(&format!("assets/create/blockstates/{block}.json")).is_none() {
            skipped.push(format!("{block}: no blockstate"));
            continue;
        }
        for sprite in [side, top, inner] {
            if cut_sheets.contains_key(sprite) {
                continue;
            }
            let src = format!("assets/create/textures/block/{sprite}_connected.png");
            let ok = match read(&src).map(|b| decode(&b)) {
                Some(Ok(img)) if img.w == img.h && img.w % 4 == 0 => {
                    let t = img.w / 4;
                    for i in 0..16u32 {
                        let (sx, sy) = ((i % 4) * t, (i / 4) * t);
                        let mut px = Vec::with_capacity((t * t * 4) as usize);
                        for y in 0..t {
                            let s = (((sy + y) * img.w + sx) * 4) as usize;
                            px.extend_from_slice(&img.px[s..s + (t * 4) as usize]);
                        }
                        out.insert(
                            format!(
                                "assets/create/optifine/ctm/tiles/{sprite}_connected/sheet_{i}.png"
                            ),
                            encode(&Image { w: t, h: t, px }),
                        );
                    }
                    true
                }
                _ => {
                    skipped.push(format!(
                        "{sprite}_connected: not a 4x4 sheet in the sources"
                    ));
                    false
                }
            };
            cut_sheets.insert(sprite, ok);
        }
        let mut n = 0;
        for shape in shapes {
            // The horizontal neighbours in its tank a block of this shape surely has.
            let (dx, dz): (i32, i32) = match shape {
                "window_nw" => (1, 1),
                "window_ne" => (-1, 1),
                "window_sw" => (1, -1),
                "window_se" => (-1, -1),
                _ => (0, 0),
            };
            for top_v in [false, true] {
                for bottom_v in [false, true] {
                    let joined = |d: V| match d {
                        [0, 1, 0] => !top_v,
                        [0, -1, 0] => !bottom_v,
                        [x, 0, 0] => x == dx,
                        [0, 0, z] => z == dz,
                        _ => false,
                    };
                    let neg = |v: V| v.map(|c| -c);
                    for (face, up, right) in faces {
                        let sprites: &[&str] = match face {
                            "up" | "down" if top == inner => &[top],
                            "up" | "down" => &[top, inner],
                            _ => &[side],
                        };
                        let ctx = Ctx {
                            up: joined(up),
                            down: joined(neg(up)),
                            left: joined(neg(right)),
                            right: joined(right),
                            ..Ctx::default()
                        };
                        if ctx == Ctx::default() {
                            continue;
                        }
                        let index = CtType::Rectangle.index(ctx);
                        for sprite in sprites {
                            if !cut_sheets.get(sprite).copied().unwrap_or(false) {
                                continue;
                            }
                            let props = format!(
                                "method=fixed\nmatchTiles=create:block/{sprite}\nmatchBlocks=create:{block}:top={top_v}:bottom={bottom_v}:shape={shape}\nfaces={face}\ntiles=create:optifine/ctm/tiles/{sprite}_connected/sheet_{index}\n"
                            );
                            out.insert(
                                format!("assets/create/optifine/ctm/{block}/tank_{n}.properties"),
                                props.into_bytes(),
                            );
                            n += 1;
                        }
                    }
                }
            }
        }
    }
}

/// The files connected textures add: one OptiFine `ctm` properties file per block and sheet,
/// and each sheet's tiles. Block models stay as Create's: only the terrain mesher swaps the
/// tiles in. `read` gives a file of the sources. Returns the files and what was left out, with
/// why.
pub fn files(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    let mut out = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut sheets: BTreeMap<String, bool> = BTreeMap::new();
    // Cuts `sheet` for `ty` once; whether its tiles are there.
    let mut cut_sheet = |out: &mut BTreeMap<String, Vec<u8>>,
                         skipped: &mut Vec<String>,
                         sheet: &str,
                         ty: CtType| {
        let dir = tiles_dir(sheet, ty);
        *sheets.entry(dir.clone()).or_insert_with(|| {
            let src = format!("assets/create/textures/block/{sheet}.png");
            if read(&format!("{src}.mcmeta")).is_some() {
                skipped.push(format!("{sheet}: animated"));
                return false;
            }
            let Some(bytes) = read(&src) else {
                skipped.push(format!("{sheet}: not in the sources"));
                return false;
            };
            match cut(&bytes, ty) {
                Ok(tiles) => {
                    for (n, png) in tiles.into_iter().enumerate() {
                        out.insert(
                            format!("assets/create/optifine/ctm/tiles/{dir}/{n}.png"),
                            png,
                        );
                    }
                    true
                }
                Err(e) => {
                    skipped.push(format!("{sheet}: {e}"));
                    false
                }
            }
        })
    };
    for plan in plans() {
        let block = plan.blocks[0].split(':').next().unwrap_or_default();
        if read(&format!("assets/create/blockstates/{block}.json")).is_none() {
            skipped.push(format!("{}: no blockstate", plan.name));
            continue;
        }
        let file =
            |name: String| format!("assets/create/optifine/ctm/{}/{name}.properties", plan.name);
        for (i, rule) in plan.rules.iter().enumerate() {
            if rule.random.is_empty() {
                if cut_sheet(&mut out, &mut skipped, &rule.sheet, rule.ty) {
                    let dir = tiles_dir(&rule.sheet, rule.ty);
                    out.insert(
                        file(i.to_string()),
                        properties(&plan, rule, &rule.original, &dir).into_bytes(),
                    );
                }
                continue;
            }
            // A random rule picks one sheet's lone tile for the face; a rule per sheet, matching
            // that tile, joins it with that sheet (a tile picked by a rule is matched again by
            // the rules naming it).
            let dirs: Vec<String> = rule
                .random
                .iter()
                .filter(|s| cut_sheet(&mut out, &mut skipped, s, rule.ty))
                .map(|s| tiles_dir(s, rule.ty))
                .collect();
            if dirs.is_empty() {
                continue;
            }
            let mut random = format!(
                "method=random\nmatchTiles={}\nmatchBlocks={}\ntiles={}\n",
                rule.original,
                plan.blocks
                    .iter()
                    .map(|b| format!("create:{b}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                dirs.iter()
                    .map(|d| tile(d, 0))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            match &rule.faces {
                Faces::All => {}
                Faces::Sides => random.push_str("faces=sides\n"),
                Faces::Ends => random.push_str("faces=top bottom\n"),
                Faces::Some(names) => random.push_str(&format!("faces={names}\n")),
            }
            out.insert(file(format!("{i}_random")), random.into_bytes());
            for (k, dir) in dirs.iter().enumerate() {
                out.insert(
                    file(format!("{i}_{k}")),
                    properties(&plan, rule, &tile(dir, 0), dir).into_bytes(),
                );
            }
        }
    }
    tank_files(read, &mut out, &mut skipped);
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
        let find = |name: &str| plans.iter().find(|p| p.name == name).unwrap();
        let plan = find("horizontal_framed_glass");
        let rule = &plan.rules[0];
        let text = properties(plan, rule, &rule.original, "x");
        assert!(text.contains("method=horizontal\n"));
        assert!(text.contains("matchTiles=create:block/palettes/framed_glass\n"));
        assert!(text.contains("matchBlocks=create:horizontal_framed_glass\n"));
        assert!(text.contains("connect=block\n"));
        assert!(text.contains("faces=sides\n"));
        assert!(text.contains("innerSeams=true\n"));
        assert!(text.contains("orient=none\n"));
        assert!(text.contains("tiles=create:optifine/ctm/tiles/x/0 "));
        let pane = find("framed_glass_pane");
        let text = properties(pane, &pane.rules[0], &pane.rules[0].original, "x");
        assert!(!text.contains("innerSeams") && !text.contains("faces="));
        // An encased shaft joins its own state on the faces off its axis.
        let shaft = find("andesite_encased_shaft_x");
        let text = properties(shaft, &shaft.rules[0], &shaft.rules[0].original, "x");
        assert!(text.contains("matchBlocks=create:andesite_encased_shaft:axis=x\n"));
        assert!(text.contains("connect=state\n"));
        assert!(text.contains("faces=north south top bottom\n"));
        // A pillar's sides read its turned texture.
        let pillar = find("granite_pillar");
        let text = properties(pillar, &pillar.rules[0], &pillar.rules[0].original, "x");
        assert!(text.contains("method=ctm\n") && text.contains("orient=texture\n"));
    }

    #[test]
    fn horizontal_sheets_by_create_and_by_texture() {
        // `HORIZONTAL`: joined right is the second tile; read by texture, joined left is.
        let side = |l, r| Ctx {
            left: l,
            right: r,
            ..Ctx::default()
        };
        assert_eq!(CtType::Horizontal.index(side(false, true)), 1);
        assert_eq!(CtType::HorizontalByTexture.index(side(true, false)), 1);
        assert_eq!(CtType::HorizontalByTexture.index(side(true, true)), 3);
    }

    #[test]
    fn the_weathered_window_picks_a_sheet_then_joins_it() {
        let sheet = |k: u8| {
            let img = Image {
                w: 4,
                h: 4,
                px: vec![k; 64],
            };
            encode(&img)
        };
        let read = |path: &str| -> Option<Vec<u8>> {
            if path.ends_with("blockstates/weathered_iron_window.json") {
                return Some(Vec::new());
            }
            let k = path
                .strip_prefix("assets/create/textures/block/palettes/weathered_iron_window_")?
                .strip_suffix("_connected.png")?;
            Some(sheet(k.parse().ok()?))
        };
        let (files, _) = files(&read);
        let text = |path: &str| {
            String::from_utf8(files[&format!("assets/create/optifine/ctm/{path}")].clone()).unwrap()
        };
        let random = text("weathered_iron_window/0_random.properties");
        assert!(random.contains("method=random\n"));
        let first =
            "create:optifine/ctm/tiles/palettes/weathered_iron_window_1_connected/rectangle/0";
        assert!(random.contains(first));
        let joined = text("weathered_iron_window/0_0.properties");
        assert!(joined.contains(&format!("matchTiles={first}\n")));
        assert!(joined.contains("method=ctm\n"));
    }
}
