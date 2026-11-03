//! Create's movement behaviours ("actors") on moving contraptions (`MovementBehaviour`,
//! `MovementContext`, `AbstractContraptionEntity.tickActors`): drills and saws breaking what
//! they pass (`BlockBreakingMovementBehaviour`, `DrillMovementBehaviour`,
//! `SawMovementBehaviour`), harvesters (`HarvesterMovementBehaviour`), ploughs
//! (`PloughMovementBehaviour`), rollers (`RollerMovementBehaviour`) and portable storage
//! interfaces (`PortableStorageInterfaceMovement`, with `crate::psi`); a contraption's storage
//! taking what they collect (`MovementBehaviour.collectOrDropItem`,
//! `ItemHandlerHelper.insertItem`); and contraption controls turning kinds of actor off
//! (`ContraptionControlsBlockEntity`, `ContraptionControlsMovement`,
//! `ContraptionControlsMovingInteraction`). Actors tick only while their contraption moves: a
//! stopped contraption has none.

use crate::kinetics::Dir;

/// The movement behaviours ported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Drill,
    Saw,
    Harvester,
    Plough,
    Roller,
    /// A portable storage interface (`fluid`: the fluid one).
    Interface {
        fluid: bool,
    },
}

impl Kind {
    /// The actor of a Create block, if it has one (`MovementBehaviour.REGISTRY`).
    pub fn of(block: crate::Block) -> Option<Kind> {
        Some(match block {
            crate::Block::MechanicalDrill => Kind::Drill,
            crate::Block::MechanicalSaw => Kind::Saw,
            crate::Block::MechanicalHarvester => Kind::Harvester,
            crate::Block::MechanicalPlough => Kind::Plough,
            crate::Block::MechanicalRoller => Kind::Roller,
            crate::Block::PortableStorageInterface => Kind::Interface { fluid: false },
            crate::Block::PortableFluidInterface => Kind::Interface { fluid: true },
            _ => return None,
        })
    }

    /// `getActiveAreaOffset`: how far ahead of the block's centre along its facing it acts.
    pub fn reach(self) -> f64 {
        match self {
            Kind::Drill | Kind::Saw => 0.65,
            Kind::Harvester | Kind::Plough | Kind::Roller => 0.45,
            Kind::Interface { .. } => crate::psi::REACH,
        }
    }

    /// The item that turns it off on contraption controls (`canBeDisabledVia`: its block).
    pub fn item(self) -> &'static str {
        match self {
            Kind::Drill => "create:mechanical_drill",
            Kind::Saw => "create:mechanical_saw",
            Kind::Harvester => "create:mechanical_harvester",
            Kind::Plough => "create:mechanical_plough",
            Kind::Roller => "create:mechanical_roller",
            Kind::Interface { fluid: false } => "create:portable_storage_interface",
            Kind::Interface { fluid: true } => "create:portable_fluid_interface",
        }
    }

    /// A `BlockBreakingMovementBehaviour`'s (`ContraptionCollider.isCollidingWithWorld` lets it
    /// into a block it can break).
    pub fn breaks(self) -> bool {
        matches!(self, Kind::Drill | Kind::Saw | Kind::Plough | Kind::Roller)
    }
}

/// `RollerBlockEntity.RollingMode`, its scroll value's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RollingMode {
    #[default]
    TunnelPave,
    StraightFill,
    WideFill,
}

impl RollingMode {
    pub const ALL: [RollingMode; 3] = [
        RollingMode::TunnelPave,
        RollingMode::StraightFill,
        RollingMode::WideFill,
    ];

    pub fn name(self) -> &'static str {
        match self {
            RollingMode::TunnelPave => "tunnel_pave",
            RollingMode::StraightFill => "straight_fill",
            RollingMode::WideFill => "wide_fill",
        }
    }

    pub fn read(v: &str) -> Option<RollingMode> {
        RollingMode::ALL.into_iter().find(|m| m.name() == v)
    }
}

/// `CKinetics.rollerFillDepth`'s default.
pub const ROLLER_FILL_DEPTH: i32 = 12;

/// What a roller finds where it would pave (`tryFill`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Already its block (`PASS`).
    Same,
    /// May be filled: replaceable, leaves, or without collision and not a portal.
    Open,
    /// Solid, a portal, or not loaded (`FAIL`).
    Blocked,
}

/// `RollerMovementBehaviour.PaveResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pave {
    Fail,
    Pass,
    Success,
}

/// What a moving contraption carries besides its blocks (`Contraption.actors`, `superglue`,
/// `disabledActors`, `stalled`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Carried {
    pub actors: Vec<Actor>,
    /// The glue it took, offsets as its blocks.
    pub glue: Vec<crate::glue::Glue>,
    /// The contraption controls blocks it carries, by their block's offset.
    pub controls: Vec<([i32; 3], Controls)>,
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
    /// Its block entity's filter as it started moving (a roller's pave material: a plain block
    /// item), and a roller's mode.
    pub filter: Option<String>,
    pub mode: RollingMode,
    /// A roller's `ReferencePos`: the block it visited while it breaks what is above it.
    pub reference: Option<[i32; 3]>,
    /// An interface's `WorkingPos`.
    pub interface: crate::psi::Moving,
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
            filter: None,
            mode: RollingMode::TunnelPave,
            reference: None,
            interface: crate::psi::Moving::default(),
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
    /// A roller's `tryFill`: one item its filter passes out of the contraption's storage, and
    /// its block (a slab doubled; air with no filter) set there.
    Pave([i32; 3]),
    /// The plough's hoe used on the top of the block (`useOn`).
    Till([i32; 3]),
}

/// What an actor reads of the world.
pub trait Probe {
    /// `canBreak` for this kind of breaker, and the block's hardness when it may.
    fn breakable(&self, kind: Kind, pos: [i32; 3]) -> Option<f32>;
    /// Whether the block is a `FallingBlock` (sand, gravel, concrete powder, anvils...).
    fn falling(&self, pos: [i32; 3]) -> bool;
    /// A roller: how many items the contraption's storage holds that `filter` passes (any item
    /// for none).
    fn material(&self, filter: Option<&str>) -> u32 {
        let _ = filter;
        0
    }
    /// A roller: what it finds filling `pos` with `filter`'s block (air for none).
    fn fill(&self, pos: [i32; 3], filter: Option<&str>) -> Fill {
        let _ = (pos, filter);
        Fill::Blocked
    }
    /// A roller: whether `pos` holds `item`'s block, or (`slab`) its slab
    /// (`getStateToPaveWithAsSlab`).
    fn is_pave(&self, pos: [i32; 3], item: &str, slab: bool) -> bool {
        let _ = (pos, item, slab);
        false
    }
    /// A roller: whether `item`'s block is a `FallingBlock`.
    fn pave_falls(&self, item: &str) -> bool {
        let _ = item;
        false
    }
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
    let mut p = [0, 1, 2]
        .map(|i| f64::from(a.local[i]) + 0.5 + f64::from(o[i]) * a.kind.reach() + translation[i]);
    // The roller's drum works the block two below its own (`subtract(0, 2, 0)`).
    if a.kind == Kind::Roller {
        p[1] -= 2.0;
    }
    p
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

    /// `isActive`: not turned off, and (a breaker) not moving away from its facing; a roller only
    /// while it moves the way it faces.
    pub fn active(&self) -> bool {
        match self.kind {
            Kind::Roller => !self.disabled && pointing_towards(self.relative_motion, self.facing),
            Kind::Interface { .. } => !self.disabled,
            _ => !self.disabled && !pointing_towards(self.relative_motion, self.facing.opposite()),
        }
    }

    /// An interface's tick (`PortableStorageInterfaceMovement`): `visitNewPosition` when it
    /// reaches a block, then `tick`, against the stationary interfaces of `world`; `contraption`
    /// is its contraption's key. Returns whether it stalls the contraption.
    pub fn tick_interface(
        &mut self,
        position: [f64; 3],
        world: &mut impl crate::psi::World,
        contraption: [i32; 3],
    ) -> bool {
        let grid = position.map(|v| v.floor() as i32);
        let visit = self.trigger(position, grid);
        self.position = Some(position);
        if !self.active() {
            return false;
        }
        if visit {
            self.interface
                .visit(world, contraption, grid, position, self.facing);
            self.first_movement = false;
        }
        self.stall = self.interface.tick(
            world,
            contraption,
            position,
            self.motion,
            self.facing,
            self.stall,
        );
        self.stall
    }

    /// `RollerMovementBehaviour.getPositionsToBreak` (no train under it): in tunnel mode, the
    /// blocks it may break from the one under its drum (only with material to pave it again) to
    /// its own level, not its pave block nor at drum level its slab. `cleared` were broken this
    /// tick.
    fn roller_breaks(
        &self,
        visited: [i32; 3],
        probe: &impl Probe,
        cleared: &[[i32; 3]],
    ) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        if self.mode != RollingMode::TunnelPave {
            return out;
        }
        let filter = self.filter.as_deref();
        let start = if filter.is_some() && probe.material(filter) > 0 {
            0
        } else {
            1
        };
        for i in start..=2 {
            let target = [visited[0], visited[1] + i, visited[2]];
            if cleared.contains(&target) {
                continue;
            }
            if let Some(item) = filter
                && ((i == 0 && probe.is_pave(target, item, false))
                    || (i == 1 && probe.is_pave(target, item, true)))
            {
                continue;
            }
            if probe.breakable(Kind::Roller, target).is_some() {
                out.push(target);
            }
        }
        out
    }

    /// `RollerMovementBehaviour.visitNewPosition`: the hardest block to break holds it (the last
    /// of equals), else it paves.
    fn roller_visit(&mut self, pos: [i32; 3], probe: &impl Probe, effects: &mut Vec<Effect>) {
        let breaks = self.roller_breaks(pos, probe, &[]);
        let mut best: Option<[i32; 3]> = None;
        let mut max = -1.0f32;
        for p in breaks {
            let h = probe.breakable(Kind::Roller, p).unwrap_or(0.0);
            if h < max {
                continue;
            }
            max = h;
            best = Some(p);
        }
        let Some(target) = best else {
            self.pave(pos, probe, effects, &[]);
            return;
        };
        self.reference = Some(pos);
        self.breaking = Some(Breaking {
            pos: target,
            ..Default::default()
        });
        self.stall = true;
    }

    /// `triggerPaver`: in tunnel mode the visited block alone; filling, layer by layer down to
    /// `ROLLER_FILL_DEPTH` (the wide fill spreading a diamond of radius `(depth + 1) / 2`) until
    /// one is placed or all are blocked; each place takes an item from the storage. A placed
    /// block holds it 2 ticks (a falling block lets it fill on).
    fn pave(
        &mut self,
        pos: [i32; 3],
        probe: &impl Probe,
        effects: &mut Vec<Effect>,
        cleared: &[[i32; 3]],
    ) {
        let filter = self.filter.as_deref();
        if self.mode != RollingMode::TunnelPave && filter.is_none() {
            return;
        }
        let mut material = probe.material(filter);
        let falls = filter.is_some_and(|f| probe.pave_falls(f));
        let mut result = Pave::Pass;
        let mut y = 0;
        while result == Pave::Pass {
            if y > ROLLER_FILL_DEPTH {
                break;
            }
            let mut layer: Vec<[i32; 3]> = Vec::new();
            if self.mode == RollingMode::WideFill {
                let radius = (y + 1) / 2;
                for i in -radius..=radius {
                    for j in -radius..=radius {
                        if i.abs() + j.abs() <= radius {
                            layer.push([pos[0] + i, pos[1] - y, pos[2] + j]);
                        }
                    }
                }
            } else {
                layer.push([pos[0], pos[1] - y, pos[2]]);
            }
            let mut blocked = true;
            let mut placed = false;
            for p in layer {
                let found = if cleared.contains(&p) {
                    if filter.is_none() {
                        Fill::Same
                    } else {
                        Fill::Open
                    }
                } else {
                    probe.fill(p, filter)
                };
                result = match found {
                    Fill::Same => Pave::Pass,
                    Fill::Blocked => Pave::Fail,
                    Fill::Open if material == 0 => Pave::Fail,
                    Fill::Open => {
                        material -= 1;
                        effects.push(Effect::Pave(p));
                        Pave::Success
                    }
                };
                if result != Pave::Fail {
                    blocked = false;
                }
                if result == Pave::Success {
                    placed = true;
                }
            }
            if placed {
                result = Pave::Success;
            } else if !blocked || y == 0 {
                result = Pave::Pass;
            }
            if result == Pave::Success && falls {
                result = Pave::Pass;
            }
            if result != Pave::Pass || self.mode == RollingMode::TunnelPave {
                break;
            }
            y += 1;
        }
        if result == Pave::Success {
            self.waiting = Some((2, pos));
            self.stall = true;
        }
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
        if self.kind == Kind::Roller {
            self.roller_visit(pos, probe, effects);
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
        let mut speed = break_speed(self.motion, self.disabled, lower);
        if self.kind == Kind::Roller {
            speed = (speed * 1.5).clamp(1.0 / 128.0, 16.0);
        }
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
            // The roller breaks the rest of what it found at once, then paves there.
            if let Some(reference) = self.reference.take() {
                let mut cleared = vec![top];
                for other in self.roller_breaks(reference, probe, &cleared) {
                    if other != origin {
                        effects.push(Effect::Destroy(other));
                        cleared.push(other);
                    }
                }
                self.pave(reference, probe, effects, &cleared);
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
        // An interface's `reset`.
        self.interface.working = None;
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

/// Create's `create:contraption_controlled` item tag (its generated data): the items a
/// contraption controls' filter takes (`ContraptionControlsBlockEntity`'s predicate).
pub const CONTRAPTION_CONTROLLED: [&str; 23] = [
    "create:portable_fluid_interface",
    "create:mechanical_drill",
    "create:mechanical_saw",
    "create:deployer",
    "create:portable_storage_interface",
    "create:redstone_contact",
    "create:mechanical_harvester",
    "create:mechanical_plough",
    "create:mechanical_roller",
    "create:andesite_funnel",
    "create:brass_funnel",
    "create:peculiar_bell",
    "create:haunted_bell",
    "create:andesite_door",
    "create:brass_door",
    "create:copper_door",
    "create:train_door",
    "create:framed_glass_door",
    "minecraft:bell",
    "minecraft:campfire",
    "minecraft:soul_campfire",
    "minecraft:dispenser",
    "minecraft:dropper",
];

/// A contraption controls block (`ContraptionControlsBlockEntity`): its filter, `disabled`
/// (`Disabled`) and `powered` (`Powered`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub filter: Filter,
    pub disabled: bool,
    pub powered: bool,
}

impl Controls {
    /// `useWithoutItem`: a click turns it off or on; whether it is now on.
    pub fn click(&mut self) -> bool {
        self.disabled = !self.disabled;
        !self.disabled
    }

    /// `updatePoweredState`: a change of signal turns it off while powered, on again after;
    /// whether anything changed.
    pub fn powered(&mut self, powered: bool) -> bool {
        if self.powered == powered {
            return false;
        }
        self.powered = powered;
        self.disabled = powered;
        true
    }
}

/// `AngleHelper.horizontalAngle`: south 0, north 180, and x facings turned back.
fn horizontal_angle(facing: Dir) -> f64 {
    match facing {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `VecHelper.rotateCentered(voxelSpace(x, y, z), deg, Y)`: a point in pixels turned about the
/// block's centre (`x cos + z sin`, `z cos - x sin`, with `Mth`'s float sine).
fn rotate_centred(px: [f64; 3], deg: f64) -> [f64; 3] {
    let angle = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (s, c) = (f64::from(angle.sin()), f64::from(angle.cos()));
    let (x, y, z) = (px[0] / 16.0 - 0.5, px[1] / 16.0, px[2] / 16.0 - 0.5);
    [x * c + z * s + 0.5, y, z * c - x * s + 0.5]
}

/// `ContraptionControlsBlockEntity.ControlsSlot`: the filter slot on top of the controls, and
/// the distance a click on it is within (`scale / 2`, scale .508).
pub fn controls_slot(facing: Dir) -> ([f64; 3], f64) {
    (
        rotate_centred([8.0, 14.0, 5.5], horizontal_angle(facing)),
        0.508 / 2.0,
    )
}

/// `RollerBlockEntity.RollerValueBox`: the filter (`filter`, 3 px one way) or mode slot on top of
/// a roller, and the distance a click on it is within (`scale / 3`).
pub fn roller_slot(facing: Dir, filter: bool) -> ([f64; 3], f64) {
    let h = if filter { 3.0 } else { -3.0 };
    (
        rotate_centred([8.0 + h, 15.5, 11.0], horizontal_angle(facing) + 180.0),
        0.5 / 3.0,
    )
}

/// `Contraption.startMoving` with `disableActorOnStart`: the actor kinds turned off as a
/// contraption starts, from its controls set off (each filter once), applied to its actors.
pub fn start_moving(carried: &mut Carried, controls: &[Controls]) {
    carried.disabled.clear();
    for c in controls.iter().filter(|c| c.disabled) {
        if !carried.disabled.contains(&c.filter) {
            carried.disabled.push(c.filter.clone());
        }
    }
    let mut effects = Vec::new();
    for f in carried.disabled.clone() {
        set_active(&mut carried.actors, &f, false, &mut effects);
    }
}

/// `ContraptionControlsMovement.stopMoving`: a controls block put back is off when its filter's
/// kind, or everything, is off on the contraption.
pub fn stays_disabled(disabled: &[Filter], filter: &Filter) -> bool {
    disabled.contains(filter) || disabled.contains(&None)
}

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

    #[test]
    fn controls_set_off_turn_their_kind_off_as_the_contraption_starts() {
        let drill = Some("create:mechanical_drill".to_owned());
        let mut carried = Carried {
            actors: vec![
                Actor::new([0, 0, 0], Kind::Drill, Dir::East, 0),
                Actor::new([0, 1, 0], Kind::Roller, Dir::East, 0),
            ],
            ..Default::default()
        };
        let mut c = Controls {
            filter: drill.clone(),
            ..Default::default()
        };
        // A click turns it off, redstone keeps it off while powered and turns it on after.
        assert!(!c.click());
        assert!(!c.powered(false));
        assert!(c.powered(true) && c.disabled);
        assert!(c.powered(false) && !c.disabled);
        c.click();
        let on = Controls::default();
        start_moving(&mut carried, &[c.clone(), c.clone(), on]);
        assert_eq!(carried.disabled, vec![drill.clone()]);
        assert!(carried.actors[0].disabled && !carried.actors[1].disabled);
        // Put back: off while its kind, or everything, is off.
        assert!(stays_disabled(&carried.disabled, &drill));
        assert!(!stays_disabled(&carried.disabled, &None));
        assert!(stays_disabled(&[None], &drill));
    }

    #[test]
    fn value_slots_turn_with_the_block() {
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        assert!(near(
            controls_slot(Dir::South).0,
            [0.5, 14.0 / 16.0, 5.5 / 16.0]
        ));
        assert!(near(
            controls_slot(Dir::North).0,
            [0.5, 14.0 / 16.0, 10.5 / 16.0]
        ));
        assert!(near(
            controls_slot(Dir::East).0,
            [5.5 / 16.0, 14.0 / 16.0, 0.5]
        ));
        assert!(near(
            controls_slot(Dir::West).0,
            [10.5 / 16.0, 14.0 / 16.0, 0.5]
        ));
        // A roller facing north: turned 0 from (11, 15.5, 11); its mode box 6 px to the west.
        assert!(near(
            roller_slot(Dir::North, true).0,
            [11.0 / 16.0, 15.5 / 16.0, 11.0 / 16.0]
        ));
        assert!(near(
            roller_slot(Dir::North, false).0,
            [5.0 / 16.0, 15.5 / 16.0, 11.0 / 16.0]
        ));
        assert!(near(
            roller_slot(Dir::South, true).0,
            [5.0 / 16.0, 15.5 / 16.0, 5.0 / 16.0]
        ));
    }

    #[test]
    fn the_controlled_items_are_create_s_tag() {
        let tag = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../../src/generated/resources/data/create/tags/item/contraption_controlled.json",
        );
        let Ok(text) = std::fs::read_to_string(tag) else {
            return;
        };
        let listed: Vec<&str> = text
            .split('"')
            .filter(|s| s.contains(':') && !s.contains(' '))
            .collect();
        assert_eq!(listed, CONTRAPTION_CONTROLLED);
    }

    /// A roller's world: blocks by position (hardness, falling), the pave block's positions,
    /// and the storage's material.
    struct Ground {
        solid: HashMap<[i32; 3], f32>,
        paved: std::cell::RefCell<Vec<[i32; 3]>>,
        material: u32,
        loaded_below: i32,
    }

    impl Probe for Ground {
        fn breakable(&self, kind: Kind, pos: [i32; 3]) -> Option<f32> {
            assert_eq!(kind, Kind::Roller);
            if self.paved.borrow().contains(&pos) {
                return Some(1.5);
            }
            self.solid.get(&pos).copied()
        }
        fn falling(&self, _: [i32; 3]) -> bool {
            false
        }
        fn material(&self, filter: Option<&str>) -> u32 {
            assert_eq!(filter, Some("minecraft:cobblestone"));
            self.material - self.paved.borrow().len() as u32
        }
        fn fill(&self, pos: [i32; 3], _: Option<&str>) -> Fill {
            if pos[1] < self.loaded_below {
                Fill::Blocked
            } else if self.paved.borrow().contains(&pos) {
                Fill::Same
            } else if self.solid.contains_key(&pos) {
                Fill::Blocked
            } else {
                Fill::Open
            }
        }
        fn is_pave(&self, pos: [i32; 3], _: &str, slab: bool) -> bool {
            !slab && self.paved.borrow().contains(&pos)
        }
    }

    /// A roller facing east pushed east at 0.125 a tick, its block at y 2 (paving y 0); the
    /// world takes each effect.
    fn roll(ground: &mut Ground, mode: RollingMode, ticks: usize) -> Vec<Effect> {
        let mut a = Actor::new([0, 2, 0], Kind::Roller, Dir::East, -1);
        a.filter = Some("minecraft:cobblestone".into());
        a.mode = mode;
        let mut offset = 0.0;
        let mut stalled = false;
        let mut all = Vec::new();
        for _ in 0..ticks {
            if !stalled {
                offset += 0.125;
            }
            let mut effects = Vec::new();
            let pos = translated_position(&a, [offset, 0.0, 0.0]);
            stalled = a.tick(pos, Dir::East, CONTROLLED_LOWER, ground, &mut effects);
            for e in &effects {
                match *e {
                    Effect::Destroy(p) => {
                        ground.solid.remove(&p);
                        ground.paved.borrow_mut().retain(|q| *q != p);
                    }
                    Effect::Pave(p) => ground.paved.borrow_mut().push(p),
                    _ => {}
                }
            }
            all.extend(effects);
        }
        all
    }

    #[test]
    fn a_roller_paves_under_its_drum_and_clears_its_tunnel() {
        // Tunnel mode over a gap at x 2 and stone in the way at x 4 (drum level y 1) and y 2.
        let mut ground = Ground {
            solid: HashMap::from([([4, 1, 0], 1.5), ([4, 2, 0], 1.5)]),
            paved: Default::default(),
            material: 64,
            loaded_below: -100,
        };
        // The ground of x 1..5 is solid except x 2.
        for x in [1, 3, 4, 5, 6] {
            ground.solid.insert([x, 0, 0], 0.6);
        }
        let effects = roll(&mut ground, RollingMode::TunnelPave, 300);
        let paved: Vec<[i32; 3]> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Pave(p) => Some(*p),
                _ => None,
            })
            .collect();
        let destroyed: Vec<[i32; 3]> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Destroy(p) => Some(*p),
                _ => None,
            })
            .collect();
        // With material, each ground block is broken and paved again; the gap is paved; the
        // stone at drum level and its own level is broken.
        assert!(paved.contains(&[2, 0, 0]), "{paved:?}");
        assert!(destroyed.contains(&[4, 1, 0]) && destroyed.contains(&[4, 2, 0]));
        assert!(destroyed.contains(&[3, 0, 0]) && paved.contains(&[3, 0, 0]));
        assert!(ground.paved.borrow().contains(&[4, 0, 0]));
    }

    #[test]
    fn rollers_fill_down_straight_or_wide() {
        // Nothing under it down to y -3 (solid below, at y -4).
        let mut ground = Ground {
            solid: HashMap::new(),
            paved: Default::default(),
            material: 1000,
            loaded_below: -3,
        };
        let effects = roll(&mut ground, RollingMode::StraightFill, 40);
        let at_x1: Vec<[i32; 3]> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Pave(p) if p[0] == 1 => Some(*p),
                _ => None,
            })
            .collect();
        // One layer a visit from the top down (filled layers pass), each placing holding it 2
        // ticks and visiting again, down to the blocked one.
        assert_eq!(at_x1, vec![[1, 0, 0], [1, -1, 0], [1, -2, 0], [1, -3, 0]]);
        // Wide: the layers below the first spread a plus of five (radius `(depth + 1) / 2`).
        let mut ground = Ground {
            solid: HashMap::new(),
            paved: Default::default(),
            material: 1000,
            loaded_below: -2,
        };
        let effects = roll(&mut ground, RollingMode::WideFill, 20);
        let at_x1: Vec<[i32; 3]> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Pave(p) if p[1] < 0 || p[0] == 1 => Some(*p),
                _ => None,
            })
            .take(11)
            .collect();
        assert_eq!(
            at_x1,
            vec![
                [1, 0, 0],
                [0, -1, 0],
                [1, -1, -1],
                [1, -1, 0],
                [1, -1, 1],
                [2, -1, 0],
                [0, -2, 0],
                [1, -2, -1],
                [1, -2, 0],
                [1, -2, 1],
                [2, -2, 0]
            ]
        );
        // No material: nothing.
        let mut ground = Ground {
            solid: HashMap::new(),
            paved: Default::default(),
            material: 0,
            loaded_below: -2,
        };
        assert!(roll(&mut ground, RollingMode::StraightFill, 40).is_empty());
    }
}
