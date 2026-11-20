//! Create's andesite tunnel (`BeltTunnelBlock`, `BeltTunnelBlockEntity`, `BeltTunnelShapes`,
//! `BeltTunnelInteractionHandler`): a cover on a cased belt segment whose shape follows the belts
//! and inputs beside the segment; as a junction it hands one item of each stack that enters its
//! segment to each side's belt or depot, keeping one on the belt.

use crate::kinetics::{Axis, Dir};

/// `BeltTunnelBlock.Shape`, by its state value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Straight,
    Window,
    Closed,
    TLeft,
    TRight,
    Cross,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Shape::Straight,
        Shape::Window,
        Shape::Closed,
        Shape::TLeft,
        Shape::TRight,
        Shape::Cross,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Straight => "straight",
            Shape::Window => "window",
            Shape::Closed => "closed",
            Shape::TLeft => "t_left",
            Shape::TRight => "t_right",
            Shape::Cross => "cross",
        }
    }

    pub fn read(name: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.name() == name)
    }

    /// `hasWindow`.
    pub fn has_window(self) -> bool {
        matches!(self, Shape::Window | Shape::Closed)
    }

    /// `isStraight`.
    pub fn straight(self) -> bool {
        self.has_window() || self == Shape::Straight
    }

    /// `isJunction`.
    pub fn junction(self) -> bool {
        matches!(self, Shape::Cross | Shape::TLeft | Shape::TRight)
    }
}

/// `Direction.get(POSITIVE, axis).getClockWise()`: the tunnel's left side.
pub fn left(axis: Axis) -> Dir {
    if axis == Axis::X {
        Dir::South
    } else {
        Dir::West
    }
}

/// `getTunnelState`: the shape from the outputs beside the belt segment (`hasValidOutput` to the
/// left and right) and, when straight, whether a window fits (`canHaveWindow`).
pub fn shape_for(on_left: bool, on_right: bool, can_window: bool) -> Shape {
    match (on_left, on_right) {
        (true, true) => Shape::Cross,
        (true, false) => Shape::TLeft,
        (false, true) => Shape::TRight,
        _ if can_window => Shape::Window,
        _ => Shape::Straight,
    }
}

/// `BeltTunnelBlock.updateShape`: the state kept when its axis and windowedness still fit (a
/// closed window stays closed), else the new one.
pub fn updated(now: (Axis, Shape), fresh: (Axis, Shape)) -> (Axis, Shape) {
    if now.0 == fresh.0 && now.1.has_window() == fresh.1.has_window() {
        now
    } else {
        fresh
    }
}

/// `updateTunnelConnections`' `sides`: the ends along its axis and the open sides across it, in
/// `Iterate.horizontalDirections` order (south, west, north, east).
pub fn sides(axis: Axis, shape: Shape) -> Vec<Dir> {
    [Dir::South, Dir::West, Dir::North, Dir::East]
        .into_iter()
        .filter(|d| {
            if d.axis() == axis {
                return true;
            }
            let positive = d.positive() ^ (d.axis() == Axis::Z);
            !(shape.straight()
                || positive && shape == Shape::TLeft
                || !positive && shape == Shape::TRight)
        })
        .collect()
}

/// `BeltTunnelShapes.getShape` in pixels: the block from 5 px below (over the belt) to the top,
/// less a 12 px wide, 15 px high opening on each open side.
pub fn boxes(axis: Axis, shape: Shape) -> Vec<[f64; 6]> {
    let open = sides(axis, shape);
    let mut b = vec![[2., -5., 2., 14., 16., 14.]];
    for (x, z) in [(0., 0.), (14., 0.), (0., 14.), (14., 14.)] {
        b.push([x, -5., z, x + 2., 16., z + 2.]);
    }
    for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let bottom = if open.contains(&d) { 10. } else { -5. };
        b.push(match d {
            Dir::North => [2., bottom, 0., 14., 16., 2.],
            Dir::South => [2., bottom, 14., 14., 16., 16.],
            Dir::West => [0., bottom, 2., 2., 16., 14.],
            _ => [14., bottom, 2., 16., 16., 14.],
        });
    }
    b
}

/// Whether an andesite tunnel splits a stack of `count` moving along `movement` (`count > 1`, a
/// junction on the belt's axis).
pub fn splits(axis: Axis, shape: Shape, movement: Dir, count: u8) -> bool {
    count > 1 && shape.junction() && movement.axis() == axis
}

/// `BrassTunnelBlockEntity.SelectionMode`, picked by the wheel or the value board.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Split,
    ForcedSplit,
    RoundRobin,
    ForcedRoundRobin,
    PreferNearest,
    Randomize,
    Synchronize,
}

impl Mode {
    /// The modes in Create's order.
    pub const ALL: [Mode; 7] = [
        Mode::Split,
        Mode::ForcedSplit,
        Mode::RoundRobin,
        Mode::ForcedRoundRobin,
        Mode::PreferNearest,
        Mode::Randomize,
        Mode::Synchronize,
    ];

    pub fn index(self) -> usize {
        Mode::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// Its name (`create.tunnel.selection_mode.*`).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Split => "Split",
            Mode::ForcedSplit => "Forced Split",
            Mode::RoundRobin => "Round Robin",
            Mode::ForcedRoundRobin => "Forced Round Robin",
            Mode::PreferNearest => "Prefer Nearest",
            Mode::Randomize => "Randomize",
            Mode::Synchronize => "Synchronize Inputs",
        }
    }

    /// Scrolled `steps` through the options, held at the ends (`ScrollOptionBehaviour`).
    pub fn scrolled(self, steps: i32) -> Mode {
        let i = (self.index() as i32 + steps).clamp(0, Mode::ALL.len() as i32 - 1);
        Mode::ALL[i as usize]
    }

    /// The mode at `SelectionMode` ordinal `i`; None past the end.
    pub fn from_option(i: i32) -> Option<Mode> {
        Mode::ALL.get(usize::try_from(i).ok()?).copied()
    }
}

/// The names of all of Create's `SelectionMode`s in order (`create.tunnel.selection_mode.*`):
/// the value board shows all seven, as Create's does.
pub const OPTIONS: [&str; 7] = [
    "Split",
    "Forced Split",
    "Round Robin",
    "Forced Round Robin",
    "Prefer Nearest",
    "Randomize",
    "Synchronize Inputs",
];

/// `BrassTunnelBlockEntity.hasDistributionBehaviour` (its mode's value box is active only then)
/// for a tunnel that is not connected to others (`connectedLeft`/`Right` are not ported): a flap
/// on a side across its axis (`updateTunnelConnections`), which a tunnel beside it (`tunnel(d)`)
/// occludes. The extended belt funnel that also occludes one is not modelled.
pub fn distributes(axis: Axis, shape: Shape, tunnel: impl Fn(Dir) -> bool) -> bool {
    sides(axis, shape)
        .into_iter()
        .any(|d| d.axis() != axis && !tunnel(d))
}

/// `BrassTunnelBlockEntity.write/read` (`:594-639`): the server state persisted across saves and
/// contraption carriage: `SyncedOutput`, `ConnectedLeft`, `ConnectedRight`, `StackToDistribute`,
/// `StackEnteredFrom`, `DistributionProgress`, `PreviousIndex`, `SelectionMode`, `Targets` and
/// `FilteredTargets`.
/// `newItemArrived` is not saved (`BrassTunnelBlockEntity.java:78`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedBrass {
    pub stack: Option<(String, u8)>,
    pub entered_from: Option<Dir>,
    pub progress: i32,
    pub previous_output: usize,
    pub mode: Mode,
    pub synced: bool,
    pub connected_left: bool,
    pub connected_right: bool,
    pub targets: Targets,
}

impl Default for SavedBrass {
    fn default() -> SavedBrass {
        SavedBrass {
            stack: None,
            entered_from: None,
            progress: -1,
            previous_output: 0,
            mode: Mode::Split,
            synced: false,
            connected_left: false,
            connected_right: false,
            targets: Targets::default(),
        }
    }
}

impl SavedBrass {
    /// Whether this state matches a freshly placed tunnel and needs no saved entry.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A brass tunnel's typed state: the stack it holds to hand out, the side it came in through,
/// `distributionProgress` (-1 to look for outputs), whether it just arrived, and
/// `previousOutputIndex`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brass {
    pub stack: Option<(String, u8)>,
    pub entered_from: Option<Dir>,
    pub progress: i32,
    pub new_item: bool,
    pub previous_output: usize,
    /// `selectionMode`.
    pub mode: Mode,
    /// The sequenced assembly progress of the stack it distributes (not saved).
    pub stack_progress: Option<crate::sequenced::Progress>,
    /// `syncedOutputActive`: a synchronizing group of tunnels all hold a stack and hand it out.
    pub synced: bool,
    /// `connectedLeft` / `connectedRight`: connected to an adjacent brass tunnel across its axis.
    pub connected_left: bool,
    pub connected_right: bool,
    /// `distributionTargets`.
    pub targets: Targets,
}

/// `BrassTunnelBlockEntity.distributionTargets`: Java hands out to the outputs found at the
/// last gather, not to ones that appeared since.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    pub filtered: Vec<([i32; 3], Dir)>,
    pub plain: Vec<([i32; 3], Dir)>,
}

impl Targets {
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty() && self.plain.is_empty()
    }

    /// `BrassTunnelBlockEntity.tick`'s `validTargets`.
    pub fn to_hand_out(
        &self,
        pos: [i32; 3],
        entered_from: Option<Dir>,
        is_tunnel: impl Fn([i32; 3]) -> bool,
    ) -> [Vec<([i32; 3], Dir)>; 2] {
        let valid = |list: &[([i32; 3], Dir)]| {
            list.iter()
                .copied()
                .filter(|&(p, d)| !(p == pos && Some(d) == entered_from) && is_tunnel(p))
                .collect()
        };
        [valid(&self.filtered), valid(&self.plain)]
    }
}

impl Default for Brass {
    fn default() -> Brass {
        Brass::from_saved(SavedBrass::default())
    }
}

impl Brass {
    /// Extracts the persisted state (`BrassTunnelBlockEntity.write`).
    pub fn to_saved(&self) -> SavedBrass {
        SavedBrass {
            stack: self.stack.clone(),
            entered_from: self.entered_from,
            progress: self.progress,
            previous_output: self.previous_output,
            mode: self.mode,
            synced: self.synced,
            connected_left: self.connected_left,
            connected_right: self.connected_right,
            targets: self.targets.clone(),
        }
    }

    /// Restores a brass tunnel from its saved state (`BrassTunnelBlockEntity.read`).
    pub fn from_saved(saved: SavedBrass) -> Brass {
        Brass {
            stack: saved.stack,
            entered_from: saved.entered_from,
            progress: saved.progress,
            new_item: false,
            previous_output: saved.previous_output,
            mode: saved.mode,
            stack_progress: None,
            synced: saved.synced,
            connected_left: saved.connected_left,
            connected_right: saved.connected_right,
            targets: saved.targets,
        }
    }

    /// Whether the tunnel needs ticking (`stackToDistribute` held or `syncedOutputActive`).
    pub fn is_running(&self) -> bool {
        self.stack.is_some() || self.synced
    }
    /// `BrassTunnelBlockEntity.canInsert`: whether an item entering from `side` is accepted by
    /// the tunnel's own filter and (when distributing) its held slot; false means `stuckAtTunnel`.
    pub fn can_insert(&self, distributing: bool, filter_passes: bool) -> bool {
        filter_passes && (!distributing || self.stack.is_none())
    }

    /// `BrassTunnelBlockEntity.canTakeItems`: holds no stack and is not handing out with a
    /// synchronized group.
    pub fn can_take(&self) -> bool {
        self.stack.is_none() && !self.synced
    }

    /// `BrassTunnelItemHandler.insertItem` (`:33-49`): where an item-handler insert goes.
    pub fn handler_target(&self, distributing: bool) -> HandlerTarget {
        if !distributing {
            HandlerTarget::BeltBelow
        } else if self.can_take() {
            HandlerTarget::Distribute
        } else {
            HandlerTarget::Busy
        }
    }

    /// `BrassTunnelBlockEntity.tick` (`:131-200`): advances the timer and decides the tick phase.
    pub fn begin_tick(&mut self, belt_moving: bool) -> BrassPhase {
        if self.progress > 0 {
            self.progress -= 1;
        }
        if !belt_moving || (self.stack.is_none() && !self.synced) {
            return BrassPhase::Idle;
        }
        if self.progress == -1 {
            self.targets = Targets::default();
            BrassPhase::Gather
        } else if self.progress != 0 {
            BrassPhase::Wait
        } else {
            BrassPhase::Distribute
        }
    }

    /// `BrassTunnelBlockEntity.tick` (`:146-163`, `:517`): checks `syncedOutputActive` *before*
    /// updating it from the group's stacks (`gatherValidOutputs`), returning `Some(new_synced)`
    /// when the whole group should update `synced`, and `true` if gathering may proceed to check
    /// targets (`validOutputs != null && !stackToDistribute.isEmpty()`).
    pub fn gather_sync(&mut self, group_has_stack: &[bool]) -> (Option<bool>, bool) {
        let was_synced = self.synced;
        let mut sync_update = None;
        if self.mode == Mode::Synchronize {
            let all_full = group_has_stack.iter().all(|h| *h);
            let all_empty = group_has_stack.iter().all(|h| !*h);
            if all_full || all_empty {
                let next_synced = !all_empty;
                self.synced = next_synced;
                sync_update = Some(next_synced);
            }
            if !was_synced {
                return (sync_update, false);
            }
        }
        (sync_update, self.stack.is_some())
    }

    /// `BrassTunnelBlockEntity.tick` (`:165-195`).
    pub fn gathered(&mut self, targets: Targets) -> bool {
        self.targets = targets;
        if self.targets.is_empty() {
            return false;
        }
        self.schedule_distribution();
        true
    }

    /// `BrassTunnelBlockEntity.tick` (`:186-195`): schedules the next distribution once targets
    /// were found.
    fn schedule_distribution(&mut self) {
        if self.new_item {
            self.new_item = false;
            self.progress = 2;
        } else if self.mode != Mode::Synchronize || self.synced {
            self.progress = BRASS_TIMER;
        }
    }
}

/// What a brass tunnel's tick (`BrassTunnelBlockEntity.tick`) needs next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrassPhase {
    /// Belt stopped, or nothing held and not synced (`:133-136`).
    Idle,
    /// `distributionProgress == -1`: gather outputs and update `syncedOutputActive` (`:140-196`).
    Gather,
    /// Waiting for countdown (`distributionProgress > 0`).
    Wait,
    /// `distributionProgress == 0`: distribute held stack (`:198-218`).
    Distribute,
}

/// Where `BrassTunnelItemHandler.insertItem` routes an insertion (`BrassTunnelItemHandler.java:33-49`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlerTarget {
    /// `!hasDistributionBehaviour()`: delegate to the belt segment below (`getBeltCapability`).
    BeltBelow,
    /// `hasDistributionBehaviour() && canTakeItems()`: take into `stackToDistribute`.
    Distribute,
    /// `hasDistributionBehaviour() && !canTakeItems()`: refuse while holding or synced.
    Busy,
}

/// `BrassTunnelBlockEntity.getAdjacent`: the adjacent position across `axis` (`left` is
/// counter-clockwise from the axis' positive direction).
pub fn adjacent_pos(pos: [i32; 3], axis: Axis, left: bool) -> [i32; 3] {
    let side = match (axis, left) {
        (Axis::X, true) => Dir::North,
        (Axis::X, false) => Dir::South,
        (_, true) => Dir::East,
        (_, false) => Dir::West,
    };
    let o = side.offset();
    [pos[0] + o[0], pos[1], pos[2] + o[2]]
}

/// `BrassTunnelBlockEntity`'s `selectionMode.withCallback`: propagates `mode` across connected
/// brass tunnels. Records every tunnel whose mode changed.
pub fn set_group_mode(
    pos: [i32; 3],
    mode: Mode,
    tunnels: &mut std::collections::HashMap<[i32; 3], Brass>,
    info: &impl Fn([i32; 3]) -> Option<(Axis, bool)>,
    changed: &mut Vec<[i32; 3]>,
) {
    let Some(b) = tunnels.get_mut(&pos) else {
        return;
    };
    if b.mode == mode {
        return;
    }
    b.mode = mode;
    changed.push(pos);
    let (left, right) = (b.connected_left, b.connected_right);
    let Some((axis, _)) = info(pos) else {
        return;
    };
    for (is_left, connected) in [(true, left), (false, right)] {
        if !connected {
            continue;
        }
        let adj = adjacent_pos(pos, axis, is_left);
        if info(adj).is_some_and(|(ax, _)| ax == axis) {
            set_group_mode(adj, mode, tunnels, info, changed);
        }
    }
}

/// `BrassTunnelBlockEntity.updateTunnelConnections`: updates `connected_left`/`connected_right`
/// from `info` (`Some((axis, has_flaps))` for brass tunnels), notifies newly connected or
/// disconnected neighbours, and hands this tunnel's mode onto them (so a tunnel joining an
/// existing group takes the group's mode when the neighbour updates back).
pub fn update_connections(
    pos: [i32; 3],
    tunnels: &mut std::collections::HashMap<[i32; 3], Brass>,
    info: &impl Fn([i32; 3]) -> Option<(Axis, bool)>,
    changed: &mut Vec<[i32; 3]>,
) {
    let Some((axis, has_flaps)) = info(pos) else {
        return;
    };
    for is_left in [true, false] {
        let now_connected = has_flaps && {
            let adj = adjacent_pos(pos, axis, is_left);
            info(adj).is_some_and(|(ax, adj_flaps)| ax == axis && adj_flaps)
        };
        let was_connected = tunnels.get(&pos).map(|b| {
            if is_left {
                b.connected_left
            } else {
                b.connected_right
            }
        });
        if was_connected == Some(now_connected) {
            continue;
        }
        if let Some(b) = tunnels.get_mut(&pos) {
            if is_left {
                b.connected_left = now_connected;
            } else {
                b.connected_right = now_connected;
            }
            changed.push(pos);
        }
        let adj = adjacent_pos(pos, axis, is_left);
        if info(adj).is_some_and(|(ax, _)| ax == axis) {
            update_connections(adj, tunnels, info, changed);
            if let Some(mode) = tunnels.get(&pos).map(|b| b.mode) {
                set_group_mode(adj, mode, tunnels, info, changed);
            }
        }
    }
}

/// `BrassTunnelFilterSlot`: where a brass tunnel (axis `axis`, shape `shape`) shows the filter
/// slot of side `side`, in the block; None on a side without one (`isValidFaceForFilter`: its
/// open sides).
pub fn filter_slot(axis: Axis, shape: Shape, side: Dir) -> Option<[f64; 3]> {
    sides(axis, shape)
        .contains(&side)
        .then(|| crate::board::sided_offset([0.5, 13.0 / 16.0, 15.5 / 16.0], side))
}

/// What a player's click on side `side` of a brass tunnel (axis `axis`, shape `shape`), at
/// `local` in the block and holding `held` (None: an empty hand), does to its filters.
/// `ValueSettingsInputHandler.onBlockActivated` takes a click on the side's filter slot
/// whatever is held, then `FilteringBehaviour.onShortInteract` sets the filter unless
/// `canShortInteract` refuses the held item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterClick {
    /// Not on a filter slot: the click goes on to the block (`useItemOn`).
    Miss,
    /// On the slot with a wrench or a mechanical arm: the click is taken, the filter kept.
    Keep,
    /// On the slot: the held item becomes the side's filter (an empty hand clears it).
    Set,
}

/// `FilterClick` for a click (see there). The caller checks `ValueSettingsInputHandler`'s
/// `canInteract` (not sneaking, may build) first.
pub fn filter_click(
    axis: Axis,
    shape: Shape,
    side: Dir,
    local: [f64; 3],
    held: Option<&str>,
) -> FilterClick {
    let Some(slot) = filter_slot(axis, shape, side) else {
        return FilterClick::Miss;
    };
    if !crate::board::hits_box(slot, local) {
        return FilterClick::Miss;
    }
    if matches!(held, Some("create:wrench" | "create:mechanical_arm")) {
        FilterClick::Keep
    } else {
        FilterClick::Set
    }
}

/// `brassTunnelTimer`: ticks between distributions.
pub const BRASS_TIMER: i32 = 10;

/// `BrassTunnelBlockEntity.distribute`: hands `count` items out to `targets` outputs from
/// `previous_output` (a randomizing tunnel from `random`, any number), as the mode says;
/// `insert(target, count, simulate)` is `insertIntoTunnel` (None: that output takes nothing of
/// it, else what it leaves). The items left over; None when a forced mode found an output that
/// takes none of it, so nothing was handed out (`distribute` returns early, its
/// `previousOutputIndex` kept).
pub fn distribute(
    count: u8,
    targets: usize,
    previous_output: usize,
    mode: Mode,
    random: usize,
    mut insert: impl FnMut(usize, u8, bool) -> Option<u8>,
) -> Option<u8> {
    if targets == 0 {
        return Some(count);
    }
    let start = match mode {
        Mode::PreferNearest | Mode::Synchronize => 0,
        Mode::Randomize => random % targets,
        _ => previous_output % targets,
    };
    let force = matches!(mode, Mode::ForcedRoundRobin | Mode::ForcedSplit);
    let split = matches!(mode, Mode::ForcedSplit | Mode::Split);
    let robin = matches!(mode, Mode::ForcedRoundRobin | Mode::RoundRobin);
    let mut distributed: Vec<(usize, u8)> = Vec::new();
    let mut full: Vec<usize> = Vec::new();
    let mut to_distribute = count;
    for again in [true, false] {
        let mut this_cycle = to_distribute;
        let mut remaining = targets;
        let mut leftovers: u8 = 0;
        for simulate in [true, false] {
            if remaining == 0 {
                break;
            }
            leftovers = 0;
            let mut index = start;
            let size = to_distribute;
            let split_size = size as usize / remaining;
            let mut split_rem = (size as usize % remaining) as i32;
            this_cycle = to_distribute;
            if !(force || split) && simulate {
                continue;
            }
            let mut visited = 0;
            while visited < targets {
                let t = index;
                index = (index + 1) % targets;
                visited += 1;
                if full.contains(&t) {
                    if split && simulate {
                        remaining -= 1;
                    }
                    continue;
                }
                let n = if split {
                    (split_size + usize::from(split_rem > 0)) as u8
                } else {
                    size
                };
                // Grown by what it was already given, to tell whether it is full after it.
                let before = distributed.iter().find(|d| d.0 == t).map(|d| d.1);
                let output = n.saturating_add(before.unwrap_or(0));
                let left = insert(t, output, true);
                let whole = if before.is_some() { n + 1 } else { n };
                if left.is_none() || left == Some(whole) {
                    if force {
                        return None;
                    }
                    if split && simulate {
                        remaining -= 1;
                    }
                    if !simulate {
                        full.push(t);
                    }
                    if robin {
                        break;
                    }
                    continue;
                }
                let left = left.unwrap_or(0);
                if left > 0 && !simulate {
                    full.push(t);
                }
                if !simulate {
                    distributed.retain(|d| d.0 != t);
                    distributed.push((t, output - left));
                }
                leftovers += left;
                this_cycle = this_cycle.saturating_sub(n);
                if this_cycle == 0 {
                    break;
                }
                split_rem -= 1;
                if !split {
                    break;
                }
            }
        }
        to_distribute = this_cycle + leftovers;
        if leftovers == 0 && again {
            break;
        }
        if !split {
            break;
        }
    }
    let failed: u8 = distributed
        .iter()
        .map(|&(t, n)| insert(t, n, false).unwrap_or(n))
        .sum();
    Some(to_distribute + failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_clicks_take_only_their_open_sides_slots() {
        // Create's own run: a `t_left` tunnel along z has sides south, west and north
        // (`Sides:[3,4,2]`); its slot is 8, 13, 15.5 px on the south side turned to each.
        let (axis, shape) = (Axis::Z, Shape::TLeft);
        let east = [0.96875, 0.8125, 0.5];
        let west = [0.03125, 0.8125, 0.5];
        assert_eq!(filter_slot(axis, shape, Dir::East), None);
        let slot = filter_slot(axis, shape, Dir::West).unwrap();
        assert!((0..3).all(|i| (slot[i] - west[i]).abs() < 1e-9));
        let click = |side, at, held| filter_click(axis, shape, side, at, held);
        // No slot on the east side: a diamond there goes on to the block.
        assert_eq!(
            click(Dir::East, east, Some("minecraft:diamond")),
            FilterClick::Miss
        );
        assert_eq!(
            click(Dir::West, west, Some("minecraft:diamond")),
            FilterClick::Set
        );
        assert_eq!(click(Dir::West, west, None), FilterClick::Set);
        // `canShortInteract` refuses them, but the handler has taken the click.
        for held in ["create:wrench", "create:mechanical_arm"] {
            assert_eq!(click(Dir::West, west, Some(held)), FilterClick::Keep);
        }
        // Off the slot (its box is 0.25 across): the block's.
        assert_eq!(
            click(Dir::West, [0.03125, 0.5, 0.5], Some("minecraft:diamond")),
            FilterClick::Miss
        );
    }

    #[test]
    fn randomizing_and_synchronizing_tunnels_start_where_create_s_do() {
        // A randomizing tunnel hands the whole stack to the first output from its random start
        // that takes it; a synchronizing one starts at the nearest.
        let mut got = Vec::new();
        let left = distribute(5, 3, 1, Mode::Randomize, 7, |t, n, sim| {
            if !sim {
                got.push((t, n));
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(0), vec![(1, 5)]));
        let mut got = Vec::new();
        distribute(5, 3, 2, Mode::Synchronize, 7, |t, n, sim| {
            if !sim {
                got.push((t, n));
            }
            Some(0)
        });
        assert_eq!(got, vec![(0, 5)]);
    }

    #[test]
    fn a_brass_tunnel_hands_out_only_to_the_outputs_it_gathered() {
        let t = [0, 9, 2];
        let mut b = Brass {
            stack: Some(("minecraft:gold_ingot".into(), 16)),
            entered_from: Some(Dir::North),
            progress: -1,
            mode: Mode::ForcedRoundRobin,
            targets: Targets {
                filtered: Vec::new(),
                plain: vec![(t, Dir::West)],
            },
            ..Default::default()
        };
        assert_eq!(b.begin_tick(true), BrassPhase::Gather);
        assert!(b.targets.is_empty(), "gathered afresh");
        let gathered = Targets {
            filtered: Vec::new(),
            plain: vec![(t, Dir::East), (t, Dir::North)],
        };
        assert!(b.gathered(gathered));
        assert_eq!(b.progress, BRASS_TIMER);
        for _ in 283..292 {
            assert_eq!(b.begin_tick(true), BrassPhase::Wait);
        }
        assert_eq!(b.begin_tick(true), BrassPhase::Distribute);
        let lists = b.targets.to_hand_out(t, b.entered_from, |p| p == t);
        assert_eq!(lists, [vec![], vec![(t, Dir::East)]]);
        let lists = b.targets.to_hand_out(t, b.entered_from, |_| false);
        assert_eq!(lists, [vec![], vec![]]);
        let mut idle = Brass {
            stack: Some(("minecraft:gold_ingot".into(), 16)),
            ..Default::default()
        };
        assert!(!idle.gathered(Targets::default()));
        assert_eq!(idle.progress, -1);
    }

    #[test]
    fn a_brass_tunnel_splits_a_stack_evenly() {
        // Three outputs taking anything: 10 items as 4, 3, 3.
        let mut got = [0u8; 3];
        let left = distribute(10, 3, 0, Mode::Split, 0, |t, n, sim| {
            if !sim {
                got[t] += n;
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(0), [4, 3, 3]));
        // One output refusing: the other two share it.
        let mut got = [0u8; 3];
        let left = distribute(9, 3, 0, Mode::Split, 0, |t, n, sim| {
            if t == 1 {
                return None;
            }
            if !sim {
                got[t] += n;
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(0), [5, 0, 4]));
        // Outputs taking at most 2: what is left stays in the tunnel.
        let left = distribute(9, 3, 0, Mode::Split, 0, |_, n, _| Some(n.saturating_sub(2)));
        assert_eq!(left, Some(3));
    }

    #[test]
    fn forced_modes_hand_out_nothing_while_an_output_refuses() {
        // Forced split: one output full, nothing moves (Create returns before inserting).
        let mut got = Vec::new();
        let left = distribute(6, 3, 0, Mode::ForcedSplit, 0, |t, n, sim| {
            if t == 2 {
                return Some(n);
            }
            if !sim {
                got.push((t, n));
            }
            Some(0)
        });
        assert_eq!((left, got.len()), (None, 0));
        // Plain split hands it to the other two.
        let mut got = [0u8; 3];
        let left = distribute(6, 3, 0, Mode::Split, 0, |t, n, sim| {
            if t == 2 {
                return Some(n);
            }
            if !sim {
                got[t] += n;
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(0), [3, 3, 0]));
        // Forced round robin: the output in turn refusing holds the stack.
        let left = distribute(4, 2, 1, Mode::ForcedRoundRobin, 0, |t, n, _| {
            if t == 1 { Some(n) } else { Some(0) }
        });
        assert_eq!(left, None);
        // Round robin waits for the output in turn too, but the next turn moves on
        // (`if (robin) break`, then `previousOutputIndex++`).
        let mut got = Vec::new();
        let left = distribute(4, 2, 1, Mode::RoundRobin, 0, |t, n, sim| {
            if t == 1 {
                return Some(n);
            }
            if !sim {
                got.push((t, n));
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(4), vec![]));
    }

    #[test]
    fn shapes_follow_the_outputs_and_open_their_sides() {
        // A tunnel on a belt along x: south is its left.
        assert_eq!(shape_for(true, false, false), Shape::TLeft);
        assert_eq!(
            sides(Axis::X, Shape::TLeft),
            vec![Dir::South, Dir::West, Dir::East]
        );
        assert_eq!(
            sides(Axis::X, Shape::TRight),
            vec![Dir::West, Dir::North, Dir::East]
        );
        assert_eq!(
            sides(Axis::Z, Shape::TLeft),
            vec![Dir::South, Dir::West, Dir::North]
        );
        assert_eq!(sides(Axis::Z, Shape::Cross).len(), 4);
        assert_eq!(sides(Axis::Z, Shape::Window), vec![Dir::South, Dir::North]);
        // A closed window stays closed while it still is a straight tunnel with a window.
        let closed = (Axis::Z, Shape::Closed);
        assert_eq!(updated(closed, (Axis::Z, Shape::Window)), closed);
        assert_eq!(
            updated(closed, (Axis::Z, Shape::Cross)),
            (Axis::Z, Shape::Cross)
        );
        // Brass tunnels distribute with an open side across their axis not against a tunnel.
        assert!(distributes(Axis::X, Shape::TLeft, |_| false));
        assert!(!distributes(Axis::X, Shape::TLeft, |d| d == Dir::South));
        assert!(!distributes(Axis::X, Shape::Window, |_| false));
        assert_eq!(Mode::from_option(4), Some(Mode::PreferNearest));
        assert_eq!(
            (
                Mode::from_option(6),
                Mode::from_option(7),
                Mode::from_option(-1)
            ),
            (Some(Mode::Synchronize), None, None)
        );
        assert_eq!(OPTIONS[Mode::Randomize.index()], Mode::Randomize.label());
        assert_eq!(OPTIONS[Mode::RoundRobin.index()], Mode::RoundRobin.label());
        // Junctions split stacks moving along them.
        assert!(splits(Axis::X, Shape::Cross, Dir::East, 2));
        assert!(!splits(Axis::X, Shape::Cross, Dir::East, 1));
        assert!(!splits(Axis::X, Shape::Window, Dir::East, 5));
        // The open sides' walls start 10 px up; closed ones reach down over the belt.
        let b = boxes(Axis::X, Shape::TLeft);
        assert!(
            b.contains(&[2., 10., 14., 14., 16., 16.]) && b.contains(&[2., -5., 0., 14., 16., 2.])
        );
    }

    #[test]
    fn tunnel_joining_connected_group_takes_groups_mode() {
        // Observed in t3-brass-mode-on-join: a tunnel at [0, 9, 2] on a Z-axis belt is set to
        // RoundRobin, then a second tunnel at [1, 9, 2] is placed beside it; updating connections
        // from the new tunnel connects both and gives [1, 9, 2] the group's RoundRobin mode.
        let mut tunnels = std::collections::HashMap::new();
        let a = [0, 9, 2];
        let b = [1, 9, 2];
        tunnels.insert(
            a,
            Brass {
                mode: Mode::RoundRobin,
                ..Default::default()
            },
        );
        tunnels.insert(b, Brass::default());
        let info = |p: [i32; 3]| (p == a || p == b).then_some((Axis::Z, true));
        let mut changed = Vec::new();
        update_connections(b, &mut tunnels, &info, &mut changed);
        assert!(tunnels[&a].connected_left);
        assert!(!tunnels[&a].connected_right);
        assert!(!tunnels[&b].connected_left);
        assert!(tunnels[&b].connected_right);
        assert_eq!(tunnels[&a].mode, Mode::RoundRobin);
        assert_eq!(tunnels[&b].mode, Mode::RoundRobin);

        // Changing the mode on one updates the connected group.
        changed.clear();
        set_group_mode(b, Mode::Synchronize, &mut tunnels, &info, &mut changed);
        assert_eq!(tunnels[&a].mode, Mode::Synchronize);
        assert_eq!(tunnels[&b].mode, Mode::Synchronize);
    }

    #[test]
    fn synchronizing_tunnel_gathers_outputs_one_tick_after_syncing() {
        // Observed in t6-brass-sync-gather: a Synchronize brass tunnel receives 3 iron ingots at
        // tick 40 (new_item=true, progress=-1, synced=false). In Java, gatherValidOutputs checks
        // syncedOutputActive before syncSet sets it true, so progress stays -1 on tick 40, becomes
        // 2 on tick 41, 1 on tick 42, and distributes on tick 43.
        let mut b = Brass {
            stack: Some(("minecraft:iron_ingot".into(), 3)),
            new_item: true,
            mode: Mode::Synchronize,
            ..Default::default()
        };

        // Tick 40: syncs the group, but validOutputs was null so progress remains -1.
        assert_eq!(b.begin_tick(true), BrassPhase::Gather);
        let (sync_update, can_gather) = b.gather_sync(&[true]);
        assert_eq!(sync_update, Some(true));
        assert!(!can_gather);
        assert_eq!((b.progress, b.synced, b.new_item), (-1, true, true));

        // Tick 41: already synced, so it gathers outputs and sets progress = 2.
        assert_eq!(b.begin_tick(true), BrassPhase::Gather);
        let (_, can_gather) = b.gather_sync(&[true]);
        assert!(can_gather);
        b.schedule_distribution();
        assert_eq!((b.progress, b.synced, b.new_item), (2, true, false));

        // Tick 42: counts down to 1.
        assert_eq!(b.begin_tick(true), BrassPhase::Wait);
        assert_eq!(b.progress, 1);

        // Tick 43: counts down to 0 and distributes.
        assert_eq!(b.begin_tick(true), BrassPhase::Distribute);
        b.stack = None;
        b.progress = -1;

        // Tick 44: all empty, clears synced.
        assert_eq!(b.begin_tick(true), BrassPhase::Gather);
        let (sync_update, can_gather) = b.gather_sync(&[false]);
        assert_eq!(sync_update, Some(false));
        assert!(!can_gather);
        assert!(!b.synced);
    }

    #[test]
    fn non_distributing_tunnel_passes_handler_inserts_to_belt() {
        // Observed in t7-brass-item-handler-belt: a straight brass tunnel with nothing beside it
        // (`hasDistributionBehaviour() == false`) delegates item-handler inserts to its belt
        // capability, even if it holds or doesn't hold distribution state.
        let mut b = Brass::default();
        assert_eq!(b.handler_target(false), HandlerTarget::BeltBelow);
        assert_eq!(b.handler_target(true), HandlerTarget::Distribute);

        b.stack = Some(("minecraft:iron_ingot".into(), 3));
        assert_eq!(b.handler_target(false), HandlerTarget::BeltBelow);
        assert_eq!(b.handler_target(true), HandlerTarget::Busy);

        b.stack = None;
        b.synced = true;
        assert_eq!(b.handler_target(true), HandlerTarget::Busy);
    }

    #[test]
    fn saved_brass_preserves_index_progress_and_synced_across_reload() {
        // Observed in t8-brass-reload-index: a RoundRobin brass tunnel between two depots hands
        // 1 iron ingot to depot 0 (previous_output becomes 1). After a save and reload, a gold
        // ingot arrives and goes immediately to depot 1 instead of retrying full depot 0.
        let mut b = Brass {
            mode: Mode::RoundRobin,
            stack: Some(("minecraft:iron_ingot".into(), 1)),
            new_item: true,
            ..Default::default()
        };
        let mut depot0_full = false;
        let mut got = Vec::new();
        let left = distribute(1, 2, b.previous_output, b.mode, 0, |t, n, sim| {
            if t == 0 && depot0_full {
                return Some(n);
            }
            if !sim {
                if t == 0 {
                    depot0_full = true;
                }
                got.push((t, n));
            }
            Some(0)
        });
        assert_eq!((left, got), (Some(0), vec![(0, 1)]));
        b.stack = None;
        b.progress = -1;
        b.previous_output = (b.previous_output + 1) % 2;
        assert_eq!(b.previous_output, 1);

        let saved = b.to_saved();
        assert!(!saved.is_default());
        let mut reloaded = Brass::from_saved(saved);
        assert_eq!(reloaded.previous_output, 1);
        assert_eq!(reloaded.progress, -1);
        assert!(!reloaded.new_item);

        reloaded.stack = Some(("minecraft:gold_ingot".into(), 1));
        reloaded.new_item = true;
        assert_eq!(reloaded.begin_tick(true), BrassPhase::Gather);
        reloaded.schedule_distribution();
        assert_eq!(reloaded.progress, 2);
        assert_eq!(reloaded.begin_tick(true), BrassPhase::Wait);
        assert_eq!(reloaded.begin_tick(true), BrassPhase::Distribute);
        let mut got2 = Vec::new();
        let left2 = distribute(
            1,
            2,
            reloaded.previous_output,
            reloaded.mode,
            0,
            |t, n, sim| {
                if t == 0 && depot0_full {
                    return Some(n);
                }
                if !sim {
                    got2.push((t, n));
                }
                Some(0)
            },
        );
        assert_eq!((left2, got2), (Some(0), vec![(1, 1)]));

        // Mid-countdown progress, synced, and connections also round-trip; running covers synced.
        let mid = Brass {
            stack: None,
            entered_from: Some(Dir::North),
            progress: 7,
            new_item: true,
            previous_output: 2,
            mode: Mode::Synchronize,
            stack_progress: None,
            synced: true,
            connected_left: true,
            connected_right: false,
            targets: Targets {
                filtered: Vec::new(),
                plain: vec![([1, 8, 2], Dir::East)],
            },
        };
        let restored = Brass::from_saved(mid.to_saved());
        assert_eq!(restored.targets, mid.targets);
        assert_eq!(restored.progress, 7);
        assert_eq!(restored.previous_output, 2);
        assert!(restored.synced);
        assert!(restored.connected_left);
        assert_eq!(restored.entered_from, Some(Dir::North));
        assert!(!restored.new_item);
        assert!(restored.is_running());
    }
}
