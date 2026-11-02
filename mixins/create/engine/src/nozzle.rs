//! Create's nozzle (`NozzleBlock`, `NozzleBlockEntity`): on an encased fan's front, it spreads
//! the fan's air in a sphere instead of a line, pushing (or pulling) the entities it sees. It
//! does no processing: the fan's own current stops at the nozzle's shape (`getFlowLimit`, a
//! depth of 0 into its base), so nothing in front of the fan is washed or smoked through it.

use crate::fan;
use crate::kinetics::Dir;

/// `setLazyTickRate(5)`: the entity search runs when the counter passes 0, every 6th tick.
pub const LAZY_RATE: i32 = 5;
/// `lazyTick`: more pulled entities than this blow up (`level.explode`, power 2) and vanish.
pub const MAX_PULLED: usize = 256;

/// An entity the nozzle acts on (`pushingEntities`): item entities and mobs, by entity id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Item(u32),
    Mob(u32),
}

impl Target {
    pub fn is_item(self) -> bool {
        matches!(self, Target::Item(_))
    }
}

/// A nozzle's block entity: its facing, the range and way of its fan's air, the lazy tick's
/// counter and the entities it caught.
#[derive(Clone, Debug, PartialEq)]
pub struct Nozzle {
    pub facing: Dir,
    pub range: f32,
    pub pushing: bool,
    pub lazy: i32,
    pub entities: Vec<Target>,
}

impl Nozzle {
    pub fn new(facing: Dir) -> Nozzle {
        Nozzle {
            facing,
            range: 0.0,
            pushing: false,
            lazy: LAZY_RATE,
            entities: Vec::new(),
        }
    }

    /// `SmartBlockEntity.tick`'s countdown: whether the lazy tick runs this tick.
    pub fn lazy_due(&mut self) -> bool {
        let due = self.lazy <= 0;
        self.lazy -= 1;
        if due {
            self.lazy = LAZY_RATE;
        }
        due
    }

    /// `lazyTick` for one entity in its search box at `at` (sneaking ones are skipped): out of
    /// range, nothing; caught when the nozzle sees it (`canSee`, asked only then), let go when
    /// it doesn't.
    pub fn look_at(&mut self, pos: [i32; 3], t: Target, at: [f64; 3], sees: impl FnOnce() -> bool) {
        let c = center(pos);
        let d = ((at[0] - c[0]).powi(2) + (at[1] - c[1]).powi(2) + (at[2] - c[2]).powi(2)).sqrt();
        if d > f64::from(self.range) {
            return;
        }
        if !sees() {
            self.entities.retain(|e| *e != t);
        } else if !self.entities.contains(&t) {
            self.entities.push(t);
        }
    }

    /// `tick`'s `calcRange` and `setRange`: the fan behind it at `fan_speed`, facing
    /// `fan_facing` (None when there is no fan). A range of 0 lets go of every entity.
    pub fn update_range(&mut self, fan: Option<(f32, Dir)>) {
        let (range, pushing) = range(fan);
        if let Some(p) = pushing {
            self.pushing = p;
        }
        if self.range != range {
            self.range = range;
            if range == 0.0 {
                self.entities.clear();
            }
        }
    }
}

/// `calcRange`: the fan's `getMaxDistance` while it turns, else 0; and whether it pushes
/// (`getAirFlowDirection() == getAirflowOriginSide()`), left as it was (None) when it doesn't
/// turn.
pub fn range(fan: Option<(f32, Dir)>) -> (f32, Option<bool>) {
    match fan {
        Some((speed, facing)) if speed != 0.0 => {
            (fan::max_distance(speed), Some(fan::pushing(speed, facing)))
        }
        _ => (0.0, None),
    }
}

/// `NozzleBlock.canSurvive`: the block behind it is an air current source whose origin side
/// (the encased fan's facing) is the nozzle's facing. `behind` is that fan's facing, if it is
/// one.
pub fn survives(facing: Dir, behind: Option<Dir>) -> bool {
    behind == Some(facing)
}

/// `VecHelper.getCenterOf`.
pub fn center(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

/// `lazyTick`'s search box: the centre inflated by half the range.
pub fn search_box(pos: [i32; 3], range: f32) -> ([f64; 3], [f64; 3]) {
    let c = center(pos);
    let r = f64::from(range / 2.0);
    (c.map(|v| v - r), c.map(|v| v + r))
}

/// What `tick` does to one caught entity at `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Push {
    /// Out of range (or sneaking, or a creative player flying): let go of it.
    Release,
    /// Pulled in and closer than 1.5 blocks: left alone this tick.
    Hold,
    /// Its motion plus this (`setDeltaMovement`), its fall forgotten.
    Add([f64; 3]),
}

/// `tick`'s push on one entity: `item` for an item entity (1/128 of the push, others 1/32).
pub fn push(
    pos: [i32; 3],
    range: f32,
    pushing: bool,
    at: [f64; 3],
    item: bool,
    sneaking: bool,
) -> Push {
    let c = center(pos);
    let diff = [at[0] - c[0], at[1] - c[1], at[2] - c[2]];
    let distance = (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt();
    if distance > f64::from(range) || sneaking {
        return Push::Release;
    }
    if !pushing && distance < 1.5 {
        return Push::Hold;
    }
    let factor = if item { 1.0 / 128.0 } else { 1.0 / 32.0 };
    // `Vec3.normalize`: zero under 1.0E-5F.
    let n = if distance < f64::from(1.0e-5f32) {
        [0.0; 3]
    } else {
        diff.map(|v| v / distance)
    };
    let scale = (f64::from(range) - distance) * if pushing { 1.0 } else { -1.0 };
    Push::Add(n.map(|v| v * scale * factor))
}

/// `canSee`: `Level.clip` from the entity to the nozzle's centre (colliders, no fluids) ends at
/// the nozzle. `solid` says whether a block stops the ray (the nozzle's own block included);
/// a ray that hits nothing ends at its target, the nozzle; one that starts inside another
/// solid block is stopped there.
pub fn sees(pos: [i32; 3], from: [f64; 3], mut solid: impl FnMut([i32; 3]) -> bool) -> bool {
    let start = from.map(|v| v.floor() as i32);
    if start == pos {
        return true;
    }
    if solid(start) {
        return false;
    }
    match crate::ejector::clip_blocks(from, center(pos), |p| p == pos || solid(p)) {
        Some((_, hit, _)) => hit == pos,
        None => true,
    }
}

/// `lazyTick`'s end: pulling with more than 256 caught entities blows up.
pub fn overloaded(pushing: bool, caught: usize) -> bool {
    !pushing && caught > MAX_PULLED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_follows_the_fan_speed_as_create_computes_it() {
        // `getMaxDistance`: lerp(min(|speed| / 256, 1), 3, 20).
        assert_eq!(range(None), (0.0, None));
        assert_eq!(range(Some((0.0, Dir::Up))), (0.0, None));
        let (r, push) = range(Some((256.0, Dir::Up)));
        assert_eq!(r, 20.0);
        assert_eq!(push, Some(fan::pushing(256.0, Dir::Up)));
        let (r, _) = range(Some((128.0, Dir::Up)));
        assert_eq!(r, 11.5);
        let (r, _) = range(Some((-32.0, Dir::North)));
        assert_eq!(r, 3.0 + 32.0 / 256.0 * 17.0);
        assert_eq!(range(Some((1000.0, Dir::East))).0, 20.0);
        // The way it blows is the fan's: reversing the speed reverses it.
        assert_ne!(
            range(Some((64.0, Dir::East))).1,
            range(Some((-64.0, Dir::East))).1
        );
    }

    #[test]
    fn range_zero_lets_go_of_everything() {
        let mut n = Nozzle::new(Dir::Up);
        n.update_range(Some((256.0, Dir::Up)));
        n.entities.push(Target::Item(1));
        n.update_range(Some((256.0, Dir::Up)));
        assert_eq!(n.entities.len(), 1, "same range: kept");
        n.update_range(Some((0.0, Dir::Up)));
        assert_eq!(n.range, 0.0);
        assert!(n.entities.is_empty());
    }

    #[test]
    fn it_survives_only_on_a_fan_s_front() {
        assert!(survives(Dir::North, Some(Dir::North)));
        assert!(!survives(Dir::North, Some(Dir::South)));
        assert!(!survives(Dir::Up, None));
    }

    #[test]
    fn the_push_grows_toward_the_centre_and_pulls_hold_close() {
        let pos = [0, 0, 0];
        // Pushing, an item 4 blocks above the centre with a range of 20: (20 - 4) / 128 up.
        match push(pos, 20.0, true, [0.5, 4.5, 0.5], true, false) {
            Push::Add(v) => {
                assert!(v[0].abs() < 1e-12 && v[2].abs() < 1e-12);
                assert!((v[1] - 16.0 / 128.0).abs() < 1e-12);
            }
            p => panic!("{p:?}"),
        }
        // A mob: 1/32.
        match push(pos, 20.0, true, [4.5, 0.5, 0.5], false, false) {
            Push::Add(v) => assert!((v[0] - 16.0 / 32.0).abs() < 1e-12),
            p => panic!("{p:?}"),
        }
        // Pulling: toward the centre, but not within 1.5 blocks.
        match push(pos, 20.0, false, [0.5, 0.5, 6.5], true, false) {
            Push::Add(v) => assert!((v[2] + 14.0 / 128.0).abs() < 1e-12),
            p => panic!("{p:?}"),
        }
        assert_eq!(
            push(pos, 20.0, false, [0.5, 1.5, 0.5], true, false),
            Push::Hold
        );
        assert_eq!(
            push(pos, 20.0, true, [0.5, 1.5, 0.5], true, true),
            Push::Release
        );
        assert_eq!(
            push(pos, 3.0, true, [0.5, 4.0, 0.5], true, false),
            Push::Release
        );
    }

    #[test]
    fn the_search_box_is_half_the_range_and_the_lazy_tick_runs_every_sixth_tick() {
        assert_eq!(search_box([0, 0, 0], 20.0), ([-9.5; 3], [10.5; 3]));
        let mut n = Nozzle::new(Dir::Up);
        let runs: Vec<bool> = (0..12).map(|_| n.lazy_due()).collect();
        assert_eq!(runs.iter().filter(|r| **r).count(), 2);
        assert!(runs[5] && runs[11]);
    }

    #[test]
    fn it_sees_what_no_block_hides() {
        let pos = [0, 0, 0];
        assert!(sees(pos, [0.5, 5.5, 0.5], |_| false));
        assert!(!sees(pos, [0.5, 5.5, 0.5], |p| p == [0, 3, 0]));
        // Inside another block: stopped there.
        assert!(!sees(pos, [0.5, 5.5, 0.5], |p| p == [0, 5, 0]));
        let mut n = Nozzle::new(Dir::Up);
        n.range = 10.0;
        n.look_at(pos, Target::Item(1), [0.5, 5.5, 0.5], || true);
        n.look_at(pos, Target::Mob(2), [0.5, 5.5, 0.5], || true);
        n.look_at(pos, Target::Mob(2), [0.5, 5.5, 0.5], || true);
        assert_eq!(n.entities, vec![Target::Item(1), Target::Mob(2)]);
        // Hidden now: let go; out of range: kept as it is (`tick` lets go of it).
        n.look_at(pos, Target::Item(1), [0.5, 5.5, 0.5], || false);
        n.look_at(pos, Target::Mob(2), [0.5, 15.5, 0.5], || false);
        assert_eq!(n.entities, vec![Target::Mob(2)]);
        assert!(overloaded(false, 257) && !overloaded(false, 256) && !overloaded(true, 300));
    }
}
