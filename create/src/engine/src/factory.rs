//! The factory gauge's geometry (`FactoryPanelBlock`, `FactoryPanelSlotPositioning`,
//! `FactoryPanelConnection.getPath`): its four panels on the face it hangs on, which one a
//! click lands on, the arrows drawn between panels, and the colours they take
//! (`FactoryPanelRenderer`). Every turn is a quarter turn, done exactly.

use crate::kinetics::Dir;
use crate::steam::Face;

/// `PanelSlot`: `TOP_LEFT`, `TOP_RIGHT`, `BOTTOM_LEFT`, `BOTTOM_RIGHT` by ordinal.
pub const SLOTS: [u8; 4] = [0, 1, 2, 3];

/// `PanelSlot.xOffset`, `yOffset`.
pub fn slot_offset(slot: u8) -> (i32, i32) {
    match slot {
        0 => (1, 1),
        1 => (0, 1),
        2 => (1, 0),
        _ => (0, 0),
    }
}

/// `PanelSlot.getSerializedName`.
pub fn slot_name(slot: u8) -> &'static str {
    match slot {
        0 => "top_left",
        1 => "top_right",
        2 => "bottom_left",
        _ => "bottom_right",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotAxis {
    X,
    Y,
}

/// Catnip's `VecHelper.rotate` by a whole number of quarter turns (`deg`).
pub fn rotate(v: [f64; 3], deg: f64, axis: RotAxis) -> [f64; 3] {
    let q = ((deg / 90.0).round() as i64).rem_euclid(4);
    let (s, c) = match q {
        1 => (1.0, 0.0),
        2 => (0.0, -1.0),
        3 => (-1.0, 0.0),
        _ => (0.0, 1.0),
    };
    let [x, y, z] = v;
    match axis {
        RotAxis::X => [x, y * c - z * s, z * c + y * s],
        RotAxis::Y => [x * c + z * s, y, z * c - x * s],
    }
}

/// `VecHelper.rotateCentered`: about the block's centre.
pub fn rotate_centered(v: [f64; 3], deg: f64, axis: RotAxis) -> [f64; 3] {
    let r = rotate([v[0] - 0.5, v[1] - 0.5, v[2] - 0.5], deg, axis);
    [r[0] + 0.5, r[1] + 0.5, r[2] + 0.5]
}

/// Catnip's `AngleHelper.horizontalAngle`: south 0, west -90, north 180, east -270.
pub fn horizontal_angle(facing: Dir) -> f64 {
    match facing {
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `FactoryPanelBlock.getXRot` in degrees.
pub fn x_rot(face: Face) -> f64 {
    match face {
        Face::Ceiling => 90.0,
        Face::Floor => -90.0,
        Face::Wall => 0.0,
    }
}

/// `FactoryPanelBlock.getYRot` in degrees.
pub fn y_rot(face: Face, facing: Dir) -> f64 {
    (if face == Face::Ceiling { 180.0 } else { 0.0 }) + horizontal_angle(facing)
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`: away from what it hangs on.
pub fn connected_direction(face: Face, facing: Dir) -> Dir {
    match face {
        Face::Ceiling => Dir::Down,
        Face::Floor => Dir::Up,
        Face::Wall => facing,
    }
}

/// A point of a panel's own frame (the floor panel's, before turning) on the gauge's face:
/// `rotateCentered` by 180 about Y, by `xRot + 90` about X, by `yRot` about Y.
pub fn to_block(v: [f64; 3], face: Face, facing: Dir) -> [f64; 3] {
    let v = rotate_centered(v, 180.0, RotAxis::Y);
    let v = rotate_centered(v, x_rot(face) + 90.0, RotAxis::X);
    rotate_centered(v, y_rot(face, facing), RotAxis::Y)
}

/// `FactoryPanelSlotPositioning.getCenterOfSlot`: the panel's value box in the block.
pub fn slot_center(face: Face, facing: Dir, slot: u8) -> [f64; 3] {
    let (x, y) = slot_offset(slot);
    to_block(
        [
            0.25 + f64::from(x) * 0.5,
            1.5 / 16.0,
            0.25 + f64::from(y) * 0.5,
        ],
        face,
        facing,
    )
}

/// `FactoryPanelBlock.getTargetedSlot`: the panel nearest a click at `local` in the block.
pub fn targeted_slot(face: Face, facing: Dir, local: [f64; 3]) -> u8 {
    let mut best = f64::MAX;
    let mut slot = 2;
    for s in SLOTS {
        let (x, y) = slot_offset(s);
        let v = to_block(
            [0.25 + f64::from(x) * 0.5, 0.0, 0.25 + f64::from(y) * 0.5],
            face,
            facing,
        );
        let d: f64 = (0..3).map(|i| (v[i] - local[i]).powi(2)).sum();
        if d > best {
            continue;
        }
        best = d;
        slot = s;
    }
    slot
}

/// `FactoryPanelSlotPositioning.testHit` (`ValueBoxTransform.getScale` 0.5): whether a hit at
/// `local` lands on the value box of panel `slot`.
pub fn hits_slot(face: Face, facing: Dir, slot: u8, local: [f64; 3]) -> bool {
    let c = slot_center(face, facing, slot);
    let d: f64 = (0..3).map(|i| (c[i] - local[i]).powi(2)).sum();
    d.sqrt() < 0.25
}

/// `FactoryPanelBlockEntity.getShape`'s box of one panel: `[x0, y0, z0, x1, y1, z1]` in the
/// block.
pub fn panel_box(face: Face, facing: Dir, slot: u8) -> [f64; 6] {
    let (x, y) = slot_offset(slot);
    let c = to_block(
        [
            0.25 + f64::from(x) * 0.5,
            1.0 / 16.0,
            0.25 + f64::from(y) * 0.5,
        ],
        face,
        facing,
    );
    let axis = connected_direction(face, facing).axis();
    let mut b = [0.0; 6];
    for (i, a) in [
        crate::kinetics::Axis::X,
        crate::kinetics::Axis::Y,
        crate::kinetics::Axis::Z,
    ]
    .into_iter()
    .enumerate()
    {
        let r = if a == axis { 1.0 / 16.0 } else { 4.0 / 16.0 };
        b[i] = c[i] - r;
        b[i + 3] = c[i] + r;
    }
    b
}

/// `FactoryPanelConnection.calculatePathDiff`: where `from` lies from `to` in the panel's own
/// frame, in half blocks' units of panel slots (y dropped).
pub fn path_diff(face: Face, facing: Dir, from: ([i32; 3], u8), to: ([i32; 3], u8)) -> [f64; 3] {
    let (tx, ty) = slot_offset(to.1);
    let (fx, fy) = slot_offset(from.1);
    let d = [0, 1, 2].map(|i| f64::from(to.0[i] - from.0[i]));
    let d = rotate(d, -y_rot(face, facing), RotAxis::Y);
    let d = rotate(d, -x_rot(face) - 90.0, RotAxis::X);
    let d = rotate(d, -180.0, RotAxis::Y);
    [
        d[0] + f64::from(tx - fx) * 0.5,
        0.0,
        d[2] + f64::from(ty - fy) * 0.5,
    ]
}

fn step(p: [i32; 2], d: Dir) -> [i32; 2] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[2]]
}

fn manhattan(p: [i32; 2]) -> i32 {
    p[0].abs() + p[1].abs()
}

/// `FactoryPanelConnection.getPath`: the steps (in the panel's frame, half a block each) from
/// the target panel toward the panel feeding it, `diff` apart (`path_diff`). With `bend` -1
/// the first of modes 0 to 3 whose steps miss everything (`blocked` at an offset of steps from
/// the start, in half blocks) is taken, else mode 4; else mode `bend`.
pub fn path(diff: [f64; 3], bend: i32, blocked: impl Fn([i32; 2]) -> bool) -> Vec<Dir> {
    let find = bend == -1;
    let mut path = Vec::new();
    'modes: for mode in 0..=4 {
        path.clear();
        if !find && mode != bend {
            continue;
        }
        let desperate = mode == 4;
        let mut first = [0, 0];
        let mut last = [
            (diff[0] * 2.0 + 0.1).floor() as i32,
            (diff[2] * 2.0 + 0.1).floor() as i32,
        ];
        if mode > 1 {
            let flip_x = (diff[0] > 0.0) ^ (mode % 2 == 1);
            let flip_z = (diff[2] > 0.0) ^ (mode % 2 == 0);
            let ceil = |a: i32| -((-a).div_euclid(2));
            let floor = |a: i32| a.div_euclid(2);
            let (cx, cz, fx, fz) = (ceil(last[0]), ceil(last[1]), floor(last[0]), floor(last[1]));
            first = [if flip_x { fx } else { cx }, if flip_z { fz } else { cz }];
            last = [if !flip_x { fx } else { cx }, if !flip_z { fz } else { cz }];
        }
        let mut last_dir: Option<Dir> = None;
        let mut current: Option<Dir> = None;
        for target in [first, last] {
            let mut t = target;
            let z_farther = t[1].abs() > t[0].abs();
            let z_preferred = if desperate { z_farther } else { mode % 2 == 1 };
            let order = if z_preferred {
                [Dir::South, Dir::North, Dir::West, Dir::East]
            } else {
                [Dir::West, Dir::East, Dir::South, Dir::North]
            };
            for _ in 0..100 {
                if t == [0, 0] {
                    break;
                }
                for d in order {
                    if last_dir.is_some_and(|l| d == l.opposite()) {
                        continue;
                    }
                    match current {
                        None => current = Some(d),
                        Some(c) if manhattan(step(t, d)) < manhattan(step(t, c)) => {
                            current = Some(d)
                        }
                        _ => {}
                    }
                }
                let Some(c) = current else { break };
                last_dir = Some(c);
                t = step(t, c);
                path.push(c);
            }
        }
        if find && !desperate {
            let mut travelled = [0, 0];
            for d in path.iter().take(path.len().saturating_sub(1)) {
                travelled = step(travelled, *d);
                if blocked(travelled) {
                    continue 'modes;
                }
            }
        }
        break;
    }
    path
}

/// A path step's offset (`travelled` half blocks in the panel's frame) as a point in the world
/// from the target panel's centre (`getPath`'s collision test).
pub fn path_point(face: Face, facing: Dir, travelled: [i32; 2]) -> [f64; 3] {
    let v = [
        f64::from(travelled[0]) * 0.5,
        0.0,
        f64::from(travelled[1]) * 0.5,
    ];
    let v = rotate(v, 180.0, RotAxis::Y);
    let v = rotate(v, x_rot(face) + 90.0, RotAxis::X);
    rotate(v, y_rot(face, facing), RotAxis::Y)
}

/// What a panel shows (`FactoryPanelBehaviour`'s fields the renderer reads).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub count: u32,
    pub missing_address: bool,
    pub redstone_powered: bool,
    pub waiting: bool,
    pub satisfied: bool,
    pub promised_satisfied: bool,
}

/// `getIngredientStatusColor`.
pub fn ingredient_color(s: Status) -> u32 {
    if s.count == 0 || s.missing_address || s.redstone_powered {
        0x888898
    } else if s.waiting {
        0x5B3B3B
    } else if s.satisfied {
        0x9EFF7F
    } else if s.promised_satisfied {
        0x22AFAF
    } else {
        0x3D6EBD
    }
}

/// Catnip's `Color.mixColors`.
pub fn mix_colors(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let mix = |s: u32| ((ch(a, s) + (ch(b, s) - ch(a, s)) * t) as u32 & 0xFF) << s;
    mix(16) | mix(8) | mix(0)
}

/// `FactoryPanelRenderer.renderPath`'s colour and lift (in 1/512 block) of an ingredient's
/// path, with the bulb's `glow` and whether the last request went out (`success`).
pub fn path_look(s: Status, glow: f32, success: bool) -> (u32, f32) {
    let mut color = ingredient_color(s);
    let mut y = 1.0
        + if s.promised_satisfied {
            1.0
        } else if s.satisfied {
            0.0
        } else {
            2.0
        };
    if !s.redstone_powered && !s.waiting && glow > 0.0 && !s.satisfied {
        let p = 1.0 - (1.0 - glow) * (1.0 - glow);
        color = mix_colors(color, if success { 0xEAF2EC } else { 0xE5654B }, p);
        if !s.promised_satisfied {
            y += (if success { 1.0 } else { 2.0 }) * p;
        }
    }
    (color, y)
}

/// Whether a path's sprite scrolls (`shiftUV(FACTORY_PANEL_CONNECTIONS)`).
pub fn path_moves(s: Status) -> bool {
    !s.missing_address && !s.waiting && !s.satisfied && !s.redstone_powered
}

/// `isMissingAddress`.
pub fn missing_address(has_inputs: bool, restocker: bool, count: u32, address: &str) -> bool {
    (has_inputs || restocker) && count != 0 && crate::package::is_blank(address)
}

/// `FactoryPanelConnectionHandler.checkForIssues` between panels: Create's message key, or
/// None when `from` may feed `to`. `same_state` compares the two gauges' states (but
/// `waterlogged` and `powered`).
#[allow(clippy::too_many_arguments)]
pub fn connection_issue(
    from_inputs: &[([i32; 3], u8)],
    to: ([i32; 3], u8),
    from_pos: [i32; 3],
    same_state: bool,
    face: Face,
    facing: Dir,
    to_restocker: bool,
    items_set: bool,
) -> Option<&'static str> {
    if from_inputs.contains(&to) {
        return Some("factory_panel.already_connected");
    }
    if from_inputs.len() >= crate::stock::MAX_INPUTS {
        return Some("factory_panel.cannot_add_more_inputs");
    }
    let diff = [0, 1, 2].map(|i| to.0[i] - from_pos[i]);
    if !same_state {
        return Some("factory_panel.same_orientation");
    }
    let axis = connected_direction(face, facing).axis();
    let along = match axis {
        crate::kinetics::Axis::X => diff[0],
        crate::kinetics::Axis::Y => diff[1],
        crate::kinetics::Axis::Z => diff[2],
    };
    if along != 0 {
        return Some("factory_panel.same_surface");
    }
    let d2: i64 = diff.iter().map(|v| i64::from(*v) * i64::from(*v)).sum();
    if d2 >= 16 * 16 {
        return Some("factory_panel.too_far_apart");
    }
    if to_restocker {
        return Some("factory_panel.input_in_restock_mode");
    }
    if !items_set {
        return Some("factory_panel.no_item");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_floor_gauge_s_slots_lie_on_the_floor() {
        // Facing south on the floor: no turn about X; turned 180 about Y.
        let c = slot_center(Face::Floor, Dir::South, 3);
        assert!((c[1] - 1.5 / 16.0).abs() < 1e-9);
        assert!((c[0] - 0.75).abs() < 1e-9 && (c[2] - 0.75).abs() < 1e-9);
        let c = slot_center(Face::Floor, Dir::South, 0);
        assert!((c[0] - 0.25).abs() < 1e-9 && (c[2] - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_wall_gauge_s_slots_lie_on_its_wall() {
        // On a wall facing south (hung on the block to the north): the panels lie at z small.
        for s in SLOTS {
            let c = slot_center(Face::Wall, Dir::South, s);
            assert!((c[2] - 1.5 / 16.0).abs() < 1e-9, "{c:?}");
        }
        // Facing north: at z near 1.
        let c = slot_center(Face::Wall, Dir::North, 0);
        assert!((c[2] - (1.0 - 1.5 / 16.0)).abs() < 1e-9, "{c:?}");
        // On the ceiling: y near 1.
        let c = slot_center(Face::Ceiling, Dir::North, 0);
        assert!((c[1] - (1.0 - 1.5 / 16.0)).abs() < 1e-9, "{c:?}");
    }

    #[test]
    fn a_click_picks_the_nearest_panel() {
        for face in [Face::Floor, Face::Wall, Face::Ceiling] {
            for facing in [Dir::North, Dir::South, Dir::East, Dir::West] {
                for s in SLOTS {
                    let c = slot_center(face, facing, s);
                    assert_eq!(targeted_slot(face, facing, c), s);
                    assert!(hits_slot(face, facing, s, c));
                }
            }
        }
    }

    #[test]
    fn a_path_runs_from_the_target_to_the_source() {
        // The source a block east of the target (in the panel's frame, `diff` is target minus
        // source): two half-block steps east, from the target.
        let p = path([-1.0, 0.0, 0.0], -1, |_| false);
        assert_eq!(p, vec![Dir::East, Dir::East]);
        // Diagonal, mode 0: x first (west/east preferred), then z.
        let p = path([1.0, 0.0, 1.0], 0, |_| false);
        assert_eq!(p, vec![Dir::West, Dir::West, Dir::North, Dir::North]);
        // Mode 1 prefers z first.
        let p = path([1.0, 0.0, 1.0], 1, |_| false);
        assert_eq!(p, vec![Dir::North, Dir::North, Dir::West, Dir::West]);
        // A blocked mode 0 falls to the next.
        let p = path([1.0, 0.0, 1.0], -1, |t| t == [-1, 0]);
        assert_eq!(p[0], Dir::North);
    }

    #[test]
    fn neighbouring_panels_of_one_gauge_differ_by_half_a_block() {
        let d = path_diff(Face::Floor, Dir::South, ([0, 0, 0], 0), ([0, 0, 0], 1));
        assert!(
            (d[0].abs() - 0.5).abs() < 1e-9 && d[2].abs() < 1e-9,
            "{d:?}"
        );
    }

    #[test]
    fn colours_follow_create_s() {
        let s = Status {
            count: 4,
            ..Status::default()
        };
        assert_eq!(ingredient_color(s), 0x3D6EBD);
        assert_eq!(
            ingredient_color(Status {
                satisfied: true,
                ..s
            }),
            0x9EFF7F
        );
        assert_eq!(ingredient_color(Status::default()), 0x888898);
        assert_eq!(mix_colors(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
    }

    #[test]
    fn connections_need_one_surface_and_items() {
        let issue = |to: [i32; 3], same, items| {
            connection_issue(
                &[],
                (to, 0),
                [0, 0, 0],
                same,
                Face::Floor,
                Dir::North,
                false,
                items,
            )
        };
        assert_eq!(issue([1, 0, 0], true, true), None);
        assert_eq!(
            issue([1, 1, 0], true, true),
            Some("factory_panel.same_surface")
        );
        assert_eq!(
            issue([1, 0, 0], false, true),
            Some("factory_panel.same_orientation")
        );
        assert_eq!(issue([1, 0, 0], true, false), Some("factory_panel.no_item"));
        assert_eq!(
            issue([16, 0, 0], true, true),
            Some("factory_panel.too_far_apart")
        );
    }
}
