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

#[cfg(test)]
mod tests {
    use super::*;

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
