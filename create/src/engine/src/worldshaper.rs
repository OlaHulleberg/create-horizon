//! Create's handheld worldshaper (`WorldshaperItem`, `ZapperItem`, `ZapperInteractionHandler`,
//! `TerrainBrushes`: `CuboidBrush`, `SphereBrush`, `CylinderBrush`, `DynamicBrush`;
//! `TerrainTools`, `PlacementOptions`, `PlacementPatterns`): the blocks a use reaches and what it
//! does to each, and what its screen offers for each brush; the flatten tool's smoothing
//! (`FlattenTool`).
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

/// `DynamicBrush.MAX_RADIUS`: the surface and cluster brushes' range.
pub const DYNAMIC_MAX: i32 = 10;

impl Brush {
    /// The brushes that are a fixed shape (`ShapedBrush`); the surface and cluster brushes
    /// (`DynamicBrush`) follow the blocks they start on.
    pub fn shaped(self) -> bool {
        matches!(self, Brush::Cuboid | Brush::Sphere | Brush::Cylinder)
    }
    /// `getMin` / `getMax` of a parameter (the screen's inputs and the stored params).
    pub fn bounds(self, index: usize) -> (i32, i32) {
        match (self, index) {
            (Brush::Cuboid, _) => (1, CUBOID_MAX),
            (Brush::Sphere, _) => (0, SPHERE_MAX),
            (Brush::Cylinder, 0) => (0, CYLINDER_MAX_RADIUS),
            (Brush::Cylinder, _) => (1, CYLINDER_MAX_HEIGHT),
            (Brush::Surface | Brush::Cluster, _) => (1, DYNAMIC_MAX),
        }
    }
    /// `amtParams`: the inputs the screen shows.
    pub fn params(self) -> usize {
        match self {
            Brush::Cuboid => 3,
            Brush::Sphere => 1,
            Brush::Cylinder => 2,
            Brush::Surface | Brush::Cluster => 1,
        }
    }
    /// `getParamLabel`: the lang key (under `create.`) of parameter `index`'s title.
    pub fn param_label(self, index: usize) -> &'static str {
        match (self, index) {
            (Brush::Sphere, _) | (Brush::Cylinder, 0) => "generic.radius",
            (Brush::Surface | Brush::Cluster, _) => "generic.range",
            (_, 0) => "generic.width",
            (_, 1) => "generic.height",
            _ => "generic.length",
        }
    }
    /// `getSupportedTools`, in the screen's order.
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Brush::Surface => &[Tool::Overlay, Tool::Replace, Tool::Clear],
            Brush::Cluster => &[Tool::Replace, Tool::Clear],
            _ => Tool::ALL,
        }
    }
    /// `hasPlacementOptions`.
    pub fn has_placement(self) -> bool {
        self.shaped()
    }
    /// `hasConnectivityOptions`: follow diagonals and ignore material borders (the second and
    /// third params, on when 0).
    pub fn has_connectivity(self) -> bool {
        !self.shaped()
    }
    /// `redirectTool`: the surface brush's overlay places on the surface it found.
    pub fn redirect(self, tool: Tool) -> Tool {
        if !self.shaped() && tool == Tool::Overlay {
            Tool::Place
        } else {
            tool
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

/// The blocks a use reaches: [`reached`], thinned by the pattern (`WorldshaperItem.activate` up
/// to `run`).
pub fn positions<G: Ground>(
    settings: &Settings,
    hit: [i32; 3],
    face: Dir,
    ground: &mut G,
    next_bool: impl FnMut() -> bool,
) -> Vec<[i32; 3]> {
    let mut out = reached(settings, hit, face, ground);
    apply_pattern(&mut out, settings.pattern, next_bool);
    out
}

/// What the surface and cluster brushes read of the world (`DynamicBrush`).
pub trait Ground {
    /// A block's identity (`getBlock`), compared for material borders.
    type Block: PartialEq;
    fn block(&mut self, pos: [i32; 3]) -> Self::Block;
    /// `canBeReplaced`.
    fn replaceable(&mut self, pos: [i32; 3]) -> bool;
    /// `getDestroySpeed == -1`.
    fn unbreakable(&mut self, pos: [i32; 3]) -> bool;
    /// `BlockHelper.hasBlockSolidSide`: the block at `pos` is not air and its collision shape
    /// fills its `side` face.
    fn solid_side(&mut self, pos: [i32; 3], side: Dir) -> bool;
}

/// `DynamicBrush.addToGlobalPositions`: from the block hit (replacing) or the space before its
/// face (overlaying), a breadth-first walk over the blocks like it (any block when material
/// borders are ignored) within `params[0]` of the start, along the faces' plane for the surface
/// brush, diagonals included when `params[1]` is 0, in Create's order.
pub fn dynamic<G: Ground>(
    surface: bool,
    params: [i32; 3],
    target: [i32; 3],
    face: Dir,
    tool: Tool,
    ground: &mut G,
) -> Vec<[i32; 3]> {
    let diagonals = params[1] == 0;
    let fuzzy = params[2] == 0;
    let replace = tool != Tool::Overlay;
    let range = f64::from(params[0]);
    let add = |a: [i32; 3], b: [i32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let axis = face.axis() as usize;
    let mut offsets = Vec::new();
    for x in -1..=1i32 {
        for y in -1..=1i32 {
            for z in -1..=1i32 {
                let o = [x, y, z];
                if (x.abs() + y.abs() + z.abs() < 2 || diagonals) && (o[axis] == 0 || !surface) {
                    offsets.push(o);
                }
            }
        }
    }
    let state = ground.block(target);
    let f = face.offset();
    let back = face.opposite().offset();
    let start = if replace { target } else { add(target, f) };
    let mut visited = std::collections::HashSet::new();
    let mut frontier = std::collections::VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(p) = frontier.pop_front() {
        if !visited.insert(p) {
            continue;
        }
        // `BlockPos.closerThan`: the squared distance below the range's square.
        let d: i64 = (0..3).map(|i| i64::from(p[i] - start[i]).pow(2)).sum();
        if d as f64 >= range * range {
            continue;
        }
        if replace {
            if ground.unbreakable(p)
                || (!fuzzy && ground.block(p) != state)
                || ground.replaceable(p)
                || (surface && ground.solid_side(add(p, f), face.opposite()))
            {
                continue;
            }
        } else {
            let on = add(p, back);
            if ground.replaceable(on)
                || (!fuzzy && ground.block(on) != state)
                || !ground.replaceable(p)
            {
                continue;
            }
        }
        out.push(p);
        frontier.extend(offsets.iter().map(|o| add(p, *o)));
    }
    out
}

/// The blocks a use reaches before its pattern: the brush's shape moved by the placement, or
/// the dynamic brushes' walk (`getOffset` then `addToGlobalPositions`). What the worldshaper
/// outlines while held (`WorldshaperRenderHandler`).
pub fn reached<G: Ground>(
    settings: &Settings,
    hit: [i32; 3],
    face: Dir,
    ground: &mut G,
) -> Vec<[i32; 3]> {
    let Some(params) = settings.params else {
        return Vec::new();
    };
    let o = offset(settings.brush, params, face, settings.placement);
    let target = [hit[0] + o[0], hit[1] + o[1], hit[2] + o[2]];
    if settings.brush.shaped() {
        return shape(settings.brush, params)
            .into_iter()
            .map(|p| [target[0] + p[0], target[1] + p[1], target[2] + p[2]])
            .collect();
    }
    dynamic(
        settings.brush == Brush::Surface,
        params,
        target,
        face,
        settings.tool,
        ground,
    )
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

/// The world the flatten tool reads and writes, a block at a time (`setBlockAndUpdate`).
pub trait FlattenWorld {
    type State: Clone;
    fn state(&mut self, pos: [i32; 3]) -> Self::State;
    /// `canBeReplaced` of the block now at `pos`.
    fn replaceable(&mut self, pos: [i32; 3]) -> bool;
    fn set(&mut self, pos: [i32; 3], state: Self::State);
    /// `state.getFluidState().createLegacyBlock()`: air, or the fluid it holds.
    fn fluid_block(&mut self, state: &Self::State) -> Self::State;
    /// Whether `state` is a `LiquidBlock` (water, lava).
    fn liquid(&mut self, state: &Self::State) -> bool;
}

/// `FlattenTool.kernel`: a gaussian with sigma 1.
const KERNEL: [[f32; 5]; 5] = [
    [0.003765, 0.015019, 0.023792, 0.015019, 0.003765],
    [0.015019, 0.059912, 0.094907, 0.059912, 0.015019],
    [0.023792, 0.094907, 0.150342, 0.094907, 0.023792],
    [0.015019, 0.059912, 0.094907, 0.059912, 0.015019],
    [0.003765, 0.015019, 0.023792, 0.015019, 0.003765],
];

/// `Mth.floor(float)`.
fn floor_f(v: f32) -> i32 {
    let i = v as i32;
    if v < i as f32 { i - 1 } else { i }
}

/// `FlattenTool.applyKernel`: each height blurred with its 5 x 5 neighbours (one outside the
/// map or unknown counts as itself).
fn apply_kernel(values: &[Vec<i32>]) -> Vec<Vec<i32>> {
    let (w, h) = (values.len() as i32, values[0].len() as i32);
    let mut out = vec![vec![0; h as usize]; w as usize];
    for i in 0..w {
        for j in 0..h {
            let value = values[i as usize][j as usize];
            let mut sum = 0f32;
            for io in -2..=2 {
                for jo in -2..=2 {
                    let (ti, tj) = (i + io, j + jo);
                    let mut r = if ti < 0 || ti >= w || tj < 0 || tj >= h {
                        value
                    } else {
                        values[ti as usize][tj as usize]
                    };
                    if r == i32::MIN {
                        r = value;
                    }
                    sum += KERNEL[(io + 2) as usize][(jo + 2) as usize] * r as f32;
                }
            }
            out[i as usize][j as usize] = floor_f(sum + 0.5);
        }
    }
    out
}

/// `FlattenTool.getCoords`: a position's two coordinates across `facing`'s axis.
fn across(p: [i32; 3], facing: Dir) -> (i32, i32) {
    match facing.axis() {
        crate::kinetics::Axis::X => (p[2], p[1]),
        crate::kinetics::Axis::Y => (p[0], p[2]),
        crate::kinetics::Axis::Z => (p[0], p[1]),
    }
}

/// `facing.getAxis().choose(x, y, z)`.
fn along(p: [i32; 3], facing: Dir) -> i32 {
    match facing.axis() {
        crate::kinetics::Axis::X => p[0],
        crate::kinetics::Axis::Y => p[1],
        crate::kinetics::Axis::Z => p[2],
    }
}

fn step(p: [i32; 3], o: [i32; 3], n: i32) -> [i32; 3] {
    [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
}

/// `FlattenTool.apply`: the surface over the brush's blocks (the first free block past each
/// along `facing`, the face clicked) is smoothed: a height map of the highest surface across
/// the face, blurred by the kernel, then each surface is lowered (its block below takes the
/// surface's state, the surface its fluid) or raised (the block past it takes the surface's
/// state, but a liquid's, and the surface the block before it) a block at a time to the
/// blurred height.
pub fn flatten<W: FlattenWorld>(world: &mut W, targets: &[[i32; 3]], facing: Dir) {
    let o = facing.offset();
    let offset = o[0] + o[1] + o[2];
    let mut surfaces = Vec::new();
    let mut map: std::collections::HashMap<(i32, i32), i32> = std::collections::HashMap::new();
    let (mut min_entry, mut max_entry) = (i32::MAX, i32::MIN);
    let (mut min1, mut min2, mut max1, mut max2) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &target in targets {
        let coords = across(target, facing);
        min1 = min1.min(coords.0);
        min2 = min2.min(coords.1);
        max1 = max1.max(coords.0);
        max2 = max2.max(coords.1);
        if world.replaceable(target) {
            map.entry(coords).or_insert(i32::MIN);
            continue;
        }
        let p = step(target, o, 1);
        if !world.replaceable(p) {
            if map.get(&coords).is_none_or(|h| *h == i32::MIN) {
                map.insert(coords, i32::MAX);
            }
            continue;
        }
        surfaces.push(p);
        let coordinate = along(p, facing);
        let replace = match map.get(&coords) {
            None => true,
            Some(&h) => {
                h == i32::MAX
                    || h == i32::MIN
                    || h.wrapping_mul(offset) < coordinate.wrapping_mul(offset)
            }
        };
        if replace {
            map.insert(coords, coordinate);
            max_entry = max_entry.max(coordinate);
            min_entry = min_entry.min(coordinate);
        }
    }
    if surfaces.is_empty() {
        return;
    }
    let heights: Vec<Vec<i32>> = (0..=(max1 - min1))
        .map(|i| {
            (0..=(max2 - min2))
                .map(|j| match map.get(&(min1 + i, min2 + j)) {
                    None => i32::MIN,
                    Some(&i32::MAX) => {
                        if offset == 1 {
                            max_entry + 2
                        } else {
                            min_entry - 2
                        }
                    }
                    Some(&i32::MIN) => {
                        if offset == 1 {
                            min_entry - 2
                        } else {
                            max_entry + 2
                        }
                    }
                    Some(&h) => h,
                })
                .collect()
        })
        .collect();
    let heights = apply_kernel(&heights);
    let back = [-o[0], -o[1], -o[2]];
    for mut p in surfaces {
        let coords = across(p, facing);
        let mut surface = along(p, facing) * offset;
        let target = heights[(coords.0 - min1) as usize][(coords.1 - min2) as usize] * offset;
        if surface == target {
            continue;
        }
        let state = world.state(p);
        let mut time_out = 1000;
        // Lower the surface.
        while surface > target {
            let below = step(p, back, 1);
            world.set(below, state.clone());
            let fluid = world.fluid_block(&state);
            world.set(p, fluid);
            p = below;
            surface -= 1;
            if time_out <= 0 {
                break;
            }
            time_out -= 1;
        }
        // Raise it.
        while surface < target {
            let above = step(p, o, 1);
            if !world.liquid(&state) {
                world.set(above, state.clone());
            }
            let before = world.state(step(p, back, 1));
            world.set(p, before);
            p = above;
            surface += 1;
            if time_out <= 0 {
                break;
            }
            time_out -= 1;
        }
    }
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
        let at = positions(&s, [800, 64, 800], Dir::Up, &mut Flat, || false);
        assert_eq!(at, vec![[799, 65, 800], [800, 65, 800], [801, 65, 800]]);
        // Unconfigured: nothing.
        let none = Settings {
            params: None,
            ..s.clone()
        };
        assert!(positions(&none, [0, 0, 0], Dir::Up, &mut Flat, || false).is_empty());
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

    /// Stone (1) below y 1 but where taken out, air (0) above; water is 2.
    struct Terrain(std::collections::HashMap<[i32; 3], u8>);

    impl FlattenWorld for Terrain {
        type State = u8;
        fn state(&mut self, pos: [i32; 3]) -> u8 {
            *self.0.get(&pos).unwrap_or(&u8::from(pos[1] <= 0))
        }
        fn replaceable(&mut self, pos: [i32; 3]) -> bool {
            self.state(pos) != 1
        }
        fn set(&mut self, pos: [i32; 3], state: u8) {
            self.0.insert(pos, state);
        }
        fn fluid_block(&mut self, state: &u8) -> u8 {
            if *state == 2 { 2 } else { 0 }
        }
        fn liquid(&mut self, state: &u8) -> bool {
            *state == 2
        }
    }

    fn slab(y0: i32, y1: i32) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        for x in -2..=2 {
            for y in y0..=y1 {
                for z in -2..=2 {
                    out.push([x, y, z]);
                }
            }
        }
        out
    }

    #[test]
    fn flattening_fills_a_pit_and_cuts_a_spike() {
        // A flat ground stays as it is.
        let mut flat = Terrain(Default::default());
        flatten(&mut flat, &slab(-1, 0), Dir::Up);
        assert!(flat.0.is_empty());
        // A pit one deep: the blurred surface over it is back at 1, so it fills with the stone
        // under it.
        let mut pit = Terrain([([0, 0, 0], 0)].into_iter().collect());
        flatten(&mut pit, &slab(-1, 0), Dir::Up);
        assert_eq!(pit.state([0, 0, 0]), 1);
        assert_eq!(pit.state([0, 1, 0]), 0);
        // A spike two high: lowered to the ground's surface, the air above it taking its place.
        let mut spike = Terrain([([0, 1, 0], 1), ([0, 2, 0], 1)].into_iter().collect());
        flatten(&mut spike, &slab(0, 2), Dir::Up);
        assert_eq!(spike.state([0, 2, 0]), 0);
        assert_eq!(spike.state([0, 1, 0]), 0);
        assert_eq!(spike.state([0, 0, 0]), 1);
        // Nothing to stand on: no surface, nothing changes.
        let mut sky = Terrain(Default::default());
        flatten(&mut sky, &slab(5, 6), Dir::Up);
        assert!(sky.0.is_empty());
        // The kernel's rounding (`Mth.floor(v + .5)`) and its edges.
        assert_eq!(apply_kernel(&[vec![3]]), vec![vec![3]]);
        assert_eq!(floor_f(-0.5), -1);
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

    /// No world: the shaped brushes don't read one.
    struct Flat;
    impl Ground for Flat {
        type Block = u8;
        fn block(&mut self, _: [i32; 3]) -> u8 {
            0
        }
        fn replaceable(&mut self, _: [i32; 3]) -> bool {
            true
        }
        fn unbreakable(&mut self, _: [i32; 3]) -> bool {
            false
        }
        fn solid_side(&mut self, _: [i32; 3], _: Dir) -> bool {
            false
        }
    }

    /// A floor of stone (1) at y 0 with a dirt (2) square at x 2..=3, air (0) above.
    struct Floor;
    impl Ground for Floor {
        type Block = u8;
        fn block(&mut self, p: [i32; 3]) -> u8 {
            match p[1] {
                0 if (2..=3).contains(&p[0]) => 2,
                0 => 1,
                _ => 0,
            }
        }
        fn replaceable(&mut self, p: [i32; 3]) -> bool {
            self.block(p) == 0
        }
        fn unbreakable(&mut self, _: [i32; 3]) -> bool {
            false
        }
        fn solid_side(&mut self, p: [i32; 3], _: Dir) -> bool {
            self.block(p) != 0
        }
    }

    #[test]
    fn dynamic_brushes_follow_the_surface_and_its_material() {
        let s = |brush, params, tool| Settings {
            brush,
            params: Some(params),
            tool,
            placement: Placement::Merged,
            pattern: Pattern::Solid,
        };
        // Range 2 from the stone at the origin, no diagonals (params 1 and 2 are 1): what
        // orthogonal steps reach within a squared distance below 4, stone only.
        let mut got = reached(
            &s(Brush::Surface, [2, 1, 1], Tool::Replace),
            [0, 0, 0],
            Dir::Up,
            &mut Floor,
        );
        got.sort();
        assert_eq!(got.len(), 9);
        assert!(
            got.iter()
                .all(|p| p[1] == 0 && p[0].abs() <= 1 && p[2].abs() <= 1)
        );
        // Ignoring material borders reaches the dirt too; the surface never leaves the floor.
        let got = reached(
            &s(Brush::Surface, [3, 1, 0], Tool::Replace),
            [1, 0, 0],
            Dir::Up,
            &mut Floor,
        );
        assert!(got.contains(&[2, 0, 0]) && got.iter().all(|p| p[1] == 0));
        // Overlaying starts above the face, on top of stone only.
        let got = reached(
            &s(Brush::Surface, [2, 1, 1], Tool::Overlay),
            [1, 0, 0],
            Dir::Up,
            &mut Floor,
        );
        assert_eq!(got.first(), Some(&[1, 1, 0]));
        assert!(!got.contains(&[2, 1, 0]));
        // The cluster walks in all three dimensions: the air above and below isn't stone.
        let got = reached(
            &s(Brush::Cluster, [2, 1, 1], Tool::Replace),
            [0, 0, 0],
            Dir::Up,
            &mut Floor,
        );
        assert_eq!(got.len(), 9);
        // A wider range stops at the dirt unless material borders are ignored.
        let got = reached(
            &s(Brush::Surface, [4, 0, 1], Tool::Replace),
            [0, 0, 0],
            Dir::Up,
            &mut Floor,
        );
        assert!(got.contains(&[1, 0, 2]) && !got.contains(&[2, 0, 0]));
        assert_eq!(Brush::Surface.redirect(Tool::Overlay), Tool::Place);
        assert_eq!(Brush::Cluster.tools(), &[Tool::Replace, Tool::Clear]);
    }
}
