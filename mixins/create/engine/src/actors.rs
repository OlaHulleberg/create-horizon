//! Create's movement behaviours ("actors") on moving contraptions (`MovementBehaviour`,
//! `MovementContext`, `AbstractContraptionEntity.tickActors`): drills and saws breaking what
//! they pass (`BlockBreakingMovementBehaviour`, `DrillMovementBehaviour`,
//! `SawMovementBehaviour`), harvesters (`HarvesterMovementBehaviour`), ploughs
//! (`PloughMovementBehaviour`); a contraption's storage taking what they collect
//! (`MovementBehaviour.collectOrDropItem`, `ItemHandlerHelper.insertItem`); and contraption
//! controls turning kinds of actor off (`ContraptionControlsMovingInteraction`). Actors tick only
//! while their contraption moves: a stopped contraption has none.

use crate::kinetics::Dir;

/// The movement behaviours ported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Drill,
    Saw,
    Harvester,
    Plough,
}

impl Kind {
    /// The actor of a Create block, if it has one (`MovementBehaviour.REGISTRY`).
    pub fn of(block: crate::Block) -> Option<Kind> {
        Some(match block {
            crate::Block::MechanicalDrill => Kind::Drill,
            crate::Block::MechanicalSaw => Kind::Saw,
            crate::Block::MechanicalHarvester => Kind::Harvester,
            crate::Block::MechanicalPlough => Kind::Plough,
            _ => return None,
        })
    }

    /// `getActiveAreaOffset`: how far ahead of the block's centre along its facing it acts.
    pub fn reach(self) -> f64 {
        match self {
            Kind::Drill | Kind::Saw => 0.65,
            Kind::Harvester | Kind::Plough => 0.45,
        }
    }

    /// The item that turns it off on contraption controls (`canBeDisabledVia`: its block).
    pub fn item(self) -> &'static str {
        match self {
            Kind::Drill => "create:mechanical_drill",
            Kind::Saw => "create:mechanical_saw",
            Kind::Harvester => "create:mechanical_harvester",
            Kind::Plough => "create:mechanical_plough",
        }
    }

    fn breaks(self) -> bool {
        !matches!(self, Kind::Harvester)
    }
}

/// What a moving contraption carries besides its blocks (`Contraption.actors`, `superglue`,
/// `disabledActors`, `stalled`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Carried {
    pub actors: Vec<Actor>,
    /// The glue it took, offsets as its blocks.
    pub glue: Vec<crate::glue::Glue>,
    pub disabled: Vec<Filter>,
    pub stalled: bool,
}

/// The block a breaker works on (`BreakingPos`, `Progress`, `TicksUntilNextProgress`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breaking {
    pub pos: [i32; 3],
    pub progress: i32,
    pub ticks_until: i32,
}

/// An actor on a contraption and its `MovementContext`.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// Its block in the contraption (as the contraption's blocks are kept).
    pub local: [i32; 3],
    pub kind: Kind,
    pub facing: Dir,
    pub position: Option<[f64; 3]>,
    pub motion: [f64; 3],
    pub relative_motion: [f64; 3],
    pub stall: bool,
    pub first_movement: bool,
    pub disabled: bool,
    pub breaking: Option<Breaking>,
    /// `WaitingTicks`, `LastPos`: a falling block broke; it looks there again after 10 ticks.
    pub waiting: Option<(i32, [i32; 3])>,
    /// `BreakerId` (`-NEXT_BREAKER_ID.incrementAndGet()`), for its cracks.
    pub breaker_id: i32,
}

impl Actor {
    /// `startMoving` (and `MovementContext`'s defaults: `firstMovement`).
    pub fn new(local: [i32; 3], kind: Kind, facing: Dir, breaker_id: i32) -> Actor {
        Actor {
            local,
            kind,
            facing,
            position: None,
            motion: [0.0; 3],
            relative_motion: [0.0; 3],
            stall: false,
            first_movement: true,
            disabled: false,
            breaking: None,
            waiting: None,
            breaker_id,
        }
    }
}

/// What an actor's tick asks of the world, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// `destroyBlockProgress`: the breaker's cracks at a block (0..=9), or none (-1).
    Crack([i32; 3], i32),
    /// The block's hit sound at a quarter volume.
    Hit([i32; 3]),
    /// `destroyBlock`: the block breaks; its drops are collected or dropped.
    Destroy([i32; 3]),
    /// `HarvesterMovementBehaviour.visitNewPosition` at a block.
    Harvest([i32; 3]),
    /// The plough's hoe used on the top of the block (`useOn`).
    Till([i32; 3]),
}

/// What an actor reads of the world.
pub trait Probe {
    /// `canBreak` for this kind of breaker, and the block's hardness when it may.
    fn breakable(&self, kind: Kind, pos: [i32; 3]) -> Option<f32>;
    /// Whether the block is a `FallingBlock` (sand, gravel, concrete powder, anvils...).
    fn falling(&self, pos: [i32; 3]) -> bool;
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `VecHelper.isVecPointingTowards`: the normalized vector within .75 of the direction.
pub fn pointing_towards(v: [f64; 3], d: Dir) -> bool {
    let l = length(v);
    let n = if l < 1.0e-4 {
        [0.0; 3]
    } else {
        v.map(|c| c / l)
    };
    let o = d.offset().map(f64::from);
    length([n[0] - o[0], n[1] - o[1], n[2] - o[2]]) < 0.75
}

/// `MovementContext.getAnimationSpeed` on the server.
pub fn animation_speed(motion: [f64; 3], disabled: bool) -> f32 {
    if disabled {
        return 0.0;
    }
    let length = -length(motion);
    if length.abs() < 1.0 / 512.0 {
        return 0.0;
    }
    (((length * 1000.0 + 100.0 * length.signum()) as i32) / 100 * 100) as f32
}

/// `getBlockBreakingSpeed`: `lower` is 1/128 on a piston, gantry or bearing (1 on a minecart,
/// 2 on a train).
pub fn break_speed(motion: [f64; 3], disabled: bool, lower: f32) -> f32 {
    (animation_speed(motion, disabled).abs() / 500.0).clamp(lower, 16.0)
}

/// The lowest breaking speed of a contraption a block entity controls.
pub const CONTROLLED_LOWER: f32 = 1.0 / 128.0;

/// Where an actor acts: its block's centre plus its reach along its facing, in the contraption,
/// moved by `translation` (blocks).
pub fn translated_position(a: &Actor, translation: [f64; 3]) -> [f64; 3] {
    let o = a.facing.offset();
    [0, 1, 2]
        .map(|i| f64::from(a.local[i]) + 0.5 + f64::from(o[i]) * a.kind.reach() + translation[i])
}

impl Actor {
    /// `shouldActorTrigger`: the motion since the last tick; a new block reached, or the first
    /// movement.
    fn trigger(&mut self, position: [f64; 3], grid: [i32; 3]) -> bool {
        let Some(previous) = self.position else {
            return false;
        };
        self.motion = [0, 1, 2].map(|i| position[i] - previous[i]);
        // Translating contraptions don't turn: the relative motion is the motion.
        self.relative_motion = self.motion;
        previous.map(|v| v.floor() as i32) != grid
            || (length(self.relative_motion) > 0.0 && self.first_movement)
    }

    /// `isActive`: not turned off, and not moving away from its facing.
    pub fn active(&self) -> bool {
        !self.disabled && !pointing_towards(self.relative_motion, self.facing.opposite())
    }

    /// One tick of `tickActors` for this actor at `position` (the contraption's motion done);
    /// `facing_now` is its facing in the world. Returns whether it stalls the contraption.
    pub fn tick(
        &mut self,
        position: [f64; 3],
        facing_now: Dir,
        lower: f32,
        probe: &impl Probe,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let grid = position.map(|v| v.floor() as i32);
        let visit = !self.stall && self.trigger(position, grid);
        self.position = Some(position);
        if !self.active() {
            return false;
        }
        if visit && !self.stall {
            self.visit(grid, facing_now, probe, effects);
            self.first_movement = false;
        }
        if self.kind.breaks() {
            self.tick_breaking(facing_now, lower, probe, effects);
        }
        self.stall
    }

    /// `visitNewPosition`.
    fn visit(
        &mut self,
        pos: [i32; 3],
        facing_now: Dir,
        probe: &impl Probe,
        effects: &mut Vec<Effect>,
    ) {
        if self.kind == Kind::Harvester {
            effects.push(Effect::Harvest(pos));
            return;
        }
        // BlockBreakingMovementBehaviour: a block it may break holds the contraption.
        if probe.breakable(self.kind, pos).is_some() {
            self.breaking = Some(Breaking {
                pos,
                ..Default::default()
            });
            self.stall = true;
        }
        // A saw facing up or down cuts nothing.
        if self.kind == Kind::Saw
            && facing_now.axis() == crate::kinetics::Axis::Y
            && self.breaking.is_some()
        {
            self.breaking = None;
            self.stall = false;
        }
        if self.kind == Kind::Plough {
            effects.push(Effect::Till([pos[0], pos[1] - 1, pos[2]]));
        }
    }

    /// `BlockBreakingMovementBehaviour.tick`: the breaker, then the wait after a falling block.
    fn tick_breaking(
        &mut self,
        facing_now: Dir,
        lower: f32,
        probe: &impl Probe,
        effects: &mut Vec<Effect>,
    ) {
        self.tick_breaker(lower, probe, effects);
        let Some((ticks, pos)) = self.waiting else {
            return;
        };
        if ticks > 0 {
            self.waiting = Some((ticks - 1, pos));
            self.stall = true;
            return;
        }
        self.waiting = None;
        self.stall = false;
        self.visit(pos, facing_now, probe, effects);
    }

    /// `tickBreaker`.
    fn tick_breaker(&mut self, lower: f32, probe: &impl Probe, effects: &mut Vec<Effect>) {
        let Some(mut b) = self.breaking else {
            self.stall = false;
            return;
        };
        if self.relative_motion == [0.0; 3] {
            self.stall = false;
            return;
        }
        if b.ticks_until > 0 {
            b.ticks_until -= 1;
            self.breaking = Some(b);
            return;
        }
        let Some(hardness) = probe.breakable(self.kind, b.pos) else {
            if b.progress != 0 {
                self.breaking = None;
                effects.push(Effect::Crack(b.pos, -1));
            }
            self.stall = false;
            return;
        };
        let speed = break_speed(self.motion, self.disabled, lower);
        b.progress += ((speed / hardness) as i32).clamp(1, 10 - b.progress);
        effects.push(Effect::Hit(b.pos));
        if b.progress >= 10 {
            effects.push(Effect::Crack(b.pos, -1));
            // Falling blocks break from the top down.
            let origin = b.pos;
            let mut top = b.pos;
            while probe.falling([top[0], top[1] + 1, top[2]]) {
                top[1] += 1;
            }
            let fell = probe.falling(top);
            self.stall = false;
            effects.push(Effect::Destroy(top));
            self.breaking = None;
            // `onBlockBroken`: a falling block broke; look again in 10 ticks (the saw doesn't).
            if fell && self.kind != Kind::Saw {
                self.waiting = Some((10, origin));
                self.stall = true;
            }
            return;
        }
        b.ticks_until = (hardness / speed) as i32;
        effects.push(Effect::Crack(b.pos, b.progress));
        self.breaking = Some(b);
    }

    /// `cancelStall` (`stopMoving`, turned off by controls): its cracks go.
    pub fn cancel(&mut self, effects: &mut Vec<Effect>) {
        if let Some(b) = self.breaking.take() {
            effects.push(Effect::Crack(b.pos, -1));
        }
        self.stall = false;
    }
}

/// A plant as the harvester reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    /// A `CropBlock` and its maximum age.
    Crop {
        max_age: u8,
    },
    SweetBerryBush,
    Cocoa,
    SugarCane,
    /// `GrowingPlantBlock`: kelp, cave vines, twisting and weeping vines.
    GrowingPlant,
    /// Another `BushBlock` (flowers, grass, ferns, saplings, mushrooms, nether wart...).
    Bush,
    Other,
}

/// The plant class of a vanilla block (`bush` when it is in `minecraft:flowers` or
/// `minecraft:saplings`, the `BushBlock`s tags name).
pub fn plant_of(name: &str, bush: bool) -> Plant {
    match name {
        "minecraft:wheat" | "minecraft:carrots" | "minecraft:potatoes" => {
            Plant::Crop { max_age: 7 }
        }
        "minecraft:beetroots" => Plant::Crop { max_age: 3 },
        // TorchflowerCropBlock: `MAX_AGE` 2 over an `age` of 0..=1, so never ripe here.
        "minecraft:torchflower_crop" => Plant::Crop { max_age: 2 },
        "minecraft:sweet_berry_bush" => Plant::SweetBerryBush,
        "minecraft:cocoa" => Plant::Cocoa,
        "minecraft:sugar_cane" => Plant::SugarCane,
        "minecraft:kelp"
        | "minecraft:kelp_plant"
        | "minecraft:cave_vines"
        | "minecraft:cave_vines_plant"
        | "minecraft:twisting_vines"
        | "minecraft:twisting_vines_plant"
        | "minecraft:weeping_vines"
        | "minecraft:weeping_vines_plant" => Plant::GrowingPlant,
        "minecraft:short_grass"
        | "minecraft:tall_grass"
        | "minecraft:fern"
        | "minecraft:large_fern"
        | "minecraft:dead_bush"
        | "minecraft:brown_mushroom"
        | "minecraft:red_mushroom"
        | "minecraft:nether_wart"
        | "minecraft:crimson_fungus"
        | "minecraft:warped_fungus"
        | "minecraft:crimson_roots"
        | "minecraft:warped_roots"
        | "minecraft:nether_sprouts"
        | "minecraft:seagrass"
        | "minecraft:tall_seagrass"
        | "minecraft:melon_stem"
        | "minecraft:pumpkin_stem"
        | "minecraft:attached_melon_stem"
        | "minecraft:attached_pumpkin_stem"
        | "minecraft:pink_petals"
        | "minecraft:lily_pad" => Plant::Bush,
        _ if bush => Plant::Bush,
        _ => Plant::Other,
    }
}

/// The highest value of a vanilla block's `age` property.
pub fn age_max(name: &str) -> Option<u8> {
    Some(match name {
        "minecraft:wheat"
        | "minecraft:carrots"
        | "minecraft:potatoes"
        | "minecraft:melon_stem"
        | "minecraft:pumpkin_stem" => 7,
        "minecraft:beetroots" | "minecraft:sweet_berry_bush" | "minecraft:nether_wart" => 3,
        "minecraft:cocoa" | "minecraft:frosted_ice" => 2,
        "minecraft:torchflower_crop" | "minecraft:bamboo" => 1,
        "minecraft:sugar_cane" | "minecraft:cactus" | "minecraft:fire" => 15,
        "minecraft:chorus_flower" => 5,
        "minecraft:kelp"
        | "minecraft:cave_vines"
        | "minecraft:twisting_vines"
        | "minecraft:weeping_vines" => 25,
        _ => return None,
    })
}

/// A block the harvester reaches: its plant class, `age` (value, maximum), whether its collision
/// shape is empty and whether it is in `minecraft:leaves`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reached {
    pub plant: Plant,
    pub age: Option<(u8, u8)>,
    pub no_collision: bool,
    pub leaves: bool,
}

/// `kinetics.harvestPartiallyGrown` and `harvesterReplants` (Create's defaults).
pub const HARVEST_PARTIAL: bool = false;
pub const REPLANTS: bool = true;

/// `isValidCrop`.
pub fn valid_crop(r: &Reached) -> bool {
    if let Plant::Crop { max_age } = r.plant {
        let age = r.age.map_or(0, |a| a.0);
        if HARVEST_PARTIAL {
            return age != 0 || !REPLANTS;
        }
        return age == max_age;
    }
    if (r.no_collision || r.plant == Plant::Cocoa)
        && let Some((age, max)) = r.age
    {
        if r.plant == Plant::SweetBerryBush && age <= 1 && REPLANTS {
            return false;
        }
        return !(age == 0 && REPLANTS || !HARVEST_PARTIAL && max != age);
    }
    false
}

/// `isValidOther`.
pub fn valid_other(r: &Reached) -> bool {
    match r.plant {
        Plant::Crop { .. } => return false,
        Plant::SugarCane => return true,
        _ => {}
    }
    if r.leaves {
        return true;
    }
    if r.plant == Plant::Cocoa {
        return r.age.is_some_and(|(a, m)| a == m);
    }
    if r.no_collision {
        if r.plant == Plant::GrowingPlant {
            return true;
        }
        if r.age.is_some() {
            return false;
        }
        return matches!(r.plant, Plant::Bush | Plant::SweetBerryBush);
    }
    false
}

/// What `cutCrop` leaves in a harvested block's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// The same block at this age.
    Age(u8),
    /// Air, or the block's fluid.
    Gone,
}

/// `cutCrop` (replanting).
pub fn cut(r: &Reached) -> Cut {
    match r.plant {
        Plant::Crop { .. } => Cut::Age(0),
        Plant::SweetBerryBush => Cut::Age(1),
        Plant::SugarCane | Plant::GrowingPlant => Cut::Gone,
        _ if (r.no_collision || r.plant == Plant::Cocoa) && r.age.is_some() => Cut::Age(0),
        _ => Cut::Gone,
    }
}

/// What a harvester does at a block (`visitNewPosition`): None when it leaves it; else whether
/// the block's own item comes out of the drops once (the replanted seed), and the tool and
/// effect chance it breaks with (shears at .45 for leaves).
pub fn harvest(r: &Reached, non_harvestable: bool) -> Option<(bool, bool, f32)> {
    if non_harvestable {
        return None;
    }
    let crop = valid_crop(r);
    if !crop && !valid_other(r) {
        return None;
    }
    let shears = r.leaves;
    Some((REPLANTS && crop, shears, if shears { 0.45 } else { 1.0 }))
}

/// A block as the plough's `canBreak` reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ploughed {
    pub air: bool,
    pub farmland_below: bool,
    /// A `LiquidBlock` or a bubble column.
    pub liquid: bool,
    /// Create's track (`ITrackBlock`).
    pub track: bool,
    pub blacklisted: bool,
    pub whitelisted: bool,
    pub no_collision: bool,
}

/// `PloughMovementBehaviour.canBreak`.
pub fn plough_breaks(p: &Ploughed) -> bool {
    if p.air || p.farmland_below || p.liquid {
        return false;
    }
    if p.track {
        return true;
    }
    if p.blacklisted {
        return false;
    }
    p.whitelisted || p.no_collision
}

/// The vanilla item a crop or plant comes from (`new ItemStack(state.getBlock())`), for the seed
/// a harvester keeps back.
pub fn planted_by(block: &str) -> Option<&'static str> {
    Some(match block {
        "minecraft:wheat" => "minecraft:wheat_seeds",
        "minecraft:carrots" => "minecraft:carrot",
        "minecraft:potatoes" => "minecraft:potato",
        "minecraft:beetroots" => "minecraft:beetroot_seeds",
        "minecraft:torchflower_crop" => "minecraft:torchflower_seeds",
        "minecraft:sweet_berry_bush" => "minecraft:sweet_berries",
        "minecraft:cocoa" => "minecraft:cocoa_beans",
        "minecraft:nether_wart" => "minecraft:nether_wart",
        "minecraft:melon_stem" => "minecraft:melon_seeds",
        "minecraft:pumpkin_stem" => "minecraft:pumpkin_seeds",
        _ => return None,
    })
}

/// `IItemHandler.insertItem` slot by slot (`ItemHandlerHelper.insertItem`, not stacked first)
/// over a contraption's storages in order: what is left of `stack`. `max` is a slot's limit
/// for the stack (`min(64, maxStackSize)`).
pub fn insert<S>(
    slots: &mut [Option<S>],
    mut stack: S,
    count: impl Fn(&S) -> u32,
    set: impl Fn(&mut S, u32),
    same: impl Fn(&S, &S) -> bool,
    max: impl Fn(&S) -> u32,
) -> Option<S>
where
    S: Clone,
{
    for slot in slots.iter_mut() {
        let n = count(&stack);
        if n == 0 {
            return None;
        }
        let limit = max(&stack);
        match slot {
            None => {
                let put = n.min(limit);
                let mut s = stack.clone();
                set(&mut s, put);
                *slot = Some(s);
                set(&mut stack, n - put);
            }
            Some(s) if same(s, &stack) => {
                let room = limit.saturating_sub(count(s));
                let put = n.min(room);
                let c = count(s);
                set(s, c + put);
                set(&mut stack, n - put);
            }
            _ => {}
        }
    }
    (count(&stack) > 0).then_some(stack)
}

/// A contraption controls' filter: an item, or none (every actor).
pub type Filter = Option<String>;

/// `ContraptionControlsMovingInteraction.handlePlayerInteraction` (not an elevator): the list
/// of turned-off actor kinds changes for a click on controls with `filter`; `kinds` are the
/// items of the contraption's actors (`canBeDisabledVia`). Returns whether `filter` is now off,
/// and the actors' items whose `disabled` follows (`setActorsActive`): every one for no filter.
pub fn toggle(disabled: &mut Vec<Filter>, filter: &Filter, kinds: &[&str]) -> bool {
    let mut disable = true;
    let mut invert = false;
    let mut i = 0;
    while i < disabled.len() {
        let present = disabled[i].clone();
        let same = present == *filter;
        if present.is_none() {
            disabled.remove(i);
            disable = false;
            if !same {
                invert = true;
            }
            continue;
        }
        if !same {
            i += 1;
            continue;
        }
        disabled.remove(i);
        disable = false;
        break;
    }
    if invert {
        for k in kinds {
            let item = Some((*k).to_owned());
            if item == *filter || disabled.contains(&item) {
                continue;
            }
            disabled.push(item);
        }
    }
    if filter.is_none() {
        disabled.clear();
    }
    if disable {
        disabled.push(filter.clone());
    }
    disable
}

/// `Contraption.setActorsActive`: the actors a filter names (all for none) turn on or off; those
/// turned off stop breaking (`onDisabledByControls`).
pub fn set_active(actors: &mut [Actor], filter: &Filter, enable: bool, effects: &mut Vec<Effect>) {
    for a in actors.iter_mut() {
        if filter.as_deref().is_some_and(|f| f != a.kind.item()) {
            continue;
        }
        a.disabled = !enable;
        if !enable {
            a.cancel(effects);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Stone (hardness 1.5) and sand (0.5, falling) by position; drills break both.
    struct World(HashMap<[i32; 3], (f32, bool)>);
    impl Probe for World {
        fn breakable(&self, _: Kind, pos: [i32; 3]) -> Option<f32> {
            self.0.get(&pos).map(|b| b.0)
        }
        fn falling(&self, pos: [i32; 3]) -> bool {
            self.0.get(&pos).is_some_and(|b| b.1)
        }
    }

    /// A piston pushing a drill east at `per_tick` blocks a tick until `ticks` pass, as
    /// `LinearActuatorBlockEntity` does: the offset holds while the actors stall. Returns each
    /// broken block with the tick it broke, and the offset reached.
    fn drive(world: &mut World, per_tick: f64, ticks: usize) -> (Vec<([i32; 3], usize)>, f64) {
        let mut a = Actor::new([0, 0, 0], Kind::Drill, Dir::East, -1);
        let mut offset = 0.0;
        let mut stalled = false;
        let mut broken = Vec::new();
        for t in 0..ticks {
            // The contraption moves first (unless stalled), then its actors tick.
            if !stalled {
                offset += per_tick;
            }
            let mut effects = Vec::new();
            let pos = translated_position(&a, [offset, 0.0, 0.0]);
            stalled = a.tick(pos, Dir::East, CONTROLLED_LOWER, world, &mut effects);
            for e in effects {
                if let Effect::Destroy(p) = e {
                    world.0.remove(&p);
                    broken.push((p, t));
                }
            }
        }
        (broken, offset)
    }

    #[test]
    fn a_drill_on_a_piston_breaks_each_block_in_its_path_and_holds_the_piston() {
        // 64 rpm: 0.125 blocks a tick; the actor's motion length 0.125 gives an animation speed
        // of 200 and a break speed of 0.4: stone takes 10 steps 3 ticks apart.
        assert_eq!(animation_speed([0.125, 0.0, 0.0], false), -200.0);
        assert_eq!(break_speed([0.125, 0.0, 0.0], false, CONTROLLED_LOWER), 0.4);
        let mut world = World(HashMap::from([
            ([1, 0, 0], (1.5, false)),
            ([2, 0, 0], (1.5, false)),
        ]));
        let (broken, offset) = drive(&mut world, 0.125, 120);
        assert_eq!(
            broken.iter().map(|b| b.0).collect::<Vec<_>>(),
            vec![[1, 0, 0], [2, 0, 0]]
        );
        // The first block is reached on the drill's first move (`firstMovement`) and holds the
        // piston for its 10 steps (every 4th tick: 3 waiting, then a step).
        assert_eq!(broken[0].1, 37);
        // The second is reached at offset 0.875 (0.5 + 0.65 + 0.875 = 2.025).
        assert!(broken[1].1 > broken[0].1 + 37);
        assert!(offset > 2.0, "it moves on once the way is clear: {offset}");
        // An unbreakable block is never held for: the contraption moves on (the collision
        // with it is the contraption's, not ported).
        let mut world = World(HashMap::new());
        let (broken, offset) = drive(&mut world, 0.125, 8);
        assert!(broken.is_empty());
        assert_eq!(offset, 1.0);
    }

    #[test]
    fn falling_blocks_break_from_the_top_and_the_drill_waits() {
        // Stone at x 1, sand stacked on it at y 1 and 2: the sand on top breaks first, then the
        // drill waits 10 ticks and looks at x 1 again.
        let mut world = World(HashMap::from([
            ([1, 0, 0], (0.5, true)),
            ([1, 1, 0], (0.5, true)),
            ([1, 2, 0], (0.5, true)),
        ]));
        let (broken, _) = drive(&mut world, 0.125, 200);
        let order: Vec<[i32; 3]> = broken.iter().map(|b| b.0).collect();
        assert_eq!(order, vec![[1, 2, 0], [1, 1, 0], [1, 0, 0]]);
    }

    #[test]
    fn a_drill_moving_backwards_does_nothing() {
        let mut a = Actor::new([0, 0, 0], Kind::Drill, Dir::East, -1);
        let world = World(HashMap::from([([0, 0, 0], (1.5, false))]));
        let mut effects = Vec::new();
        a.tick(
            [1.15, 0.5, 0.5],
            Dir::East,
            CONTROLLED_LOWER,
            &world,
            &mut effects,
        );
        a.tick(
            [1.0, 0.5, 0.5],
            Dir::East,
            CONTROLLED_LOWER,
            &world,
            &mut effects,
        );
        assert!(!a.active() && effects.is_empty());
    }

    #[test]
    fn harvesters_take_ripe_crops_and_replant() {
        let wheat = |age| Reached {
            plant: Plant::Crop { max_age: 7 },
            age: Some((age, 7)),
            no_collision: true,
            leaves: false,
        };
        assert_eq!(harvest(&wheat(7), false), Some((true, false, 1.0)));
        assert_eq!(cut(&wheat(7)), Cut::Age(0));
        assert_eq!(harvest(&wheat(3), false), None);
        assert_eq!(harvest(&wheat(7), true), None, "create:non_harvestable");
        let berries = |age| Reached {
            plant: Plant::SweetBerryBush,
            age: Some((age, 3)),
            no_collision: true,
            leaves: false,
        };
        assert_eq!(harvest(&berries(1), false), None);
        assert_eq!(harvest(&berries(3), false), Some((true, false, 1.0)));
        assert_eq!(cut(&berries(3)), Cut::Age(1));
        let cane = Reached {
            plant: Plant::SugarCane,
            age: Some((0, 15)),
            no_collision: true,
            leaves: false,
        };
        assert_eq!(harvest(&cane, false), Some((false, false, 1.0)));
        assert_eq!(cut(&cane), Cut::Gone);
        let leaves = Reached {
            plant: Plant::Other,
            age: None,
            no_collision: false,
            leaves: true,
        };
        assert_eq!(harvest(&leaves, false), Some((false, true, 0.45)));
        let grass = Reached {
            plant: Plant::Bush,
            age: None,
            no_collision: true,
            leaves: false,
        };
        assert_eq!(harvest(&grass, false), Some((false, false, 1.0)));
        let stone = Reached {
            plant: Plant::Other,
            age: None,
            no_collision: false,
            leaves: false,
        };
        assert_eq!(harvest(&stone, false), None);
        assert_eq!(planted_by("minecraft:wheat"), Some("minecraft:wheat_seeds"));
    }

    #[test]
    fn a_harvester_visits_each_block_it_reaches() {
        struct Nothing;
        impl Probe for Nothing {
            fn breakable(&self, _: Kind, _: [i32; 3]) -> Option<f32> {
                None
            }
            fn falling(&self, _: [i32; 3]) -> bool {
                false
            }
        }
        let mut a = Actor::new([0, 0, 0], Kind::Harvester, Dir::East, 0);
        let mut seen = Vec::new();
        for t in 0..20 {
            let mut effects = Vec::new();
            let pos = translated_position(&a, [f64::from(t) * 0.25, 0.0, 0.0]);
            assert!(!a.tick(pos, Dir::East, CONTROLLED_LOWER, &Nothing, &mut effects));
            seen.extend(effects);
        }
        // At rest it reaches x 0.95, its own block: the first move into x 1 is its first visit.
        assert_eq!(
            seen,
            vec![
                Effect::Harvest([1, 0, 0]),
                Effect::Harvest([2, 0, 0]),
                Effect::Harvest([3, 0, 0]),
                Effect::Harvest([4, 0, 0]),
                Effect::Harvest([5, 0, 0]),
            ]
        );
    }

    #[test]
    fn ploughs_break_what_has_no_collision_and_till_below() {
        let grass = Ploughed {
            no_collision: true,
            ..Default::default()
        };
        assert!(plough_breaks(&grass));
        assert!(!plough_breaks(&Ploughed {
            farmland_below: true,
            ..grass
        }));
        assert!(!plough_breaks(&Ploughed::default()));
        assert!(plough_breaks(&Ploughed {
            track: true,
            ..Default::default()
        }));
    }

    #[test]
    fn storage_fills_slot_by_slot() {
        let mut slots: Vec<Option<(&str, u32)>> =
            vec![Some(("stone", 60)), None, Some(("dirt", 1))];
        let left = insert(
            &mut slots,
            ("dirt", 70),
            |s| s.1,
            |s, n| s.1 = n,
            |a, b| a.0 == b.0,
            |_| 64,
        );
        // Slot 0 holds stone; slot 1 takes 64; slot 2 takes 6 more.
        assert_eq!(left, None);
        assert_eq!(
            slots,
            vec![Some(("stone", 60)), Some(("dirt", 64)), Some(("dirt", 7))]
        );
        let left = insert(
            &mut slots,
            ("stone", 10),
            |s| s.1,
            |s, n| s.1 = n,
            |a, b| a.0 == b.0,
            |_| 64,
        );
        assert_eq!(left, Some(("stone", 6)));
    }

    #[test]
    fn controls_turn_kinds_of_actor_off_and_on() {
        let kinds = ["create:mechanical_drill", "create:mechanical_saw"];
        let drill = Some("create:mechanical_drill".to_owned());
        let mut off = Vec::new();
        assert!(toggle(&mut off, &drill, &kinds));
        assert_eq!(off, vec![drill.clone()]);
        assert!(!toggle(&mut off, &drill, &kinds));
        assert!(off.is_empty());
        // No filter: everything off, then everything on.
        assert!(toggle(&mut off, &None, &kinds));
        assert_eq!(off, vec![None]);
        // A drill filter while everything is off: the others stay off, the drill comes on.
        assert!(!toggle(&mut off, &drill, &kinds));
        assert_eq!(off, vec![Some("create:mechanical_saw".to_owned())]);
        let mut actors = vec![
            Actor::new([0, 0, 0], Kind::Drill, Dir::East, 0),
            Actor::new([1, 0, 0], Kind::Saw, Dir::East, 0),
        ];
        let mut effects = Vec::new();
        set_active(&mut actors, &drill, false, &mut effects);
        assert!(actors[0].disabled && !actors[1].disabled);
        set_active(&mut actors, &None, true, &mut effects);
        assert!(!actors[0].disabled);
    }
}
