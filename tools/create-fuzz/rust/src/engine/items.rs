//! Item entities Create puts into the world, as the glue does through the host
//! (glue/logistics/items.rs `drop_item`: `new ItemEntity(..)`, `setDeltaMovement`). The harness
//! discards every item entity at the end of the server tick that spawned it
//! (`CaseIsolation.disposeUnobservedDrops`), before one ticks: none flies, lands or is picked
//! up, so each is observed where and how it was put.
use super::*;
use crate::transport::{DroppedItem, Stack};

pub(super) struct ItemEntity {
    stack: Stack,
    at: [f64; 3],
    motion: [f64; 3],
}

impl Run {
    /// `items::drop_item`: `count` of `item` at `at`, moving `motion` blocks a tick.
    pub(super) fn spawn_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8) {
        if count == 0 {
            return;
        }
        self.item_entities.push(ItemEntity {
            stack: Stack {
                item: item.into(),
                count,
            },
            at,
            motion,
        });
    }

    /// The end of the server tick: the harness discards the item entities.
    pub(super) fn dispose_item_entities(&mut self) {
        let tick = self.now;
        for e in std::mem::take(&mut self.item_entities) {
            self.drops.push(DroppedItem {
                tick,
                stack: e.stack,
                pos: e.at,
                motion: e.motion,
            });
        }
    }
}
