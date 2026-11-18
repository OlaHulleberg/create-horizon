//! Level.blockEntityTickers registration order, with one indexed transport/fluid view.
//! Hash lookups and doubly linked slots make registration/removal O(1) on average;
//! column unload visits only that column. Walks borrow no world and copy no entries.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

type Pos = [i32; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    All,
    /// Belts, depots, tanks, valves, spouts, drains and pumps share their turns.
    Transport,
}

#[derive(Clone, Copy, Default)]
struct Links {
    previous: Option<usize>,
    next: Option<usize>,
}

struct Slot {
    pos: Pos,
    sequence: u64,
    live: bool,
    view: Option<View>,
    links: [Links; 2],
}

#[derive(Default)]
struct List {
    first: Option<usize>,
    last: Option<usize>,
}

#[derive(Default)]
pub(crate) struct BlockEntityOrder {
    positions: HashMap<Pos, usize>,
    columns: HashMap<(i32, i32), HashSet<Pos>>,
    slots: Vec<Slot>,
    free: Vec<usize>,
    lists: [List; 2],
    sequence: u64,
    /// Slots removed during a walk retain their forward links until all walks end.
    /// This lease prevents reuse while a paused walk could still reference them.
    walks: Rc<()>,
}

impl BlockEntityOrder {
    pub(crate) fn register(&mut self, pos: Pos, view: Option<View>) -> u64 {
        if let Some(&index) = self.positions.get(&pos) {
            return self.slots[index].sequence;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("entity order exhausted");
        let slot = Slot {
            pos,
            sequence: self.sequence,
            live: true,
            view,
            links: [Links::default(); 2],
        };
        let index = if Rc::strong_count(&self.walks) == 1 {
            self.free.pop()
        } else {
            None
        };
        let index = match index {
            Some(index) => {
                self.slots[index] = slot;
                index
            }
            None => {
                self.slots.push(slot);
                self.slots.len() - 1
            }
        };
        for view in [Some(View::All), view].into_iter().flatten() {
            let v = view as usize;
            let last = self.lists[v].last;
            self.slots[index].links[v].previous = last;
            if let Some(last) = last {
                self.slots[last].links[v].next = Some(index);
            } else {
                self.lists[v].first = Some(index);
            }
            self.lists[v].last = Some(index);
        }
        self.positions.insert(pos, index);
        self.columns
            .entry((pos[0] >> 4, pos[2] >> 4))
            .or_default()
            .insert(pos);
        self.sequence
    }

    pub(crate) fn registration_order(&self, pos: Pos) -> Option<u64> {
        self.positions
            .get(&pos)
            .map(|&index| self.slots[index].sequence)
    }

    pub(crate) fn remove(&mut self, pos: Pos) {
        let Some(index) = self.positions.remove(&pos) else {
            return;
        };
        for view in [Some(View::All), self.slots[index].view]
            .into_iter()
            .flatten()
        {
            let v = view as usize;
            let Links { previous, next } = self.slots[index].links[v];
            if let Some(previous) = previous {
                self.slots[previous].links[v].next = next;
            } else {
                self.lists[v].first = next;
            }
            if let Some(next) = next {
                self.slots[next].links[v].previous = previous;
            } else {
                self.lists[v].last = previous;
            }
        }
        self.slots[index].live = false;
        self.free.push(index);
        let column = (pos[0] >> 4, pos[2] >> 4);
        if let Some(positions) = self.columns.get_mut(&column) {
            positions.remove(&pos);
            if positions.is_empty() {
                self.columns.remove(&column);
            }
        }
    }

    pub(crate) fn unload(&mut self, cx: i32, cz: i32) {
        if let Some(positions) = self.columns.remove(&(cx, cz)) {
            for pos in positions {
                self.remove(pos);
            }
        }
    }

    /// Walk the registration prefix ending at an existing entity, without collecting it.
    pub(crate) fn walk_through(&self, view: View, pos: Pos) -> Walk {
        let mut walk = self.walk(view);
        walk.end = self.registration_order(pos).unwrap_or(0);
        walk
    }

    pub(crate) fn walk(&self, view: View) -> Walk {
        Walk {
            next: self.lists[view as usize].first,
            end: self.sequence,
            view,
            lease: Some(self.walks.clone()),
        }
    }
}

/// A resumable walk excludes later registrations and skips entities removed before their turn.
/// Removed slots keep their successor, so deleting the next entity cannot truncate the walk.
pub(crate) struct Walk {
    next: Option<usize>,
    end: u64,
    view: View,
    lease: Option<Rc<()>>,
}

impl Walk {
    pub(crate) fn next(&mut self, order: &BlockEntityOrder) -> Option<Pos> {
        while let Some(index) = self.next {
            let slot = &order.slots[index];
            if slot.sequence > self.end {
                self.next = None;
                self.lease = None;
                return None;
            }
            self.next = slot.links[self.view as usize].next;
            if slot.live {
                return Some(slot.pos);
            }
        }
        self.lease = None;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(order: &BlockEntityOrder, view: View) -> Vec<Pos> {
        let mut walk = order.walk(view);
        std::iter::from_fn(|| walk.next(order)).collect()
    }

    #[test]
    fn prefix_walk_keeps_its_boundary_when_the_target_is_replaced() {
        let mut order = BlockEntityOrder::default();
        let (a, b, c) = ([0, 0, 0], [1, 0, 0], [2, 0, 0]);
        for pos in [a, b, c] {
            order.register(pos, None);
        }
        let mut prefix = order.walk_through(View::All, b);
        assert_eq!(prefix.next(&order), Some(a));
        order.remove(b);
        order.register(b, None);
        assert_eq!(prefix.next(&order), None);
        let mut prefix = order.walk_through(View::All, b);
        assert_eq!(prefix.next(&order), Some(a));
        assert_eq!(prefix.next(&order), Some(c));
        assert_eq!(prefix.next(&order), Some(b));
        assert_eq!(prefix.next(&order), None);
    }

    #[test]
    fn state_change_keeps_slot_and_replacement_registers_at_end() {
        let mut order = BlockEntityOrder::default();
        let (a, b, c) = ([0, 0, 0], [1, 0, 0], [2, 0, 0]);
        let sequence = order.register(a, Some(View::Transport));
        order.register(b, None);
        order.register(c, Some(View::Transport));
        assert_eq!(order.register(a, Some(View::Transport)), sequence);
        assert_eq!(positions(&order, View::All), [a, b, c]);
        order.remove(a);
        assert!(order.register(a, Some(View::Transport)) > sequence);
        assert_eq!(positions(&order, View::All), [b, c, a]);
        assert_eq!(positions(&order, View::Transport), [c, a]);
    }

    #[test]
    fn paused_walk_survives_removal_replacement_and_unload() {
        let mut order = BlockEntityOrder::default();
        let (a, b, c, d) = ([0, 0, 0], [1, 0, 0], [16, 0, 0], [-1, 0, -1]);
        for p in [a, b, c, d] {
            order.register(p, Some(View::Transport));
        }
        let mut walk = order.walk(View::Transport);
        assert_eq!(walk.next(&order), Some(a));
        order.unload(0, 0);
        order.register(b, Some(View::Transport));
        assert_eq!(walk.next(&order), Some(c));
        order.remove(c);
        assert_eq!(walk.next(&order), Some(d));
        assert_eq!(walk.next(&order), None);
        drop(walk);
        let slots = order.slots.len();
        order.register(a, Some(View::Transport));
        assert_eq!(order.slots.len(), slots, "reuse slots after walks end");
        assert!(order.registration_order(a).unwrap() > order.registration_order(b).unwrap());
        order.unload(-1, -1);
        assert_eq!(positions(&order, View::All), [b, a]);
    }
}
