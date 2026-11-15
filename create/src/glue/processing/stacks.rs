//! Item stacks as the engine's rules see them (an item id and a count), and the reads and
//! writes of block entity slots, item entities and players' hands they need.

use crate::glue::server::ServerLevel;
use create_engine::processing::Slot;
use mod_api::{
    Compound, EntityRef, Host, HostTransaction, Identifier, ItemEntityRef, ItemKey, ItemStack,
    PlayerRef, Tag,
};

/// The key of item `id` (`minecraft:stone`).
pub fn key(host: &dyn Host, id: &str) -> Option<ItemKey> {
    host.item_key(&Identifier::parse(id).ok()?)
}

/// The id of a stack's item.
pub fn id(host: &dyn Host, stack: &ItemStack) -> String {
    stack.item().id(host)
}

/// A plain stack of `count` of `id` (no components); None for an unknown item or none.
pub fn stack(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    ItemStack::new(key(host, id)?, count)
}

/// `getCraftingRemainingItem` of item `id` (a bucket's for a water bucket), by its id.
pub fn remainder(host: &dyn Host, id: &str) -> Option<String> {
    let r = host.item_crafting_remainder(&stack(host, id, 1)?)?;
    Some(self::id(host, &r))
}

/// An item's stack limit, 1 to 64 (`getMaxStackSize`; 64 for an unknown one).
pub fn limit(host: &dyn Host, id: &str) -> u8 {
    key(host, id)
        .and_then(|k| host.max_stack_size(k))
        .map_or(64, |n| n.clamp(1, 64) as u8)
}

/// A slot as the processing rules see it.
pub fn slot_of(host: &dyn Host, s: &Option<ItemStack>) -> Slot {
    s.as_ref().map(|s| (id(host, s), s.count().min(255) as u8))
}

/// The engine's slot as a plain stack.
pub fn from_slot(host: &dyn Host, s: &Slot) -> Option<ItemStack> {
    s.as_ref().and_then(|(i, n)| stack(host, i, u32::from(*n)))
}

/// Whether item `id` is in the item tag `tag`.
pub fn item_tagged(host: &dyn Host, id: &str, tag: &str) -> bool {
    let (Some(item), Some(tag)) = (
        key(host, id),
        Identifier::parse(tag)
            .ok()
            .and_then(|t| host.tags().key(&t)),
    ) else {
        return false;
    };
    host.tags().is_in_tag(item, tag).unwrap_or(false)
}

/// `AllDataComponents.SEQUENCED_ASSEMBLY` of a stack (kept in `minecraft:custom_data`).
const SEQUENCED_KEY: &str = "create:sequenced_assembly";
const CUSTOM_DATA: &str = "minecraft:custom_data";

/// The sequenced assembly a stack is part way through: the recipe and the step it is at.
pub fn sequenced_progress(stack: &ItemStack) -> Option<create_engine::sequenced::Progress> {
    let nbt = stack.components()?.as_nbt();
    let Some(Tag::Compound(custom)) = nbt.get(&CUSTOM_DATA.into()) else {
        return None;
    };
    let Some(Tag::Compound(c)) = custom.get(&SEQUENCED_KEY.into()) else {
        return None;
    };
    let id = match c.get(&"id".into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => return None,
    };
    let step = match c.get(&"step".into()) {
        Some(Tag::Int(n)) => (*n).max(0) as u32,
        _ => return None,
    };
    Some(create_engine::sequenced::Progress { id, step })
}

/// The stack marked with assembly `id` at `step` (`progress` the fraction of all its steps).
pub fn set_sequenced_progress(stack: &mut ItemStack, id: &str, step: u32, progress: f32) {
    let mut c = Compound::new();
    c.insert("id".into(), Tag::String(id.into()));
    c.insert("step".into(), Tag::Int(step as i32));
    c.insert("progress".into(), Tag::Float(progress.to_bits()));
    let mut custom = stack
        .components()
        .and_then(|c| match c.as_nbt().get(&CUSTOM_DATA.into()) {
            Some(Tag::Compound(d)) => Some(d.clone()),
            _ => None,
        })
        .unwrap_or_default();
    custom.insert(SEQUENCED_KEY.into(), Tag::Compound(c));
    let patch = Compound::from([(CUSTOM_DATA.into(), Tag::Compound(custom))]);
    let _ = stack.apply_components(&patch);
}

impl ServerLevel<'_> {
    /// The first `n` item slots of the block entity at `pos`; None without a block entity with
    /// slots there.
    pub fn slots(&self, pos: [i32; 3], n: usize) -> Option<Vec<Option<ItemStack>>> {
        let entity = self.entity_at(pos)?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(entity.item_slot(&*self.host, i as u32).ok()?);
        }
        Some(out)
    }

    /// Slot `slot` of the block entity at `pos`.
    pub fn slot(&self, pos: [i32; 3], slot: usize) -> Option<ItemStack> {
        self.entity_at(pos)?
            .item_slot(&*self.host, slot as u32)
            .ok()
            .flatten()
    }

    /// Writes `changes` (slot, stack) into the block entity at `pos`, all or nothing. False
    /// when there is no such block entity or a write is refused.
    pub fn set_slots(&mut self, pos: [i32; 3], changes: &[(usize, Option<ItemStack>)]) -> bool {
        let Some(entity) = self.entity_at(pos) else {
            return false;
        };
        self.host
            .transaction(|host, tx| {
                for (slot, stack) in changes {
                    entity
                        .set_item_slot(host, tx, *slot as u32, stack.clone())
                        .map_err(|_| ())?;
                }
                Ok::<(), ()>(())
            })
            .is_ok()
    }

    /// Reads slots `from..from + n` of `pos` as the engine's slots.
    pub fn read_slots(&self, pos: [i32; 3], from: usize, n: usize) -> Option<Vec<Slot>> {
        let all = self.slots(pos, from + n)?;
        Some(
            all[from..]
                .iter()
                .map(|s| slot_of(&*self.host, s))
                .collect(),
        )
    }

    /// An item entity of `stack` at `at` moving at `motion` (`summon minecraft:item`).
    pub fn spawn_stack(&mut self, at: [f64; 3], stack: ItemStack, motion: [f64; 3]) {
        let _ = self.level.spawn_item_with_motion(
            &mut *self.host,
            at[0],
            at[1],
            at[2],
            stack,
            motion,
            0,
        );
    }

    /// An item entity of `count` of `item` at `at`, still.
    pub fn spawn_items(&mut self, at: [f64; 3], item: &str, count: u32) {
        self.spawn_moving(at, item, count, [0.0; 3]);
    }

    /// An item entity of `count` of `item` at `at` moving at `motion`.
    pub fn spawn_moving(&mut self, at: [f64; 3], item: &str, count: u32, motion: [f64; 3]) {
        if let Some(s) = stack(&*self.host, item, count) {
            self.spawn_stack(at, s, motion);
        }
    }

    /// Gives `stack` to `player` (`Inventory.placeItemBackInInventory`: what doesn't fit drops).
    pub fn give(&mut self, player: PlayerRef, stack: ItemStack) {
        let _ = player.give_stack(&mut *self.host, stack);
    }

    /// The stack in `player`'s main hand.
    pub fn held(&self, player: PlayerRef) -> Option<ItemStack> {
        let slot = player.selected_slot(&*self.host)?;
        player
            .inventory_slots(&*self.host)?
            .into_iter()
            .find(|(i, _)| *i as usize == slot)
            .map(|(_, s)| s)
    }

    /// Puts `stack` in `player`'s main hand.
    pub fn set_held(&mut self, player: PlayerRef, stack: Option<ItemStack>) {
        let Some(slot) = player.selected_slot(&*self.host) else {
            return;
        };
        let _ = player.set_inventory_slot(&mut *self.host, slot as u32, stack);
    }

    /// `n` of the main hand's stack used up.
    pub fn take_held(&mut self, player: PlayerRef, n: u32) {
        let Some(held) = self.held(player) else {
            return;
        };
        let left = held.count().saturating_sub(n);
        let next = if left == 0 {
            None
        } else {
            let mut s = held;
            let _ = s.set_count(left);
            Some(s)
        };
        self.set_held(player, next);
    }

    /// Whether `player` is in creative mode.
    pub fn creative(&self, player: PlayerRef) -> bool {
        player.is_creative(&*self.host).unwrap_or(false)
    }

    /// The item entities whose position is in `lo`..`hi` (`Level.getEntitiesOfClass(ItemEntity
    /// .class, box)`, tested by position).
    pub fn items_in(&self, lo: [f64; 3], hi: [f64; 3]) -> Vec<Dropped> {
        let host = &*self.host;
        let grown = (lo.map(|v| v - 0.25), hi.map(|v| v + 0.25));
        self.level
            .entities_in(host, grown.0, grown.1)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entity| {
                let item = host.item_entity(entity)?;
                let at = entity.position(host)?;
                if !create_engine::belt::item_touches(at, lo, hi) {
                    return None;
                }
                Some(Dropped {
                    entity,
                    item,
                    stack: item.stack(host)?,
                    at,
                    velocity: entity.velocity(host).unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Changes a dropped item: its stack (None: it is gone) and its motion.
    pub fn set_dropped(&mut self, d: &Dropped, stack: Option<ItemStack>, motion: [f64; 3]) {
        match stack {
            None => {
                d.entity.discard(&mut *self.host);
            }
            Some(s) => {
                if s != d.stack {
                    let _ = d.item.set_stack(&mut *self.host, s);
                }
                if motion != d.velocity {
                    d.entity.set_velocity(&mut *self.host, motion);
                }
            }
        }
    }
}

/// An item entity as the machines see it.
pub struct Dropped {
    pub entity: EntityRef,
    pub item: ItemEntityRef,
    pub stack: ItemStack,
    pub at: [f64; 3],
    pub velocity: [f64; 3],
}

impl Dropped {
    /// Its id (the network id progress is kept by).
    pub fn id(&self) -> u32 {
        self.entity.id()
    }
}
