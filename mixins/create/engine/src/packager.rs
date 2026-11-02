//! Create's packager (`PackagerBlock`, `PackagerBlockEntity`, `PackagerItemHandler`): a redstone
//! pulse packs the inventory behind it into one package (`package::pack`), which slides out over
//! `CYCLE` ticks and then waits in the packager for a funnel, chute, arm or frogport to take it;
//! a package put in is unpacked into the inventory behind (`package::unpack_fits`) while its
//! tray slides in. Only packagers animating, cooling down or still powered tick. The box `P`
//! is the glue's stack.

/// `PackagerBlockEntity.CYCLE`.
pub const CYCLE: u32 = 20;
/// `activate`: no second package while a button's signal lasts.
pub const BUTTON_COOLDOWN: u32 = 40;
/// SmartBlockEntity's default `lazyTickRate`.
pub const LAZY_RATE: u32 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Packager<P> {
    /// `redstonePowered`: set by a pulse, read back from `powered` at each lazy tick.
    pub powered: bool,
    pub button_cooldown: u32,
    pub animation_ticks: u32,
    /// `animationInward`: unpacking.
    pub inward: bool,
    /// `heldBox` while it is still sliding out; `held` once it is out.
    pub sliding: Option<P>,
    /// `heldBox` at rest: what automation takes (`PackagerItemHandler.extractItem`).
    pub held: Option<P>,
    /// `signBasedAddress` (set by `/create_debug address`, standing in for an adjacent sign).
    pub address: String,
    lazy: u32,
}

impl<P> Default for Packager<P> {
    fn default() -> Self {
        Packager {
            powered: false,
            button_cooldown: 0,
            animation_ticks: 0,
            inward: true,
            sliding: None,
            held: None,
            address: String::new(),
            lazy: LAZY_RATE,
        }
    }
}

/// What a packager's tick asks of the glue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// The lazy tick of a powered packager: read `powered` again (`lazyTick`), and pack if it
    /// still is and nothing holds it back (`attemptToSend`).
    pub recheck: bool,
    /// The animation ended: an unpacked tray is back in; a package is out (now in `held`), and
    /// a frogport above may take it (`wakeTheFrogs`).
    pub done: bool,
}

impl<P> Packager<P> {
    /// Whether it needs ticks.
    pub fn active(&self) -> bool {
        self.animation_ticks > 0 || self.button_cooldown > 0 || self.powered
    }

    /// `attemptToSend(null)`'s guard: nothing held, not animating, not cooling down.
    pub fn can_send(&self) -> bool {
        self.held.is_none()
            && self.sliding.is_none()
            && self.animation_ticks == 0
            && self.button_cooldown == 0
    }

    /// A pulse (`PackagerBlock.neighborChanged` → `activate`): the caller packs if `can_send`,
    /// then calls `after_pulse`.
    pub fn pulse(&mut self) {
        self.powered = true;
    }

    /// The end of `activate`: the button cooldown starts if none runs.
    pub fn after_pulse(&mut self) {
        if self.button_cooldown == 0 {
            self.button_cooldown = BUTTON_COOLDOWN;
        }
    }

    /// A package was made (`attemptToSend`): it slides out.
    pub fn send(&mut self, box_: P) {
        self.sliding = Some(box_);
        self.inward = false;
        self.animation_ticks = CYCLE;
    }

    /// `unwrapBox`'s guard: not while animating; and `PackagerItemHandler.insertItem`'s: nothing
    /// held.
    pub fn can_unwrap(&self) -> bool {
        self.animation_ticks == 0 && self.held.is_none() && self.sliding.is_none()
    }

    /// A package was unpacked: its tray slides in.
    pub fn unwrapped(&mut self) {
        self.inward = true;
        self.animation_ticks = CYCLE;
    }

    /// `extractItem`: the held box, only while not animating.
    pub fn take(&mut self) -> Option<P> {
        if self.animation_ticks != 0 {
            return None;
        }
        self.held.take()
    }

    /// `getAnalogOutputSignal`: 15 while holding or animating.
    pub fn comparator(&self) -> u8 {
        if self.held.is_some() || self.animation_ticks != 0 {
            15
        } else {
            0
        }
    }

    /// One tick (`SmartBlockEntity.tick`: the lazy tick first, then `PackagerBlockEntity.tick`).
    pub fn tick(&mut self) -> Step {
        let mut step = Step::default();
        if self.powered {
            self.lazy = self.lazy.saturating_sub(1);
            if self.lazy == 0 {
                self.lazy = LAZY_RATE;
                step.recheck = true;
            }
        } else {
            self.lazy = LAZY_RATE;
        }
        if self.button_cooldown > 0 {
            self.button_cooldown -= 1;
        }
        if self.animation_ticks == 0 {
            return step;
        }
        self.animation_ticks -= 1;
        if self.animation_ticks == 0 {
            if let Some(b) = self.sliding.take() {
                self.held = Some(b);
            }
            step.done = true;
        }
        step
    }

    /// `getTrayOffset(partialTicks)`: the tray's slide, 0 in to 1 out.
    pub fn tray_offset(&self, partial: f32) -> f32 {
        let ticks = self.animation_ticks as f32;
        let cycle = if self.inward {
            ticks - partial
        } else {
            ticks - 5.0 - partial
        };
        let progress = (cycle / (CYCLE - 5) as f32 * 2.0 - 1.0).clamp(-1.0, 1.0);
        let progress = 1.0 - progress * progress;
        progress * progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pulse_sends_one_package_and_holds_it_after_the_cycle() {
        let mut p = Packager::<&str>::default();
        assert!(!p.active());
        p.pulse();
        assert!(p.can_send());
        p.send("box");
        p.after_pulse();
        assert_eq!(p.button_cooldown, BUTTON_COOLDOWN);
        assert_eq!(p.take(), None, "not while sliding out");
        for t in 1..CYCLE {
            assert!(!p.tick().done, "tick {t}");
        }
        assert!(p.tick().done);
        assert_eq!(p.held, Some("box"));
        assert!(!p.can_send(), "holding a box");
        assert_eq!(p.comparator(), 15);
        assert_eq!(p.take(), Some("box"));
        // Still powered and cooling down: the lazy tick rechecks, but the cooldown holds it back
        // until 40 ticks after the pulse.
        assert!(p.button_cooldown > 0 && !p.can_send());
        let mut rechecks = 0;
        while p.button_cooldown > 0 {
            if p.tick().recheck {
                rechecks += 1;
            }
        }
        assert_eq!(rechecks, 2, "lazy ticks at 30 and 40");
        assert!(p.can_send());
        p.powered = false;
        assert!(!p.active(), "idle once unpowered");
    }

    #[test]
    fn unwrapping_slides_the_tray_in_and_blocks_meanwhile() {
        let mut p = Packager::<&str>::default();
        assert!(p.can_unwrap());
        p.unwrapped();
        assert!(!p.can_unwrap());
        assert!(!p.can_send());
        for _ in 0..CYCLE - 1 {
            p.tick();
        }
        assert!(p.tick().done);
        assert_eq!(p.held, None);
        assert!(p.can_unwrap());
        // The tray: out in the middle of the cycle, in at its ends.
        p.unwrapped();
        assert_eq!(p.tray_offset(0.0), 0.0);
        p.animation_ticks = 10;
        assert!(p.tray_offset(0.0) > 0.75, "{}", p.tray_offset(0.0));
    }
}
