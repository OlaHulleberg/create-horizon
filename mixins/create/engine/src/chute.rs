//! Create's chute (`ChuteBlockEntity`), without fans: one stack falls through each chute at
//! gravity's 4 chutes a second, into the inventory below, the chute below, or out of the bottom.

/// `getItemMotion` with no fan: `-gravity / 20` chutes a tick.
pub const MOTION: f32 = -4.0 / 20.0;

/// What a tick of a falling stack does at `position` (1 at the top, 0 at the bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fall {
    /// Still falling, now here.
    At(f32),
    /// At the bottom: hand it on now (`handleDownwardOutput`).
    Out,
}

/// The downward part of `tick`: past the middle it only goes on when `can_output` (the place
/// below takes it); past the bottom it leaves.
pub fn fall(position: f32, can_output: bool) -> Fall {
    let next = position + MOTION;
    if next < 0.5 {
        if !can_output {
            return Fall::At(0.5);
        }
        if next < 0.0 {
            return Fall::Out;
        }
    }
    Fall::At(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_falls_through_in_five_ticks_or_waits_in_the_middle() {
        let mut p = 1.0;
        let mut ticks = 0;
        while let Fall::At(next) = fall(p, true) {
            p = next;
            ticks += 1;
        }
        assert_eq!(ticks, 5);
        assert_eq!(fall(0.6, false), Fall::At(0.5));
        assert_eq!(fall(0.5, false), Fall::At(0.5));
    }
}
