//! Create's handheld worldshaper (`WorldshaperItem`, `ZapperItem`, `ZapperInteractionHandler`,
//! `TerrainBrushes`: `CuboidBrush`, `SphereBrush`, `CylinderBrush`; `TerrainTools`,
//! `PlacementOptions`, `PlacementPatterns`): the blocks a use reaches and what it does to each.
//!
//! Its settings ride the stack's `minecraft:custom_data` under Create's component names
//! (`create:shaper_brush`, `create:shaper_brush_params`, `create:shaper_tool`,
//! `create:shaper_placement_options`, `create:placement_pattern`, `create:shaper_block_used`).

use crate::kinetics::Dir;
use std::collections::BTreeMap;

pub const WORLDSHAPER: &str = "create:handheld_worldshaper";
pub const BRUSH_KEY: &str = "create:shaper_brush";
pub const PARAMS_KEY: &str = "create:shaper_brush_params";
pub const TOOL_KEY: &str = "create:shaper_tool";
pub const PLACEMENT_KEY: &str = "create:shaper_placement_options";
pub const PATTERN_KEY: &str = "create:placement_pattern";
pub const BLOCK_KEY: &str = "create:shaper_block_used";
/// `WorldshaperItem.getZappingRange`.
pub const RANGE: f64 = 128.0;
/// `WorldshaperItem.getCooldownDelay`.
pub const COOLDOWN: u32 = 2;

macro_rules! named {
    ($t:ident { $($v:ident = $n:literal),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum $t { #[default] $($v),* }
        impl $t {
            pub const ALL: &[$t] = &[$($t::$v),*];
            /// `getSerializedName` (`Lang.asId(name())`).
            pub fn name(self) -> &'static str { match self { $($t::$v => $n),* } }
            pub fn parse(s: &str) -> Option<$t> { Self::ALL.iter().copied().find(|v| v.name() == s) }
        }
    };
}

named!(Brush { Cuboid = "cuboid", Sphere = "sphere", Cylinder = "cylinder", Surface = "surface", Cluster = "cluster" });
named!(Tool { Fill = "fill", Place = "place", Replace = "replace", Clear = "clear", Overlay = "overlay", Flatten = "flatten" });
named!(Placement { Merged = "merged", Attached = "attached", Inserted = "inserted" });
named!(Pattern { Solid = "solid", Checkered = "checkered", InverseCheckered = "inverse_checkered", Chance25 = "chance25", Chance50 = "chance50", Chance75 = "chance75" });

/// `CuboidBrush.MAX_SIZE`, `SphereBrush.MAX_RADIUS`, `CylinderBrush.MAX_RADIUS` / `MAX_HEIGHT`.
pub const CUBOID_MAX: i32 = 32;
pub const SPHERE_MAX: i32 = 10;
pub const CYLINDER_MAX_RADIUS: i32 = 8;
pub const CYLINDER_MAX_HEIGHT: i32 = 8;

impl Brush {
    /// Whether the port has it (the surface and cluster brushes, `DynamicBrush`, are not).
    pub fn ported(self) -> bool {
        matches!(self, Brush::Cuboid | Brush::Sphere | Brush::Cylinder)
    }
    /// `getMin` / `getMax` of a parameter (`amtParams` of them: 3, 1, 2).
    pub fn bounds(self, index: usize) -> (i32, i32) {
        match (self, index) {
            (Brush::Cuboid, _) => (1, CUBOID_MAX),
            (Brush::Sphere, _) => (0, SPHERE_MAX),
            (Brush::Cylinder, 0) => (0, CYLINDER_MAX_RADIUS),
            (Brush::Cylinder, _) => (1, CYLINDER_MAX_HEIGHT),
            _ => (0, i32::MAX),
        }
    }
    pub fn params(self) -> usize {
        match self {
            Brush::Cuboid => 3,
            Brush::Sphere => 1,
            Brush::Cylinder => 2,
            Brush::Surface | Brush::Cluster => 3,
        }
    }
}

impl Tool {
    /// `requiresSelectedBlock`.
    pub fn requires_block(self) -> bool {
        !matches!(self, Tool::Clear | Tool::Flatten)
    }
}

/// The worldshaper's settings (`configureSettings`); `params` is `SHAPER_BRUSH_PARAMS` (None
/// until set: the worldshaper asks to be configured first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub brush: Brush,
    pub params: Option<[i32; 3]>,
    pub tool: Tool,
    pub placement: Placement,
    pub pattern: Pattern,
}

/// Java's integer division (toward zero), as `/` on `int`.
fn jdiv(a: i32, b: i32) -> i32 {
    a / b
}

/// `ShapedBrush.getIncludedPositions` of a brush with `params`.
pub fn shape(brush: Brush, params: [i32; 3]) -> Vec<[i32; 3]> {
    let center_dist = |p: [i32; 3]| {
        (f64::from(p[0]).powi(2) + f64::from(p[1]).powi(2) + f64::from(p[2]).powi(2)).sqrt()
    };
    match brush {
        Brush::Cuboid => {
            // `betweenClosed(-(p - 1) / 2, p / 2)`, x fastest then y then z
            // (`BlockPos.betweenClosedStream`).
            let lo = params.map(|p| jdiv(p - 1, -2));
            let hi = params.map(|p| jdiv(p, 2));
            let mut out = Vec::new();
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        out.push([x, y, z]);
                    }
                }
            }
            out
        }
        Brush::Sphere => {
            let r = params[0];
            if !(0..=SPHERE_MAX).contains(&r) {
                return Vec::new();
            }
            let mut out = Vec::new();
            for z in -r - 1..=r + 1 {
                for y in -r - 1..=r + 1 {
                    for x in -r - 1..=r + 1 {
                        // Centres' distance below `radius + .5f`.
                        if center_dist([x, y, z]) < f64::from(r as f32 + 0.5) {
                            out.push([x, y, z]);
                        }
                    }
                }
            }
            out
        }
        Brush::Cylinder => {
            let (r, h) = (params[0], params[1]);
            if !(0..=CYLINDER_MAX_RADIUS).contains(&r) || !(0..=CYLINDER_MAX_HEIGHT).contains(&h) {
                return Vec::new();
            }
            let mut disc = Vec::new();
            for z in -r - 1..=r + 1 {
                for x in -r - 1..=r + 1 {
                    if center_dist([x, 0, z]) < f64::from(r as f32 + 0.42) {
                        disc.push([x, 0, z]);
                    }
                }
            }
            let mut out = Vec::new();
            for layer in 0..h {
                let y = layer - h / 2;
                out.extend(disc.iter().map(|p| [p[0], y, p[2]]));
            }
            out
        }
        Brush::Surface | Brush::Cluster => Vec::new(),
    }
}

/// `Brush.getOffset(ray, face, option)`: where the shape's centre goes from the block hit.
pub fn offset(brush: Brush, params: [i32; 3], face: Dir, placement: Placement) -> [i32; 3] {
    if placement == Placement::Merged {
        return [0; 3];
    }
    let attached = placement == Placement::Attached;
    let negative = face.offset().iter().sum::<i32>() < 0;
    let axis = face.axis() as usize;
    let along = match brush {
        Brush::Cuboid => {
            let off = if attached {
                if negative { 2 } else { 1 }
            } else {
                0
            };
            let half = |p: i32| jdiv(p + if p == 0 { 0 } else { off }, 2);
            [half(params[0]), half(params[1]), half(params[2])][axis]
        }
        Brush::Sphere => params[0] + 1 + if attached { 0 } else { -1 },
        Brush::Cylinder => {
            let r = params[0] + 1 + if attached { 0 } else { -1 };
            let y_off = match (attached, negative) {
                (true, true) => 1,
                (true, false) => 2,
                (false, true) => 0,
                (false, false) => -1,
            };
            let y = jdiv(params[1] + if params[1] == 0 { 0 } else { y_off }, 2);
            if face.axis() == crate::kinetics::Axis::Y {
                y
            } else {
                r
            }
        }
        Brush::Surface | Brush::Cluster => 0,
    };
    let n = if attached { along } else { -along };
    face.offset().map(|o| o * n)
}

/// `PlacementPatterns.applyPattern`: the positions kept. `next_bool` is the level's
/// `random.nextBoolean`, drawn per position as Create draws it.
pub fn apply_pattern(
    positions: &mut Vec<[i32; 3]>,
    pattern: Pattern,
    mut next_bool: impl FnMut() -> bool,
) {
    positions.retain(|p| {
        let removed = match pattern {
            Pattern::Solid => false,
            Pattern::Chance25 => next_bool() || next_bool(),
            Pattern::Chance50 => next_bool(),
            Pattern::Chance75 => next_bool() && next_bool(),
            // Java's `%` keeps the sign: -1 % 2 is -1, not 0.
            Pattern::Checkered => (p[0] + p[1] + p[2]) % 2 == 0,
            Pattern::InverseCheckered => (p[0] + p[1] + p[2]) % 2 != 0,
        };
        !removed
    });
}

/// The blocks a use reaches: the shape around the block hit, moved by the placement, thinned by
/// the pattern (`WorldshaperItem.activate` up to `run`).
pub fn positions(
    settings: &Settings,
    hit: [i32; 3],
    face: Dir,
    next_bool: impl FnMut() -> bool,
) -> Vec<[i32; 3]> {
    let Some(params) = settings.params else {
        return Vec::new();
    };
    let o = offset(settings.brush, params, face, settings.placement);
    let target = [hit[0] + o[0], hit[1] + o[1], hit[2] + o[2]];
    let mut out: Vec<[i32; 3]> = shape(settings.brush, params)
        .into_iter()
        .map(|p| [target[0] + p[0], target[1] + p[1], target[2] + p[2]])
        .collect();
    apply_pattern(&mut out, settings.pattern, next_bool);
    out
}

/// What a tool writes at a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Air,
    Paint,
}

/// What the tool sees at a position: whether it can be replaced (`canBeReplaced`) and whether it
/// is already the painted state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub replaceable: bool,
    pub painted: bool,
}

/// `TerrainTools.run` (no flattening): the writes in order, each `setBlockAndUpdate`.
pub fn run(
    tool: Tool,
    positions: &[[i32; 3]],
    mut seen: impl FnMut([i32; 3]) -> Seen,
) -> Vec<([i32; 3], Write)> {
    let mut out = Vec::new();
    for &p in positions {
        match tool {
            Tool::Clear => out.push((p, Write::Air)),
            Tool::Place => out.push((p, Write::Paint)),
            Tool::Fill => {
                if seen(p).replaceable {
                    out.push((p, Write::Paint));
                }
            }
            Tool::Replace => {
                if !seen(p).replaceable {
                    out.push((p, Write::Paint));
                }
            }
            Tool::Overlay => {
                let s = seen(p);
                if s.replaceable || s.painted {
                    continue;
                }
                let up = [p[0], p[1] + 1, p[2]];
                if seen(up).replaceable {
                    out.push((up, Write::Paint));
                }
            }
            Tool::Flatten => {}
        }
    }
    out
}

/// `ZapperInteractionHandler.trySelect`'s state, or None when it can't be picked (a door's or
/// a plant's half, an attached, hanging or bed block); then `BlockHelper.setZeroAge`, as the
/// use takes it.
pub fn selected_state(props: &BTreeMap<String, String>) -> Option<BTreeMap<String, String>> {
    let is = |k: &str, values: &[&str]| props.get(k).is_some_and(|v| values.contains(&v.as_str()));
    if is("half", &["upper", "lower"])
        || props.contains_key("attached")
        || props.contains_key("hanging")
        || is("part", &["head", "foot"])
    {
        return None;
    }
    let mut out = props.clone();
    if is(
        "shape",
        &[
            "straight",
            "inner_left",
            "inner_right",
            "outer_left",
            "outer_right",
        ],
    ) && props.contains_key("half")
    {
        out.insert("shape".into(), "straight".into());
    }
    if out.contains_key("persistent") {
        out.insert("persistent".into(), "true".into());
    }
    if out.contains_key("waterlogged") {
        out.insert("waterlogged".into(), "false".into());
    }
    Some(out)
}

/// `BlockHelper.setZeroAge`: the first of `age`, `honey_level`, `hatch`, `stage` set to 0.
pub fn zero_age(props: &mut BTreeMap<String, String>) {
    for k in ["age", "honey_level", "hatch", "stage"] {
        if let Some(v) = props.get_mut(k) {
            *v = "0".into();
            return;
        }
    }
}

/// A ray through blocks to the first outline box it meets (`Level.clip` with `Block.OUTLINE`,
/// no fluids): the block, the face entered and the point. `boxes(pos)` gives a block's outline
/// boxes in block space (empty for air).
pub fn clip(
    from: [f64; 3],
    to: [f64; 3],
    mut boxes: impl FnMut([i32; 3]) -> Vec<[f64; 6]>,
) -> Option<([i32; 3], Dir, [f64; 3])> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut cell = from.map(|v| v.floor() as i32);
    let step = d.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let next = |i: usize, c: i32| -> f64 {
        if d[i] == 0.0 {
            return f64::INFINITY;
        }
        let edge = if d[i] > 0.0 {
            f64::from(c + 1)
        } else {
            f64::from(c)
        };
        (edge - from[i]) / d[i]
    };
    let mut t_max = [next(0, cell[0]), next(1, cell[1]), next(2, cell[2])];
    let t_delta = d.map(|v| {
        if v == 0.0 {
            f64::INFINITY
        } else {
            1.0 / v.abs()
        }
    });
    for _ in 0..1024 {
        let mut best: Option<(f64, Dir)> = None;
        for b in boxes(cell) {
            let lo = [
                f64::from(cell[0]) + b[0],
                f64::from(cell[1]) + b[1],
                f64::from(cell[2]) + b[2],
            ];
            let hi = [
                f64::from(cell[0]) + b[3],
                f64::from(cell[1]) + b[4],
                f64::from(cell[2]) + b[5],
            ];
            if let Some(hit) = ray_box(from, d, lo, hi)
                && best.is_none_or(|(t, _)| hit.0 < t)
            {
                best = Some(hit);
            }
        }
        if let Some((t, face)) = best {
            let at = [0, 1, 2].map(|i| from[i] + d[i] * t);
            return Some((cell, face, at));
        }
        let i = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[i] > 1.0 {
            return None;
        }
        cell[i] += step[i];
        t_max[i] += t_delta[i];
    }
    None
}

/// The ray's entry into a box within `0..=1` of its length, and the face it enters through.
fn ray_box(from: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<(f64, Dir)> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut face = None;
    for i in 0..3 {
        if d[i] == 0.0 {
            if from[i] < lo[i] || from[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[i] - from[i]) / d[i], (hi[i] - from[i]) / d[i]);
        let (near, far) = if a < b { (a, b) } else { (b, a) };
        if near > t0 {
            t0 = near;
            // Entering a low side through its negative face, a high side its positive one.
            let dir = match (i, d[i] > 0.0) {
                (0, true) => Dir::West,
                (0, false) => Dir::East,
                (1, true) => Dir::Down,
                (1, false) => Dir::Up,
                (_, true) => Dir::North,
                (_, false) => Dir::South,
            };
            face = Some(dir);
        }
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    // A ray starting inside a box hits it at once, on the face it looks away from.
    let face = face.unwrap_or_else(|| {
        let i = (0..3)
            .max_by(|a, b| d[*a].abs().total_cmp(&d[*b].abs()))
            .unwrap_or(0);
        Dir::ALL
            .into_iter()
            .find(|f| f.axis() as usize == i && (f.offset()[i] > 0) == (d[i] < 0.0))
            .unwrap_or(Dir::Up)
    });
    Some((t0, face))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuboids_spheres_and_cylinders_cover_create_s_blocks() {
        // A 3×3×3 cuboid around its centre; an even size leans positive.
        assert_eq!(shape(Brush::Cuboid, [3, 3, 3]).len(), 27);
        let even = shape(Brush::Cuboid, [2, 1, 4]);
        assert_eq!(even.len(), 8);
        assert_eq!(even.first(), Some(&[0, 0, -1]));
        assert_eq!(even.last(), Some(&[1, 0, 2]));
        // Spheres: centres nearer than radius + 1/2: radius 0 is one block, radius 1 the 19
        // within √2, radius 2 the 81 within √6.
        assert_eq!(shape(Brush::Sphere, [0, 0, 0]), vec![[0, 0, 0]]);
        assert_eq!(shape(Brush::Sphere, [1, 0, 0]).len(), 19);
        assert_eq!(shape(Brush::Sphere, [2, 0, 0]).len(), 81);
        assert!(shape(Brush::Sphere, [11, 0, 0]).is_empty());
        // Cylinders: a disc of radius 1 (the 9 within √2 < 1.42) stacked around the middle,
        // height 3 from -1 to 1, height 0 nothing; radius 2 has the 21 within √5.
        let c = shape(Brush::Cylinder, [1, 3, 0]);
        assert_eq!(c.len(), 27);
        assert!(c.iter().all(|p| (-1..=1).contains(&p[1])));
        assert_eq!(shape(Brush::Cylinder, [2, 1, 0]).len(), 21);
        assert!(shape(Brush::Cylinder, [1, 0, 0]).is_empty());
        let tall = shape(Brush::Cylinder, [0, 4, 0]);
        assert_eq!(
            tall.iter().map(|p| p[1]).collect::<Vec<_>>(),
            vec![-2, -1, 0, 1]
        );
    }

    #[test]
    fn placements_move_the_brush_off_the_face() {
        // Merged: centred on the block hit.
        assert_eq!(
            offset(Brush::Cuboid, [3, 3, 3], Dir::Up, Placement::Merged),
            [0, 0, 0]
        );
        // Attached on top: (3 + 1) / 2 = 2 up; under it: (3 + 2) / 2 = 2 down.
        assert_eq!(
            offset(Brush::Cuboid, [3, 3, 3], Dir::Up, Placement::Attached),
            [0, 2, 0]
        );
        assert_eq!(
            offset(Brush::Cuboid, [3, 4, 3], Dir::Down, Placement::Attached),
            [0, -3, 0]
        );
        // Inserted: 3 / 2 = 1 into the face.
        assert_eq!(
            offset(Brush::Cuboid, [3, 3, 3], Dir::East, Placement::Inserted),
            [-1, 0, 0]
        );
        // Spheres: radius + 1 out, radius in.
        assert_eq!(
            offset(Brush::Sphere, [2, 0, 0], Dir::North, Placement::Attached),
            [0, 0, -3]
        );
        assert_eq!(
            offset(Brush::Sphere, [2, 0, 0], Dir::North, Placement::Inserted),
            [0, 0, 2]
        );
        // Cylinders: by height on top, by radius on a side.
        assert_eq!(
            offset(Brush::Cylinder, [2, 3, 0], Dir::Up, Placement::Attached),
            [0, 2, 0]
        );
        assert_eq!(
            offset(Brush::Cylinder, [2, 3, 0], Dir::South, Placement::Attached),
            [0, 0, 3]
        );
        assert_eq!(
            offset(Brush::Cylinder, [2, 3, 0], Dir::Down, Placement::Inserted),
            [0, 1, 0]
        );
    }

    #[test]
    fn tools_and_patterns_pick_their_blocks() {
        let s = Settings {
            brush: Brush::Cuboid,
            params: Some([3, 1, 1]),
            tool: Tool::Fill,
            placement: Placement::Attached,
            pattern: Pattern::Solid,
        };
        let at = positions(&s, [800, 64, 800], Dir::Up, || false);
        assert_eq!(at, vec![[799, 65, 800], [800, 65, 800], [801, 65, 800]]);
        // Unconfigured: nothing.
        let none = Settings {
            params: None,
            ..s.clone()
        };
        assert!(positions(&none, [0, 0, 0], Dir::Up, || false).is_empty());
        // Checkered keeps odd sums (negative ones too: Java's -1 % 2 is -1).
        let mut p = vec![[0, 0, 0], [1, 0, 0], [-1, 0, 0], [2, 0, 0]];
        apply_pattern(&mut p, Pattern::Checkered, || false);
        assert_eq!(p, vec![[1, 0, 0], [-1, 0, 0]]);
        let mut p = vec![[0, 0, 0], [1, 0, 0]];
        apply_pattern(&mut p, Pattern::Chance50, || true);
        assert!(p.is_empty());
        // Tools: air at 1, stone at 2 and 3.
        let world = |p: [i32; 3]| Seen {
            replaceable: p[0] == 1 || p[1] == 1,
            painted: p[0] == 3,
        };
        let at = [[1, 0, 0], [2, 0, 0], [3, 0, 0]];
        let fill = run(Tool::Fill, &at, world);
        assert_eq!(fill, vec![([1, 0, 0], Write::Paint)]);
        let replace = run(Tool::Replace, &at, world);
        assert_eq!(replace.len(), 2);
        assert_eq!(run(Tool::Place, &at, world).len(), 3);
        assert!(
            run(Tool::Clear, &at, world)
                .iter()
                .all(|w| w.1 == Write::Air)
        );
        // Overlay: on top of solid blocks not already painted.
        assert_eq!(
            run(Tool::Overlay, &at, world),
            vec![([2, 1, 0], Write::Paint)]
        );
        assert!(run(Tool::Flatten, &at, world).is_empty());
        assert!(Tool::Fill.requires_block() && !Tool::Clear.requires_block());
        assert_eq!(Pattern::parse("chance25"), Some(Pattern::Chance25));
    }

    #[test]
    fn selection_drops_unpickable_states_and_straightens_stairs() {
        let p = |kv: &[(&str, &str)]| -> BTreeMap<String, String> {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(selected_state(&p(&[("half", "lower"), ("facing", "north")])).is_none());
        assert!(selected_state(&p(&[("part", "head")])).is_none());
        assert!(selected_state(&p(&[("hanging", "false")])).is_none());
        let s = selected_state(&p(&[
            ("half", "top"),
            ("shape", "outer_left"),
            ("waterlogged", "true"),
        ]))
        .unwrap();
        assert_eq!(
            (s["shape"].as_str(), s["waterlogged"].as_str()),
            ("straight", "false")
        );
        let leaves = selected_state(&p(&[("persistent", "false"), ("distance", "7")])).unwrap();
        assert_eq!(leaves["persistent"], "true");
        let mut crop = p(&[("age", "7")]);
        zero_age(&mut crop);
        assert_eq!(crop["age"], "0");
    }

    #[test]
    fn the_ray_finds_the_first_box_and_its_face() {
        let solid = |p: [i32; 3]| {
            if p == [5, 0, 0] || p == [9, 0, 0] {
                vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            } else if p == [3, 0, 0] {
                // A slab-high box the ray passes over.
                vec![[0.0, 0.0, 0.0, 1.0, 0.25, 1.0]]
            } else {
                Vec::new()
            }
        };
        let (pos, face, at) = clip([0.5, 0.5, 0.5], [20.5, 0.5, 0.5], solid).unwrap();
        assert_eq!((pos, face), ([5, 0, 0], Dir::West));
        assert!((at[0] - 5.0).abs() < 1e-9);
        // Downward onto the slab.
        let (pos, face, _) = clip([3.5, 3.0, 0.5], [3.5, -3.0, 0.5], solid).unwrap();
        assert_eq!((pos, face), ([3, 0, 0], Dir::Up));
        // Too short to reach anything.
        assert!(clip([0.5, 0.5, 0.5], [4.5, 0.5, 0.5], solid).is_none());
        // Backwards, from the far side.
        let (pos, face, _) = clip([12.5, 0.5, 0.5], [0.5, 0.5, 0.5], solid).unwrap();
        assert_eq!((pos, face), ([9, 0, 0], Dir::East));
    }
}
