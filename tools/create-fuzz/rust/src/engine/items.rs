//! Item entities that join the world, as the glue spawns them through the host
//! (glue/logistics/items.rs `drop_item`, and the host's loot for `destroyBlock(pos, true)`).
//! The harness discards every item entity at the end of each server tick
//! (`CaseIsolation.disposeUnobservedDrops`). One a block entity spawned joined after the
//! level's entity pass and is observed where and how it was put; one an input put before the
//! level tick flew a tick first, which the driver does not follow.
use super::*;
use crate::transport::{DroppedItem, Placed, Stack};
use create_engine::{Block, belt};

pub(super) struct ItemEntity {
    stack: Stack,
    at: [f64; 3],
    motion: [f64; 3],
    placed: Placed,
}

impl Run {
    /// `count` of `item` at `at`, moving `motion` blocks a tick.
    pub(super) fn spawn_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8) {
        let placed = if self.entity_pass_done {
            Placed::Exact
        } else {
            Placed::Flown
        };
        self.join(at, motion, item, count, placed);
    }

    /// `Block.popResource` at `p`: somewhere inside the block.
    pub(super) fn pop_resource(&mut self, p: Pos, item: &str, count: u8) {
        let at = p.map(|v| f64::from(v) + 0.5);
        self.join(at, [0.0; 3], item, count, Placed::Spread);
    }

    /// `Containers.dropItemStack` at `at`: somewhere inside the block `at` is in.
    pub(super) fn drop_item_stack(&mut self, at: [f64; 3], item: &str, count: u8) {
        let p = at.map(|v| v.floor() as i32);
        self.pop_resource(p, item, count);
    }

    fn join(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8, placed: Placed) {
        if count == 0 || item == "minecraft:air" {
            return;
        }
        self.item_entities.push(ItemEntity {
            stack: Stack {
                item: item.into(),
                count,
            },
            at,
            motion,
            placed,
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
                placed: Some(e.placed),
            });
        }
        self.entity_pass_done = false;
    }

    /// `destroyBlock(pos, true)`, by the actor or not: the block's loot, then air. The actor
    /// drops nothing in creative mode, nor with its empty hand from a block that needs a tool.
    pub(super) fn break_block(&mut self, p: Pos, by_player: bool) {
        let state = self.at(p);
        if state == AIR {
            return;
        }
        let name = self.name(state);
        let kept = by_player && (self.creative || crate::loot::requires_tool(&name) == Some(true));
        let loot = if kept {
            Some(Vec::new())
        } else {
            crate::loot::drops(&name)
        };
        match loot {
            Some(drops) => {
                for (item, count) in drops {
                    self.pop_resource(p, &item, count);
                }
                // `BeltBlock.getDrops`: a pulley's shaft too.
                if !kept
                    && let Some((Block::Belt, i)) = self.w.table.get(state)
                    && i.belt.is_some_and(|s| s.part != belt::Part::Middle)
                {
                    for (item, count) in crate::loot::drops("create:shaft").unwrap_or_default() {
                        self.pop_resource(p, &item, count);
                    }
                }
            }
            None => self
                .notes
                .push(format!("adapter gap: loot of {name} at {p:?}")),
        }
        self.set(p, AIR);
    }

    /// What block entities the engine removed drop (`destroy`): a belt's items thrown off it,
    /// a depot's merging stacks and an ejector's stacks in flight.
    pub(super) fn flush_block_drops(&mut self) {
        let d = &mut self.w.dims[0];
        let belts = std::mem::take(&mut d.belt_drops);
        let depots = std::mem::take(&mut d.depot_drops);
        let ejectors = std::mem::take(&mut d.ejector_drops);
        for (c, seg, length, item) in belts {
            let (at, motion) = belt::eject(c, &seg, length, 0.0, item.position);
            self.spawn_item(at, motion, &item.item, item.count);
        }
        for (p, item, count) in depots {
            self.pop_resource(p, &item, count);
        }
        for (at, motion, item, count) in ejectors {
            self.spawn_item(at, motion, &item, count);
        }
    }
}
