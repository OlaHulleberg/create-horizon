//! A depot's output buffer (`DepotBehaviour.processingOutputBuffer`, the glue's block entity
//! slots `depot::OUTPUTS`): eight slots that keep their places, so an item handler reaching
//! slot `k` (`DepotItemHandler`, slots 1..8) finds the stack the buffer put there.
use crate::transport::Stack;
use serde::{Deserialize, Serialize};

pub const SLOTS: usize = create_engine::depot::OUTPUTS.end - create_engine::depot::OUTPUTS.start;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outputs([Option<Stack>; SLOTS]);

impl Outputs {
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }
    /// The stacks it holds, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &Stack> {
        self.0.iter().flatten()
    }
    /// The stack in buffer slot `i` (the handler's slot `i + 1`).
    pub fn slot(&self, i: usize) -> Option<&Stack> {
        self.0.get(i)?.as_ref()
    }
    /// The glue's `processing::insert_stacked` (`ItemHandlerHelper.insertItemStacked`): onto
    /// stacks of the same item first, then into empty slots. The count that did not fit.
    pub fn insert_stacked(&mut self, s: &Stack) -> u8 {
        let mut slots: Vec<create_engine::processing::Slot> = self
            .0
            .iter()
            .map(|o| o.as_ref().map(|s| (s.item.clone(), s.count)))
            .collect();
        let left =
            create_engine::processing::insert_stacked(&mut slots, &s.item, s.count, s.limit());
        for (o, n) in self.0.iter_mut().zip(slots) {
            *o = n.map(|(item, count)| Stack { item, count });
        }
        left
    }
    /// Takes up to `n` out of buffer slot `i` (the glue's `take`); `simulate` leaves it there.
    pub fn take(&mut self, i: usize, n: u8, simulate: bool) -> Option<Stack> {
        let slot = self.0.get_mut(i)?;
        let held = slot.as_mut()?;
        let k = n.min(held.count);
        if k == 0 {
            return None;
        }
        let taken = Stack {
            item: held.item.clone(),
            count: k,
        };
        if !simulate {
            held.count -= k;
            if held.count == 0 {
                *slot = None;
            }
        }
        Some(taken)
    }
    /// Puts `s` back into buffer slot `i` (None empties it), as a slot write does.
    pub fn set(&mut self, i: usize, s: Option<Stack>) {
        if let Some(slot) = self.0.get_mut(i) {
            *slot = s.filter(|s| s.count > 0);
        }
    }
}

impl IntoIterator for Outputs {
    type Item = Stack;
    type IntoIter = std::iter::Flatten<std::array::IntoIter<Option<Stack>, SLOTS>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(item: &str, count: u8) -> Stack {
        Stack {
            item: item.into(),
            count,
        }
    }
    #[test]
    fn slots_keep_their_places() {
        let mut o = Outputs::default();
        assert_eq!(o.insert_stacked(&s("create:builders_tea", 16)), 0);
        assert_eq!(o.insert_stacked(&s("create:builders_tea", 3)), 0);
        assert_eq!(o.slot(1), Some(&s("create:builders_tea", 3)));
        assert_eq!(o.take(0, 64, false), Some(s("create:builders_tea", 16)));
        assert_eq!(o.slot(0), None);
        assert_eq!(o.slot(1), Some(&s("create:builders_tea", 3)));
        // A new stack goes onto the same item first, then into the first empty slot.
        assert_eq!(o.insert_stacked(&s("create:builders_tea", 14)), 0);
        assert_eq!(o.slot(1), Some(&s("create:builders_tea", 16)));
        assert_eq!(o.slot(0), Some(&s("create:builders_tea", 1)));
        assert_eq!(o.take(1, 4, true), Some(s("create:builders_tea", 4)));
        assert_eq!(o.slot(1).map(|s| s.count), Some(16));
    }
}
