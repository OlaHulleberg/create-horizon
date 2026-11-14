//! Create's wand of symmetry (`SymmetryWandItem`, `SymmetryMirror`, `PlaneMirror`,
//! `CrossPlaneMirror`, `TriplePlaneMirror`, `EmptyMirror`, `SymmetryHandler`): a mirror the wand
//! keeps, and the positions and block state turns a placement or a break is copied to.
//!
//! The wand's data (`SYMMETRY_WAND`, `SYMMETRY_WAND_ENABLE`) rides the stack's
//! `minecraft:custom_data` under Create's component names; the mirror's codec fields are kept
//! (`orientation_index`, `position`, `type`, `enable`).

use crate::schematic::{Mirror as Flip, Rotation, mirror_state, rotate_state};
use std::collections::BTreeMap;

pub const WAND: &str = "create:wand_of_symmetry";
pub const MIRROR_KEY: &str = "create:symmetry_wand";
pub const ENABLE_KEY: &str = "create:symmetry_wand_enable";
/// `CEquipment.maxSymmetryWandRange`'s default.
pub const MAX_RANGE: f64 = 50.0;
/// `useOn`'s `addCooldown(this, 5)`.
pub const COOLDOWN: u32 = 5;

/// The mirror's type (`typeName`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Empty,
    Plane,
    CrossPlane,
    TriplePlane,
}

impl Kind {
    pub const ALL: [Kind; 4] = [
        Kind::Empty,
        Kind::Plane,
        Kind::CrossPlane,
        Kind::TriplePlane,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Empty => "empty",
            Kind::Plane => "plane",
            Kind::CrossPlane => "cross_plane",
            Kind::TriplePlane => "triple_plane",
        }
    }
    /// `SymmetryMirror.create`: any other name is an empty mirror.
    pub fn parse(s: &str) -> Kind {
        Kind::ALL
            .into_iter()
            .find(|k| k.name() == s)
            .unwrap_or(Kind::Empty)
    }
    /// How many orientations it cycles through (`Align.values().length`).
    fn orientations(self) -> usize {
        match self {
            Kind::Plane | Kind::CrossPlane => 2,
            Kind::Empty => 1,
            Kind::TriplePlane => 1,
        }
    }
}

/// The wand's mirror (`SymmetryMirror`), with the stack's `SYMMETRY_WAND_ENABLE`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wand {
    pub kind: Kind,
    /// `orientationIndex`: a plane's XY (0) or YZ (1); a cross's Y (0) or D (1).
    pub orientation: usize,
    pub position: [f64; 3],
    /// The mirror's own `enable` (kept, never read).
    pub mirror_enable: bool,
    /// `SYMMETRY_WAND_ENABLE`.
    pub enabled: bool,
}

/// One of the mirror's turns of a block state (`flipX`, `flipZ`, `flipD1`, `flipD2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// `mirror(FRONT_BACK)`.
    X,
    /// `mirror(LEFT_RIGHT)`.
    Z,
    /// `rotate(COUNTERCLOCKWISE_90).mirror(FRONT_BACK)`.
    D1,
    /// `rotate(COUNTERCLOCKWISE_90).mirror(LEFT_RIGHT)`.
    D2,
}

impl Turn {
    /// The turn applied to a state's properties (`name` is its block id).
    pub fn apply(self, name: &str, props: &mut BTreeMap<String, String>) {
        match self {
            Turn::X => mirror_state(name, props, Flip::FrontBack),
            Turn::Z => mirror_state(name, props, Flip::LeftRight),
            Turn::D1 => {
                rotate_state(name, props, Rotation::Counterclockwise90);
                mirror_state(name, props, Flip::FrontBack);
            }
            Turn::D2 => {
                rotate_state(name, props, Rotation::Counterclockwise90);
                mirror_state(name, props, Flip::LeftRight);
            }
        }
    }
}

/// A copy of a placement: where, and the turns its state takes, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub pos: [i32; 3],
    pub turns: Vec<Turn>,
}

impl Wand {
    /// `checkComponents`: a wand with no mirror holds an empty one at the origin, disabled.
    pub fn blank() -> Wand {
        Wand::default()
    }

    /// `setOrientation(int)`: kept in range (a triple plane keeps none).
    pub fn set_orientation(&mut self, index: usize) {
        if self.kind != Kind::TriplePlane {
            self.orientation = index % self.kind.orientations();
        }
    }

    /// `isEnabled` (the simulation flag is never set here).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// `getIDiff`: `(int)` of the offset from the mirror, toward zero.
    fn diff(&self, p: [i32; 3]) -> [i32; 3] {
        [0, 1, 2].map(|i| (f64::from(p[i]) - self.position[i]) as i32)
    }

    fn flip(&self, p: [i32; 3], t: Turn) -> [i32; 3] {
        let d = self.diff(p);
        match t {
            Turn::X => [p[0] - 2 * d[0], p[1], p[2]],
            Turn::Z => [p[0], p[1], p[2] - 2 * d[2]],
            Turn::D1 => [p[0] - d[0] - d[2], p[1], p[2] - d[2] - d[0]],
            Turn::D2 => [p[0] - d[0] + d[2], p[1], p[2] - d[2] + d[0]],
        }
    }

    /// `process(position, block)`: the copies in Create's order (`flipD1(flipX(p))` turns by
    /// X, then D1).
    fn process(&self, p: [i32; 3]) -> Vec<Copy> {
        use Turn::*;
        let chains: &[&[Turn]] = match (self.kind, self.orientation) {
            (Kind::Empty, _) => &[],
            (Kind::Plane, 0) => &[&[Z]],
            (Kind::Plane, _) => &[&[X]],
            (Kind::CrossPlane, 1) => &[&[D1], &[D2], &[D2, D1]],
            (Kind::CrossPlane, _) => &[&[X], &[Z], &[Z, X]],
            (Kind::TriplePlane, _) => {
                &[&[X], &[Z], &[Z, X], &[D1], &[X, D1], &[Z, D1], &[Z, X, D1]]
            }
        };
        chains
            .iter()
            .map(|c| Copy {
                pos: c.iter().fold(p, |q, t| self.flip(q, *t)),
                turns: c.to_vec(),
            })
            .collect()
    }

    /// `SymmetryWandItem.apply` / `remove`'s set (`process(Map)`): the copies of a block
    /// placed or broken at `pos`, the later of two at one position kept, `pos` itself left out
    /// (its block stays as placed); none when disabled or `pos` is over `MAX_RANGE` from the
    /// mirror (`distanceTo(atLowerCornerOf(pos))`).
    pub fn copies(&self, pos: [i32; 3]) -> Vec<Copy> {
        if !self.is_enabled() {
            return Vec::new();
        }
        let d = [0, 1, 2].map(|i| self.position[i] - f64::from(pos[i]));
        if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > MAX_RANGE {
            return Vec::new();
        }
        let mut out: Vec<Copy> = Vec::new();
        for c in self.process(pos) {
            match out.iter_mut().find(|o| o.pos == c.pos) {
                Some(o) => *o = c,
                None => out.push(c),
            }
        }
        out.retain(|c| c.pos != pos);
        out
    }

    /// `useOn` without sneaking, in the main hand: the mirror moves to the block before the
    /// clicked face (an empty one becomes a plane), facing the player; the wand is enabled.
    /// `facing_z`: the player faces north or south (`getDirection`); `head_yaw` is
    /// `getYHeadRot`.
    pub fn clicked(&mut self, at: [i32; 3], facing_z: bool, head_yaw: f32) {
        self.enabled = true;
        self.position = at.map(f64::from);
        let plane = if facing_z { 0 } else { 1 };
        match self.kind {
            Kind::Empty => {
                self.kind = Kind::Plane;
                self.orientation = plane;
                self.mirror_enable = true;
            }
            Kind::Plane => self.orientation = plane,
            Kind::CrossPlane => {
                // Java's float `%` keeps the dividend's sign.
                let abs = (head_yaw % 90.0).abs();
                let diagonal = abs > 22.0 && abs < 45.0 + 22.0;
                self.orientation = usize::from(diagonal);
            }
            Kind::TriplePlane => {}
        }
    }

    /// `use` without sneaking: the mirror switched off.
    pub fn used(&mut self) {
        self.enabled = false;
    }
}

/// `Entity.getDirection` along z (north or south) from a yaw in degrees.
pub fn faces_z(yaw: f32) -> bool {
    // `Direction.fromYRot`: floor(yaw / 90 + 0.5) & 3 → 0 south, 1 west, 2 north, 3 east.
    let i = ((f64::from(yaw) / 90.0 + 0.5).floor() as i64) & 3;
    i == 0 || i == 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wand(kind: Kind, orientation: usize, at: [f64; 3]) -> Wand {
        Wand {
            kind,
            orientation,
            position: at,
            mirror_enable: true,
            enabled: true,
        }
    }

    fn props(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_plane_mirrors_across_its_block() {
        // XY: across z through the mirror's block.
        let w = wand(Kind::Plane, 0, [800.0, 64.0, 800.0]);
        assert_eq!(
            w.copies([802, 64, 803]),
            vec![Copy {
                pos: [802, 64, 797],
                turns: vec![Turn::Z]
            }]
        );
        // On the plane: nothing else to place.
        assert!(w.copies([805, 70, 800]).is_empty());
        // YZ: across x.
        let w = wand(Kind::Plane, 1, [800.0, 64.0, 800.0]);
        assert_eq!(w.copies([803, 64, 801])[0].pos, [797, 64, 801]);
        // Disabled, empty or too far: nothing.
        let mut off = w;
        off.used();
        assert!(off.copies([803, 64, 801]).is_empty());
        assert!(wand(Kind::Empty, 0, [0.0; 3]).copies([1, 0, 0]).is_empty());
        assert!(w.copies([851, 64, 800]).is_empty());
        assert_eq!(w.copies([850, 64, 800]).len(), 1);
    }

    #[test]
    fn crosses_and_triple_planes_copy_to_every_quarter() {
        let w = wand(Kind::CrossPlane, 0, [800.0, 64.0, 800.0]);
        let mut at: Vec<[i32; 3]> = w.copies([802, 65, 801]).iter().map(|c| c.pos).collect();
        at.sort();
        assert_eq!(at, vec![[798, 65, 799], [798, 65, 801], [802, 65, 799]]);
        // Diagonal: across the two diagonals.
        let w = wand(Kind::CrossPlane, 1, [800.0, 64.0, 800.0]);
        let mut at: Vec<[i32; 3]> = w.copies([802, 64, 801]).iter().map(|c| c.pos).collect();
        at.sort();
        assert_eq!(at, vec![[798, 64, 799], [799, 64, 798], [801, 64, 802]]);
        // Triple: all eight octants but its own.
        let w = wand(Kind::TriplePlane, 0, [800.0, 64.0, 800.0]);
        let c = w.copies([802, 64, 801]);
        assert_eq!(c.len(), 7);
        let mut at: Vec<[i32; 3]> = c.iter().map(|c| c.pos).collect();
        at.sort();
        assert_eq!(
            at,
            vec![
                [798, 64, 799],
                [798, 64, 801],
                [799, 64, 798],
                [799, 64, 802],
                [801, 64, 798],
                [801, 64, 802],
                [802, 64, 799]
            ]
        );
        // Truncation toward zero of a fractional mirror.
        let w = wand(Kind::Plane, 1, [800.5, 64.0, 800.0]);
        assert_eq!(w.copies([802, 64, 800])[0].pos, [800, 64, 800]);
    }

    #[test]
    fn states_turn_as_the_mirror_does() {
        // Stairs facing south mirrored across z face north.
        let mut s = props(&[
            ("facing", "south"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ]);
        Turn::Z.apply("minecraft:oak_stairs", &mut s);
        assert_eq!(s["facing"], "north");
        // Across x they keep facing south.
        let mut s = props(&[
            ("facing", "south"),
            ("half", "bottom"),
            ("shape", "straight"),
        ]);
        Turn::X.apply("minecraft:oak_stairs", &mut s);
        assert_eq!(s["facing"], "south");
        // A log along x across a diagonal lies along z.
        let mut s = props(&[("axis", "x")]);
        Turn::D1.apply("minecraft:oak_log", &mut s);
        assert_eq!(s["axis"], "z");
        // A furnace facing east across D1: counterclockwise to north, then front-back: north.
        let mut s = props(&[("facing", "east"), ("lit", "false")]);
        Turn::D1.apply("minecraft:furnace", &mut s);
        assert_eq!(s["facing"], "north");
        let mut s = props(&[("facing", "east"), ("lit", "false")]);
        Turn::D2.apply("minecraft:furnace", &mut s);
        assert_eq!(s["facing"], "south");
    }

    #[test]
    fn clicking_moves_and_faces_the_mirror() {
        let mut w = Wand::blank();
        assert!(!w.is_enabled());
        w.clicked([800, 65, 800], true, 0.0);
        assert_eq!(
            (w.kind, w.orientation, w.position, w.enabled),
            (Kind::Plane, 0, [800.0, 65.0, 800.0], true)
        );
        w.clicked([801, 65, 800], false, 90.0);
        assert_eq!(w.orientation, 1);
        w.kind = Kind::CrossPlane;
        w.clicked([801, 65, 800], false, 45.0);
        assert_eq!(w.orientation, 1);
        w.clicked([801, 65, 800], false, -100.0);
        assert_eq!(w.orientation, 0);
        w.used();
        assert!(!w.is_enabled());
        assert!(faces_z(0.0) && faces_z(180.0) && !faces_z(90.0) && !faces_z(-90.0));
        assert!(faces_z(44.0) && !faces_z(46.0));
        let mut t = Wand {
            kind: Kind::TriplePlane,
            ..Wand::blank()
        };
        t.set_orientation(1);
        assert_eq!(t.orientation, 0);
        assert_eq!(Kind::parse("nope"), Kind::Empty);
    }
}
