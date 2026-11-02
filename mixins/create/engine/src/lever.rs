//! Create's analog lever (`AnalogLeverBlock`, `AnalogLeverBlockEntity`, `AnalogLeverRenderer`):
//! a lever whose block entity holds a signal of 0 to 15, raised by a click and lowered by a
//! sneaking one. Its signal is read at once; its neighbours are told 15 ticks after the last
//! change (`lastChange`). Only levers with a change pending tick.

use crate::kinetics::Dir;
use crate::steam::Face;
use std::collections::{BTreeSet, HashMap};

/// `changeState`: `lastChange = 15`, the ticks before the neighbours are updated.
pub const CHANGE_DELAY: i64 = 15;

/// The mod message channel for levers' states, server to client.
pub const CHANNEL: &str = "create:lever";

/// One analog lever's block entity: its `state`, and the game time its neighbours are told
/// (`lastChange` counted down), if a change is pending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lever {
    pub state: u8,
    pub due: Option<i64>,
}

/// `AnalogLeverBlockEntity.changeState`: one up, or down when sneaking (`back`), within 0 to 15;
/// whether it changed.
pub fn change(state: u8, back: bool) -> u8 {
    if back {
        state.saturating_sub(1)
    } else {
        (state + 1).min(15)
    }
}

/// `AnalogLeverBlock.useWithoutItem`'s click pitch for the state after the click:
/// `.25 + ((state + 5) / 15) * .5`.
pub fn click_pitch(state: u8) -> f32 {
    0.25 + ((f32::from(state) + 5.0) / 15.0) * 0.5
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`: up from the floor, down from
/// the ceiling, else its facing.
pub fn connected(face: Face, facing: Dir) -> Dir {
    match face {
        Face::Floor => Dir::Up,
        Face::Ceiling => Dir::Down,
        Face::Wall => facing,
    }
}

/// `getSignal` (weak, every side) and `getDirectSignal` (only toward `connected`): its state.
/// `side` is vanilla's direction, from the reader toward the lever.
pub fn signal(state: u8, connected: Dir, side: Dir, direct: bool) -> i32 {
    if direct && connected != side {
        return 0;
    }
    i32::from(state)
}

/// `AnalogLeverRenderer.renderSafe`'s handle: turned to the lever's face and facing
/// (`transform`: `rotateCentered` by `horizontalAngle(facing)` about y, then by 0, 90 or 180
/// about x), then about x through (8, 1, 8) px by `state / 15 * 90` degrees.
pub fn handle_pose(face: Face, facing: Dir, state: u8) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    let rx = match face {
        Face::Floor => 0.0f32,
        Face::Wall => 90.0,
        Face::Ceiling => 180.0,
    };
    let ry = crate::ejector::horizontal_angle(facing);
    let centre = Vec3::splat(0.5);
    let centred = |m: Mat4| Mat4::from_translation(centre) * m * Mat4::from_translation(-centre);
    let pivot = Vec3::new(0.5, 1.0 / 16.0, 0.5);
    let angle = f32::from(state) / 15.0 * 90.0;
    centred(Mat4::from_rotation_y(ry.to_radians()))
        * centred(Mat4::from_rotation_x(rx.to_radians()))
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(angle.to_radians())
        * Mat4::from_translation(-pivot)
}

/// The analog levers of one dimension: their block entities, the ones whose neighbours are due
/// to be told (by game time), those clients must see again, and those removed with a signal.
#[derive(Default)]
pub struct Levers {
    pub levers: HashMap<[i32; 3], Lever>,
    saved: HashMap<[i32; 3], Lever>,
    pub due: BTreeSet<(i64, [i32; 3])>,
    pub shown: Vec<[i32; 3]>,
    /// Levers broken while giving a signal (`onRemove`: `updateNeighbors`).
    pub removed: Vec<[i32; 3]>,
}

impl Levers {
    /// A lever placed or loaded (or its state changed): its saved block entity, else a new one.
    pub fn placed(&mut self, pos: [i32; 3]) {
        if !self.levers.contains_key(&pos) {
            let l = self.saved.remove(&pos).unwrap_or_default();
            if let Some(t) = l.due {
                self.due.insert((t, pos));
            }
            self.levers.insert(pos, l);
        }
        self.shown.push(pos);
    }

    /// A lever gone: true when it had state to save no more.
    pub fn removed(&mut self, pos: [i32; 3]) -> bool {
        let Some(l) = self.levers.remove(&pos) else {
            return false;
        };
        if let Some(t) = l.due {
            self.due.remove(&(t, pos));
        }
        if l.state != 0 {
            self.removed.push(pos);
        }
        self.shown.push(pos);
        true
    }

    /// `changeState` at game time `now`: the new state, None for no lever there. A change
    /// restarts the 15 ticks before the neighbours are told.
    pub fn click(&mut self, pos: [i32; 3], back: bool, now: i64) -> Option<u8> {
        let l = self.levers.get_mut(&pos)?;
        let state = change(l.state, back);
        if state != l.state {
            l.state = state;
            if let Some(t) = l.due.take() {
                self.due.remove(&(t, pos));
            }
            let t = now + CHANGE_DELAY;
            l.due = Some(t);
            self.due.insert((t, pos));
        }
        self.shown.push(pos);
        Some(state)
    }

    /// The levers whose neighbours are told now (`tick`: `lastChange` reached 0).
    pub fn take_due(&mut self, now: i64) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        while let Some(&(t, pos)) = self.due.first() {
            if t > now {
                break;
            }
            self.due.pop_first();
            if let Some(l) = self.levers.get_mut(&pos) {
                l.due = None;
            }
            out.push(pos);
        }
        out
    }

    /// A lever's state for the signal hook: 0 for none.
    pub fn state(&self, pos: [i32; 3]) -> u8 {
        self.levers.get(&pos).map_or(0, |l| l.state)
    }

    /// Saved block entities (`State`, `ChangeTimer` as a game time) of levers not found yet.
    pub fn load(&mut self, saved: impl Iterator<Item = ([i32; 3], Lever)>) {
        self.saved.extend(saved);
    }

    /// A column's levers that are not at rest at 0, to save (`write`).
    pub fn column(&self, cx: i32, cz: i32) -> Vec<([i32; 3], Lever)> {
        let mut v: Vec<([i32; 3], Lever)> = self
            .levers
            .iter()
            .filter(|(p, l)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && **l != Lever::default())
            .map(|(p, l)| (*p, *l))
            .collect();
        v.sort_by_key(|e| e.0);
        v
    }

    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        self.levers.retain(|p, _| !inside(p));
        self.saved.retain(|p, _| !inside(p));
        self.due.retain(|(_, p)| !inside(p));
        self.shown.retain(|p| !inside(p));
        self.removed.retain(|p| !inside(p));
    }
}

/// What clients are told of a lever: its state (None: gone), face and facing.
pub type Shown = ([i32; 3], Option<u8>, Face, Dir);

/// `[x, y, z]` then the state (255: gone), face and facing, per lever.
pub fn encode(levers: &[Shown]) -> Vec<u8> {
    let mut out = Vec::with_capacity(levers.len() * 15);
    for (pos, state, face, facing) in levers {
        for v in pos {
            out.extend(v.to_le_bytes());
        }
        out.push(state.unwrap_or(255));
        out.push(*face as u8);
        out.push(*facing as u8);
    }
    out
}

pub fn decode(data: &[u8]) -> Vec<Shown> {
    data.chunks_exact(15)
        .filter_map(|c| {
            let at = |i: usize| i32::from_le_bytes(c[i..i + 4].try_into().unwrap());
            let face = [Face::Floor, Face::Wall, Face::Ceiling]
                .get(usize::from(c[13]))
                .copied()?;
            let facing = Dir::ALL.get(usize::from(c[14])).copied()?;
            let state = (c[12] <= 15).then_some(c[12]);
            Some(([at(0), at(4), at(8)], state, face, facing))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_count_up_and_sneaking_down_within_0_to_15() {
        let mut l = Levers::default();
        let p = [1, 64, 2];
        l.placed(p);
        assert_eq!(l.click(p, true, 0), Some(0));
        // Unchanged: nothing to tell the neighbours.
        assert!(l.due.is_empty());
        for i in 1..=15 {
            assert_eq!(l.click(p, false, i), Some(i as u8));
        }
        assert_eq!(l.click(p, false, 20), Some(15));
        // The last change was at 15: the neighbours are told 15 ticks later, once.
        assert!(l.take_due(29).is_empty());
        assert_eq!(l.take_due(30), [p]);
        assert!(l.take_due(31).is_empty());
        assert_eq!(l.click(p, true, 40), Some(14));
        assert_eq!(l.state(p), 14);
        // Pitch of `useWithoutItem`: .25 + (state + 5) / 15 * .5.
        assert!((click_pitch(0) - (0.25 + 5.0 / 15.0 * 0.5)).abs() < 1e-6);
        assert!((click_pitch(15) - (0.25 + 20.0 / 15.0 * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn its_signal_is_weak_everywhere_and_direct_into_its_support() {
        let up = connected(Face::Floor, Dir::North);
        assert_eq!(up, Dir::Up);
        assert_eq!(connected(Face::Ceiling, Dir::North), Dir::Down);
        assert_eq!(connected(Face::Wall, Dir::East), Dir::East);
        assert_eq!(signal(7, up, Dir::North, false), 7);
        assert_eq!(signal(7, up, Dir::North, true), 0);
        // The block below reads it with direction up.
        assert_eq!(signal(7, up, Dir::Up, true), 7);
    }

    #[test]
    fn a_removed_lever_with_a_signal_updates_its_neighbours_and_saves_and_messages() {
        let mut l = Levers::default();
        let p = [0, 0, 0];
        l.load(
            [(
                p,
                Lever {
                    state: 9,
                    due: Some(5),
                },
            )]
            .into_iter(),
        );
        l.placed(p);
        assert_eq!(l.state(p), 9);
        assert_eq!(
            l.column(0, 0),
            [(
                p,
                Lever {
                    state: 9,
                    due: Some(5)
                }
            )]
        );
        assert_eq!(l.take_due(5), [p]);
        assert!(l.removed(p));
        assert_eq!(l.removed, [p]);
        let m = encode(&[
            (p, Some(9), Face::Wall, Dir::East),
            ([1, 2, 3], None, Face::Floor, Dir::North),
        ]);
        assert_eq!(
            decode(&m),
            [
                (p, Some(9), Face::Wall, Dir::East),
                ([1, 2, 3], None, Face::Floor, Dir::North)
            ]
        );
    }

    #[test]
    fn the_handle_turns_about_its_pivot() {
        // On the floor facing south (no turn), at 15 the handle's tip (8, 1, 0 + ...) turns
        // a quarter about x through (8, 1, 8) px.
        let m = handle_pose(Face::Floor, Dir::South, 15);
        let pivot = m.transform_point3(glam::Vec3::new(0.5, 1.0 / 16.0, 0.5));
        assert!((pivot - glam::Vec3::new(0.5, 1.0 / 16.0, 0.5)).length() < 1e-5);
        let p = m.transform_point3(glam::Vec3::new(0.5, 1.0 / 16.0 + 0.25, 0.5));
        assert!((p - glam::Vec3::new(0.5, 1.0 / 16.0, 0.75)).length() < 1e-5);
        // At 0 on the floor facing south, nothing moves.
        assert!(handle_pose(Face::Floor, Dir::South, 0).abs_diff_eq(glam::Mat4::IDENTITY, 1e-6));
    }
}
