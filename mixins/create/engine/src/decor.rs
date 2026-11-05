//! Create's decorative and simple blocks, after their Java: girders (`GirderBlock`,
//! `GirderEncasedShaftBlock`), brackets (`BracketBlock`, `BracketBlockItem`), the placard
//! (`PlacardBlock`, `PlacardBlockEntity`), the desk bell (`DeskBellBlock`, `DeskBellBlockEntity`),
//! the peculiar bell's conversion (`PeculiarBellBlock.tryConvert`), the item hatch
//! (`ItemHatchBlock`), the creative crate (`BottomlessItemHandler`), the vertical gearbox
//! (`VerticalGearboxItem`), cardboard blocks, the clipboard's placement and table cloths'
//! placement helper, and the copper shingles' and tiles' weathering (`CopperBlockSet`).

use crate::kinetics::{Axis, Dir};

// ---------------------------------------------------------------------------------------------
// Girders

/// A metal girder's state (`GirderBlock`: `axis`, `x`, `z`, `top`, `bottom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Girder {
    pub axis: Axis,
    pub x: bool,
    pub z: bool,
    pub top: bool,
    pub bottom: bool,
}

/// What a girder reads of the block beside it on one side `d` (the glue fills it from the
/// world).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Side {
    /// `isAir`.
    pub air: bool,
    /// A metal girder there, with its `x` and `z`.
    pub girder: Option<(bool, bool)>,
    /// A girder encased shaft there, with its horizontal axis.
    pub encased: Option<Axis>,
    /// A placard whose `connectedDirection` is `d`, or a shaft whose bracket faces `d`
    /// (`isFacingBracket`); nixie tubes facing `d` too.
    pub toward: bool,
    /// For `d` up or down (`updateVerticalProperty`): a block with `up` true (`WallBlock.UP`), a
    /// display board, a lantern hanging the right way, a chain on y, a `face` ceiling below or
    /// floor above.
    pub holds: bool,
    /// `isFaceFull(shape, d.opposite) && isSolid`.
    pub full_face: bool,
    /// Its shape is empty.
    pub empty: bool,
    /// A chute facing down (`AbstractChuteBlock.getChuteFacing`).
    pub chute_down: bool,
}

impl Girder {
    fn get(&self, d: Dir) -> bool {
        match d {
            Dir::Up => self.top,
            Dir::Down => self.bottom,
            d if d.axis() == Axis::X => self.x,
            _ => self.z,
        }
    }

    fn set(&mut self, d: Dir, v: bool) {
        match d {
            Dir::Up => self.top = v,
            Dir::Down => self.bottom = v,
            d if d.axis() == Axis::X => self.x = v,
            _ => self.z = v,
        }
    }
}

/// `GirderBlock.updateVerticalProperty`: whether a girder (or an encased shaft, `axis_y` false)
/// holds on up or down to `side`.
pub fn attaches_vertically(axis_y: bool, side: &Side) -> bool {
    axis_y || side.girder.is_some() || side.encased.is_some() || side.holds || side.toward
}

/// `GirderBlock.updateState` toward `d`: sets the side's property when something holds it
/// there; it never clears one. `track` is a girdable track above beside it, along `d`'s axis
/// (`TrackShape.XO` / `ZO`).
pub fn update_state(mut g: Girder, d: Dir, side: &Side, track: bool) -> Girder {
    let axis = d.axis();
    if axis == Axis::Y {
        if attaches_vertically(g.axis == Axis::Y, side) {
            g.set(d, true);
        }
        return g;
    }
    let along = |(x, z): (bool, bool)| if axis == Axis::X { x } else { z };
    if g.axis == axis
        || side.encased.is_some_and(|a| a != axis)
        || side.girder.is_some_and(along)
        || side.toward
        || track
    {
        g.set(d, true);
    }
    g
}

/// `GirderBlock.isConnected` toward `d`.
pub fn is_connected(g: &Girder, d: Dir, side: &Side) -> bool {
    if !g.get(d) {
        return false;
    }
    if side.air {
        return false;
    }
    if side.toward {
        return true;
    }
    if side.empty {
        return false;
    }
    side.full_face || side.chute_down
}

/// `GirderBlock.getStateForPlacement`: its axis from the clicked face, then every side.
pub fn placement(face: Dir, side: impl Fn(Dir) -> Side, track: impl Fn(Dir) -> bool) -> Girder {
    let axis = face.axis();
    let mut g = Girder {
        axis,
        x: axis == Axis::X,
        z: axis == Axis::Z,
        top: false,
        bottom: false,
    };
    for d in Dir::ALL {
        g = update_state(g, d, &side(d), track(d));
    }
    g
}

/// `GirderBlock.updateShape` for a change on side `dir`: a beam across its axis that holds on to
/// nothing on either end goes; a top or bottom over nothing goes; then both sides on `dir`'s axis
/// look again. `support_empty` is the neighbour's `getBlockSupportShape` being empty.
pub fn update_shape(
    mut g: Girder,
    dir: Dir,
    side: impl Fn(Dir) -> Side,
    support_empty: impl Fn(Dir) -> bool,
    track: impl Fn(Dir) -> bool,
) -> Girder {
    let axis = dir.axis();
    if axis != Axis::Y {
        if g.axis != axis
            && !is_connected(&g, dir, &side(dir))
            && !is_connected(&g, dir.opposite(), &side(dir.opposite()))
        {
            g.set(dir, false);
        }
    } else if g.axis != Axis::Y {
        if support_empty(Dir::Up) {
            g.top = false;
        }
        if support_empty(Dir::Down) {
            g.bottom = false;
        }
    }
    for d in [Dir::from_axis(axis, true), Dir::from_axis(axis, false)] {
        g = update_state(g, d, &side(d), track(d));
    }
    g
}

/// `GirderEncasedShaftBlock.updateShape` toward up or down: (top, bottom) after it.
pub fn encased_vertical(
    (mut top, mut bottom): (bool, bool),
    d: Dir,
    side: &Side,
    support_empty: bool,
) -> (bool, bool) {
    let v = if d == Dir::Up { &mut top } else { &mut bottom };
    if support_empty {
        *v = false;
    }
    if attaches_vertically(false, side) {
        *v = true;
    }
    (top, bottom)
}

/// `GirderBlock.useItemOn` with a shaft: the encased shaft's axis (z when the girder runs along x
/// or the click was on a z face).
pub fn encased_axis(girder_x: bool, clicked: Dir) -> Axis {
    if girder_x || clicked.axis() == Axis::Z {
        Axis::Z
    } else {
        Axis::X
    }
}

// ---------------------------------------------------------------------------------------------
// Brackets

/// `BracketBlock.BracketType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BracketKind {
    Pipe,
    Cog,
    Shaft,
}

impl BracketKind {
    pub fn name(self) -> &'static str {
        match self {
            BracketKind::Pipe => "pipe",
            BracketKind::Cog => "cog",
            BracketKind::Shaft => "shaft",
        }
    }

    pub fn read(s: &str) -> Option<BracketKind> {
        Some(match s {
            "pipe" => BracketKind::Pipe,
            "cog" => BracketKind::Cog,
            "shaft" => BracketKind::Shaft,
            _ => return None,
        })
    }
}

/// A bracket on a shaft, cog or pipe: the bracket block's state (`BracketedBlockEntityBehaviour
/// .bracket`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bracket {
    pub wooden: bool,
    pub facing: Dir,
    /// `axis_along_first`.
    pub along_first: bool,
    pub kind: BracketKind,
}

impl Bracket {
    pub fn block(&self) -> &'static str {
        if self.wooden {
            "create:wooden_bracket"
        } else {
            "create:metal_bracket"
        }
    }
}

/// `BracketBlock.getSuitableBracket(axis, direction, type)`: none along the target's own axis.
pub fn suitable_bracket(
    wooden: bool,
    target: Option<Axis>,
    direction: Dir,
    kind: BracketKind,
) -> Option<Bracket> {
    let axis = direction.axis();
    let target = target.filter(|t| *t != axis)?;
    let along_first = if axis != Axis::Z {
        target == Axis::Z
    } else {
        target == Axis::Y
    };
    Some(Bracket {
        wooden,
        facing: direction,
        along_first: !along_first,
        kind,
    })
}

/// `BracketBlockItem.useOn`: the bracket for the clicked face, else for the face toward the
/// player (`orderedByNearest(player)[0].getOpposite()`).
pub fn bracket_for_click(
    wooden: bool,
    target: Option<Axis>,
    kind: BracketKind,
    clicked: Dir,
    toward_player: Dir,
) -> Option<Bracket> {
    suitable_bracket(wooden, target, clicked, kind)
        .or_else(|| suitable_bracket(wooden, target, toward_player, kind))
}

// ---------------------------------------------------------------------------------------------
// Placard, desk bell, bells

/// `PlacardBlockEntity.poweredTicks` set by a matching item.
pub const PLACARD_PULSE: u8 = 19;
/// `DeskBellBlockEntity.blockStateTimer` set by a ding.
pub const DESK_BELL_TICKS: u8 = 20;

/// What a click on a placard does (`PlacardBlock.useItemOn`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacardUse {
    /// Sneaking: the click is the default's.
    Pass,
    /// Nothing happens (`FAIL`).
    Fail,
    /// The held item (one of it) goes on the placard.
    Put,
    /// The held item matches: powered for `PLACARD_PULSE` ticks, the confirm sound.
    Pulse,
    /// It doesn't: the deny sound.
    Deny,
}

/// `PlacardBlock.useItemOn`, from what is held and shown. `same` is
/// `ItemStack.isSameItemSameComponents` of the two.
pub fn use_placard(
    sneaking: bool,
    may_build: bool,
    held_empty: bool,
    shown_empty: bool,
    powered: bool,
    same: bool,
) -> PlacardUse {
    if sneaking {
        return PlacardUse::Pass;
    }
    if !may_build || held_empty || !shown_empty {
        if shown_empty || held_empty || powered {
            return PlacardUse::Fail;
        }
        return if same {
            PlacardUse::Pulse
        } else {
            PlacardUse::Deny
        };
    }
    PlacardUse::Put
}

/// `getConnectedDirection` of a face-attached block (placard, clipboard): up on the floor, down
/// on the ceiling, else its facing.
pub fn connected_direction(face: &str, facing: Dir) -> Dir {
    match face {
        "floor" => Dir::Up,
        "ceiling" => Dir::Down,
        _ => facing,
    }
}

/// `PlacardBlock.getStateForPlacement` (on the floor its facing turns round) and
/// `ClipboardBlock`'s (on the floor or ceiling): the facing after the face-attached placement.
pub fn placed_facing(placard: bool, face: &str, facing: Dir) -> Dir {
    let flip = if placard {
        face == "floor"
    } else {
        face != "wall"
    };
    if flip { facing.opposite() } else { facing }
}

/// `DeskBellBlock.getDirectSignal` / `getSignal`: 15 while pressed, strong only into the block it
/// stands on (`side` the direction from the reader to the bell, as vanilla's).
pub fn desk_bell_signal(powered: bool, facing: Dir, side: Dir, direct: bool) -> i32 {
    if !powered || (direct && facing != side) {
        0
    } else {
        15
    }
}

/// `PeculiarBellBlock.tryConvert`: soul fire or a soul campfire below makes it haunted.
pub fn haunts(below: &str) -> bool {
    matches!(below, "minecraft:soul_fire" | "minecraft:soul_campfire")
}

// ---------------------------------------------------------------------------------------------
// Item hatch, creative crate

/// `ItemHatchBlock.useItemOn`: the inventory slots (vanilla's 36) a click deposits: the selected
/// hotbar slot, or (sneaking) every slot outside the hotbar.
pub fn hatch_slots(sneaking: bool, selected: usize) -> Vec<usize> {
    (0..36)
        .filter(|&i| {
            let hotbar = i < 9;
            if hotbar == sneaking {
                return false;
            }
            sneaking || i == selected
        })
        .collect()
}

/// `ItemHatchBlock`'s scheduled tick closes it 10 ticks after a deposit.
pub const HATCH_OPEN_TICKS: i64 = 10;

/// `HatchFilterSlot.testHit`: a hit at `local` (in the block) on the item hatch's filter, 5.15 px
/// up and 9.5 px in, turned to its facing, within a quarter of its 0.965-scaled box.
pub fn hatch_filter_hit(facing: Dir, local: [f64; 3]) -> bool {
    let (s, c) = horizontal_angle(facing).to_radians().sin_cos();
    // `voxelSpace(8, 5.15, 9.5)` about the centre: x is the centre itself.
    let v = [0.0, 5.15 / 16.0 - 0.5, 9.5 / 16.0 - 0.5];
    let x = f64::from(c) * v[0] + f64::from(s) * v[2];
    let z = f64::from(c) * v[2] - f64::from(s) * v[0];
    let slot = [x + 0.5, v[1] + 0.5 - 0.125, z + 0.5];
    let d2: f64 = (0..3).map(|i| (local[i] - slot[i]).powi(2)).sum();
    d2.sqrt() < 0.5 * 0.965 / 2.0
}

/// `CreativeCrateBlockEntity.createFilter`: its supply's value box, on top at 13.5 px.
pub const CRATE_SLOT: [f64; 3] = [0.5, 13.5 / 16.0, 0.5];

/// `BottomlessItemHandler.extractItem`: the supply item, up to a stack, as often as asked; none
/// without a supply. `insertItem` takes everything (nothing comes back).
pub fn crate_extract(supply: Option<&str>, amount: u32, max_stack: u32) -> Option<(&str, u32)> {
    let item = supply.filter(|s| !s.is_empty())?;
    let n = amount.min(max_stack);
    (n > 0).then_some((item, n))
}

// ---------------------------------------------------------------------------------------------
// Vertical gearbox, cardboard

/// `Entity.getDirection` from a yaw in degrees.
pub fn horizontal_facing(yaw: f32) -> Dir {
    match ((yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

/// `VerticalGearboxItem.updateCustomBlockEntityTag`: across the one horizontal axis on which
/// shafts face it (`shafts` the horizontal sides with a shaft toward the gearbox), else the axis
/// clockwise of the player's facing.
pub fn vertical_gearbox_axis(shafts: &[Dir], player: Dir) -> Axis {
    let mut preferred: Option<Axis> = None;
    for &side in shafts {
        match preferred {
            Some(a) if a != side.axis() => {
                preferred = None;
                break;
            }
            _ => preferred = Some(side.axis()),
        }
    }
    match preferred {
        None => {
            // `getClockWise().getAxis()`: north/south turn to x, east/west to z.
            if player.axis() == Axis::Z {
                Axis::X
            } else {
                Axis::Z
            }
        }
        Some(Axis::X) => Axis::Z,
        Some(_) => Axis::X,
    }
}

// ---------------------------------------------------------------------------------------------
// Table cloths

/// `IPlacementHelper.orderedByDistanceExceptAxis(pos, hit, Y, ..)`: the horizontal directions
/// nearest the hit first (`hit` relative to the block's centre).
pub fn table_cloth_directions(hit: [f64; 3]) -> Vec<Dir> {
    let mut dirs: Vec<(Dir, f64)> = [Dir::North, Dir::South, Dir::West, Dir::East]
        .into_iter()
        .map(|d| {
            let o = d.offset();
            let dist = (0..3)
                .map(|i| (f64::from(o[i]) - hit[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            (d, dist)
        })
        .collect();
    dirs.sort_by(|a, b| a.1.total_cmp(&b.1));
    dirs.into_iter().map(|d| d.0).collect()
}

// ---------------------------------------------------------------------------------------------
// What clients draw: brackets and placards' items

/// The channel bringing clients a block's bracket or shown item.
pub const CHANNEL: &str = "create:decor";

/// What a client draws at a block: its bracket (`create:metal_bracket[...]` as text) or its
/// placard's item; an empty text clears it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub pos: [i32; 3],
    /// 0: a bracket, 1: a placard's item.
    pub kind: u8,
    pub text: String,
}

pub fn encode(list: &[Shown]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in list {
        for v in s.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(s.kind);
        let t = s.text.as_bytes();
        let n = t.len().min(u16::MAX as usize);
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&t[..n]);
    }
    out
}

pub fn decode(mut d: &[u8]) -> Vec<Shown> {
    let mut out = Vec::new();
    while d.len() >= 15 {
        let int = |i: usize| i32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        let pos = [int(0), int(4), int(8)];
        let kind = d[12];
        let n = usize::from(u16::from_le_bytes([d[13], d[14]]));
        if d.len() < 15 + n {
            break;
        }
        let text = String::from_utf8_lossy(&d[15..15 + n]).into_owned();
        out.push(Shown { pos, kind, text });
        d = &d[15 + n..];
    }
    out
}

/// Catnip's `AngleHelper.horizontalAngle`: south 0, west 90, north 180, east 270, x negated.
pub fn horizontal_angle(d: Dir) -> f32 {
    match d {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `PlacardRenderer`: the held item's pose in the block (`FIXED` display), by its face and
/// facing; a block item at half size, others at three eighths.
pub fn placard_pose(face: &str, facing: Dir, block_item: bool) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    let ceiling = face == "ceiling";
    let yaw = if ceiling { std::f32::consts::PI } else { 0.0 }
        + (180.0 + horizontal_angle(facing)).to_radians();
    let pitch = match face {
        "ceiling" => -std::f32::consts::FRAC_PI_2,
        "floor" => std::f32::consts::FRAC_PI_2,
        _ => 0.0,
    };
    Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_translation(Vec3::new(0.0, 0.0, 4.5 / 16.0))
        * Mat4::from_scale(Vec3::splat(if block_item { 0.5 } else { 0.375 }))
}

// ---------------------------------------------------------------------------------------------
// Copper shingles and tiles

/// `CopperBlockSet`'s weathering chain (`CopperRegistries.addWeathering`): the next age of an
/// unwaxed copper shingle or tile block, in the same shape. None for oxidized and waxed blocks
/// (vanilla's `WeatheringCopper.getNext`), and for every other block.
pub fn copper_next(block: crate::Block) -> Option<crate::Block> {
    use crate::Block::*;
    Some(match block {
        CopperShingles => ExposedCopperShingles,
        ExposedCopperShingles => WeatheredCopperShingles,
        WeatheredCopperShingles => OxidizedCopperShingles,
        CopperShingleSlab => ExposedCopperShingleSlab,
        ExposedCopperShingleSlab => WeatheredCopperShingleSlab,
        WeatheredCopperShingleSlab => OxidizedCopperShingleSlab,
        CopperShingleStairs => ExposedCopperShingleStairs,
        ExposedCopperShingleStairs => WeatheredCopperShingleStairs,
        WeatheredCopperShingleStairs => OxidizedCopperShingleStairs,
        CopperTiles => ExposedCopperTiles,
        ExposedCopperTiles => WeatheredCopperTiles,
        WeatheredCopperTiles => OxidizedCopperTiles,
        CopperTileSlab => ExposedCopperTileSlab,
        ExposedCopperTileSlab => WeatheredCopperTileSlab,
        WeatheredCopperTileSlab => OxidizedCopperTileSlab,
        CopperTileStairs => ExposedCopperTileStairs,
        ExposedCopperTileStairs => WeatheredCopperTileStairs,
        WeatheredCopperTileStairs => OxidizedCopperTileStairs,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copper_weathers_one_age_at_a_time_in_its_shape() {
        use crate::Block;
        let mut steps = 0;
        for b in Block::ALL {
            let name = b.name();
            let Some(next) = copper_next(b) else {
                continue;
            };
            steps += 1;
            // CopperBlockSet names each age by a prefix on the unaffected block's name.
            let (from, to) = [
                ("", "exposed_"),
                ("exposed_", "weathered_"),
                ("weathered_", "oxidized_"),
            ]
            .into_iter()
            .find(|(from, to)| {
                next.name()
                    .strip_prefix(to)
                    .is_some_and(|rest| name == format!("{from}{rest}"))
            })
            .expect(name);
            assert!(
                !name.starts_with("waxed_") && !name.starts_with("oxidized_"),
                "{name} -> {from}{to}"
            );
        }
        assert_eq!(steps, 18);
        assert_eq!(copper_next(Block::OxidizedCopperTiles), None);
        assert_eq!(copper_next(Block::WaxedCopperShingles), None);
    }

    #[test]
    fn clients_get_brackets_and_placard_items() {
        let list = vec![
            Shown {
                pos: [700, -3, 701],
                kind: 0,
                text: "create:metal_bracket[facing=north]".into(),
            },
            Shown {
                pos: [-1, 2, 3],
                kind: 1,
                text: String::new(),
            },
        ];
        assert_eq!(decode(&encode(&list)), list);
        // A placard on a wall facing south hangs on the block north of it: its item lies 4.5 px
        // north of the centre, over the plate.
        let p = placard_pose("wall", Dir::South, false).transform_point3(glam::Vec3::ZERO);
        assert!(
            (p - glam::Vec3::new(0.5, 0.5, 0.5 - 4.5 / 16.0)).length() < 1e-5,
            "{p}"
        );
        // On the floor, above it.
        let p = placard_pose("floor", Dir::North, true).transform_point3(glam::Vec3::ZERO);
        assert!((p.y - (0.5 - 4.5 / 16.0)).abs() < 1e-5 || (p.y - (0.5 + 4.5 / 16.0)).abs() < 1e-5);
    }

    fn wall() -> Side {
        Side {
            full_face: true,
            ..Side::default()
        }
    }

    #[test]
    fn girders_join_beams_poles_and_walls_as_create_s() {
        // Placed on the side of a block (clicked face east): a beam along x.
        let air = Side {
            air: true,
            empty: true,
            ..Side::default()
        };
        let g = placement(Dir::East, |_| air, |_| false);
        assert_eq!(
            (g.axis, g.x, g.z, g.top, g.bottom),
            (Axis::X, true, false, false, false)
        );
        // Placed on top: a pole, holding on up and down whatever is there.
        let g = placement(Dir::Up, |_| air, |_| false);
        assert_eq!(
            (g.axis, g.top, g.bottom, g.x, g.z),
            (Axis::Y, true, true, false, false)
        );
        // A pole beside a z beam takes a z beam of its own; beside an encased shaft on x it
        // joins across (z).
        let beam = Side {
            girder: Some((false, true)),
            ..Side::default()
        };
        let g = update_state(g, Dir::South, &beam, false);
        assert!(g.z);
        let shaft = Side {
            encased: Some(Axis::X),
            ..Side::default()
        };
        assert!(
            update_state(
                placement(Dir::Up, |_| air, |_| false),
                Dir::North,
                &shaft,
                false
            )
            .z
        );
        // A beam along x crossing z at a wall on the north: kept while one end holds on.
        let mut cross = placement(Dir::East, |_| air, |_| false);
        cross.z = true;
        let kept = update_shape(
            cross,
            Dir::North,
            |d| if d == Dir::North { wall() } else { air },
            |_| false,
            |_| false,
        );
        assert!(kept.z);
        let gone = update_shape(cross, Dir::North, |_| air, |_| false, |_| false);
        assert!(!gone.z, "nothing on either end: the z beam goes");
        // A beam's top goes over nothing, and comes back on a girder above.
        let mut top = cross;
        top.top = true;
        let t = update_shape(top, Dir::Up, |_| air, |d| d == Dir::Up, |_| false);
        assert!(!t.top);
        let girder_above = Side {
            girder: Some((true, false)),
            ..Side::default()
        };
        let t = update_shape(
            t,
            Dir::Up,
            |d| if d == Dir::Up { girder_above } else { air },
            |_| false,
            |_| false,
        );
        assert!(t.top);
        assert_eq!(encased_axis(true, Dir::Up), Axis::Z);
        assert_eq!(encased_axis(false, Dir::East), Axis::X);
        assert_eq!(encased_axis(false, Dir::North), Axis::Z);
        assert_eq!(
            encased_vertical((true, false), Dir::Up, &air, true),
            (false, false)
        );
        assert_eq!(
            encased_vertical((false, false), Dir::Down, &girder_above, false),
            (false, true)
        );
    }

    #[test]
    fn brackets_go_across_their_shaft() {
        // A shaft on y clicked on its north face: facing north, `axis_along_first` true
        // (alongFirst = target == z is false).
        let b = suitable_bracket(false, Some(Axis::Y), Dir::North, BracketKind::Shaft).unwrap();
        assert_eq!((b.facing, b.along_first), (Dir::North, false));
        let b = suitable_bracket(false, Some(Axis::Z), Dir::Up, BracketKind::Cog).unwrap();
        assert_eq!((b.facing, b.along_first), (Dir::Up, false));
        let b = suitable_bracket(true, Some(Axis::X), Dir::Up, BracketKind::Cog).unwrap();
        assert!(b.along_first);
        assert_eq!(b.block(), "create:wooden_bracket");
        // Not along the shaft: the clicked end of a shaft falls back to the player's side.
        assert_eq!(
            suitable_bracket(false, Some(Axis::Y), Dir::Up, BracketKind::Shaft),
            None
        );
        let b = bracket_for_click(false, Some(Axis::Y), BracketKind::Shaft, Dir::Up, Dir::West)
            .unwrap();
        assert_eq!(b.facing, Dir::West);
    }

    #[test]
    fn placards_pulse_on_a_match_and_bells_signal_while_pressed() {
        assert_eq!(
            use_placard(false, true, false, true, false, false),
            PlacardUse::Put
        );
        assert_eq!(
            use_placard(false, true, false, false, false, true),
            PlacardUse::Pulse
        );
        assert_eq!(
            use_placard(false, true, false, false, false, false),
            PlacardUse::Deny
        );
        assert_eq!(
            use_placard(false, true, false, false, true, true),
            PlacardUse::Fail
        );
        assert_eq!(
            use_placard(false, true, true, false, false, false),
            PlacardUse::Fail
        );
        assert_eq!(
            use_placard(false, true, true, true, false, false),
            PlacardUse::Fail
        );
        assert_eq!(
            use_placard(true, true, false, true, false, false),
            PlacardUse::Pass
        );
        assert_eq!(
            use_placard(false, false, false, true, false, false),
            PlacardUse::Fail
        );
        assert_eq!((PLACARD_PULSE, DESK_BELL_TICKS), (19, 20));
        assert_eq!(connected_direction("floor", Dir::North), Dir::Up);
        assert_eq!(connected_direction("wall", Dir::East), Dir::East);
        assert_eq!(placed_facing(true, "floor", Dir::North), Dir::South);
        assert_eq!(placed_facing(true, "ceiling", Dir::North), Dir::North);
        assert_eq!(placed_facing(false, "ceiling", Dir::North), Dir::South);
        assert_eq!(placed_facing(false, "wall", Dir::North), Dir::North);
        // A bell on the floor (facing up) powers the block below strongly: the reader below
        // asks toward up.
        assert_eq!(desk_bell_signal(true, Dir::Up, Dir::Up, true), 15);
        assert_eq!(desk_bell_signal(true, Dir::Up, Dir::North, true), 0);
        assert_eq!(desk_bell_signal(true, Dir::Up, Dir::North, false), 15);
        assert_eq!(desk_bell_signal(false, Dir::Up, Dir::Up, false), 0);
        assert!(haunts("minecraft:soul_campfire") && !haunts("minecraft:campfire"));
    }

    #[test]
    fn hatches_crates_gearboxes_and_cloths() {
        // The hatch facing north (its inventory north): the filter 6.5 px from the north side.
        assert!(hatch_filter_hit(Dir::North, [0.5, 3.15 / 16.0, 6.5 / 16.0]));
        assert!(!hatch_filter_hit(
            Dir::North,
            [0.5, 3.15 / 16.0, 12.0 / 16.0]
        ));
        assert!(hatch_filter_hit(Dir::East, [9.5 / 16.0, 3.15 / 16.0, 0.5]));
        assert_eq!(hatch_slots(false, 3), vec![3]);
        let all = hatch_slots(true, 3);
        assert_eq!((all.len(), all[0], all[26]), (27, 9, 35));
        assert_eq!(
            crate_extract(Some("minecraft:stone"), 64, 64),
            Some(("minecraft:stone", 64))
        );
        assert_eq!(
            crate_extract(Some("minecraft:ender_pearl"), 64, 16),
            Some(("minecraft:ender_pearl", 16))
        );
        assert_eq!(crate_extract(None, 64, 64), None);
        assert_eq!(horizontal_facing(0.0), Dir::South);
        assert_eq!(horizontal_facing(90.0), Dir::West);
        assert_eq!(horizontal_facing(180.0), Dir::North);
        assert_eq!(horizontal_facing(-90.0), Dir::East);
        // A shaft west of it: across, on z; shafts on both axes: the player's.
        assert_eq!(vertical_gearbox_axis(&[Dir::West], Dir::North), Axis::Z);
        assert_eq!(
            vertical_gearbox_axis(&[Dir::West, Dir::North], Dir::North),
            Axis::X
        );
        assert_eq!(vertical_gearbox_axis(&[], Dir::East), Axis::Z);
        // A click on the east edge of a cloth's top: east first.
        assert_eq!(table_cloth_directions([0.45, 0.06, 0.1])[0], Dir::East);
    }
}
