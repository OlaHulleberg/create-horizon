//! Create's rose quartz lamp (`RoseQuartzLampBlock`): a lamp that gains a signal lights itself
//! and turns off every lamp joined to it, so one lamp of a cluster is lit at a time. A lit lamp
//! (`powering`) gives 15 to anything but another lamp.

use crate::kinetics::Dir;

/// A lamp's state: `powered` (a signal into it), `powering` (lit) and `activate` (to be lit at
/// its next tick).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lamp {
    pub powered: bool,
    pub powering: bool,
    pub activate: bool,
}

/// What `neighborChanged` does with the lamp's signal now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// The signal is gone: `powered` off (`UPDATE_CLIENTS`), nothing else.
    Unpower,
    /// A new signal: every lamp of its cluster off with an activation tick (`powering` false),
    /// this one `powered`, `powering` and `activate` with its neighbours updated and a tick.
    Activate,
}

/// `RoseQuartzLampBlock.neighborChanged` with `signal` (`hasNeighborSignal`).
pub fn neighbor_changed(l: &Lamp, signal: bool) -> Change {
    if l.powered == signal {
        Change::None
    } else if l.powered {
        Change::Unpower
    } else {
        Change::Activate
    }
}

/// `RoseQuartzLampBlock.tick`: the new state when it changes (`activate` cleared, `powering`
/// taken from it); its neighbours are updated either way.
pub fn tick(l: &Lamp) -> Option<Lamp> {
    (l.powering || l.activate).then_some(Lamp {
        activate: false,
        powering: l.activate,
        ..*l
    })
}

/// `forEachInCluster`: the lamps joined to `start` face to face (`is_lamp`), breadth first in
/// `Iterate.directions` order, none further than 16 blocks (Manhattan) from it; `start` itself
/// is not among them.
pub fn cluster(start: [i32; 3], mut is_lamp: impl FnMut([i32; 3]) -> bool) -> Vec<[i32; 3]> {
    let mut frontier = std::collections::VecDeque::from([start]);
    let mut visited = std::collections::HashSet::from([start]);
    let mut out = Vec::new();
    while let Some(pos) = frontier.pop_front() {
        for d in Dir::ALL {
            let o = d.offset();
            let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let dist: i32 = (0..3).map(|i| (at[i] - start[i]).abs()).sum();
            if dist > 16 || !visited.insert(at) || !is_lamp(at) {
                continue;
            }
            out.push(at);
            frontier.push_back(at);
        }
    }
    out
}

/// `getSignal` toward a reader that is not a lamp (another lamp reads 0) or a comparator: 15
/// while lit. A lamp gives no direct signal.
pub fn signal(l: &Lamp, reader_is_lamp: bool, direct: bool) -> i32 {
    if direct || reader_is_lamp || !l.powering {
        0
    } else {
        15
    }
}

/// `getDistanceToPowered` for a comparator reading the lamp: walking from it away from the
/// comparator, 15 at the lamp itself, one less each lamp further, to the first lit one; 0 when
/// the column of lamps ends first. `lit(i)` is the `i`th block's `powering` (None: not a lamp).
pub fn distance_to_powered(mut lit: impl FnMut(i32) -> Option<bool>) -> i32 {
    for (i, power) in (1..=15).rev().enumerate() {
        match lit(i as i32) {
            None => return 0,
            Some(true) => return power,
            Some(false) => {}
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_signal_lights_the_lamp_at_its_next_tick() {
        let l = Lamp::default();
        assert_eq!(neighbor_changed(&l, false), Change::None);
        assert_eq!(neighbor_changed(&l, true), Change::Activate);
        // Activate sets all three; its tick keeps it lit and clears `activate`.
        let on = Lamp {
            powered: true,
            powering: true,
            activate: true,
        };
        let ticked = tick(&on).unwrap();
        assert!(ticked.powering && !ticked.activate);
        assert_eq!(signal(&ticked, false, false), 15);
        assert_eq!(signal(&ticked, true, false), 0, "another lamp reads 0");
        assert_eq!(signal(&ticked, false, true), 0, "no direct signal");
        // The signal gone: only `powered` goes; the lamp stays lit.
        assert_eq!(neighbor_changed(&ticked, false), Change::Unpower);
    }

    #[test]
    fn the_rest_of_the_cluster_goes_dark() {
        // A lamp switched off by its cluster: `powering` false and a tick that leaves it so.
        let dark = Lamp {
            powered: false,
            powering: false,
            activate: false,
        };
        assert_eq!(tick(&dark), None);
        let was_lit = Lamp {
            powering: true,
            ..dark
        };
        assert_eq!(tick(&was_lit).map(|l| l.powering), Some(false));
    }

    #[test]
    fn a_cluster_reaches_16_blocks_and_skips_its_start() {
        // A line of lamps along x from -30 to 30.
        let line = |p: [i32; 3]| p[1] == 0 && p[2] == 0 && p[0].abs() <= 30;
        let c = cluster([0, 0, 0], line);
        assert_eq!(c.len(), 32);
        assert!(!c.contains(&[0, 0, 0]));
        assert!(c.contains(&[16, 0, 0]) && c.contains(&[-16, 0, 0]));
        assert!(!c.contains(&[17, 0, 0]));
        // Iterate.directions: west before east.
        assert_eq!(c[..2], [[-1, 0, 0], [1, 0, 0]]);
        // A lamp alone has no cluster.
        assert!(cluster([0, 0, 0], |_| false).is_empty());
    }

    #[test]
    fn a_comparator_reads_how_far_the_lit_lamp_is() {
        let column = [Some(false), Some(false), Some(true)];
        assert_eq!(
            distance_to_powered(|i| column.get(i as usize).copied().flatten()),
            13
        );
        assert_eq!(distance_to_powered(|_| Some(true)), 15);
        assert_eq!(
            distance_to_powered(|i| (i < 2).then_some(false)),
            0,
            "no lit lamp"
        );
        assert_eq!(distance_to_powered(|_| Some(false)), 0);
    }
}
