//! Create's connected crafter inputs (`ConnectedInputHandler`, `ConnectedInput`): crafters
//! facing the same way, side by side, joined with the wrench share one inventory, filled in
//! reading order (top row first). A group's controller holds the offsets of all its crafters
//! (its own first); the others hold the offset to their controller.

use crate::kinetics::{Axis, Dir};
use std::collections::HashMap;

/// One crafter's `ConnectedInput`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub controller: bool,
    pub data: Vec<[i32; 3]>,
}

impl Default for Input {
    /// `new ConnectedInput()`: its own controller, alone.
    fn default() -> Input {
        Input {
            controller: true,
            data: vec![[0, 0, 0]],
        }
    }
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl Input {
    /// `attachTo`: no longer a controller, pointing at `controller` from `me`.
    fn attach_to(&mut self, controller: [i32; 3], me: [i32; 3]) {
        self.controller = false;
        self.data = vec![sub(controller, me)];
    }

    /// The group's controller.
    pub fn controller_of(&self, me: [i32; 3]) -> [i32; 3] {
        add(me, self.data.first().copied().unwrap_or([0, 0, 0]))
    }
}

/// The inputs of the crafters at hand, by position (a crafter not in it has its own).
pub type Inputs = HashMap<[i32; 3], Input>;

fn input(inputs: &Inputs, p: [i32; 3]) -> Input {
    inputs.get(&p).cloned().unwrap_or_default()
}

/// `initAndAddAll`: `crafter` the controller of a new group of itself and `positions`.
fn init_and_add_all(inputs: &mut Inputs, crafter: [i32; 3], positions: &[[i32; 3]]) {
    let mut own = Input::default();
    for &p in positions {
        let mut i = input(inputs, p);
        i.attach_to(crafter, p);
        inputs.insert(p, i);
        own.data.push(sub(p, crafter));
    }
    inputs.insert(crafter, own);
}

/// `toggleConnection` between the crafters at `a` and `b`: in one group, it splits there (`b`
/// and what still joins it from `a`'s); else `b`'s group joins `a`'s.
pub fn toggle(inputs: &mut Inputs, a: [i32; 3], b: [i32; 3]) {
    let (ca, cb) = (
        input(inputs, a).controller_of(a),
        input(inputs, b).controller_of(b),
    );
    if ca == cb {
        let mut positions: Vec<[i32; 3]> = input(inputs, ca)
            .data
            .iter()
            .map(|o| add(ca, *o))
            .filter(|p| *p != a && *p != b)
            .collect();
        let mut frontier = vec![b];
        let mut split = Vec::new();
        while !frontier.is_empty() {
            let current = frontier.remove(0);
            for d in Dir::ALL {
                let next = add(current, d.offset());
                if let Some(i) = positions.iter().position(|p| *p == next) {
                    positions.remove(i);
                    split.push(next);
                    frontier.push(next);
                }
            }
        }
        init_and_add_all(inputs, a, &positions);
        init_and_add_all(inputs, b, &split);
        return;
    }
    // `connectControllers`: every crafter of `b`'s group, then its controller, under `a`'s.
    let mut one = input(inputs, ca);
    for o in input(inputs, cb).data {
        if o == [0, 0, 0] {
            continue;
        }
        let p = add(cb, o);
        let mut i = input(inputs, p);
        i.attach_to(ca, p);
        one.data.push(sub(p, ca));
        inputs.insert(p, i);
    }
    let mut c2 = input(inputs, cb);
    c2.attach_to(ca, cb);
    inputs.insert(cb, c2);
    one.data.push(sub(cb, ca));
    inputs.insert(ca, one);
}

/// `getInventories`: the crafters whose slots a group's inventory is, for the crafter at `pos`
/// facing `facing`: top row first, then along the row as a player in front reads it. None of
/// them for a crafter whose controller doesn't lead a group.
pub fn inventories(inputs: &Inputs, pos: [i32; 3], facing: Dir) -> Vec<[i32; 3]> {
    let own = input(inputs, pos);
    if !own.controller {
        let c = own.controller_of(pos);
        let ci = input(inputs, c);
        if c == pos || !ci.controller {
            return Vec::new();
        }
        return inventories(inputs, c, facing);
    }
    let compare = crate::wrench::clockwise(facing, Axis::Y).axis();
    let step = if facing.positive() { 1 } else { -1 };
    let modifier = step * if compare == Axis::Z { -1 } else { 1 };
    let along = |p: &[i32; 3]| match compare {
        Axis::X => p[0],
        Axis::Y => p[1],
        Axis::Z => p[2],
    };
    let mut data = own.data.clone();
    data.sort_by(|p1, p2| {
        p2[1].cmp(&p1[1]).then_with(|| {
            let o = along(p1).cmp(&along(p2));
            if modifier < 0 { o.reverse() } else { o }
        })
    });
    data.into_iter().map(|o| add(pos, o)).collect()
}

/// `MechanicalCrafterBlock.onRemove`: a crafter broken leaves its group, which splits where it
/// stood (each neighbour across its facing axis in the same group is toggled away from it).
pub fn remove(inputs: &mut Inputs, pos: [i32; 3], facing: Dir) {
    for d in Dir::ALL {
        if d.axis() == facing.axis() {
            continue;
        }
        let other = add(pos, d.offset());
        if !inputs.contains_key(&pos) || !inputs.contains_key(&other) {
            continue;
        }
        if input(inputs, pos).controller_of(pos) == input(inputs, other).controller_of(other) {
            toggle(inputs, pos, other);
        }
    }
    inputs.remove(&pos);
}

/// `shouldConnect`: from the crafter at `pos` facing `facing`, clicked on `face`, the
/// neighbour toward `direction` (facing the same way) may join; never along its facing axis or
/// on its front.
pub fn should_connect(facing: Dir, face: Dir, direction: Dir, neighbour: Option<Dir>) -> bool {
    direction.axis() != facing.axis() && face != facing && neighbour == Some(facing)
}

/// `EdgeInteractionHandler.getBB`: the strip across the edge of the block at `pos` toward
/// `direction` (3/8 wide, half on each side of the edge), where a click joins or parts it from
/// the neighbour there.
pub fn edge_box(pos: [i32; 3], direction: Dir) -> ([f64; 3], [f64; 3]) {
    let v = direction.offset();
    let margin = 10.0 / 16.0;
    let mut min = [0.0; 3];
    let mut max = [0.0; 3];
    for k in 0..3 {
        // contract(a) takes `a` off the far side; then move(a / 2 + v / 2), inflate(1/256).
        let a = f64::from(v[k].abs()) * margin;
        let shift = a / 2.0 + f64::from(v[k]) / 2.0;
        min[k] = f64::from(pos[k]) + shift - 1.0 / 256.0;
        max[k] = f64::from(pos[k]) + 1.0 - a + shift + 1.0 / 256.0;
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crafters_join_read_top_first_and_split_where_parted() {
        // Four crafters facing south in a 2×2: x 0..1, y 0..1, z 0.
        let mut inputs = Inputs::new();
        let (tl, tr, bl, br) = ([0, 1, 0], [1, 1, 0], [0, 0, 0], [1, 0, 0]);
        toggle(&mut inputs, bl, br);
        toggle(&mut inputs, bl, tl);
        toggle(&mut inputs, tl, tr);
        let all = inventories(&inputs, tr, Dir::South);
        // The top row first; facing south (`modifier` 1, along x), west to east: left to right
        // for a player in front, looking north.
        assert_eq!(all, vec![tl, tr, bl, br]);
        assert_eq!(
            inventories(&inputs, br, Dir::South),
            all,
            "the same group from any"
        );
        // Parting the bottom pair: the bottom right leads what still joins it (the top row).
        toggle(&mut inputs, bl, br);
        assert_eq!(inventories(&inputs, bl, Dir::South), vec![bl]);
        assert_eq!(inventories(&inputs, br, Dir::South), vec![tl, tr, br]);
        // A broken crafter leaves its group.
        remove(&mut inputs, tl, Dir::South);
        assert!(!inventories(&inputs, bl, Dir::South).contains(&tl));
    }

    #[test]
    fn the_edge_strip_straddles_the_edge() {
        let (min, max) = edge_box([0, 0, 0], Dir::East);
        assert!((min[0] - (0.8125 - 1.0 / 256.0)).abs() < 1e-9);
        assert!((max[0] - (1.1875 + 1.0 / 256.0)).abs() < 1e-9);
        assert!(min[1] < 0.0 && max[1] > 1.0);
        let (min, max) = edge_box([0, 0, 0], Dir::Down);
        assert!(
            (min[1] + 0.1875 + 1.0 / 256.0).abs() < 1e-9
                && (max[1] - 0.1875 - 1.0 / 256.0).abs() < 1e-9
        );
    }
}
