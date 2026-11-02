//! Create's copycat blocks (`content/decoration/copycat`): a copycat panel or step holds a
//! material, a block state given by using a block item on it (`CopycatBlock.useItemOn`), taken
//! back with the wrench (`onWrenched`), and is drawn as that material cut to its own shape
//! (`CopycatPanelModel`, `CopycatStepModel`; the copycat base without one). Only the rules are
//! here; the glue reads the game and sets the clients' model overrides.

use crate::kinetics::{Axis, Dir};

/// The mod message that tells clients what a copycat shows.
pub const CHANNEL: &str = "create:copycat";

/// `AllBlocks.COPYCAT_BASE`: the material of a copycat that holds none.
pub const BASE: &str = "create:copycat_base";

/// A copycat's shape, from its block state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `CopycatPanelBlock`: 3 pixels thick on the side opposite its `facing`.
    Panel(Dir),
    /// `CopycatStepBlock`: half a block high (`top` for `half=top`) and deep, on its `facing`
    /// side.
    Step { facing: Dir, top: bool },
}

/// A box in block units, `[min, max]`.
pub type Box3 = [[f32; 3]; 2];

impl Shape {
    /// The box the material is cut to: Create's model draws the material in its block shape
    /// (`AllShapes.CASING_3PX` turned to the panel's facing, `STEP_BOTTOM` / `STEP_TOP` to the
    /// step's). One box, so no two boxes touch.
    pub fn boxes(self) -> Vec<Box3> {
        const T: f32 = 3.0 / 16.0;
        let b = match self {
            Shape::Panel(facing) => match facing {
                Dir::Up => [[0.0, 0.0, 0.0], [1.0, T, 1.0]],
                Dir::Down => [[0.0, 1.0 - T, 0.0], [1.0, 1.0, 1.0]],
                Dir::North => [[0.0, 0.0, 1.0 - T], [1.0, 1.0, 1.0]],
                Dir::South => [[0.0, 0.0, 0.0], [1.0, 1.0, T]],
                Dir::West => [[1.0 - T, 0.0, 0.0], [1.0, 1.0, 1.0]],
                Dir::East => [[0.0, 0.0, 0.0], [T, 1.0, 1.0]],
            },
            Shape::Step { facing, top } => {
                let (y0, y1) = if top { (0.5, 1.0) } else { (0.0, 0.5) };
                let (x0, x1, z0, z1) = match facing {
                    Dir::North => (0.0, 1.0, 0.0, 0.5),
                    Dir::West => (0.0, 0.5, 0.0, 1.0),
                    Dir::East => (0.5, 1.0, 0.0, 1.0),
                    _ => (0.0, 1.0, 0.5, 1.0),
                };
                [[x0, y0, z0], [x1, y1, z1]]
            }
        };
        vec![b]
    }

    pub fn is_panel(self) -> bool {
        matches!(self, Shape::Panel(_))
    }
}

/// What a copycat is told of the block an item would place (its default state):
/// `CopycatBlock.getAcceptedBlockState`'s tests and `CopycatSpecialCases`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    /// The block is a copycat itself (`instanceof CopycatBlock`).
    pub copycat: bool,
    /// In `create:copycat_allow` / `create:copycat_deny`.
    pub allow: bool,
    pub deny: bool,
    /// An `EntityBlock` (it has a block entity).
    pub entity: bool,
    /// A `StairBlock`.
    pub stairs: bool,
    /// Its shape's bounds are the whole block (`Shapes.block().bounds()`), and not empty.
    pub full: bool,
    /// Its collision shape is not empty.
    pub collides: bool,
    /// `CopycatSpecialCases.isBarsMaterial` (an `IronBarsBlock` that is no glass pane).
    pub bars: bool,
    /// `CopycatSpecialCases.isTrapdoorMaterial` (a `TrapDoorBlock` with `half`, `open` and
    /// `facing`).
    pub trapdoor: bool,
}

/// `getAcceptedBlockState` (with a level): whether a copycat of `shape` takes the block. Only
/// panels take bars and trapdoors regardless (`CopycatPanelBlock.isAcceptedRegardless`).
pub fn accepts(shape: Shape, c: &Candidate) -> bool {
    if c.copycat {
        return false;
    }
    let regardless = shape.is_panel() && (c.bars || c.trapdoor);
    if c.allow || regardless {
        return true;
    }
    !(c.deny || c.entity || c.stairs) && c.full && c.collides
}

/// `getAcceptedBlockState`'s orientation from the clicked face: `facing` (FACING, or
/// HORIZONTAL_FACING when the face is not up or down) to the face, `axis` (AXIS, or
/// HORIZONTAL_AXIS when not Y) to its axis. The glue sets each the material has and allows.
pub fn oriented(face: Dir) -> [(&'static str, &'static str); 2] {
    [("facing", dir_name(face)), ("axis", axis_name(face.axis()))]
}

/// `CopycatPanelBlock.prepareMaterial` for a trapdoor: on a floor or ceiling panel it lies
/// closed, facing the player (upper half on a ceiling); on a wall it stands open, facing the
/// panel's way, in the half that was clicked.
pub fn trapdoor_material(
    panel: Dir,
    player: Dir,
    clicked_near_top: bool,
) -> [(&'static str, &'static str); 3] {
    if panel.axis() == Axis::Y {
        let half = if panel == Dir::Down { "top" } else { "bottom" };
        return [
            ("half", half),
            ("facing", dir_name(player)),
            ("open", "false"),
        ];
    }
    let half = if clicked_near_top { "top" } else { "bottom" };
    [
        ("open", "true"),
        ("half", half),
        ("facing", dir_name(panel)),
    ]
}

/// What a click with a block item does to a copycat (`CopycatBlock.useItemOn`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// `PASS_TO_DEFAULT_BLOCK_INTERACTION`: the item's own use (placing its block) follows.
    Pass,
    /// The material is set and one item consumed (none in creative).
    Apply,
    /// The same block again: its material turns (`cycleMaterial`), nothing consumed.
    Cycle,
}

/// `useItemOn`: `accepted` is whether the item's block is taken, `same_block` whether the
/// material is already that block, `custom` whether it holds a material, `cycles` whether
/// `cycleMaterial` would change it.
pub fn use_with(accepted: bool, same_block: bool, custom: bool, cycles: bool) -> Use {
    if !accepted {
        return Use::Pass;
    }
    if same_block {
        return if cycles { Use::Cycle } else { Use::Pass };
    }
    if custom { Use::Pass } else { Use::Apply }
}

/// The properties `cycleMaterial` reads of a material.
#[derive(Clone, Copy, Debug, Default)]
pub struct Material {
    /// `facing`, and whether it may point up (FACING, else HORIZONTAL_FACING).
    pub facing: Option<(Dir, bool)>,
    /// `axis`, and whether it may be Y (AXIS, else HORIZONTAL_AXIS).
    pub axis: Option<(Axis, bool)>,
    /// A trapdoor's `half` (top) and `open`.
    pub half_top: Option<bool>,
    pub open: Option<bool>,
    pub lit: Option<bool>,
    /// The rose quartz lamp's `powering`.
    pub powering: Option<bool>,
}

/// `CopycatBlockEntity.cycleMaterial`: the property it changes and its new value, or None when
/// the material has none of them.
pub fn cycle(m: &Material) -> Option<(&'static str, &'static str)> {
    let flag = |b: bool| if b { "true" } else { "false" };
    if let (Some(top), Some(true)) = (m.half_top, m.open) {
        return Some(("half", if top { "bottom" } else { "top" }));
    }
    if let Some((f, six)) = m.facing {
        // `cycle` runs through the values in their order; HORIZONTAL_FACING turns clockwise.
        let next = if six {
            Dir::ALL[(Dir::ALL.iter().position(|d| *d == f).unwrap_or(0) + 1) % 6]
        } else {
            match f {
                Dir::North => Dir::East,
                Dir::East => Dir::South,
                Dir::South => Dir::West,
                _ => Dir::North,
            }
        };
        return Some(("facing", dir_name(next)));
    }
    if let Some((a, three)) = m.axis {
        let next = match (a, three) {
            (Axis::X, true) => Axis::Y,
            (Axis::Y, _) => Axis::Z,
            (Axis::X, false) => Axis::Z,
            (Axis::Z, _) => Axis::X,
        };
        return Some(("axis", axis_name(next)));
    }
    if let Some(lit) = m.lit {
        return Some(("lit", flag(!lit)));
    }
    if let Some(p) = m.powering {
        return Some(("powering", flag(!p)));
    }
    None
}

/// `getStateForPlacement`: a panel faces away from where the player looks
/// (`getNearestLookingDirection().getOpposite()`); a step faces the player's horizontal
/// direction, in the upper half when placed under a block or on the upper half of a side.
pub fn placed(panel: bool, nearest: Dir, horizontal: Dir, clicked: Dir, hit_y: f64) -> Shape {
    if panel {
        return Shape::Panel(nearest.opposite());
    }
    let top = match clicked {
        Dir::Up => false,
        Dir::Down => true,
        _ => hit_y > 0.5,
    };
    Shape::Step {
        facing: horizontal,
        top,
    }
}

/// What a client draws for a copycat's material (`CopycatPanelModel.getCroppedQuads`): a
/// trapdoor whole, bars as the copycat bars' model turned to the panel, any other block cut
/// to the copycat's shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Drawn {
    Whole,
    Bars(Dir),
    Cut(Vec<Box3>),
}

/// `CopycatSpecialCases`: the materials a panel draws its own way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Special {
    #[default]
    None,
    Bars,
    Trapdoor,
}

impl Special {
    pub fn of(c: &Candidate) -> Special {
        if c.trapdoor {
            Special::Trapdoor
        } else if c.bars {
            Special::Bars
        } else {
            Special::None
        }
    }
}

pub fn drawn(shape: Shape, special: Special) -> Drawn {
    match (shape, special) {
        (Shape::Panel(_), Special::Trapdoor) => Drawn::Whole,
        (Shape::Panel(f), Special::Bars) => Drawn::Bars(f),
        (s, _) => Drawn::Cut(s.boxes()),
    }
}

/// `CopycatBlockEntity.read` on a server: a saved material stays only while the saved item
/// would still give that block (`getAcceptedBlockState(consumedItem)`), else the copycat is
/// empty again and keeps no item.
pub fn keeps_material(item_block: Option<&str>, accepted: bool, material_block: &str) -> bool {
    accepted && item_block == Some(material_block)
}

/// One copycat as its column saves it (`CopycatBlockEntity.write`): its material as state text
/// (`""` for none) and whether it keeps an item (saved beside it by the glue).
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub pos: [i32; 3],
    pub material: String,
}

/// A column's copycats as positions (three ints each) and materials, in position order.
pub fn save(list: &[Saved]) -> (Vec<i32>, Vec<String>) {
    let mut list = list.to_vec();
    list.sort_by_key(|s| s.pos);
    let pos = list.iter().flat_map(|s| s.pos).collect();
    let materials = list.into_iter().map(|s| s.material).collect();
    (pos, materials)
}

/// `save` read back; a broken list gives what lines up.
pub fn load(pos: &[i32], materials: &[String]) -> Vec<Saved> {
    pos.chunks_exact(3)
        .zip(materials)
        .map(|(p, m)| Saved {
            pos: [p[0], p[1], p[2]],
            material: m.clone(),
        })
        .collect()
}

/// What a client is told of one copycat: its shape and material (`""` for the base), or
/// nothing when it is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub pos: [i32; 3],
    pub shape: Option<Shape>,
    pub special: Special,
    pub material: String,
}

impl Shown {
    /// How the client draws it: None for the block's own model (gone).
    pub fn drawn(&self) -> Option<Drawn> {
        self.shape.map(|s| drawn(s, self.special))
    }
}

pub fn encode(list: &[Shown]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in list {
        for v in s.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let (kind, dir, top) = match s.shape {
            None => (0u8, Dir::Up, false),
            Some(Shape::Panel(d)) => (1, d, false),
            Some(Shape::Step { facing, top }) => (2, facing, top),
        };
        let special = match s.special {
            Special::None => 0,
            Special::Bars => 2,
            Special::Trapdoor => 4,
        };
        out.push(kind);
        out.push(dir as u8);
        out.push(u8::from(top) | special);
        let t = s.material.as_bytes();
        let n = t.len().min(u16::MAX as usize);
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&t[..n]);
    }
    out
}

pub fn decode(mut d: &[u8]) -> Vec<Shown> {
    let mut out = Vec::new();
    while d.len() >= 17 {
        let int = |i: usize| i32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        let pos = [int(0), int(4), int(8)];
        let dir = Dir::ALL[usize::from(d[13]).min(5)];
        let shape = match d[12] {
            1 => Some(Shape::Panel(dir)),
            2 => Some(Shape::Step {
                facing: dir,
                top: d[14] & 1 != 0,
            }),
            _ => None,
        };
        let special = match d[14] & 6 {
            2 => Special::Bars,
            4 => Special::Trapdoor,
            _ => Special::None,
        };
        let n = usize::from(u16::from_le_bytes([d[15], d[16]]));
        if d.len() < 17 + n {
            break;
        }
        let material = String::from_utf8_lossy(&d[17..17 + n]).into_owned();
        out.push(Shown {
            pos,
            shape,
            special,
            material,
        });
        d = &d[17 + n..];
    }
    out
}

pub fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    }
}

pub fn dir_named(v: &str) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| dir_name(*d) == v)
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Candidate {
        Candidate {
            full: true,
            collides: true,
            ..Candidate::default()
        }
    }

    /// `getAcceptedBlockState`: full blocks with collision; not copycats, block entities,
    /// stairs, denied blocks, partial or empty shapes; allowed blocks always; bars and
    /// trapdoors on panels only.
    #[test]
    fn accepted_materials() {
        let panel = Shape::Panel(Dir::Up);
        let step = Shape::Step {
            facing: Dir::South,
            top: false,
        };
        // Planks, stone, a casing.
        assert!(accepts(panel, &full()) && accepts(step, &full()));
        for c in [
            Candidate {
                copycat: true,
                ..full()
            },
            Candidate {
                entity: true,
                ..full()
            },
            Candidate {
                stairs: true,
                ..full()
            },
            Candidate {
                deny: true,
                ..full()
            },
            // A slab, a carpet: not the whole block.
            Candidate {
                full: false,
                ..full()
            },
            // Tall grass: no collision.
            Candidate {
                collides: false,
                ..full()
            },
        ] {
            assert!(!accepts(panel, &c), "{c:?}");
            assert!(!accepts(step, &c), "{c:?}");
        }
        // The barrel: a block entity, but `create:copycat_allow`.
        let barrel = Candidate {
            entity: true,
            allow: true,
            ..full()
        };
        assert!(accepts(panel, &barrel) && accepts(step, &barrel));
        // Allowed even when it is a copycat? No: copycats never.
        assert!(!accepts(
            panel,
            &Candidate {
                copycat: true,
                allow: true,
                ..full()
            }
        ));
        // Iron bars and an oak trapdoor: thin, but panels take them.
        for c in [
            Candidate {
                bars: true,
                ..Candidate::default()
            },
            Candidate {
                trapdoor: true,
                ..Candidate::default()
            },
        ] {
            assert!(accepts(panel, &c));
            assert!(!accepts(step, &c));
        }
    }

    /// The material's box: 3 pixels on the side opposite the panel's facing (CASING_3PX); half
    /// high and deep on the step's facing side (STEP_BOTTOM / STEP_TOP).
    #[test]
    fn boxes_per_shape_and_facing() {
        let t = 3.0 / 16.0;
        assert_eq!(
            Shape::Panel(Dir::Up).boxes(),
            vec![[[0.0, 0.0, 0.0], [1.0, t, 1.0]]]
        );
        assert_eq!(
            Shape::Panel(Dir::Down).boxes(),
            vec![[[0.0, 1.0 - t, 0.0], [1.0, 1.0, 1.0]]]
        );
        // Facing north, on the south side (as `to_facing` in create-data turns CASING_3PX).
        assert_eq!(
            Shape::Panel(Dir::North).boxes(),
            vec![[[0.0, 0.0, 1.0 - t], [1.0, 1.0, 1.0]]]
        );
        assert_eq!(
            Shape::Panel(Dir::South).boxes(),
            vec![[[0.0, 0.0, 0.0], [1.0, 1.0, t]]]
        );
        assert_eq!(
            Shape::Panel(Dir::East).boxes(),
            vec![[[0.0, 0.0, 0.0], [t, 1.0, 1.0]]]
        );
        assert_eq!(
            Shape::Panel(Dir::West).boxes(),
            vec![[[1.0 - t, 0.0, 0.0], [1.0, 1.0, 1.0]]]
        );
        let step = |facing, top| Shape::Step { facing, top }.boxes();
        // STEP_BOTTOM is written facing south: z 8 to 16, y 0 to 8.
        assert_eq!(
            step(Dir::South, false),
            vec![[[0.0, 0.0, 0.5], [1.0, 0.5, 1.0]]]
        );
        assert_eq!(
            step(Dir::South, true),
            vec![[[0.0, 0.5, 0.5], [1.0, 1.0, 1.0]]]
        );
        assert_eq!(
            step(Dir::North, false),
            vec![[[0.0, 0.0, 0.0], [1.0, 0.5, 0.5]]]
        );
        assert_eq!(
            step(Dir::East, true),
            vec![[[0.5, 0.5, 0.0], [1.0, 1.0, 1.0]]]
        );
        assert_eq!(
            step(Dir::West, false),
            vec![[[0.0, 0.0, 0.0], [0.5, 0.5, 1.0]]]
        );
        // Every box lies in the block and has a volume.
        for f in Dir::ALL {
            for b in Shape::Panel(f).boxes() {
                assert!((0..3).all(|i| b[0][i] >= 0.0 && b[1][i] <= 1.0 && b[0][i] < b[1][i]));
            }
        }
    }

    /// Bars on a panel draw the copycat bars' model turned to it, a trapdoor itself; steps
    /// and other blocks are cut.
    #[test]
    fn special_materials_draw_whole() {
        let panel = Shape::Panel(Dir::West);
        assert_eq!(drawn(panel, Special::Trapdoor), Drawn::Whole);
        assert_eq!(drawn(panel, Special::Bars), Drawn::Bars(Dir::West));
        assert_eq!(drawn(panel, Special::None), Drawn::Cut(panel.boxes()));
        let step = Shape::Step {
            facing: Dir::East,
            top: true,
        };
        assert_eq!(drawn(step, Special::Bars), Drawn::Cut(step.boxes()));
    }

    #[test]
    fn uses_and_cycles() {
        assert_eq!(use_with(false, false, false, false), Use::Pass);
        assert_eq!(use_with(true, false, false, false), Use::Apply);
        assert_eq!(use_with(true, false, true, true), Use::Pass);
        assert_eq!(use_with(true, true, true, true), Use::Cycle);
        assert_eq!(use_with(true, true, true, false), Use::Pass);
        // Logs turn their axis, a horizontal axis skips Y; observers their facing in order,
        // furnaces-like blocks clockwise; an open trapdoor its half; a lamp lights.
        fn m(f: impl FnOnce(&mut Material)) -> Option<(&'static str, &'static str)> {
            let mut m = Material::default();
            f(&mut m);
            cycle(&m)
        }
        assert_eq!(m(|m| m.axis = Some((Axis::Y, true))), Some(("axis", "z")));
        assert_eq!(m(|m| m.axis = Some((Axis::Z, true))), Some(("axis", "x")));
        assert_eq!(m(|m| m.axis = Some((Axis::X, false))), Some(("axis", "z")));
        assert_eq!(
            m(|m| m.facing = Some((Dir::Up, true))),
            Some(("facing", "north"))
        );
        assert_eq!(
            m(|m| m.facing = Some((Dir::East, true))),
            Some(("facing", "down"))
        );
        assert_eq!(
            m(|m| m.facing = Some((Dir::East, false))),
            Some(("facing", "south"))
        );
        assert_eq!(
            m(|m| {
                m.half_top = Some(false);
                m.open = Some(true);
                m.facing = Some((Dir::North, false));
            }),
            Some(("half", "top"))
        );
        // A closed trapdoor turns its facing instead.
        assert_eq!(
            m(|m| {
                m.half_top = Some(false);
                m.open = Some(false);
                m.facing = Some((Dir::North, false));
            }),
            Some(("facing", "east"))
        );
        assert_eq!(m(|m| m.lit = Some(false)), Some(("lit", "true")));
        assert_eq!(m(|m| m.powering = Some(true)), Some(("powering", "false")));
        assert_eq!(m(|_| {}), None);
    }

    #[test]
    fn placement_and_trapdoors() {
        assert_eq!(
            placed(true, Dir::Down, Dir::North, Dir::Up, 0.0),
            Shape::Panel(Dir::Up)
        );
        assert_eq!(
            placed(false, Dir::Down, Dir::North, Dir::Up, 0.9),
            Shape::Step {
                facing: Dir::North,
                top: false
            }
        );
        assert_eq!(
            placed(false, Dir::North, Dir::North, Dir::South, 0.75),
            Shape::Step {
                facing: Dir::North,
                top: true
            }
        );
        assert_eq!(
            placed(false, Dir::Up, Dir::East, Dir::Down, 0.0),
            Shape::Step {
                facing: Dir::East,
                top: true
            }
        );
        assert_eq!(
            trapdoor_material(Dir::Down, Dir::West, false),
            [("half", "top"), ("facing", "west"), ("open", "false")]
        );
        assert_eq!(
            trapdoor_material(Dir::North, Dir::West, true),
            [("open", "true"), ("half", "top"), ("facing", "north")]
        );
        assert_eq!(oriented(Dir::East), [("facing", "east"), ("axis", "x")]);
    }

    /// What a server saved stays when its item still gives the material.
    #[test]
    fn saved_materials_are_validated() {
        assert!(keeps_material(
            Some("minecraft:oak_log"),
            true,
            "minecraft:oak_log"
        ));
        assert!(!keeps_material(
            Some("minecraft:oak_log"),
            true,
            "minecraft:stone"
        ));
        assert!(!keeps_material(None, true, "minecraft:stone"));
        assert!(!keeps_material(
            Some("minecraft:chest"),
            false,
            "minecraft:chest"
        ));
    }

    #[test]
    fn saves_round_trip() {
        let list = vec![
            Saved {
                pos: [17, 64, -3],
                material: "minecraft:oak_log[axis=x]".into(),
            },
            Saved {
                pos: [16, -64, -16],
                material: String::new(),
            },
        ];
        let (pos, materials) = save(&list);
        assert_eq!(pos, vec![16, -64, -16, 17, 64, -3]);
        let back = load(&pos, &materials);
        assert_eq!(back, vec![list[1].clone(), list[0].clone()]);
        assert_eq!(load(&pos[..4], &materials).len(), 1);
    }

    #[test]
    fn messages_round_trip() {
        let list = vec![
            Shown {
                pos: [1, -64, 3],
                shape: Some(Shape::Panel(Dir::North)),
                special: Special::None,
                material: "minecraft:oak_log[axis=z]".into(),
            },
            Shown {
                pos: [-5, 70, 1300],
                shape: Some(Shape::Step {
                    facing: Dir::West,
                    top: true,
                }),
                special: Special::Bars,
                material: String::new(),
            },
            Shown {
                pos: [0, 0, 0],
                shape: None,
                special: Special::None,
                material: String::new(),
            },
        ];
        assert_eq!(decode(&encode(&list)), list);
        assert!(decode(&encode(&list)[..20]).is_empty());
    }
}
