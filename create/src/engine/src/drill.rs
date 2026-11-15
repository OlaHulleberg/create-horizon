//! Create's mechanical drill (`DrillBlockEntity`, `BlockBreakingKineticBlockEntity`): turning, it
//! breaks the block in front, in steps that come faster at speed and slower in hard blocks.

/// The breaker's state (`ticksUntilNextProgress`, `destroyProgress`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breaker {
    pub ticks_until: i32,
    pub progress: i32,
}

impl Default for Breaker {
    fn default() -> Breaker {
        Breaker {
            ticks_until: -1,
            progress: 0,
        }
    }
}

/// What a tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// The cracks reached this stage (0..10).
    Progress(i32),
    /// The block breaks now.
    Broken,
    /// Nothing breakable there: the cracks go.
    Reset,
}

impl Breaker {
    /// `lazyTick` (every 10 ticks): a breaker that finished looks again.
    pub fn lazy(&mut self) {
        if self.ticks_until == -1 {
            self.ticks_until = 1;
        }
    }

    /// `tick` at `speed` rpm against a block of `hardness` (None: not breakable, `isBreakable`).
    pub fn tick(&mut self, speed: f32, hardness: Option<f32>) -> Step {
        if speed == 0.0 || self.ticks_until < 0 {
            return Step::Wait;
        }
        if self.ticks_until > 0 {
            self.ticks_until -= 1;
            return Step::Wait;
        }
        self.ticks_until -= 1;
        let Some(hardness) = hardness else {
            let had = self.progress != 0;
            self.progress = 0;
            return if had { Step::Reset } else { Step::Wait };
        };
        let break_speed = (speed / 100.0).abs();
        self.progress += ((break_speed / hardness) as i32).clamp(1, 10 - self.progress);
        if self.progress >= 10 {
            self.progress = 0;
            self.ticks_until = -1;
            return Step::Broken;
        }
        self.ticks_until = (hardness / break_speed) as i32;
        Step::Progress(self.progress)
    }
}

/// `isBreakable`'s hardness test: unbreakable (-1) blocks stay.
pub fn breakable(hardness: f32) -> Option<f32> {
    (hardness >= 0.0).then_some(hardness)
}

/// `DrillBlock.getDamage`: what a turning drill or saw deals an entity inside it, 1 to 10 by
/// speed.
pub fn damage(speed: f32) -> f32 {
    let s = speed.abs();
    let sum = (s / 16.0).min(2.0) + (s / 32.0).min(4.0) + (s / 64.0).min(4.0);
    sum.clamp(1.0, 10.0)
}

/// `entityInside`'s test: the entity's box `[min, max]` meets the block at `pos` shrunk by 0.1
/// on every side (`new AABB(pos).deflate(.1f)`).
pub fn inside(pos: [i32; 3], min: [f64; 3], max: [f64; 3]) -> bool {
    (0..3).all(|a| {
        let lo = f64::from(pos[a]) + 0.1;
        let hi = f64::from(pos[a]) + 0.9;
        min[a] < hi && max[a] > lo
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blades_hurt_by_speed_within_their_block() {
        assert_eq!(damage(0.0), 1.0);
        assert_eq!(damage(16.0), 1.75);
        assert_eq!(damage(-64.0), 2.0 + 2.0 + 1.0);
        assert_eq!(damage(256.0), 10.0);
        // A mob standing on an upright saw (12 px tall) is inside; one on a full block above is not.
        assert!(inside([0, 0, 0], [0.2, 0.75, 0.2], [0.8, 2.5, 0.8]));
        assert!(!inside([0, 0, 0], [0.2, 0.95, 0.2], [0.8, 2.5, 0.8]));
    }

    #[test]
    fn stone_breaks_in_steps_at_speed() {
        // 64 rpm: break speed 0.64; stone (hardness 1.5): one step every 2 ticks, 10 steps.
        let mut b = Breaker::default();
        assert_eq!(
            b.tick(64.0, Some(1.5)),
            Step::Wait,
            "waits for its lazy tick"
        );
        b.lazy();
        let mut ticks = 0;
        loop {
            ticks += 1;
            if b.tick(64.0, Some(1.5)) == Step::Broken {
                break;
            }
        }
        assert_eq!(
            ticks,
            2 + 9 * 3,
            "first step on the second tick, then every third"
        );
        // Dirt (0.5) at 256 rpm: 2.56 / 0.5 = 5 steps at once.
        let mut d = Breaker::default();
        d.lazy();
        d.tick(256.0, Some(0.5));
        assert_eq!(d.tick(256.0, Some(0.5)), Step::Progress(5));
        assert_eq!(breakable(-1.0), None);
    }
}
