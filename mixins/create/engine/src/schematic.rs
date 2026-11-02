//! Create's schematic files and items (`content/schematics`): vanilla structure templates
//! (`StructureTemplate.save`/`load`, the `.nbt` files Create writes with `SchematicExport` and
//! reads with `SchematicItem.loadSchematic`), their placement transforms (`StructureTemplate
//! .transform`, `BlockState.mirror`/`rotate`), the schematic item's data and the block facts the
//! printer reads of each palette entry, resolved once per loaded schematic.
//!
//! The tree here is a plain copy of NBT: the game's binary reader and writer (gzip, `NbtIo`) are
//! Horizon's; the glue converts between the two once per file.

use std::collections::BTreeMap;

/// An NBT tag (`net.minecraft.nbt.Tag`); floats keep their bit patterns, as Horizon's do.
#[derive(Clone, Debug, PartialEq)]
pub enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Nbt>),
    Compound(BTreeMap<String, Nbt>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    pub fn compound<'a>(entries: impl IntoIterator<Item = (&'a str, Nbt)>) -> Nbt {
        Nbt::Compound(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(c) => c.get(key),
            _ => None,
        }
    }
    fn int(&self) -> Option<i32> {
        match self {
            Nbt::Int(v) => Some(*v),
            Nbt::Byte(v) => Some(i32::from(*v)),
            Nbt::Short(v) => Some(i32::from(*v)),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Nbt::String(s) => Some(s),
            _ => None,
        }
    }
    /// `ListTag.getInt(i)` over a list of ints, or an int array, as three coordinates.
    fn ints3(&self) -> [i32; 3] {
        let mut out = [0; 3];
        match self {
            Nbt::List(l) => {
                for (o, v) in out.iter_mut().zip(l) {
                    *o = v.int().unwrap_or(0);
                }
            }
            Nbt::IntArray(a) => {
                for (o, v) in out.iter_mut().zip(a) {
                    *o = *v;
                }
            }
            _ => {}
        }
        out
    }
}

fn int_list(v: [i32; 3]) -> Nbt {
    Nbt::List(v.iter().map(|i| Nbt::Int(*i)).collect())
}

/// `SharedConstants.getCurrentVersion().getDataVersion()` of Create 6.0.11's game (1.21.1), which
/// `NbtUtils.addCurrentDataVersion` writes into each schematic.
pub const DATA_VERSION: i32 = 3955;

/// A palette entry (`NbtUtils.writeBlockState`): the block and its properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl PaletteState {
    pub fn air() -> Self {
        Self {
            name: AIR.into(),
            properties: BTreeMap::new(),
        }
    }
}

/// `StructureBlockInfo`: a block's position in the template, its palette index and its block
/// entity's data.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockInfo {
    pub pos: [i32; 3],
    pub state: usize,
    pub nbt: Option<Nbt>,
}

/// `StructureEntityInfo`, kept as read (the printer doesn't place entities yet).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityInfo {
    pub pos: [f64; 3],
    pub block_pos: [i32; 3],
    pub nbt: Nbt,
}

/// A structure template as `StructureTemplate` holds it: its size, its (first) palette, its
/// blocks in file order and its entities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Structure {
    pub data_version: Option<i32>,
    pub size: [i32; 3],
    pub palette: Vec<PaletteState>,
    pub blocks: Vec<BlockInfo>,
    pub entities: Vec<EntityInfo>,
}

pub const AIR: &str = "minecraft:air";
pub const STRUCTURE_VOID: &str = "minecraft:structure_void";

/// How `StructureTemplate.fillFromWorld`'s `buildInfoList` orders a captured block: full blocks
/// first, then the others, then those with a block entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Full,
    Special,
    WithEntity,
}

impl Structure {
    /// `StructureTemplate.load`: `size`, the `palette` (or the first of `palettes`, which a
    /// single-palette placement uses), `blocks` and `entities` (those with `nbt`). A palette entry
    /// is read as `NbtUtils.readBlockState` reads it, `Name` and `Properties` (26.3's `id` and
    /// `properties` too); a missing block name reads as air. None when `tag` isn't a compound.
    pub fn from_nbt(tag: &Nbt) -> Option<Structure> {
        let Nbt::Compound(_) = tag else {
            return None;
        };
        let size = tag.get("size").map_or([0; 3], Nbt::ints3);
        let palette_list = match (tag.get("palettes"), tag.get("palette")) {
            (Some(Nbt::List(lists)), _) => match lists.first() {
                Some(Nbt::List(p)) => p.as_slice(),
                _ => &[],
            },
            (_, Some(Nbt::List(p))) => p.as_slice(),
            _ => &[],
        };
        let palette = palette_list
            .iter()
            .map(|t| {
                let name = t
                    .get("Name")
                    .or_else(|| t.get("id"))
                    .and_then(Nbt::text)
                    .unwrap_or(AIR)
                    .to_owned();
                let properties = match t.get("Properties").or_else(|| t.get("properties")) {
                    Some(Nbt::Compound(p)) => p
                        .iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.text()?.to_owned())))
                        .collect(),
                    _ => BTreeMap::new(),
                };
                PaletteState { name, properties }
            })
            .collect();
        let blocks = match tag.get("blocks") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter(|b| matches!(b, Nbt::Compound(_)))
                .map(|b| BlockInfo {
                    pos: b.get("pos").map_or([0; 3], Nbt::ints3),
                    state: b.get("state").and_then(Nbt::int).unwrap_or(0).max(0) as usize,
                    nbt: b
                        .get("nbt")
                        .filter(|n| matches!(n, Nbt::Compound(_)))
                        .cloned(),
                })
                .collect(),
            _ => Vec::new(),
        };
        let entities = match tag.get("entities") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let nbt = e.get("nbt").filter(|n| matches!(n, Nbt::Compound(_)))?;
                    let mut pos = [0.0; 3];
                    if let Some(Nbt::List(p)) = e.get("pos") {
                        for (o, v) in pos.iter_mut().zip(p) {
                            if let Nbt::Double(bits) = v {
                                *o = f64::from_bits(*bits);
                            }
                        }
                    }
                    Some(EntityInfo {
                        pos,
                        block_pos: e.get("blockPos").map_or([0; 3], Nbt::ints3),
                        nbt: nbt.clone(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Structure {
            data_version: tag.get("DataVersion").and_then(Nbt::int),
            size,
            palette,
            blocks,
            entities,
        })
    }

    /// `StructureTemplate.save`: `DataVersion`, `size`, `palette`, `blocks` (with `nbt` where a
    /// block has data) and `entities`.
    pub fn to_nbt(&self) -> Nbt {
        let palette = self
            .palette
            .iter()
            .map(|p| {
                let mut c = BTreeMap::new();
                c.insert("Name".to_owned(), Nbt::String(p.name.clone()));
                if !p.properties.is_empty() {
                    c.insert(
                        "Properties".to_owned(),
                        Nbt::Compound(
                            p.properties
                                .iter()
                                .map(|(k, v)| (k.clone(), Nbt::String(v.clone())))
                                .collect(),
                        ),
                    );
                }
                Nbt::Compound(c)
            })
            .collect();
        let blocks = self
            .blocks
            .iter()
            .map(|b| {
                let mut c = BTreeMap::new();
                c.insert("pos".to_owned(), int_list(b.pos));
                c.insert("state".to_owned(), Nbt::Int(b.state as i32));
                if let Some(n) = &b.nbt {
                    c.insert("nbt".to_owned(), n.clone());
                }
                Nbt::Compound(c)
            })
            .collect();
        let entities = self
            .entities
            .iter()
            .map(|e| {
                Nbt::compound([
                    (
                        "pos",
                        Nbt::List(e.pos.iter().map(|v| Nbt::Double(v.to_bits())).collect()),
                    ),
                    ("blockPos", int_list(e.block_pos)),
                    ("nbt", e.nbt.clone()),
                ])
            })
            .collect();
        Nbt::compound([
            (
                "DataVersion",
                Nbt::Int(self.data_version.unwrap_or(DATA_VERSION)),
            ),
            ("size", int_list(self.size)),
            ("palette", Nbt::List(palette)),
            ("blocks", Nbt::List(blocks)),
            ("entities", Nbt::List(entities)),
        ])
    }

    /// `SchematicExport.saveSchematic`'s template: `fillFromWorld(level, origin, bounds, true,
    /// Blocks.AIR)` over the captured cells (position from the origin, block, class), air left
    /// out; `buildInfoList`'s order (each class by y, then x, then z) and palette (in that order
    /// of first use); then `replaceStructureVoidWithAir` renames structure voids in the palette.
    pub fn capture(
        size: [i32; 3],
        cells: impl IntoIterator<Item = ([i32; 3], PaletteState, Class)>,
    ) -> Structure {
        let mut cells: Vec<([i32; 3], PaletteState, Class)> =
            cells.into_iter().filter(|c| c.1.name != AIR).collect();
        cells.sort_by_key(|(p, _, class)| (*class, p[1], p[0], p[2]));
        let mut palette: Vec<PaletteState> = Vec::new();
        let mut blocks = Vec::with_capacity(cells.len());
        for (pos, state, _) in cells {
            let index = match palette.iter().position(|s| *s == state) {
                Some(i) => i,
                None => {
                    palette.push(state);
                    palette.len() - 1
                }
            };
            blocks.push(BlockInfo {
                pos,
                state: index,
                nbt: None,
            });
        }
        for p in &mut palette {
            if p.name == STRUCTURE_VOID {
                p.name = AIR.into();
            }
        }
        Structure {
            data_version: Some(DATA_VERSION),
            size,
            palette,
            blocks,
            entities: Vec::new(),
        }
    }
}

/// `net.minecraft.world.level.block.Rotation`, in its ordinal order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];
    /// `Rotation.name()`.
    pub fn name(self) -> &'static str {
        [
            "NONE",
            "CLOCKWISE_90",
            "CLOCKWISE_180",
            "COUNTERCLOCKWISE_90",
        ][self as usize]
    }
    /// `Rotation.valueOf`, ignoring case; also the serialized names (`clockwise_90`, `180`).
    pub fn parse(s: &str) -> Option<Rotation> {
        let s = s.to_ascii_uppercase();
        Rotation::ALL
            .into_iter()
            .find(|r| r.name() == s)
            .or(match s.as_str() {
                "180" => Some(Rotation::Clockwise180),
                _ => None,
            })
    }
    /// Quarter turns clockwise (seen from above).
    fn quarters(self) -> usize {
        [0, 1, 2, 3][self as usize]
    }
}

/// `net.minecraft.world.level.block.Mirror`, in its ordinal order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mirror {
    #[default]
    None,
    LeftRight,
    FrontBack,
}

impl Mirror {
    pub const ALL: [Mirror; 3] = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack];
    pub fn name(self) -> &'static str {
        ["NONE", "LEFT_RIGHT", "FRONT_BACK"][self as usize]
    }
    pub fn parse(s: &str) -> Option<Mirror> {
        let s = s.to_ascii_uppercase();
        Mirror::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// `StructureTemplate.transform(pos, mirror, rotation, pivot)`.
pub fn transform(pos: [i32; 3], mirror: Mirror, rotation: Rotation, pivot: [i32; 3]) -> [i32; 3] {
    let [mut x, y, mut z] = pos;
    match mirror {
        Mirror::LeftRight => z = -z,
        Mirror::FrontBack => x = -x,
        Mirror::None => {}
    }
    let (l, i1) = (pivot[0], pivot[2]);
    match rotation {
        Rotation::Counterclockwise90 => [l - i1 + z, y, l + i1 - x],
        Rotation::Clockwise90 => [l + i1 - z, y, i1 - l + x],
        Rotation::Clockwise180 => [l + l - x, y, i1 + i1 - z],
        Rotation::None => [x, y, z],
    }
}

const HORIZONTAL: [&str; 4] = ["north", "east", "south", "west"];

/// `Rotation.rotate(Direction)`: up and down stay.
fn rotate_dir(d: &str, r: Rotation) -> String {
    match HORIZONTAL.iter().position(|h| *h == d) {
        Some(i) => HORIZONTAL[(i + r.quarters()) % 4].to_owned(),
        None => d.to_owned(),
    }
}

/// `Mirror.getRotation(Direction)`: a half turn for a facing along the mirrored axis.
fn mirror_rotation(m: Mirror, facing: &str) -> Rotation {
    match (m, facing) {
        (Mirror::LeftRight, "north" | "south") | (Mirror::FrontBack, "east" | "west") => {
            Rotation::Clockwise180
        }
        _ => Rotation::None,
    }
}

/// `Rotation.rotate(int, 16)` and `Mirror.mirror(int, 16)` of a 16-step `rotation`.
fn rotate16(r: i32, rot: Rotation) -> i32 {
    (r + rot.quarters() as i32 * 4).rem_euclid(16)
}
fn mirror16(r: i32, m: Mirror) -> i32 {
    match m {
        Mirror::LeftRight => (16 - r).rem_euclid(16),
        Mirror::FrontBack => (8 - r).rem_euclid(16),
        Mirror::None => r,
    }
}

const RAIL_SHAPES: [&str; 10] = [
    "north_south",
    "east_west",
    "ascending_east",
    "ascending_west",
    "ascending_north",
    "ascending_south",
    "south_east",
    "south_west",
    "north_west",
    "north_east",
];

/// `RailBlock.rotate` (and the straight rails'): a rail shape turned.
fn rotate_rail(shape: &str, r: Rotation) -> String {
    // Each shape's directions turned clockwise once.
    let cw = |s: &str| -> &'static str {
        match s {
            "north_south" => "east_west",
            "east_west" => "north_south",
            "ascending_east" => "ascending_south",
            "ascending_west" => "ascending_north",
            "ascending_north" => "ascending_east",
            "ascending_south" => "ascending_west",
            "south_east" => "south_west",
            "south_west" => "north_west",
            "north_west" => "north_east",
            "north_east" => "south_east",
            _ => "",
        }
    };
    let mut s = shape.to_owned();
    for _ in 0..r.quarters() {
        let next = cw(&s);
        if next.is_empty() {
            return shape.to_owned();
        }
        s = next.to_owned();
    }
    s
}

/// `RailBlock.mirror`.
fn mirror_rail(shape: &str, m: Mirror) -> String {
    let swap = |a: &str, b: &str| -> Option<String> {
        if shape == a {
            Some(b.into())
        } else if shape == b {
            Some(a.into())
        } else {
            None
        }
    };
    let out = match m {
        Mirror::LeftRight => swap("ascending_north", "ascending_south")
            .or_else(|| swap("south_east", "north_east"))
            .or_else(|| swap("south_west", "north_west")),
        Mirror::FrontBack => swap("ascending_east", "ascending_west")
            .or_else(|| swap("south_east", "south_west"))
            .or_else(|| swap("north_west", "north_east")),
        Mirror::None => None,
    };
    out.unwrap_or_else(|| shape.to_owned())
}

fn is_rail(name: &str, props: &BTreeMap<String, String>) -> bool {
    name.ends_with("rail")
        && props
            .get("shape")
            .is_some_and(|s| RAIL_SHAPES.contains(&s.as_str()))
}

fn is_stairs(props: &BTreeMap<String, String>) -> bool {
    props.contains_key("facing")
        && props.contains_key("half")
        && props.get("shape").is_some_and(|s| {
            matches!(
                s.as_str(),
                "straight" | "inner_left" | "inner_right" | "outer_left" | "outer_right"
            )
        })
}

/// `BlockState.rotate(rotation)`, by the properties vanilla's and Create's blocks turn: a
/// sixteen-step `rotation`, a rail's `shape`, `facing`, `axis` (x and z swap on a quarter turn),
/// the four sides of fences, walls, panes, wire and vines, and Create's `axis_along_first`
/// (`DirectionalAxisKineticBlock`: it flips on a quarter turn).
pub fn rotate_state(name: &str, props: &mut BTreeMap<String, String>, r: Rotation) {
    if r == Rotation::None {
        return;
    }
    if let Some(v) = props.get_mut("rotation")
        && let Ok(n) = v.parse::<i32>()
    {
        *v = rotate16(n, r).to_string();
    }
    if is_rail(name, props) {
        let shape = props["shape"].clone();
        props.insert("shape".into(), rotate_rail(&shape, r));
    }
    if let Some(f) = props.get_mut("facing") {
        *f = rotate_dir(f, r);
    }
    if r != Rotation::Clockwise180 {
        if let Some(a) = props.get_mut("axis") {
            match a.as_str() {
                "x" => *a = "z".into(),
                "z" => *a = "x".into(),
                _ => {}
            }
        }
        if let Some(v) = props.get_mut("axis_along_first") {
            *v = if v == "true" { "false" } else { "true" }.into();
        }
    }
    if HORIZONTAL.iter().all(|h| props.contains_key(*h)) {
        let old: Vec<String> = HORIZONTAL.iter().map(|h| props[*h].clone()).collect();
        for (i, h) in HORIZONTAL.iter().enumerate() {
            // The side that turns to `h`.
            let from = (i + 4 - r.quarters()) % 4;
            props.insert((*h).into(), old[from].clone());
        }
    }
}

/// `BlockState.mirror(mirror)`: a sixteen-step `rotation` mirrored; stairs (`StairBlock.mirror`:
/// along their facing's axis they turn round and their corners change hands, otherwise they stay);
/// doors (`DoorBlock.mirror`: turned and `hinge` swapped) and chests (`ChestBlock.mirror`: turned
/// and `type` swapped); a rail's `shape`; any other `facing` turned round when it lies along the
/// mirrored axis (`state.rotate(mirror.getRotation(facing))`); the four sides swapped across it.
pub fn mirror_state(name: &str, props: &mut BTreeMap<String, String>, m: Mirror) {
    if m == Mirror::None {
        return;
    }
    if let Some(v) = props.get_mut("rotation")
        && let Ok(n) = v.parse::<i32>()
    {
        *v = mirror16(n, m).to_string();
        return;
    }
    if is_stairs(props) {
        let facing = props["facing"].clone();
        if mirror_rotation(m, &facing) == Rotation::Clockwise180 {
            let shape = match props["shape"].as_str() {
                "inner_left" => "inner_right",
                "inner_right" => "inner_left",
                "outer_left" => "outer_right",
                "outer_right" => "outer_left",
                s => s,
            }
            .to_owned();
            rotate_state(name, props, Rotation::Clockwise180);
            props.insert("shape".into(), shape);
        }
        return;
    }
    if is_rail(name, props) {
        let shape = props["shape"].clone();
        props.insert("shape".into(), mirror_rail(&shape, m));
        return;
    }
    if let Some(facing) = props.get("facing").cloned() {
        let turn = mirror_rotation(m, &facing);
        rotate_state(name, props, turn);
        if let Some(h) = props.get_mut("hinge") {
            *h = if h == "left" { "right" } else { "left" }.into();
        }
        if let Some(t) = props.get_mut("type") {
            match t.as_str() {
                "left" => *t = "right".into(),
                "right" => *t = "left".into(),
                _ => {}
            }
        }
        return;
    }
    if HORIZONTAL.iter().all(|h| props.contains_key(*h)) {
        let (a, b) = match m {
            Mirror::LeftRight => ("north", "south"),
            _ => ("east", "west"),
        };
        let (va, vb) = (props[a].clone(), props[b].clone());
        props.insert(a.into(), vb);
        props.insert(b.into(), va);
    }
}

/// `info.state.mirror(mirror).rotate(rotation)`, as `StructureTemplate.placeInWorld` turns each
/// block.
pub fn transform_state(
    name: &str,
    props: &mut BTreeMap<String, String>,
    mirror: Mirror,
    rotation: Rotation,
) {
    mirror_state(name, props, mirror);
    rotate_state(name, props, rotation);
}

/// The schematic item's data (`AllDataComponents.SCHEMATIC_*`): its file, owner, whether it is
/// deployed, where and how; `bounds` is the template's size (`writeSize`). Kept in the stack's
/// `minecraft:custom_data` under Create's former NBT keys (Horizon adds no component types).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchematicData {
    pub file: Option<String>,
    pub owner: Option<String>,
    pub deployed: Option<bool>,
    pub anchor: Option<[i32; 3]>,
    pub rotation: Rotation,
    pub mirror: Mirror,
    pub bounds: Option<[i32; 3]>,
}

impl SchematicData {
    /// `SchematicItem.create(level, schematic, owner)`: not deployed, anchored at zero, unturned,
    /// with the template's size.
    pub fn written(file: &str, owner: &str, bounds: [i32; 3]) -> Self {
        Self {
            file: Some(file.to_owned()),
            owner: Some(owner.to_owned()),
            deployed: Some(false),
            anchor: Some([0; 3]),
            rotation: Rotation::None,
            mirror: Mirror::None,
            bounds: Some(bounds),
        }
    }

    /// The keys this data takes in `minecraft:custom_data`.
    pub fn to_entries(&self) -> Vec<(&'static str, Nbt)> {
        let mut out = Vec::new();
        if let Some(f) = &self.file {
            out.push(("File", Nbt::String(f.clone())));
        }
        if let Some(o) = &self.owner {
            out.push(("Owner", Nbt::String(o.clone())));
        }
        if let Some(d) = self.deployed {
            out.push(("Deployed", Nbt::Byte(i8::from(d))));
        }
        if let Some(a) = self.anchor {
            out.push(("Anchor", Nbt::IntArray(a.to_vec())));
        }
        if self.anchor.is_some() {
            out.push(("Rotation", Nbt::String(self.rotation.name().into())));
            out.push(("Mirror", Nbt::String(self.mirror.name().into())));
        }
        if let Some(b) = self.bounds {
            out.push(("Bounds", Nbt::IntArray(b.to_vec())));
        }
        out
    }

    /// Read back from `minecraft:custom_data` (`get` gives a key's tag).
    pub fn from_entries<'a>(get: impl Fn(&str) -> Option<&'a Nbt>) -> Self {
        let text = |k: &str| get(k).and_then(Nbt::text).map(str::to_owned);
        let ints = |k: &str| match get(k) {
            Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
            _ => None,
        };
        Self {
            file: text("File"),
            owner: text("Owner"),
            deployed: get("Deployed").and_then(Nbt::int).map(|v| v != 0),
            anchor: ints("Anchor"),
            rotation: text("Rotation")
                .and_then(|r| Rotation::parse(&r))
                .unwrap_or_default(),
            mirror: text("Mirror")
                .and_then(|m| Mirror::parse(&m))
                .unwrap_or_default(),
            bounds: ints("Bounds"),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// `FilesHelper.slug`: lower case, each run of non-word characters one `_`.
pub fn slug(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut run = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            run = false;
        } else if !run {
            out.push('_');
            run = true;
        }
    }
    out
}

/// `SchematicExport.saveSchematic`'s file name: Create's fallback name when empty; without
/// overwriting, `findFirstValidFilename` (the slug, then `_1`, `_2`... while `exists`); `.nbt`
/// added.
pub fn export_name(name: &str, overwrite: bool, exists: impl Fn(&str) -> bool) -> String {
    let mut name = if name.is_empty() {
        "My Schematic".to_owned()
    } else {
        name.to_owned()
    };
    if !overwrite {
        let mut index = 0;
        loop {
            let candidate = if index == 0 {
                format!("{}.nbt", slug(&name))
            } else {
                format!("{}_{index}.nbt", slug(&name))
            };
            index += 1;
            if !exists(&candidate) {
                name = candidate;
                break;
            }
        }
    }
    if !name.ends_with(".nbt") {
        name.push_str(".nbt");
    }
    name
}

/// A file name a player may name on the server (`ServerSchematicLoader`'s path checks: no way
/// out of the player's folder) with Create's `.nbt` format.
pub fn valid_file(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".nbt")
        && !name.contains(['/', '\\'])
        && name != ".."
        && !name.starts_with("..")
}

/// `ServerSchematicLoader.validateSchematicSizeOnServer`: `maxTotalSchematicSize` (256 KB) in
/// thousands of bytes.
pub const MAX_TOTAL_SCHEMATIC_SIZE_KB: u64 = 256;
/// `CSchematics.maxSchematics`: uploads a player keeps before the oldest is replaced.
pub const MAX_SCHEMATICS: usize = 10;

pub fn size_allowed(bytes: u64) -> bool {
    bytes <= MAX_TOTAL_SCHEMATIC_SIZE_KB * 1000
}

/// What a block in a schematic costs (`ItemRequirement.defaultOf`): nothing for air, else its
/// item (`item` names another one) and how many.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    Nothing,
    Item {
        count: u32,
        item: Option<&'static str>,
    },
}

pub fn need(name: &str, props: &BTreeMap<String, String>) -> Need {
    let p = |k: &str| props.get(k).map(String::as_str);
    let n = |k: &str| p(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
    let item = |count, item| Need::Item { count, item };
    match name {
        AIR => Need::Nothing,
        _ if p("type") == Some("double") && name.ends_with("_slab") => item(2, None),
        "minecraft:turtle_egg" => item(n("eggs"), None),
        "minecraft:sea_pickle" => item(n("pickles"), None),
        "minecraft:snow" => item(n("layers"), None),
        "minecraft:farmland" | "minecraft:dirt_path" => item(1, Some("minecraft:dirt")),
        "minecraft:tall_grass" => item(2, Some("minecraft:short_grass")),
        "minecraft:large_fern" => item(2, Some("minecraft:fern")),
        _ => item(1, None),
    }
}

const CAULDRONS: [&str; 4] = [
    "minecraft:cauldron",
    "minecraft:water_cauldron",
    "minecraft:lava_cauldron",
    "minecraft:powder_snow_cauldron",
];

/// The state the printer reads of a block (`SchematicLevel.processBlockStateForPrinting`: a lit
/// furnace unlit) and aims for (`BlockHelper.setZeroAge`: the first of its age, honey level,
/// hatch, stage, cauldron, composter level and piston extension reset).
pub fn printed_state(name: &mut String, props: &mut BTreeMap<String, String>) {
    if matches!(
        name.as_str(),
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker"
    ) && let Some(lit) = props.get_mut("lit")
    {
        *lit = "false".into();
    }
    for key in ["age", "honey_level", "hatch", "stage"] {
        if let Some(v) = props.get_mut(key) {
            *v = "0".into();
            return;
        }
    }
    if CAULDRONS.contains(&name.as_str()) {
        *name = "minecraft:cauldron".into();
        props.clear();
        return;
    }
    if name == "minecraft:composter"
        && let Some(v) = props.get_mut("level")
    {
        *v = "0".into();
        return;
    }
    if let Some(v) = props.get_mut("extended") {
        *v = "false".into();
    }
}

/// What `BlockHelper.placeSchematicBlock` puts in the world for a printed state: pistons
/// retracted, nothing waterlogged, a composter and any cauldron empty.
pub fn placed_state(name: &mut String, props: &mut BTreeMap<String, String>) {
    if let Some(v) = props.get_mut("extended") {
        *v = "false".into();
    }
    if let Some(v) = props.get_mut("waterlogged") {
        *v = "false".into();
    }
    if name == "minecraft:composter" {
        props.clear();
    } else if CAULDRONS.contains(&name.as_str()) {
        *name = "minecraft:cauldron".into();
        props.clear();
    }
}

/// `SchematicannonBlockEntity.shouldIgnoreBlockState` past its requirement checks: structure
/// voids; the upper half of a double block, a bed's head and a piston head (placed with the
/// rest); a belt's middle segments.
pub fn ignored(name: &str, props: &BTreeMap<String, String>, need: Option<&Need>) -> bool {
    if name == STRUCTURE_VOID {
        return true;
    }
    match need {
        Some(Need::Nothing) | None => return false,
        Some(Need::Item { .. }) => {}
    }
    let p = |k: &str| props.get(k).map(String::as_str);
    if p("half") == Some("upper") {
        return true;
    }
    if p("part") == Some("head") && props.contains_key("occupied") {
        return true;
    }
    if name == "minecraft:piston_head" {
        return true;
    }
    if name == "create:belt" {
        return p("part") == Some("middle");
    }
    false
}

/// The other block a two-block state replaces (`SchematicPrinter.shouldPlaceBlock`'s
/// `toReplaceOther`): toward a bed foot's facing, or above a lower half.
pub fn other_half(props: &BTreeMap<String, String>) -> Option<[i32; 3]> {
    let p = |k: &str| props.get(k).map(String::as_str);
    if p("part") == Some("foot") && props.contains_key("occupied") {
        return match p("facing")? {
            "north" => Some([0, 0, -1]),
            "south" => Some([0, 0, 1]),
            "west" => Some([-1, 0, 0]),
            "east" => Some([1, 0, 0]),
            _ => None,
        };
    }
    (p("half") == Some("lower")).then_some([0, 1, 0])
}

/// `create:brittle`'s own blocks (its nested tags are matched by name in `brittle`).
const BRITTLE: [&str; 36] = [
    "create:nozzle",
    "create:hand_crank",
    "create:copper_valve_handle",
    "create:white_valve_handle",
    "create:orange_valve_handle",
    "create:magenta_valve_handle",
    "create:light_blue_valve_handle",
    "create:yellow_valve_handle",
    "create:lime_valve_handle",
    "create:pink_valve_handle",
    "create:gray_valve_handle",
    "create:light_gray_valve_handle",
    "create:cyan_valve_handle",
    "create:purple_valve_handle",
    "create:blue_valve_handle",
    "create:brown_valve_handle",
    "create:green_valve_handle",
    "create:red_valve_handle",
    "create:black_valve_handle",
    "create:rope",
    "create:pulley_magnet",
    "create:redstone_link",
    "create:peculiar_bell",
    "create:haunted_bell",
    "minecraft:bell",
    "minecraft:cocoa",
    "minecraft:flower_pot",
    "minecraft:moss_carpet",
    "minecraft:bamboo_sapling",
    "minecraft:big_dripleaf",
    "minecraft:vine",
    "minecraft:twisting_vines_plant",
    "minecraft:twisting_vines",
    "minecraft:weeping_vines_plant",
    "minecraft:weeping_vines",
    "minecraft:cake",
];

const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// `BlockMovementChecks.isBrittle` (`isBrittleFallback`): hanging blocks, ladders, torches,
/// signs, pressure plates, buttons and levers, rails (not the cart assembler), diodes, wire,
/// wool carpets, whistles, belt funnels and `create:brittle` (with its corals, cave vines,
/// banners, beds and doors).
pub fn brittle(name: &str, props: &BTreeMap<String, String>) -> bool {
    let path = name.split_once(':').map_or(name, |(_, p)| p);
    if props.contains_key("hanging") {
        return true;
    }
    let carpet = path
        .strip_suffix("_carpet")
        .is_some_and(|c| DYES.contains(&c));
    path.ends_with("ladder")
        || path.ends_with("torch")
        || path.ends_with("_sign")
        || path.ends_with("_pressure_plate")
        || path.ends_with("_button")
        || path == "lever"
        || path.ends_with("rail")
        || matches!(
            name,
            "minecraft:repeater"
                | "minecraft:comparator"
                | "minecraft:redstone_wire"
                | "create:pulse_repeater"
                | "create:pulse_extender"
                | "create:pulse_timer"
                | "create:powered_latch"
                | "create:powered_toggle_latch"
                | "create:steam_whistle"
                | "create:steam_whistle_extension"
                | "create:andesite_belt_funnel"
                | "create:brass_belt_funnel"
                | "minecraft:cave_vines"
                | "minecraft:cave_vines_plant"
        )
        || carpet
        || BRITTLE.contains(&name)
        || (name.starts_with("minecraft:") && path.contains("coral") && !path.ends_with("_block"))
        || path.ends_with("_banner")
        || path.ends_with("_bed")
        || path.ends_with("_door")
}

/// `SchematicPrinter.shouldDeferBlock`: gantry carriages, mechanical arms and brittle blocks are
/// printed after the rest.
pub fn deferred(name: &str, props: &BTreeMap<String, String>) -> bool {
    matches!(name, "create:gantry_carriage" | "create:mechanical_arm") || brittle(name, props)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(p: &[(&str, &str)]) -> BTreeMap<String, String> {
        p.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn state(name: &str, p: &[(&str, &str)]) -> PaletteState {
        PaletteState {
            name: name.into(),
            properties: props(p),
        }
    }

    #[test]
    fn a_structure_round_trips_through_its_nbt() {
        let s = Structure {
            data_version: Some(DATA_VERSION),
            size: [3, 2, 1],
            palette: vec![
                state("minecraft:stone", &[]),
                state(
                    "minecraft:oak_stairs",
                    &[("facing", "east"), ("half", "bottom")],
                ),
            ],
            blocks: vec![
                BlockInfo {
                    pos: [0, 0, 0],
                    state: 0,
                    nbt: None,
                },
                BlockInfo {
                    pos: [2, 1, 0],
                    state: 1,
                    nbt: Some(Nbt::compound([("id", Nbt::String("x".into()))])),
                },
            ],
            entities: vec![EntityInfo {
                pos: [0.5, 1.0, 0.25],
                block_pos: [0, 1, 0],
                nbt: Nbt::compound([("id", Nbt::String("minecraft:armor_stand".into()))]),
            }],
        };
        let tag = s.to_nbt();
        assert_eq!(tag.get("DataVersion"), Some(&Nbt::Int(3955)));
        assert_eq!(
            tag.get("palette")
                .and_then(|p| match p {
                    Nbt::List(l) => l[1].get("Properties")?.get("facing").cloned(),
                    _ => None,
                })
                .as_ref(),
            Some(&Nbt::String("east".into()))
        );
        assert_eq!(Structure::from_nbt(&tag), Some(s));
    }

    #[test]
    fn reading_takes_the_first_palette_and_26_3_names() {
        let tag = Nbt::compound([
            (
                "size",
                Nbt::List(vec![Nbt::Int(1), Nbt::Int(1), Nbt::Int(1)]),
            ),
            (
                "palettes",
                Nbt::List(vec![
                    Nbt::List(vec![Nbt::compound([(
                        "id",
                        Nbt::String("minecraft:dirt".into()),
                    )])]),
                    Nbt::List(vec![Nbt::compound([(
                        "Name",
                        Nbt::String("minecraft:stone".into()),
                    )])]),
                ]),
            ),
            (
                "blocks",
                Nbt::List(vec![Nbt::compound([
                    (
                        "pos",
                        Nbt::List(vec![Nbt::Int(0), Nbt::Int(0), Nbt::Int(0)]),
                    ),
                    ("state", Nbt::Int(0)),
                ])]),
            ),
        ]);
        let s = Structure::from_nbt(&tag).unwrap();
        assert_eq!(s.palette, vec![state("minecraft:dirt", &[])]);
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.data_version, None);
    }

    #[test]
    fn capture_orders_blocks_as_build_info_list_and_skips_air() {
        let stone = state("minecraft:stone", &[]);
        let torch = state("minecraft:torch", &[]);
        let s = Structure::capture(
            [2, 2, 1],
            [
                ([1, 1, 0], stone.clone(), Class::Full),
                ([0, 1, 0], torch.clone(), Class::Special),
                ([1, 0, 0], stone.clone(), Class::Full),
                ([0, 0, 0], PaletteState::air(), Class::Full),
                ([0, 0, 0], state(STRUCTURE_VOID, &[]), Class::Special),
            ],
        );
        let order: Vec<[i32; 3]> = s.blocks.iter().map(|b| b.pos).collect();
        assert_eq!(order, vec![[1, 0, 0], [1, 1, 0], [0, 0, 0], [0, 1, 0]]);
        // Structure voids become air in the palette (`replaceStructureVoidWithAir`).
        assert_eq!(s.palette, vec![stone, PaletteState::air(), torch]);
        assert_eq!(s.blocks[2].state, 1);
    }

    #[test]
    fn positions_transform_as_structure_template() {
        let p = [2, 5, 1];
        let z = [0; 3];
        assert_eq!(transform(p, Mirror::None, Rotation::None, z), [2, 5, 1]);
        assert_eq!(
            transform(p, Mirror::None, Rotation::Clockwise90, z),
            [-1, 5, 2]
        );
        assert_eq!(
            transform(p, Mirror::None, Rotation::Clockwise180, z),
            [-2, 5, -1]
        );
        assert_eq!(
            transform(p, Mirror::None, Rotation::Counterclockwise90, z),
            [1, 5, -2]
        );
        assert_eq!(
            transform(p, Mirror::LeftRight, Rotation::None, z),
            [2, 5, -1]
        );
        assert_eq!(
            transform(p, Mirror::FrontBack, Rotation::None, z),
            [-2, 5, 1]
        );
        // Mirror first, then rotate.
        assert_eq!(
            transform(p, Mirror::FrontBack, Rotation::Clockwise90, z),
            [-1, 5, -2]
        );
        // About a pivot (vanilla's formulas).
        assert_eq!(
            transform(p, Mirror::None, Rotation::Clockwise90, [1, 0, 1]),
            [1, 5, 2]
        );
    }

    #[test]
    fn states_turn_as_vanilla_blocks_do() {
        let turned = |name: &str, p: &[(&str, &str)], m: Mirror, r: Rotation| {
            let mut s = props(p);
            transform_state(name, &mut s, m, r);
            s
        };
        // A furnace facing north, turned a quarter clockwise, faces east.
        assert_eq!(
            turned(
                "minecraft:furnace",
                &[("facing", "north")],
                Mirror::None,
                Rotation::Clockwise90
            ),
            props(&[("facing", "east")])
        );
        // Logs swap x and z on a quarter turn only.
        assert_eq!(
            turned(
                "minecraft:oak_log",
                &[("axis", "x")],
                Mirror::None,
                Rotation::Counterclockwise90
            ),
            props(&[("axis", "z")])
        );
        assert_eq!(
            turned(
                "minecraft:oak_log",
                &[("axis", "x")],
                Mirror::None,
                Rotation::Clockwise180
            ),
            props(&[("axis", "x")])
        );
        // A sign's sixteen steps.
        assert_eq!(
            turned(
                "minecraft:oak_sign",
                &[("rotation", "3")],
                Mirror::None,
                Rotation::Clockwise90
            ),
            props(&[("rotation", "7")])
        );
        assert_eq!(
            turned(
                "minecraft:oak_sign",
                &[("rotation", "3")],
                Mirror::LeftRight,
                Rotation::None
            ),
            props(&[("rotation", "13")])
        );
        assert_eq!(
            turned(
                "minecraft:oak_sign",
                &[("rotation", "3")],
                Mirror::FrontBack,
                Rotation::None
            ),
            props(&[("rotation", "5")])
        );
        // A fence's sides turn round.
        let fence = [
            ("east", "false"),
            ("north", "true"),
            ("south", "false"),
            ("west", "false"),
        ];
        assert_eq!(
            turned(
                "minecraft:oak_fence",
                &fence,
                Mirror::None,
                Rotation::Clockwise90
            ),
            props(&[
                ("east", "true"),
                ("north", "false"),
                ("south", "false"),
                ("west", "false")
            ])
        );
        assert_eq!(
            turned(
                "minecraft:oak_fence",
                &fence,
                Mirror::LeftRight,
                Rotation::None
            ),
            props(&[
                ("east", "false"),
                ("north", "false"),
                ("south", "true"),
                ("west", "false")
            ])
        );
        // Stairs along the mirrored axis turn round and their corners change hands; across it
        // they stay (StairBlock.mirror).
        fn stairs(f: &'static str, shape: &'static str) -> [(&'static str, &'static str); 3] {
            [("facing", f), ("half", "bottom"), ("shape", shape)]
        }
        assert_eq!(
            turned(
                "minecraft:oak_stairs",
                &stairs("north", "inner_left"),
                Mirror::LeftRight,
                Rotation::None
            ),
            props(&stairs("south", "inner_right"))
        );
        assert_eq!(
            turned(
                "minecraft:oak_stairs",
                &stairs("east", "inner_left"),
                Mirror::LeftRight,
                Rotation::None
            ),
            props(&stairs("east", "inner_left"))
        );
        // A door mirrored swaps its hinge; a chest its half.
        assert_eq!(
            turned(
                "minecraft:oak_door",
                &[("facing", "east"), ("half", "lower"), ("hinge", "left")],
                Mirror::FrontBack,
                Rotation::None
            ),
            props(&[("facing", "west"), ("half", "lower"), ("hinge", "right")])
        );
        assert_eq!(
            turned(
                "minecraft:chest",
                &[("facing", "north"), ("type", "left")],
                Mirror::FrontBack,
                Rotation::None
            ),
            props(&[("facing", "north"), ("type", "right")])
        );
        // Rails.
        assert_eq!(
            turned(
                "minecraft:rail",
                &[("shape", "south_east")],
                Mirror::None,
                Rotation::Clockwise90
            ),
            props(&[("shape", "south_west")])
        );
        assert_eq!(
            turned(
                "minecraft:rail",
                &[("shape", "ascending_north")],
                Mirror::LeftRight,
                Rotation::None
            ),
            props(&[("shape", "ascending_south")])
        );
        assert_eq!(
            turned(
                "minecraft:rail",
                &[("shape", "north_south")],
                Mirror::None,
                Rotation::Counterclockwise90
            ),
            props(&[("shape", "east_west")])
        );
        // Create's pistons flip `axis_along_first` on a quarter turn.
        assert_eq!(
            turned(
                "create:mechanical_piston",
                &[("axis_along_first", "false"), ("facing", "up")],
                Mirror::None,
                Rotation::Clockwise90
            ),
            props(&[("axis_along_first", "true"), ("facing", "up")])
        );
    }

    #[test]
    fn item_data_round_trips_and_names_follow_create() {
        let d = SchematicData::written("house.nbt", "Alex", [3, 4, 5]);
        let entries = d.to_entries();
        let map: BTreeMap<&str, Nbt> = entries.into_iter().collect();
        assert_eq!(SchematicData::from_entries(|k| map.get(k)), d);
        assert_eq!(slug("My Schematic"), "my_schematic");
        assert_eq!(slug("a--b c!"), "a_b_c_");
        assert_eq!(export_name("", false, |_| false), "my_schematic.nbt");
        assert_eq!(
            export_name("House", false, |f| f == "house.nbt" || f == "house_1.nbt"),
            "house_2.nbt"
        );
        assert_eq!(export_name("House", true, |_| true), "House.nbt");
        assert!(valid_file("house.nbt"));
        assert!(
            !valid_file("../house.nbt") && !valid_file("a/b.nbt") && !valid_file("house.schem")
        );
        assert!(size_allowed(256_000) && !size_allowed(256_001));
    }

    #[test]
    fn requirements_and_printed_states_follow_create() {
        assert_eq!(need(AIR, &props(&[])), Need::Nothing);
        assert_eq!(
            need("minecraft:stone_slab", &props(&[("type", "double")])),
            Need::Item {
                count: 2,
                item: None
            }
        );
        assert_eq!(
            need("minecraft:sea_pickle", &props(&[("pickles", "3")])),
            Need::Item {
                count: 3,
                item: None
            }
        );
        assert_eq!(
            need("minecraft:farmland", &props(&[("moisture", "7")])),
            Need::Item {
                count: 1,
                item: Some("minecraft:dirt")
            }
        );
        let mut name = "minecraft:wheat".to_owned();
        let mut p = props(&[("age", "7")]);
        printed_state(&mut name, &mut p);
        assert_eq!(p, props(&[("age", "0")]));
        let mut name = "minecraft:water_cauldron".to_owned();
        let mut p = props(&[("level", "3")]);
        printed_state(&mut name, &mut p);
        assert_eq!((name.as_str(), p.len()), ("minecraft:cauldron", 0));
        let mut name = "minecraft:oak_stairs".to_owned();
        let mut p = props(&[("waterlogged", "true")]);
        placed_state(&mut name, &mut p);
        assert_eq!(p, props(&[("waterlogged", "false")]));
        let bed = props(&[("facing", "east"), ("occupied", "false"), ("part", "head")]);
        let item = Need::Item {
            count: 1,
            item: None,
        };
        assert!(ignored("minecraft:red_bed", &bed, Some(&item)));
        assert!(ignored(
            "minecraft:oak_door",
            &props(&[("half", "upper")]),
            Some(&item)
        ));
        assert!(!ignored(
            "minecraft:oak_stairs",
            &props(&[("half", "top")]),
            Some(&item)
        ));
        assert!(ignored(STRUCTURE_VOID, &props(&[]), None));
        let foot = props(&[("facing", "east"), ("occupied", "false"), ("part", "foot")]);
        assert_eq!(other_half(&foot), Some([1, 0, 0]));
        assert_eq!(other_half(&props(&[("half", "lower")])), Some([0, 1, 0]));
        assert!(deferred(
            "minecraft:wall_torch",
            &props(&[("facing", "north")])
        ));
        assert!(deferred(
            "minecraft:lantern",
            &props(&[("hanging", "false")])
        ));
        assert!(deferred("create:mechanical_arm", &props(&[])));
        assert!(deferred("minecraft:white_carpet", &props(&[])));
        assert!(!deferred("minecraft:stone", &props(&[])));
        assert!(!deferred("minecraft:torchflower", &props(&[])));
        assert!(!deferred("minecraft:brain_coral_block", &props(&[])));
        assert!(deferred("minecraft:brain_coral_fan", &props(&[])));
    }
}
