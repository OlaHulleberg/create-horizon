//! Create's item drain (`ItemDrainBlockEntity`, `GenericItemEmptying`): a stack put on it
//! travels across it the way it came in, an eighth of a block a tick; an item that holds a fluid
//! stops in the middle, is emptied into the 1,500 mB tank over a 20-tick cycle (waiting while
//! the tank has no room) and goes on as what is left of it; at the far side the stack is handed
//! on (`DirectBeltInputBehaviour`) or thrown off.

use crate::kinetics::Dir;
use crate::spout::FILLING_TIME;
use crate::tank::{BUCKET, Fluid};

/// `SmartFluidTankBehaviour.single(this, 1500)`.
pub const CAPACITY: u32 = 1500;

/// `itemMovementPerTick`.
pub const SPEED: f32 = 1.0 / 8.0;

/// The stack on a drain (`heldItem`, a `TransportedItemStack`).
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub item: String,
    pub count: u8,
    /// How far across it is (`beltPosition`), 0 to 1, and where it was a tick ago.
    pub position: f32,
    pub prev: f32,
    /// The way it travels (`insertedFrom`): it leaves by that side.
    pub from: Dir,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drain {
    pub held: Option<Held>,
    /// `processingTicks`: counting down while an item is emptied.
    pub processing: i32,
    pub fluid: Option<(Fluid, u32)>,
}

/// What emptying an item gives (`GenericItemEmptying`): a fluid and how much, and what is left
/// of the item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emptying {
    pub fluid: Fluid,
    pub amount: u32,
    pub leftover: Option<String>,
}

/// `GenericItemEmptying.emptyItem`: a potion's quarter bucket leaving its glass bottle
/// (`PotionFluidHandler.emptyPotion`), a `create:emptying` recipe for the item (`recipes`, by
/// item), else a filled bucket's worth leaving the bucket.
pub fn contents(item: &str, recipes: &[(String, Emptying)]) -> Option<Emptying> {
    if let Some(fluid) = crate::tank::potion_fluid(item) {
        return Some(Emptying {
            fluid,
            amount: crate::spout::BOTTLE,
            leftover: Some("minecraft:glass_bottle".into()),
        });
    }
    if let Some((_, e)) = recipes.iter().find(|(i, _)| i == item) {
        return Some(e.clone());
    }
    Fluid::of_bucket(item).map(|fluid| Emptying {
        fluid,
        amount: BUCKET,
        leftover: Some("minecraft:bucket".into()),
    })
}

/// `tryInsertingFromSide`: `count` of `item` coming in travelling toward `side`. Only while the
/// drain holds nothing; of a stack that can be emptied it takes one. How many it took.
pub fn insert(
    d: &mut Drain,
    item: &str,
    count: u8,
    side: Dir,
    recipes: &[(String, Emptying)],
) -> u8 {
    if d.held.is_some() || count == 0 {
        return 0;
    }
    let taken = if count > 1 && contents(item, recipes).is_some() {
        1
    } else {
        count
    };
    let position = if side.axis() == crate::kinetics::Axis::Y {
        0.5
    } else {
        0.0
    };
    d.held = Some(Held {
        item: item.to_owned(),
        count: taken,
        position,
        prev: position,
        from: side,
    });
    taken
}

/// What a tick did that the world must know of.
#[derive(Clone, Debug, PartialEq)]
pub enum Tick {
    /// Nothing to tell (it moved, or waits).
    Idle,
    /// The held item was emptied into the tank (`notifyUpdate`).
    Emptied,
    /// The stack reached the far side: hand it on that way, or throw it off.
    Out(Dir),
}

/// `tick` and `continueProcessing` on the server.
pub fn tick(d: &mut Drain, recipes: &[(String, Emptying)]) -> Tick {
    let Some(held) = d.held.as_mut() else {
        d.processing = 0;
        return Tick::Idle;
    };
    if d.processing > 0 {
        held.prev = 0.5;
        d.processing -= 1;
        if d.processing < 5 {
            return Tick::Idle;
        }
        let Some(e) = contents(&held.item, recipes) else {
            d.processing = 0;
            return Tick::Idle;
        };
        let room = d
            .fluid
            .is_none_or(|(f, n)| f == e.fluid && n + e.amount <= CAPACITY);
        if d.processing > 5 {
            if !room {
                d.processing = FILLING_TIME;
            }
            return Tick::Idle;
        }
        if !room {
            d.processing = FILLING_TIME;
            return Tick::Idle;
        }
        d.fluid = Some((e.fluid, d.fluid.map_or(0, |f| f.1) + e.amount));
        match e.leftover {
            Some(left) => held.item = left,
            None => d.held = None,
        }
        return Tick::Emptied;
    }
    held.prev = held.position;
    held.position += SPEED;
    if held.position > 1.0 {
        held.position = 1.0;
        return Tick::Out(held.from);
    }
    if held.prev < 0.5 && held.position >= 0.5 {
        if contents(&held.item, recipes).is_none() {
            return Tick::Idle;
        }
        held.position = 0.5;
        d.processing = FILLING_TIME;
    }
    Tick::Idle
}

/// Drained by a pump: up to `amount`.
pub fn drain(d: &mut Drain, amount: u32) -> Option<(Fluid, u32)> {
    let (fluid, have) = d.fluid?;
    let out = amount.min(have);
    d.fluid = (have > out).then_some((fluid, have - out));
    (out > 0).then_some((fluid, out))
}

/// Whether the drain's stack moves this tick (clients draw it each frame).
pub fn moving(d: &Drain) -> bool {
    d.held
        .as_ref()
        .is_some_and(|h| h.position != h.prev || d.processing > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(d: &mut Drain, recipes: &[(String, Emptying)], ticks: usize) -> Vec<Tick> {
        (0..ticks)
            .map(|_| tick(d, recipes))
            .filter(|t| *t != Tick::Idle)
            .collect()
    }

    #[test]
    fn a_bucket_crosses_is_emptied_in_the_middle_and_leaves() {
        let mut d = Drain::default();
        assert_eq!(insert(&mut d, "minecraft:water_bucket", 2, Dir::East, &[]), 1);
        assert_eq!(insert(&mut d, "minecraft:stone", 1, Dir::East, &[]), 0);
        // Four ticks to the middle, the 20-tick cycle emptying at its 5th-last.
        let events = run(&mut d, &[], 4 + 15);
        assert_eq!(events, vec![Tick::Emptied]);
        assert_eq!(d.fluid, Some((Fluid::Water, 1000)));
        assert_eq!(d.held.as_ref().unwrap().item, "minecraft:bucket");
        assert_eq!(d.held.as_ref().unwrap().position, 0.5);
        // The cycle's end, then four more ticks to the far side.
        let events = run(&mut d, &[], 5 + 5);
        assert_eq!(events, vec![Tick::Out(Dir::East)]);
    }

    #[test]
    fn it_waits_for_room_and_passes_other_items() {
        let mut d = Drain {
            fluid: Some((Fluid::Water, 1000)),
            ..Drain::default()
        };
        insert(&mut d, "minecraft:water_bucket", 1, Dir::North, &[]);
        assert!(run(&mut d, &[], 200).is_empty(), "no room: it waits");
        assert_eq!(drain(&mut d, 600), Some((Fluid::Water, 600)));
        assert_eq!(run(&mut d, &[], 40)[0], Tick::Emptied);
        let mut d = Drain::default();
        insert(&mut d, "minecraft:stone", 5, Dir::West, &[]);
        assert_eq!(d.held.as_ref().unwrap().count, 5);
        assert_eq!(run(&mut d, &[], 9), vec![Tick::Out(Dir::West)]);
    }

    #[test]
    fn honey_bottles_empty_by_their_recipe() {
        // data/create/recipe/emptying/honey_bottle.json
        let recipes = vec![(
            "minecraft:honey_bottle".to_owned(),
            Emptying {
                fluid: Fluid::Honey,
                amount: 250,
                leftover: Some("minecraft:glass_bottle".into()),
            },
        )];
        let mut d = Drain::default();
        assert_eq!(insert(&mut d, "minecraft:honey_bottle", 7, Dir::South, &recipes), 1);
        assert_eq!(run(&mut d, &recipes, 30)[0], Tick::Emptied);
        assert_eq!(d.fluid, Some((Fluid::Honey, 250)));
        assert_eq!(d.held.as_ref().unwrap().item, "minecraft:glass_bottle");
    }
}
