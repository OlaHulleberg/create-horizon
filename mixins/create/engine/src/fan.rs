//! Create's encased fan (`EncasedFanBlockEntity`, `AirCurrent`, `FanProcessing`,
//! `AllFanProcessingTypes`): a current in front of the fan (or into it), split into segments by
//! the catalysts it passes (water washes, soul fire haunts, fire smokes, lava blasts), and the
//! items lying in it on depots and belts processed after a while.

use crate::kinetics::{Dir, convert_to_direction};
use crate::processing::{Book, Recipe, Rng, roll};

/// `FanProcessingType`s, in `FanProcessingTypeRegistry`'s order (highest priority first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Splashing,
    Haunting,
    Smoking,
    Blasting,
}

/// Blaze burner heat (`BlazeBurnerBlock.HeatLevel`), in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Heat {
    None,
    Smouldering,
    Fading,
    Kindled,
    Seething,
}

/// What lies at a block of the current, as `isValidAt` reads it: the catalyst fluid tags, the
/// catalyst block tags, and the state properties they look at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Catalyst {
    /// Fluid tags `fan_processing_catalysts/splashing` and `.../blasting` (water, lava).
    pub water: bool,
    pub lava: bool,
    /// Block tags `fan_processing_catalysts/{splashing,haunting,smoking,blasting}`.
    pub splashing: bool,
    pub haunting: bool,
    pub smoking: bool,
    pub blasting: bool,
    /// `LIT` of a block in `#minecraft:campfires`.
    pub campfire_lit: Option<bool>,
    /// `FLAME_TYPE` of a lit blaze burner: true for soul.
    pub soul_flame: Option<bool>,
    /// `HEAT_LEVEL` of a blaze burner.
    pub heat: Option<Heat>,
}

/// `FanProcessingType.getAt`: the first type, by priority, valid at the block.
pub fn kind_at(c: &Catalyst) -> Option<Kind> {
    if c.water || c.splashing {
        return Some(Kind::Splashing);
    }
    let unlit = c.campfire_lit == Some(false);
    if c.haunting && !unlit && c.soul_flame != Some(false) {
        return Some(Kind::Haunting);
    }
    if c.smoking
        && !unlit
        && c.soul_flame != Some(true)
        && c.heat.is_none_or(|h| h == Heat::Smouldering)
    {
        return Some(Kind::Smoking);
    }
    if c.lava || (c.blasting && c.heat.is_none_or(|h| h >= Heat::Fading)) {
        return Some(Kind::Blasting);
    }
    None
}

/// `kinetics.fanPushDistance`, `fanPullDistance`, `fanRotationArgmax`, `fanProcessingTime`.
pub const PUSH_DISTANCE: f32 = 20.0;
pub const PULL_DISTANCE: f32 = 20.0;
pub const ROTATION_ARGMAX: f32 = 256.0;
pub const PROCESSING_TIME: i32 = 150;
/// `fanBlockCheckRate`: ticks between looks at what blocks the current.
pub const CHECK_RATE: u32 = 30;

/// `IAirCurrentSource.getMaxDistance`.
pub fn max_distance(speed: f32) -> f32 {
    let factor = (speed.abs() / ROTATION_ARGMAX).min(1.0);
    if speed > 0.0 {
        3.0 + factor * (PUSH_DISTANCE - 3.0)
    } else {
        3.0 + factor * (PULL_DISTANCE - 3.0)
    }
}

/// `getAirFlowDirection` against `getAirflowOriginSide` (its facing): pushing when the air
/// leaves through its facing.
pub fn pushing(speed: f32, facing: Dir) -> bool {
    convert_to_direction(speed, facing) > 0.0
}

/// A block in the current's way, for `getFlowLimit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Way {
    /// Nothing that stops air (no collision, or `fan_transparent`).
    Open,
    /// A full block: the current ends before it.
    Full,
    /// A partial shape: how deep the air gets into it (`findMaxDepth`); infinite for a hole.
    Depth(f32),
    /// Not loaded.
    Unloaded,
}

/// `AirCurrent.getFlowLimit`.
pub fn flow_limit(max: f32, way: impl Fn(i32) -> Way) -> f32 {
    let mut i = 0;
    while (i as f32) < max {
        match way(i + 1) {
            Way::Unloaded | Way::Full => return i as f32,
            Way::Open => {}
            Way::Depth(d) if d.is_infinite() => {}
            Way::Depth(d) => return (i as f32 + d + 1.0 / 32.0).min(max),
        }
        i += 1;
    }
    max
}

/// The current: its reach and, block by block from the fan, the processing at each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Current {
    pub max: f32,
    pub pushing: bool,
    /// Segments as `AirCurrent.rebuild` makes them: (start, end, type) in offsets from the fan.
    pub segments: Vec<(i32, i32, Option<Kind>)>,
}

impl Current {
    /// `AirCurrent.getLimit`.
    pub fn limit(&self) -> i32 {
        if self.max.fract() == 0.0 {
            self.max as i32
        } else {
            self.max as i32 + 1
        }
    }

    /// `AirCurrent.rebuild` past the flow limit: `kind(i)` the type valid at offset `i`.
    pub fn build(max: f32, pushing: bool, kind: impl Fn(i32) -> Option<Kind>) -> Current {
        let mut c = Current {
            max,
            pushing,
            segments: Vec::new(),
        };
        let limit = c.limit();
        let (start, end, step, to_offset) = if pushing {
            (1, limit, 1, -1)
        } else {
            (limit, 1, -1, 0)
        };
        let mut current: Option<(i32, Option<Kind>)> = None;
        let mut ty = None;
        let mut i = start;
        while i * step <= end * step {
            if let Some(k) = kind(i) {
                ty = Some(k);
            }
            match current {
                None => current = Some((i + to_offset, ty)),
                Some((s, t)) if t != ty => {
                    c.segments.push((s, i + to_offset, t));
                    current = Some((i + to_offset, ty));
                }
                _ => {}
            }
            i += step;
        }
        if let Some((s, t)) = current {
            c.segments.push((s, end + step + to_offset, t));
        }
        c
    }

    /// `AirCurrent.getTypeAt`.
    pub fn type_at(&self, offset: f32) -> Option<Kind> {
        if !(0.0..=self.max).contains(&offset) {
            return None;
        }
        if self.pushing {
            for &(_, end, t) in &self.segments {
                if offset <= end as f32 {
                    return t;
                }
            }
        } else {
            for &(_, end, t) in &self.segments {
                if offset >= end as f32 {
                    return t;
                }
            }
        }
        None
    }
}

/// An item's progress in a current (`TransportedItemStack.processedBy` / `processingTime`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub by: Option<Kind>,
    pub time: i32,
}

/// `FanProcessing.applyProcessing` for a transported stack, one tick: true when it is done now
/// (the caller applies the recipe). `can` says whether `kind` processes the stack at all.
pub fn advance(p: &mut Progress, kind: Kind, count: u32, can: bool) -> bool {
    if p.by != Some(kind) {
        p.by = Some(kind);
        let modifier = (count.max(1) as i32 - 1) / 16 + 1;
        p.time = PROCESSING_TIME * modifier + 1;
        if !can {
            p.time = -1;
        }
        return false;
    }
    if p.time == -1 {
        return false;
    }
    let was = p.time;
    p.time -= 1;
    was <= 0
}

/// `AirCurrent.bounds`: the block in front of the fan stretched `max` blocks along `facing`
/// (empty under a quarter block), as (min, max) corners.
pub fn bounds(fan: [i32; 3], facing: Dir, max: f32) -> Option<([f64; 3], [f64; 3])> {
    if max < 0.25 {
        return None;
    }
    let o = facing.offset();
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for a in 0..3 {
        let start = f64::from(fan[a] + o[a]);
        lo[a] = start;
        hi[a] = start + 1.0;
        let reach = f64::from(max) - 1.0;
        if o[a] > 0 {
            hi[a] += reach;
        } else if o[a] < 0 {
            lo[a] -= reach;
        }
    }
    Some((lo, hi))
}

/// Catnip's `VecHelper.alignedDistanceToFace`: how far `pos` is from the fan's `facing` face along
/// its axis.
pub fn aligned_distance(pos: [f64; 3], fan: [i32; 3], facing: Dir) -> f64 {
    let a = match facing.axis() {
        crate::kinetics::Axis::X => 0,
        crate::kinetics::Axis::Y => 1,
        crate::kinetics::Axis::Z => 2,
    };
    let face = f64::from(fan[a]) + if facing.positive() { 1.0 } else { 0.0 };
    (pos[a] - face).abs()
}

/// `tickAffectedEntities`' push on an entity that isn't sneaking (an item, a mob, a player): its
/// new motion, `flow` the way the air goes, `distance` from the fan's centre.
pub fn push(flow: Dir, speed: f32, max: f32, distance: f64, motion: [f64; 3]) -> [f64; 3] {
    let o = flow.offset();
    // `sneakModifier`: 512 (4096 for a sneaking entity).
    let acceleration = (f64::from(speed.abs()) / 512.0 / (distance / f64::from(max))) as f32;
    let mut out = motion;
    for a in 0..3 {
        let add = (f64::from(o[a] as f32 * acceleration) - motion[a]).clamp(-5.0, 5.0);
        out[a] = motion[a] + add / 8.0;
    }
    out
}

/// `FanProcessing.canProcess` for an item entity: its own progress decides once it has some.
pub fn entity_can(p: &Progress, kind: Kind, can: bool) -> bool {
    if p.by == Some(kind) {
        return p.time >= 0;
    }
    can
}

/// `decrementProcessingTime` for an item entity, one tick: true when done now. Unlike a stack on
/// a belt it counts down from the first tick and is done at 0.
pub fn advance_entity(p: &mut Progress, kind: Kind, count: u32) -> bool {
    if p.by != Some(kind) {
        p.by = Some(kind);
        p.time = PROCESSING_TIME * ((count.max(1) as i32 - 1) / 16 + 1) + 1;
    }
    p.time -= 1;
    p.time == 0
}

/// The recipe books the fan's types read: Create's splashing and haunting, vanilla's smoking,
/// smelting and blasting (cooking recipes: one ingredient, one result, chance 1).
#[derive(Clone, Copy)]
pub struct Books<'a> {
    pub splashing: &'a Book,
    pub haunting: &'a Book,
    pub smoking: &'a Book,
    pub smelting: &'a Book,
    pub blasting: &'a Book,
}

/// `getRecipeFor(...).filter(CAN_BE_AUTOMATED)`: the first recipe the item fits, unless that one
/// is `_manual_only`.
fn automated<'a>(book: &'a Book, item: &str) -> Option<&'a Recipe> {
    let r = book.get(book.find_single(item)?);
    (!r.id.ends_with("_manual_only")).then_some(r)
}

/// `canProcess`. A blasting current takes any item that doesn't resist fire: without a recipe it
/// burns up.
pub fn can_process(kind: Kind, item: &str, fire_resistant: bool, b: Books) -> bool {
    match kind {
        Kind::Splashing => b.splashing.find_single(item).is_some(),
        Kind::Haunting => b.haunting.find_single(item).is_some(),
        Kind::Smoking => automated(b.smoking, item).is_some(),
        Kind::Blasting => {
            automated(b.smelting, item).is_some()
                || automated(b.blasting, item).is_some()
                || !fire_resistant
        }
    }
}

/// `process` with `RecipeApplier.applyRecipeOn`: the stacks `count` of `item` turn into, or None
/// when nothing happens (`null`). An empty list destroys the stack. `max` gives stack limits.
pub fn process(
    kind: Kind,
    item: &str,
    count: u8,
    b: Books,
    max: impl Fn(&str) -> u8,
    rng: &mut Rng,
) -> Option<Vec<(String, u8)>> {
    match kind {
        Kind::Splashing | Kind::Haunting => {
            let book = if kind == Kind::Splashing {
                b.splashing
            } else {
                b.haunting
            };
            let r = book.get(book.find_single(item)?);
            let mut stacks: Vec<(String, u8)> = Vec::new();
            for _ in 0..count {
                for (it, mut n) in roll(r, rng) {
                    for (prev, m) in stacks.iter_mut() {
                        if n > 0 && *prev == it {
                            let moved = n.min(max(prev).saturating_sub(*m));
                            *m += moved;
                            n -= moved;
                        }
                    }
                    if n > 0 {
                        stacks.push((it, n));
                    }
                }
            }
            Some(stacks)
        }
        Kind::Smoking => Some(multiplied(automated(b.smoking, item)?, count, &max)),
        Kind::Blasting => {
            let smelt = automated(b.smelting, item).or_else(|| automated(b.blasting, item));
            let Some(smelt) = smelt else {
                return Some(Vec::new());
            };
            // What a smoker cooks to the same result burns up instead (an empty list).
            fn same(r: &Recipe) -> Option<&str> {
                r.results.first().map(|o| o.item.as_str())
            }
            match automated(b.smoking, item) {
                Some(smoked) if same(smoked) == same(smelt) => Some(Vec::new()),
                _ => Some(multiplied(smelt, count, &max)),
            }
        }
    }
}

/// `ItemHelper.multipliedOutput` of a cooking recipe's result.
fn multiplied(r: &Recipe, count: u8, max: &impl Fn(&str) -> u8) -> Vec<(String, u8)> {
    let Some(out) = r.results.first() else {
        return Vec::new();
    };
    let limit = u32::from(max(&out.item).max(1));
    let mut total = u32::from(count) * u32::from(out.count);
    let mut stacks = Vec::new();
    while total > limit {
        stacks.push((out.item.clone(), limit as u8));
        total -= limit;
    }
    stacks.push((out.item.clone(), total as u8));
    stacks
}

/// `EncasedFanRenderer`: the propeller turns five times the fan's speed, at least 80 and at most
/// 1,280 rpm either way.
pub fn propeller_rpm(speed: f32) -> f32 {
    let s = speed * 5.0;
    if s > 0.0 {
        s.clamp(80.0, 1280.0)
    } else if s < 0.0 {
        s.clamp(-1280.0, -80.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(recipes: &[(&str, &str, &str, u8, f32)]) -> Book {
        Book::new(
            recipes
                .iter()
                .map(|&(id, input, out, count, chance)| Recipe {
                    id: id.into(),
                    ingredients: vec![vec![input.into()]],
                    results: vec![crate::processing::Output {
                        item: out.into(),
                        count,
                        chance,
                    }],
                    time: 0,
                })
                .collect(),
        )
    }

    #[test]
    fn types_take_their_recipes() {
        let splashing = book(&[(
            "create:splashing/sand",
            "minecraft:sand",
            "minecraft:clay_ball",
            1,
            0.25,
        )]);
        let haunting = Book::default();
        let smoking = book(&[(
            "minecraft:cooked_beef_from_smoking",
            "minecraft:beef",
            "minecraft:cooked_beef",
            1,
            1.0,
        )]);
        let smelting = book(&[
            (
                "minecraft:glass",
                "minecraft:sand",
                "minecraft:glass",
                1,
                1.0,
            ),
            (
                "minecraft:cooked_beef",
                "minecraft:beef",
                "minecraft:cooked_beef",
                1,
                1.0,
            ),
            (
                "x:iron_manual_only",
                "minecraft:raw_iron",
                "minecraft:iron_ingot",
                1,
                1.0,
            ),
        ]);
        let blasting = book(&[(
            "minecraft:iron_from_blasting",
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            1,
            1.0,
        )]);
        let b = Books {
            splashing: &splashing,
            haunting: &haunting,
            smoking: &smoking,
            smelting: &smelting,
            blasting: &blasting,
        };
        let max = |_: &str| 64;
        let mut rng = Rng::new(7);
        // Sand: smelted to glass, 40 of it in one stack; beef smokes to the same thing: burnt.
        assert_eq!(
            process(Kind::Blasting, "minecraft:sand", 40, b, max, &mut rng),
            Some(vec![("minecraft:glass".into(), 40)])
        );
        assert_eq!(
            process(Kind::Blasting, "minecraft:beef", 1, b, max, &mut rng),
            Some(vec![])
        );
        assert_eq!(
            process(Kind::Smoking, "minecraft:beef", 2, b, max, &mut rng),
            Some(vec![("minecraft:cooked_beef".into(), 2)])
        );
        assert_eq!(
            process(Kind::Smoking, "minecraft:sand", 1, b, max, &mut rng),
            None
        );
        // The first smelting recipe for raw iron is manual only: blasting's is used.
        assert!(can_process(Kind::Blasting, "minecraft:raw_iron", false, b));
        assert!(can_process(Kind::Blasting, "minecraft:dirt", false, b));
        assert!(!can_process(
            Kind::Blasting,
            "minecraft:netherite_ingot",
            true,
            b
        ));
        // A quarter of 64 sand washes to clay, merged in one stack.
        let washed = process(Kind::Splashing, "minecraft:sand", 64, b, max, &mut rng).unwrap();
        assert_eq!(washed.len(), 1);
        assert!((5..30).contains(&washed[0].1), "{washed:?}");
    }

    #[test]
    fn items_in_the_air() {
        // A fan facing east at 0,0,0 reaching 4: the box from x 1 to 5.
        assert_eq!(
            bounds([0, 0, 0], Dir::East, 4.0),
            Some(([1.0, 0.0, 0.0], [5.0, 1.0, 1.0]))
        );
        assert_eq!(
            bounds([0, 0, 0], Dir::West, 2.0),
            Some(([-2.0, 0.0, 0.0], [0.0, 1.0, 1.0]))
        );
        assert_eq!(aligned_distance([3.5, 0.2, 0.5], [0, 0, 0], Dir::East), 2.5);
        // Pushed east: an eighth of the way to speed/512 over distance/max.
        let v = push(Dir::East, 64.0, 4.0, 2.0, [0.0, 0.0, 0.0]);
        assert!((v[0] - 0.25 / 8.0).abs() < 1e-6 && v[1] == 0.0, "{v:?}");
        // An entity's timer: 151 ticks for one stack, then never again by that type.
        let mut p = Progress::default();
        let ticks = (1..)
            .find(|_| advance_entity(&mut p, Kind::Splashing, 1))
            .unwrap();
        assert_eq!(ticks, 151);
        assert!(entity_can(&p, Kind::Splashing, false));
        advance_entity(&mut p, Kind::Splashing, 1);
        assert!(!entity_can(&p, Kind::Splashing, true));
    }

    #[test]
    fn reach_grows_with_speed() {
        assert_eq!(max_distance(0.0001), 3.0 + 0.0001 / 256.0 * 17.0);
        assert_eq!(max_distance(128.0), 11.5);
        assert_eq!(max_distance(-512.0), 20.0);
        assert!(pushing(16.0, Dir::North) != pushing(-16.0, Dir::North));
    }

    #[test]
    fn water_two_blocks_out_washes_from_there_on() {
        // Pushing, reach 6: water at offset 2; segments: before it untyped, from it on washing.
        let c = Current::build(6.0, true, |i| (i == 2).then_some(Kind::Splashing));
        assert_eq!(c.type_at(0.5), None);
        assert_eq!(c.type_at(1.0), None, "the first segment ends at 1");
        assert_eq!(c.type_at(1.01), Some(Kind::Splashing));
        assert_eq!(c.type_at(5.5), Some(Kind::Splashing));
        assert_eq!(c.type_at(7.0), None);
        let water_and_lava = Catalyst {
            water: true,
            lava: true,
            ..Default::default()
        };
        assert_eq!(kind_at(&water_and_lava), Some(Kind::Splashing));
        let unlit = Catalyst {
            smoking: true,
            campfire_lit: Some(false),
            ..Default::default()
        };
        assert_eq!(kind_at(&unlit), None);
        let burner = |heat| Catalyst {
            smoking: true,
            blasting: true,
            heat: Some(heat),
            ..Default::default()
        };
        assert_eq!(kind_at(&burner(Heat::Smouldering)), Some(Kind::Smoking));
        assert_eq!(kind_at(&burner(Heat::Kindled)), Some(Kind::Blasting));
    }

    #[test]
    fn a_block_ends_the_current() {
        assert_eq!(
            flow_limit(10.0, |i| if i == 4 { Way::Full } else { Way::Open }),
            3.0
        );
        assert_eq!(
            flow_limit(10.0, |i| if i == 2 { Way::Depth(0.5) } else { Way::Open }),
            1.0 + 0.5 + 1.0 / 32.0
        );
        assert_eq!(flow_limit(5.0, |_| Way::Open), 5.0);
    }

    #[test]
    fn a_stack_is_done_after_its_time() {
        let mut p = Progress::default();
        assert!(!advance(&mut p, Kind::Splashing, 1, true));
        assert_eq!(p.time, 151);
        let mut ticks = 0;
        while !advance(&mut p, Kind::Splashing, 1, true) {
            ticks += 1;
        }
        assert_eq!(ticks, 151);
        // 17 items take twice as long; an item no recipe takes never finishes.
        let mut q = Progress::default();
        advance(&mut q, Kind::Smoking, 17, true);
        assert_eq!(q.time, 301);
        let mut r = Progress::default();
        advance(&mut r, Kind::Smoking, 1, false);
        assert!((0..1000).all(|_| !advance(&mut r, Kind::Smoking, 1, false)));
        assert_eq!(propeller_rpm(4.0), 80.0);
        assert_eq!(propeller_rpm(-512.0), -1280.0);
    }
}
