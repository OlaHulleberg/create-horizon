//! Create's latches (`ToggleLatchBlock`, `PoweredLatchBlock`) on vanilla's `DiodeBlock`: a
//! toggle latch flips its output on each rising edge at its back; a powered latch is set by its
//! back and reset by its sides. Both act one tick after an input changes (a scheduled tick of
//! delay 1) and give 15 from their front while on.

use crate::kinetics::Dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Toggle,
    Powered,
}

/// A latch's state: `facing` (toward its input), `powered` (its back), `powering` (its output),
/// and a powered latch's `powered_side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Latch {
    pub facing: Dir,
    pub powered: bool,
    pub powering: bool,
    pub powered_side: bool,
}

/// `DiodeBlock.checkTickOnNeighbor` (and the powered latch's): whether its inputs differ from
/// what it last took, so a tick must come.
pub fn needs_tick(kind: Kind, l: &Latch, back: bool, side: bool) -> bool {
    match kind {
        Kind::Toggle => l.powered != back,
        Kind::Powered => l.powered != back || l.powered_side != side,
    }
}

/// The scheduled tick: the new state, and whether another tick follows (`DiodeBlock.tick`
/// switches on for one tick even when the input is already gone, then off).
pub fn tick(kind: Kind, l: &Latch, back: bool, side: bool) -> (Latch, bool) {
    let mut n = *l;
    match kind {
        Kind::Toggle => {
            let mut again = false;
            if l.powered && !back {
                n.powered = false;
            } else if !l.powered {
                n.powered = true;
                again = !back;
            }
            // ToggleLatchBlock.tick: a rising edge flips the output.
            if n.powered && !l.powered {
                n.powering = !n.powering;
            }
            (n, again)
        }
        Kind::Powered => {
            if l.powered != back {
                n.powered = back;
                if back {
                    n.powering = true;
                } else if l.powered_side {
                    n.powering = false;
                }
            }
            if l.powered_side != side {
                n.powered_side = side;
                if side {
                    n.powering = false;
                } else if n.powered {
                    n.powering = true;
                }
            }
            (n, false)
        }
    }
}

/// A right click (`activated`): the output flips; a powered latch only while its back and sides
/// agree.
pub fn activated(kind: Kind, l: &Latch) -> Option<Latch> {
    if kind == Kind::Powered && l.powered != l.powered_side {
        return None;
    }
    Some(Latch {
        powering: !l.powering,
        ..*l
    })
}

/// `getSignal` / `getDirectSignal`: 15 toward the side it faces while on.
pub fn signal(l: &Latch, direction: Dir) -> i32 {
    if l.facing == direction && l.powering {
        15
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latch() -> Latch {
        Latch {
            facing: Dir::North,
            powered: false,
            powering: false,
            powered_side: false,
        }
    }

    #[test]
    fn a_toggle_latch_flips_on_each_rising_edge() {
        let mut l = latch();
        assert!(needs_tick(Kind::Toggle, &l, true, false));
        (l, _) = tick(Kind::Toggle, &l, true, false);
        assert!(l.powered && l.powering);
        assert_eq!(signal(&l, Dir::North), 15);
        assert_eq!(signal(&l, Dir::South), 0);
        // Held: nothing; released: off at the back only, the output stays.
        assert!(!needs_tick(Kind::Toggle, &l, true, false));
        (l, _) = tick(Kind::Toggle, &l, false, false);
        assert!(!l.powered && l.powering);
        // The next press turns it off.
        (l, _) = tick(Kind::Toggle, &l, true, false);
        assert!(!l.powering);
    }

    #[test]
    fn a_pulse_shorter_than_its_tick_still_toggles() {
        // The input was gone again by the tick: on for a tick, a second tick switches it off.
        let (l, again) = tick(Kind::Toggle, &latch(), false, false);
        assert!(l.powered && l.powering && again);
        let (l, again) = tick(Kind::Toggle, &l, false, false);
        assert!(!l.powered && l.powering && !again);
    }

    #[test]
    fn a_powered_latch_sets_from_its_back_and_resets_from_its_sides() {
        let mut l = latch();
        (l, _) = tick(Kind::Powered, &l, true, false);
        assert!(l.powering);
        (l, _) = tick(Kind::Powered, &l, false, false);
        assert!(l.powering, "it holds");
        (l, _) = tick(Kind::Powered, &l, false, true);
        assert!(!l.powering, "reset");
        assert!(
            activated(Kind::Powered, &l).is_none(),
            "back and sides disagree"
        );
        (l, _) = tick(Kind::Powered, &l, false, false);
        assert_eq!(activated(Kind::Powered, &l).map(|x| x.powering), Some(true));
    }
}
