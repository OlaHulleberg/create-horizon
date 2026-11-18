//! Create's state for one open world: what each Create block state is (decoded once), and each
//! dimension's kinetic network. The game glue feeds it block changes and chunk loads; nothing here
//! runs per tick unless something changed.

use crate::Block;
use crate::block_entity_order::{BlockEntityOrder, View, Walk};
use crate::kinetics::{Event, Kinetics, SolveStats};
use crate::parts::{Settings, StateInfo, is_generating, is_kinetic, parts};
use crate::states;
use std::collections::{HashMap, HashSet};

fn entity_view(block: Block) -> Option<View> {
    match block {
        Block::Depot
        | Block::Belt
        | Block::Spout
        | Block::ItemDrain
        | Block::MechanicalPump
        | Block::FluidPipe
        | Block::GlassFluidPipe
        | Block::EncasedFluidPipe
        | Block::SmartFluidPipe => Some(View::Transport),
        Block::FluidValve | Block::FluidTank | Block::CreativeFluidTank => Some(View::Transport),
        _ => None,
    }
}

/// Whether Create keeps the block entity when `old` is replaced by `new`: the same block, or one
/// whose `onRemove` keeps it for its own kind (`ValveHandleBlock`, `NixieTubeBlock` and
/// `ToolboxBlock` for another colour; `FunnelBlock` and `BeltFunnelBlock` for the other form of
/// the same material, `isOfSameType`).
fn keeps_block_entity(old: Block, new: Block) -> bool {
    use crate::dye::{is_nixie_tube, is_toolbox, is_valve_handle};
    let funnel = |b| match b {
        Block::AndesiteFunnel | Block::AndesiteBeltFunnel => Some(false),
        Block::BrassFunnel | Block::BrassBeltFunnel => Some(true),
        _ => None,
    };
    old == new
        || (is_valve_handle(old) && is_valve_handle(new))
        || (is_nixie_tube(old) && is_nixie_tube(new))
        || (is_toolbox(old) && is_toolbox(new))
        || funnel(old).is_some_and(|brass| funnel(new) == Some(brass))
}

/// `KineticBlock.areStatesKineticallyEquivalent` and its block overrides.
fn kinetically_equivalent(a: Block, x: &StateInfo, b: Block, y: &StateInfo) -> bool {
    let axis = |b, s| parts(b, s, &Settings::default())[0].map(|p| p.axis);
    a == b
        && axis(a, x) == axis(b, y)
        && match b {
            Block::SequencedGearshift => false,
            Block::AdjustableChainGearshift | Block::GantryShaft => x.powered == y.powered,
            Block::AndesiteEncasedCogwheel
            | Block::BrassEncasedCogwheel
            | Block::AndesiteEncasedLargeCogwheel
            | Block::BrassEncasedLargeCogwheel => {
                x.top_shaft == y.top_shaft && x.bottom_shaft == y.bottom_shaft
            }
            Block::Belt => x.belt.map(|s| s.part) == y.belt.map(|s| s.part),
            _ => true,
        }
}

/// Every Create block state, by `state - first`: its block, its decoded properties, and whether it
/// holds kinetic parts. Built once when a world opens, never per tick.
pub struct StateTable {
    first: u32,
    block: Vec<Block>,
    info: Vec<StateInfo>,
    kinetic: Vec<bool>,
    entity: Vec<bool>,
}

impl StateTable {
    /// From the registry's states `first..first + n`: each name (`create:shaft`) and properties.
    pub fn new<'a, P>(first: u32, states: impl Iterator<Item = (&'a str, P)>) -> StateTable
    where
        P: Iterator<Item = (&'a str, &'a str)>,
    {
        let mut t = StateTable {
            first,
            block: Vec::new(),
            info: Vec::new(),
            kinetic: Vec::new(),
            entity: Vec::new(),
        };
        for (name, props) in states {
            let block = name
                .strip_prefix("create:")
                .and_then(Block::from_name)
                // A Create id this build doesn't know (another version's): inert.
                .unwrap_or(Block::AndesiteCasing);
            t.kinetic.push(keeps_state(block));
            t.block.push(block);
            // AllBlockEntityTypes supplies block eligibility; these newBlockEntity
            // overrides additionally decide whether this particular state creates one:
            // TrackBlock requires HAS_BE; SlidingDoorBlock excludes the upper half.
            let mut track_entity = false;
            let mut upper = false;
            t.info.push(StateInfo::read(props.inspect(|&(key, value)| {
                if key == "has_be" {
                    track_entity = value == "true";
                }
                if key == "half" {
                    upper = value == "upper";
                }
            })));
            t.entity.push(
                block.has_block_entity()
                    && (block != Block::Track || track_entity)
                    && (!crate::dye::is_sliding_door(block) || !upper),
            );
        }
        t
    }

    /// The Create block of a state id, if it is one.
    #[inline]
    pub fn get(&self, state: u32) -> Option<(Block, &StateInfo)> {
        let i = states::index(state)? as usize;
        Some((*self.block.get(i)?, self.info.get(i)?))
    }

    /// Whether a state is a Create block this keeps (kinetic, or with state of its own like a
    /// depot): the bit test of the hot hooks.
    #[inline]
    pub fn is_kinetic(&self, state: u32) -> bool {
        states::index(state).is_some_and(|i| self.kinetic.get(i as usize).copied().unwrap_or(false))
    }

    /// Whether this state creates an entity (TrackBlock.HAS_BE and lower door halves included).
    fn has_block_entity(&self, state: u32) -> bool {
        states::index(state).is_some_and(|i| self.entity.get(i as usize).copied().unwrap_or(false))
    }

    pub fn first(&self) -> u32 {
        self.first
    }

    pub fn len(&self) -> usize {
        self.block.len()
    }

    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }
}

/// One dimension's Create state.
#[derive(Default)]
pub struct Dimension {
    pub kinetics: Kinetics,
    /// Kinetic block positions per loaded column, to forget them when it unloads.
    columns: HashMap<(i32, i32), Vec<[i32; 3]>>,
    /// Block entity settings that change a part (a motor's speed), by position.
    settings: HashMap<[i32; 3], Settings>,
    /// The state of each kinetic block, by position.
    states: HashMap<[i32; 3], u32>,
    /// Columns whose saved data changed, to mark unsaved when the dimension is in place.
    pub unsaved: Vec<(i32, i32)>,
    /// Java crank/valve block entity use state, including the unsaved valve cooldown.
    handles: HashMap<[i32; 3], crate::valve::Handle>,
    /// Handles whose saved use state changed since the glue last wrote it.
    handle_changes: HashSet<[i32; 3]>,
    /// Sliding doors whose `open` changed (position, open before), for the glue to apply
    /// Create's rules after the tick; and the lower halves whose panel still slides
    /// (`SlidingDoorBlockEntity.animation`), the only doors that tick.
    pub door_changes: Vec<([i32; 3], bool)>,
    pub doors: HashMap<[i32; 3], crate::door::Slide>,
    /// Millstones and their state (`MillstoneBlockEntity.timer`, last recipe).
    pub millstones: HashMap<[i32; 3], crate::processing::Millstone>,
    /// Blocks whose settings changed (a generator's speed may have): the sources tell their
    /// clients what they generate.
    pub generator_changes: Vec<[i32; 3]>,
    /// Water wheels, to look at their flow every 60 ticks (`lazyTick`), staggered by position.
    wheels: Vec<[i32; 3]>,
    /// Water wheels to look at now: placed, or a neighbour changed.
    wheel_checks: Vec<[i32; 3]>,
    /// Large water wheels placed or whose neighbour changed: their structure and `extension`
    /// are looked at after the tick (`LargeWaterWheelBlock.onPlace`'s scheduled tick,
    /// `updateShape`); large wheels removed (with their axis), whose structure goes; structure
    /// blocks removed (with the way they pointed), whose wheel breaks.
    pub large_wheel_checks: Vec<[i32; 3]>,
    pub large_wheels_gone: Vec<([i32; 3], crate::kinetics::Axis)>,
    pub wheel_structures_gone: Vec<([i32; 3], crate::kinetics::Dir)>,
    /// Blocks whose redstone power may have changed (a clutch or gearshift that was placed or
    /// whose neighbour changed), checked after the level's tick.
    redstone: Vec<[i32; 3]>,
    /// Encased chain drives whose place in their chain may have changed (placed, or a neighbour
    /// changed), consumed during the block change (`ChainDriveBlock.updateShape`).
    chain_checks: Vec<[i32; 3]>,
    /// Sequenced gearshifts and their sequence (`SequencedGearshiftBlockEntity`).
    pub sequencers: HashMap<[i32; 3], crate::sequencer::Sequencer>,
    /// Sequenced gearshifts whose instruction runs: the only ones that tick.
    sequencing: Vec<[i32; 3]>,
    /// Sequenced gearshifts whose neighbour changed: their redstone is looked at after the
    /// level's tick (Create's scheduled block tick).
    sequencer_checks: Vec<[i32; 3]>,
    /// Steps sequenced gearshifts took when their speed changed, for their blocks to show.
    sequencer_steps: Vec<([i32; 3], crate::sequencer::Step)>,
    /// Depots and their held stack's motion; the stacks are in their block entities' slots.
    pub depots: HashMap<[i32; 3], Option<crate::depot::Held>>,
    /// Depots whose held stack still slides.
    pub sliding: Vec<[i32; 3]>,
    block_entities: BlockEntityOrder,
    /// Depots whose contents clients must see again (put down, taken, filled by a hopper).
    pub depot_changes: Vec<[i32; 3]>,
    /// Mechanical presses and their cycle.
    pub presses: HashMap<[i32; 3], crate::press::Press>,
    /// Depots that ask the press above them each tick (`DepotBehaviour.tick` after its stack
    /// arrived), until the press has nothing to do with them.
    pub depot_work: Vec<[i32; 3]>,
    /// Presses whose cycle clients must see again (started, head down, done).
    pub press_changes: Vec<[i32; 3]>,
    /// Belts by controller, and each segment's controller.
    pub belts: HashMap<[i32; 3], Belt>,
    pub belt_of: HashMap<[i32; 3], [i32; 3]>,
    /// Belt segments placed or loaded whose chain is looked for after the tick.
    belt_dirty: Vec<[i32; 3]>,
    /// Belts whose items or shape clients must see again.
    pub belt_changes: Vec<[i32; 3]>,
    /// Controllers whose persisted inventory changed since publication.
    belt_saved_changes: std::collections::HashSet<[i32; 3]>,
    /// Segments to replace because their neighbour broke, and items a broken belt drops (with
    /// the controller and segment they were on).
    pub belt_breaks: Vec<BeltBreak>,
    pub belt_drops: Vec<([i32; 3], crate::belt::Segment, i32, crate::belt::Item)>,
    /// Items saved with a belt, kept until its chain is whole again after loading.
    belt_saved: HashMap<[i32; 3], crate::belt::Inventory>,
    /// Belt segments the belt slicer takes away (`UPDATE_MOVE_BY_PISTON`): their removal
    /// breaks no neighbour and drops nothing.
    belt_quiet: std::collections::HashSet<[i32; 3]>,
    /// Latches whose inputs may have changed, looked at after the tick
    /// (`checkTickOnNeighbor`), and their scheduled ticks (position, game time due).
    pub latch_checks: Vec<[i32; 3]>,
    pub latch_ticks: Vec<([i32; 3], i64)>,
    /// Brass diodes' block entities; the ones whose tick changes something (the only ones that
    /// tick); what was saved for those not placed yet (`state`, delay); diodes whose input may
    /// have changed, with the delay of the tick that follows (`checkTickOnNeighbor`,
    /// `setPlacedBy`); and their scheduled block ticks (position, game time due).
    pub diodes: HashMap<[i32; 3], crate::diode::Diode>,
    pub diode_running: Vec<[i32; 3]>,
    diode_saved: HashMap<[i32; 3], (i32, i32)>,
    pub diode_checks: Vec<([i32; 3], i64)>,
    pub diode_ticks: Vec<([i32; 3], i64)>,
    /// Rose quartz lamps just placed (`getStateForPlacement`: `powered` as placed), those whose
    /// neighbours changed, and their scheduled ticks (position, game time due).
    pub lamp_placed: Vec<[i32; 3]>,
    pub lamp_checks: Vec<[i32; 3]>,
    pub lamp_ticks: Vec<([i32; 3], i64)>,
    /// Fluid tanks: each tank block's controller and each controller's tank; tanks saved with
    /// a column wait here for their controller.
    pub tanks: crate::tank::Tanks,
    tank_saved: HashMap<[i32; 3], crate::tank::SavedTank>,
    /// Creative fluid tanks joined as tanks are (`CreativeFluidTankBlockEntity` is a
    /// `FluidTankBlockEntity` of its own type): each controller's size and fluid; the fluid of
    /// each block is also `plumbing.creative`'s. Saved sizes wait here for their controller.
    pub creative_tanks: crate::tank::Tanks,
    creative_tank_saved: HashMap<[i32; 3], crate::tank::Multi>,
    /// Tank blocks clients must see again (a controller's fluid, or none).
    pub tank_changes: Vec<[i32; 3]>,
    /// Tank blocks whose `top`, `bottom` and `shape` follow their tank again.
    pub tank_states: Vec<[i32; 3]>,
    /// Windmill bearings whose sails are assembled, by bearing; ones saved with a column wait
    /// for their bearing; right-clicks waiting for the next tick (`assembleNextTick`); bearings
    /// clients must see again; and structures whose bearing broke, to put back.
    pub windmills: HashMap<[i32; 3], Windmill>,
    windmill_saved: HashMap<[i32; 3], Windmill>,
    pub windmill_assemble: Vec<[i32; 3]>,
    pub windmill_changes: Vec<[i32; 3]>,
    pub windmill_orphans: Vec<([i32; 3], Windmill)>,
    /// Encased fans, and the ones turning: only those tick.
    pub fans: HashMap<[i32; 3], Fan>,
    pub running_fans: Vec<[i32; 3]>,
    /// Nozzles (`NozzleBlockEntity`): they act only while the fan behind them ticks; those whose
    /// neighbour changed look at their fan after the tick (`neighborChanged`, `canSurvive`).
    pub nozzles: HashMap<[i32; 3], crate::nozzle::Nozzle>,
    pub nozzle_checks: Vec<[i32; 3]>,
    /// Item entities' fan progress (Create keeps it in their persistent data), by entity id; kept
    /// while they are in a current.
    pub item_fan: HashMap<u32, crate::fan::Progress>,
    /// Crushing wheels whose controllers must be looked at (placed, removed, speed changed), and
    /// the controllers: their state and crushing speed; the turning ones tick. Contents saved with
    /// a column wait for their controller.
    pub crusher_checks: Vec<[i32; 3]>,
    pub crushers: HashMap<[i32; 3], (crate::crusher::Crusher, f32)>,
    pub running_crushers: Vec<[i32; 3]>,
    crusher_saved: HashMap<[i32; 3], crate::crusher::Crusher>,
    /// Blaze burners' fuel, the burning ones (the only ones that tick), and fuel saved with a
    /// column waiting for its burner.
    pub burners: HashMap<[i32; 3], crate::burner::Burner>,
    pub burning: Vec<[i32; 3]>,
    /// Burners whose block changed: their heat is checked against their fuel's in the next tick
    /// (`BlazeBurnerBlockEntity.tick`'s `updateBlockState`, which a burner set by a command meets).
    pub burner_checks: Vec<[i32; 3]>,
    burner_saved: HashMap<[i32; 3], crate::burner::Burner>,
    /// Redstone links and their networks; the saved frequencies and input of links not placed
    /// yet; links whose input to read again (placed, a neighbour changed, toggled); receivers
    /// whose signal changed (their block and its neighbours to update).
    pub links: crate::link::Links,
    /// Curved track at its track blocks (`TrackBlockEntity.connections`).
    pub track_curves: crate::track_curves::Curves,
    link_saved: HashMap<[i32; 3], (crate::link::Key, i32)>,
    pub link_checks: Vec<[i32; 3]>,
    pub link_changes: Vec<[i32; 3]>,
    /// Links whose frequencies clients should see again (tuned, loaded, gone).
    pub link_shown: Vec<[i32; 3]>,
    /// Frequencies held down on linked controllers (`LinkedControllerServerHandler`), and the
    /// lecterns holding a controller.
    pub controllers: crate::controller::Server,
    pub lecterns: HashMap<[i32; 3], crate::controller::Lectern>,
    /// Andesite tunnels to shape again: after their own neighbour changed (`updateShape`, a
    /// state that still fits is kept), and after their belt's did or they were placed
    /// (`updateTunnel`, `getStateForPlacement`: the new shape at once); those just placed
    /// (their belt gets a casing).
    pub tunnel_checks: Vec<[i32; 3]>,
    /// Deployers and their cycle and held stack, those mid-stroke or waiting out their pause
    /// (the only ones that tick), what was saved for those not placed yet, and those whose
    /// stroke clients must see again; the cracks of removed ones (deployer, block) to clear.
    pub deployers: HashMap<[i32; 3], crate::deployer::Deployer>,
    pub deploying: Vec<[i32; 3]>,
    deployer_saved: HashMap<[i32; 3], crate::deployer::Deployer>,
    brass_saved: HashMap<[i32; 3], crate::tunnel::Brass>,
    pub deployer_changes: Vec<[i32; 3]>,
    pub deployer_drops: Vec<([i32; 3], (String, u8))>,
    pub deployer_cracks: Vec<([i32; 3], [i32; 3])>,
    /// Gantry shafts whose place in their line may have changed (`updateShape`), carriages to try
    /// to start (`queueAssembly`), and the moving gantries.
    pub gantry_checks: Vec<[i32; 3]>,
    /// Mechanical crafters and their phase and grid, those at work (the only ones that tick),
    /// those to look at (an item put in, turning), and what broken ones held.
    pub crafters: HashMap<[i32; 3], crate::crafter::Crafter>,
    pub crafting: Vec<[i32; 3]>,
    pub crafter_checks: Vec<[i32; 3]>,
    pub crafter_drops: Vec<([i32; 3], String)>,
    /// Crafters joined with the wrench (`ConnectedInput`), saved with their chunk; one alone
    /// has no entry.
    pub crafter_inputs: crate::crafter_input::Inputs,
    /// Crafters with a slot cover (`covered`), saved with their chunk.
    pub crafter_covers: HashSet<[i32; 3]>,
    /// Crafters whose neighbours changed, to look at their power; those seen unpowered (any
    /// other counts as powered before, as Create's `wasPoweredBefore` starts).
    pub crafter_power_checks: Vec<[i32; 3]>,
    pub crafter_unpowered: HashSet<[i32; 3]>,
    /// Deployers whose neighbours changed, to look at their power (`redstoneLocked`).
    pub deployer_power_checks: Vec<[i32; 3]>,
    pub gantry_assemble: Vec<[i32; 3]>,
    pub gantries: Vec<crate::gantry::Moving>,
    /// Mechanical bearings whose speed changed (`assembleNextTick`).
    pub bearing_checks: Vec<[i32; 3]>,
    pub tunnel_fresh: Vec<[i32; 3]>,
    /// Brass tunnels and what they hold to hand out, and those holding a stack (the only ones
    /// that tick).
    pub brass: HashMap<[i32; 3], crate::tunnel::Brass>,
    pub brass_running: Vec<[i32; 3]>,
    /// The loaded basins, whose spout output looks for somewhere to go (`lazyTick`).
    pub basins: HashSet<[i32; 3]>,
    /// Basins' fluid tanks (`BasinBlockEntity`'s inputs and outputs).
    pub basin_fluids: HashMap<[i32; 3], crate::basin::Tanks>,
    /// Basins whose tanks changed: their clients are told (taken by the glue).
    pub basin_changes: Vec<[i32; 3]>,
    /// Linear chassis ranges set with the wrench (`ChassisScrollValueBehaviour`; others 8).
    pub chassis_range: HashMap<[i32; 3], i32>,
    /// The option picked on a contraption anchor's value box (`ScrollOptionBehaviour`): a
    /// bearing's or piston's movement mode, a windmill's direction, a clock's hands; 0 (the
    /// first option) when none is kept.
    pub scroll_options: HashMap<[i32; 3], u8>,
    pub tunnels_new: Vec<[i32; 3]>,
    /// Mechanical mixers and their cycle, the running ones (the only ones that tick), and those
    /// to look at their basin again (`basinChecker`: placed, speed changed, basin changed).
    pub mixers: HashMap<[i32; 3], crate::basin::Mixer>,
    pub mixing: Vec<[i32; 3]>,
    pub mixer_checks: Vec<[i32; 3]>,
    /// Presses compacting in the basin below them, with the recipe (`currentRecipe`).
    pub press_basin: HashMap<[i32; 3], usize>,
    /// Pumps and what their pipes reach, the turning ones (the only ones that tick), pipes to
    /// connect again, and whether pipes, pumps or tanks changed since the ends were found.
    pub fluid_transport: crate::fluids::runtime::Transport,
    pub pipe_checks: Vec<[i32; 3]>,
    /// Fluid valves, smart pipe filters, hose pulleys, creative fluid tanks, steam whistles.
    pub plumbing: crate::plumbing::Plumbing,
    /// Mechanical pistons: those asked to start or stop (`assembleNextTick`, after a speed
    /// change), and the moving ones with what they carry (the only ones that tick).
    pub piston_checks: Vec<[i32; 3]>,
    pub pistons: HashMap<[i32; 3], Moving>,
    /// Moving pistons whose block went (`remove` → `disassemble`): their structures go back.
    pub piston_orphans: Vec<([i32; 3], Moving)>,
    /// Super glue areas (`SuperGlueEntity`), saved with the column holding each.
    pub glue: Vec<crate::glue::Glue>,
    /// Stickers whose neighbour changed: their power is read again after the level's tick.
    pub sticker_checks: Vec<[i32; 3]>,
    /// Clockwork bearings, rope pulleys, elevators and their contacts (`lifts.rs`).
    pub lifts: crate::lifts::Lifts,
    /// Display links, the nixie tubes and display boards they write to (`display.rs`).
    pub displays: crate::display::Displays,
    /// Analog levers (`lever.rs`), smart observers and threshold switches (`observe.rs`).
    pub levers: crate::lever::Levers,
    pub detectors: crate::observe::Detectors,
    /// Track, stations and signals, the graph built from them, and the trains on it.
    pub railway: crate::trains::Railway,
    /// Chutes holding a stack: where it is (1 top, 0 bottom); the only ones that tick.
    pub chutes: HashMap<[i32; 3], f32>,
    /// Empty chutes that may take from the inventory above them.
    pub chute_pulls: crate::chute::Pulls,
    /// Smart chutes' filters, saved with their column.
    pub chute_filters: HashMap<[i32; 3], crate::filter::Filter>,
    /// The amount boards of brass funnels' and smart chutes' filter slots that were set (the
    /// others take up to a stack), saved with their column and kept by the same blocks as
    /// `chute_filters`.
    pub filter_amounts: HashMap<[i32; 3], crate::board::FilterAmount>,
    /// Chutes whose shape or facing is looked at again after the tick
    /// (`AbstractChuteBlock.updateDiagonalNeighbour` and `onRemove` schedule them a tick).
    pub chute_shapes: Vec<[i32; 3]>,
    /// Brass tunnels' side filters (`SidedFilteringBehaviour`), by tunnel and side.
    pub tunnel_filters: HashMap<([i32; 3], crate::kinetics::Dir), crate::filter::Filter>,
    /// Filter items whose block was broken (`FilteringBehaviour.destroy`): the glue drops them.
    pub dropped_filters: Vec<([i32; 3], crate::filter::Filter)>,
    /// Item vault blocks and the vaults they form.
    pub vaults: crate::vault::Vaults,
    /// Drills and their breaking, the turning ones (the only ones that tick).
    pub drills: HashMap<[i32; 3], crate::drill::Breaker>,
    pub drilling: Vec<[i32; 3]>,
    /// Cuckoo clocks that turn (the only ones that look at the time), and the animations
    /// running: what, and its progress.
    pub cuckoo_turning: Vec<[i32; 3]>,
    pub cuckoos: HashMap<[i32; 3], (crate::cuckoo::Animation, u32)>,
    /// Saws holding items (the only ones that tick) and their processing.
    pub saws: HashMap<[i32; 3], crate::saw::Saw>,
    /// What removed saws held (`SawBlockEntity.destroy` drops its inventory).
    pub saw_drops: Vec<([i32; 3], String, u8)>,
    /// Spouts and their tank and cycle; those holding fluid tick.
    pub spouts: HashMap<[i32; 3], crate::spout::Spout>,
    /// Item drains, their held stack and tank; those holding items tick.
    pub drains: HashMap<[i32; 3], crate::drain::Drain>,
    /// Funnels and belt funnels and the game tick each looks again at (`extractionCooldown`; 0
    /// now, `u64::MAX` dormant until its state changes), and those times in order: only due
    /// funnels do anything.
    pub funnels: HashMap<[i32; 3], u64>,
    pub funnel_timers: crate::funnel::Timers,
    /// Funnels whose block below changed (`updateShape`): a horizontal funnel over a belt becomes
    /// a belt funnel, a belt funnel off its belt a funnel again, and a belt funnel's shape
    /// follows the belt.
    pub funnel_shapes: Vec<[i32; 3]>,
    /// Mechanical arms: their points, phase and held stack, the awake ones and what wakes them;
    /// what removed ones held.
    pub arms: crate::arm::Arms,
    pub arm_drops: Vec<([i32; 3], String, u8)>,
    /// Arms and weighted ejectors whose redstone input is read again (placed, a neighbour
    /// changed: `redstoneUpdate`, `updateSignal`).
    pub arm_signals: Vec<[i32; 3]>,
    /// Weighted ejectors (their launcher, plate and stacks in flight; the stacks on them are
    /// their depot's), those that tick, what was saved for those not placed yet, those clients
    /// must see again, and stacks in flight of removed ones to drop (position, motion, item).
    pub ejectors: HashMap<[i32; 3], crate::ejector::Ejector>,
    pub ejecting: Vec<[i32; 3]>,
    pub ejector_saved: HashMap<[i32; 3], crate::ejector::Ejector>,
    pub ejector_changes: Vec<[i32; 3]>,
    pub ejector_drops: Vec<([f64; 3], [f64; 3], String, u8)>,
    /// Speedometers and stressometers and what clients were last told, and those clients must
    /// be told of again (their needle moved after a solve).
    pub gauges: HashMap<[i32; 3], crate::gauge::Gauge>,
    pub gauge_changes: Vec<[i32; 3]>,
    /// Scratch for the gauges a solve looked at, kept to avoid allocating per solve.
    pub gauges_touched: Vec<[i32; 3]>,
    /// Blocks whose comparator output may have changed (a vault's or tank's block stands for
    /// the whole vault or tank): read again after the level's tick, comparators told when it
    /// moved (`setChanged`, `updateNeighbourForOutputSignal`). Nothing runs while empty.
    pub analog_checks: Vec<[i32; 3]>,
    /// The output comparators were last told of, for blocks whose contents change every tick
    /// while worked (fluid tanks, spouts, item drains): told again only when it moves.
    pub analog_levels: HashMap<[i32; 3], u8>,
    /// Boilers: tanks with steam engines on them (`BoilerData.isActive`), with those engines.
    /// Only they tick.
    pub boilers: HashMap<[i32; 3], SteamBoiler>,
    /// Tanks whose engines are counted again (`FluidTankBlockEntity.updateBoilerState`).
    pub boiler_checks: Vec<[i32; 3]>,
    /// Boilers whose heat is read again (`needsHeatLevelUpdate`).
    pub boiler_heat_checks: Vec<[i32; 3]>,
    /// Scratch for the boilers sampling their supply this tick, kept to not allocate.
    pub boiler_sampled: Vec<[i32; 3]>,
    /// Where engines placed or removed have their shaft: it becomes a powered shaft or a plain
    /// one again (`SteamEngineBlock.onPlace`, `PoweredShaftBlock.tick`).
    pub engine_shafts: Vec<[i32; 3]>,
    /// Powered shafts and the engine driving each (`PoweredShaftBlockEntity.enginePos`).
    pub shaft_engine: HashMap<[i32; 3], [i32; 3]>,
    /// Backtanks and their air (`BacktankBlockEntity`); the saved ones not placed yet.
    pub backtanks: HashMap<[i32; 3], crate::equipment::Backtank>,
    backtank_saved: HashMap<[i32; 3], crate::equipment::Backtank>,
    /// Stationary portable storage interfaces (`PortableStorageInterfaceBlockEntity`), those
    /// whose timer runs (ticked), and those whose neighbour changed (their power read).
    pub psis: HashMap<[i32; 3], crate::psi::Stationary>,
    pub psi_running: Vec<[i32; 3]>,
    pub psi_checks: Vec<[i32; 3]>,
    /// Contraption controls blocks (filter, `Disabled`, `Powered`) and those whose neighbour
    /// changed; mechanical rollers' filter and mode; track observers' filters.
    pub controls: HashMap<[i32; 3], crate::actors::Controls>,
    pub controls_checks: Vec<[i32; 3]>,
    pub rollers: HashMap<[i32; 3], (Option<String>, crate::actors::RollingMode)>,
    pub observer_filters: HashMap<[i32; 3], crate::filter::Filter>,
    /// Controller rails whose neighbour changed: their power is worked out again.
    pub rail_checks: Vec<[i32; 3]>,
}

impl Dimension {
    /// A deployer about to be put back at `pos` by a contraption: placed, it takes `d`.
    pub fn deployer_saved_insert(&mut self, pos: [i32; 3], d: crate::deployer::Deployer) {
        self.deployer_saved.insert(pos, d);
    }

    /// What waited at `pos` for a deployer that was not placed there.
    pub fn deployer_saved_remove(&mut self, pos: [i32; 3]) -> Option<crate::deployer::Deployer> {
        self.deployer_saved.remove(&pos)
    }
}

pub use crate::trains::TrainOnTrack;

/// A piston's moving structure (`ControlledContraptionEntity` of a `PistonContraption`): its
/// blocks (offset from the anchor at offset 0, state), how far it is out and can go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moving {
    pub anchor: [i32; 3],
    pub blocks: Vec<([i32; 3], u32)>,
    pub offset: f32,
    pub range: i32,
    /// Its actors, glue and turned-off actor kinds (`Contraption`).
    pub carried: crate::actors::Carried,
    /// `initialExtensionProgress`: the offset it started from.
    pub initial: i32,
    /// The piston's facing (its structure goes back along it when the piston is gone).
    pub facing: Option<crate::kinetics::Dir>,
    /// `waitingForSpeedChange`: stopped and kept together (its movement mode), until its speed
    /// changes.
    pub waiting: bool,
    /// `sequencedOffsetLimit`: how far a sequenced gearshift's turn by distance still lets it
    /// move (none for no limit), saved with it.
    pub limit: Option<f32>,
}

impl Moving {
    /// `MechanicalPistonBlockEntity.getMovementSpeed`: [`crate::piston::movement`] held to
    /// the limit left.
    pub fn step(&self, rpm: f32, facing: crate::kinetics::Dir) -> f32 {
        let v = crate::piston::movement(rpm, facing, self.offset, self.range);
        match self.limit {
            Some(l) => v.clamp(-l, l),
            None => v,
        }
    }
}

/// An assembled windmill: its blocks (offset from the bearing, state) and its sails.
#[derive(Clone, Debug, PartialEq)]
pub struct Windmill {
    pub blocks: Vec<([i32; 3], u32)>,
    pub sails: u32,
    pub clockwise: bool,
    /// A mechanical bearing's turning; None for a windmill (which generates instead).
    pub bearing: Option<crate::contraption::Turn>,
    /// What it carries besides its blocks, storage and actors (its actors are found again from
    /// its blocks): its contraption controls, the kinds they turned off and its glue.
    pub extras: crate::actors::Carried,
}

/// An encased fan (`EncasedFanBlockEntity`): its facing, speed and air current, and the depots
/// and belt segments in the current with the processing each gets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fan {
    pub facing: Option<crate::kinetics::Dir>,
    pub speed: f32,
    pub current: crate::fan::Current,
    /// `airCurrentUpdateCooldown`: ticks until the current is looked at again.
    pub cooldown: u32,
    /// `updateAirFlow`: rebuild the current this tick.
    pub rebuild: bool,
    pub handlers: Vec<([i32; 3], Option<crate::fan::Kind>)>,
    /// Clients must see its propeller and current again.
    pub changed: bool,
    /// `entitySearchCooldown` and the item entities its last search caught (`caughtEntities`).
    pub search_cooldown: u8,
    pub caught: Vec<u32>,
    /// The other entities it caught (mobs, players, carts...), pushed by entity id.
    pub others: Vec<u32>,
}

/// Whether the world keeps state for `block`: a kinetic part, or a block with state of its own
/// (a depot, a sequenced gearshift). These are the blocks the glue gives a block entity.
pub fn keeps_state(block: Block) -> bool {
    is_kinetic(block) || has_state(block)
}

/// Blocks that keep state of their own here but are not kinetic: depots.
fn has_state(block: Block) -> bool {
    matches!(
        block,
        Block::Depot
            | Block::AndesiteFunnel
            | Block::BrassFunnel
            | Block::AndesiteBeltFunnel
            | Block::BrassBeltFunnel
            | Block::ItemDrain
            | Block::Spout
            | Block::Chute
            | Block::SmartChute
            | Block::ItemVault
            | Block::FluidPipe
            | Block::Basin
            | Block::BlazeBurner
            | Block::CrushingWheelController
            | Block::Belt
            | Block::PoweredLatch
            | Block::PoweredToggleLatch
            | Block::PulseRepeater
            | Block::PulseExtender
            | Block::PulseTimer
            | Block::RoseQuartzLamp
            | Block::RedstoneLink
            | Block::AndesiteTunnel
            | Block::BrassTunnel
            | Block::TrackSignal
            | Block::Track
            | Block::TrackStation
            | Block::GantryShaft
            | Block::GantryCarriage
            | Block::MechanicalCrafter
            | Block::FluidTank
            | Block::WindmillBearing
            | Block::SteamEngine
            | Block::ElevatorContact
            | Block::RedstoneContact
            | Block::DisplayLink
            | Block::DisplayBoard
            | Block::AnalogLever
            | Block::WaterWheelStructure
            | Block::ContentObserver
            | Block::StockpileSwitch
            | Block::Sticker
            | Block::TrackObserver
            | Block::PortableStorageInterface
            | Block::PortableFluidInterface
            | Block::ContraptionControls
            | Block::LecternController
            | Block::MechanicalRoller
            | Block::MechanicalHarvester
            | Block::ControllerRail
            | Block::CartAssembler
            | Block::Nozzle
            | Block::FactoryGauge
            | Block::StockTicker
            | Block::PackageFrogport
            | Block::DeskBell
            | Block::Packager
            | Block::Repackager
            | Block::Controls
            | Block::PeculiarBell
            | Block::HauntedBell
    ) || crate::display::is_nixie(block)
        || crate::dye::is_postbox(block)
        || is_plumbing(block)
}

/// Pipes, tanks and whistles with state of their own (`plumbing.rs`).
fn is_plumbing(block: Block) -> bool {
    matches!(
        block,
        Block::FluidValve
            | Block::SmartFluidPipe
            | Block::GlassFluidPipe
            | Block::EncasedFluidPipe
            | Block::HosePulley
            | Block::CreativeFluidTank
            | Block::SteamWhistle
            | Block::SteamWhistleExtension
    )
}

/// Blocks fluid passes through or is kept in: a change to one makes pumps look for their ends
/// again and the pipes beside it connect again (`FluidPropagator.propagateChangedPipe`).
pub fn is_fluid_block(block: Block) -> bool {
    matches!(
        block,
        Block::FluidPipe
            | Block::MechanicalPump
            | Block::FluidTank
            | Block::Spout
            | Block::ItemDrain
            | Block::FluidValve
            | Block::SmartFluidPipe
            | Block::GlassFluidPipe
            | Block::EncasedFluidPipe
            | Block::HosePulley
            | Block::CreativeFluidTank
    )
}

/// The way a steam whistle's state is attached (`WhistleBlock.getAttachedDirection`).
fn whistle_attached(info: &StateInfo) -> Option<crate::kinetics::Dir> {
    Some(crate::whistle::attached(info.wall, info.facing?))
}

/// A boiler (`FluidTankBlockEntity.boiler` of a controller): its data, the engines on its tank
/// and its tank's size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SteamBoiler {
    pub data: crate::steam::Boiler,
    pub engines: Vec<[i32; 3]>,
    /// The steam whistles on its tank (`attachedWhistles`).
    pub whistles: Vec<[i32; 3]>,
    /// `getTotalTankSize`: width × width × height.
    pub size: i32,
}

/// The way a steam engine's state points (`SteamEngineBlock.getFacing`).
fn engine_facing(info: &StateInfo) -> Option<crate::kinetics::Dir> {
    Some(crate::steam::connected_direction(info.face?, info.facing?))
}

/// Create's latches, by block.
/// A gauge's kind (`GaugeBlock.Type`), for a speedometer or a stressometer.
pub fn gauge_kind(block: Block) -> Option<crate::gauge::Kind> {
    match block {
        Block::Speedometer => Some(crate::gauge::Kind::Speed),
        Block::Stressometer => Some(crate::gauge::Kind::Stress),
        _ => None,
    }
}

pub fn latch_kind(block: Block) -> Option<crate::latch::Kind> {
    match block {
        Block::PoweredLatch => Some(crate::latch::Kind::Powered),
        Block::PoweredToggleLatch => Some(crate::latch::Kind::Toggle),
        _ => None,
    }
}

/// Create's brass diodes, by block.
pub fn diode_kind(block: Block) -> Option<crate::diode::Kind> {
    match block {
        Block::PulseRepeater => Some(crate::diode::Kind::Repeater),
        Block::PulseExtender => Some(crate::diode::Kind::Extender),
        Block::PulseTimer => Some(crate::diode::Kind::Timer),
        _ => None,
    }
}

/// A whole belt: its segments from the controller, the controller's segment, its items.
#[derive(Clone, Debug)]
pub struct Belt {
    pub chain: Vec<[i32; 3]>,
    pub segment: crate::belt::Segment,
    pub items: crate::belt::Inventory,
}

/// Work at a level block entity's turn, rather than a subsystem phase.
#[derive(Debug, PartialEq)]
pub enum TransportTick {
    Fluid([i32; 3]),
    Spout([i32; 3]),
    Drain([i32; 3]),
    Pump([i32; 3]),
    Pipe([i32; 3]),
    Depot([i32; 3]),
    Belt([i32; 3]),
}

/// Resumable level transport walk; inventories are inspected at their turn.
pub struct TransportTicks {
    dim: usize,
    entities: Walk,
}
impl TransportTicks {
    fn controller_turn(world: &mut CreateWorld, dim: usize, c: [i32; 3]) -> bool {
        let rpm = world.speed(dim, c).unwrap_or(0.0);
        let Some(b) = world.dims[dim].belts.get_mut(&c) else {
            return false;
        };
        if !b.items.is_empty() {
            return true;
        }
        let speed = crate::belt::direction_aware_speed(rpm, b.segment.facing);
        if b.items.update_direction(speed) {
            world.dims[dim].belt_changes.push(c);
            world.belt_inventory_changed(dim, c);
        }
        false
    }
    pub fn next(&mut self, world: &mut CreateWorld) -> Option<TransportTick> {
        while let Some(p) = self.entities.next(&world.dims[self.dim].block_entities) {
            match world.block_at(self.dim, p) {
                Some(Block::FluidValve | Block::FluidTank | Block::CreativeFluidTank) => {
                    return Some(TransportTick::Fluid(p));
                }
                Some(Block::Spout) => return Some(TransportTick::Spout(p)),
                Some(Block::ItemDrain) => return Some(TransportTick::Drain(p)),
                Some(Block::MechanicalPump) => return Some(TransportTick::Pump(p)),
                Some(
                    Block::FluidPipe
                    | Block::GlassFluidPipe
                    | Block::EncasedFluidPipe
                    | Block::SmartFluidPipe,
                ) => return Some(TransportTick::Pipe(p)),
                _ => {}
            }
            if world.block_at(self.dim, p) == Some(Block::Depot) {
                return Some(TransportTick::Depot(p));
            }
            if world.block_at(self.dim, p) == Some(Block::Belt) {
                world.initialize_belt(self.dim, p);
                if Self::controller_turn(world, self.dim, p) {
                    return Some(TransportTick::Belt(p));
                }
            }
        }
        None
    }
}

/// What breaking a belt segment does to the next ones (`BeltBlock.onRemove`): each becomes a
/// shaft on the belt's axis (a pulley) or air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeltBreak {
    pub pos: [i32; 3],
    pub shaft: Option<crate::kinetics::Axis>,
}

/// Blocks that follow redstone power (`powered`): Create's split shafts, and the cart assembler
/// (`CartAssemblerBlock.neighborChanged`).
fn follows_power(block: Block) -> bool {
    matches!(
        block,
        Block::Clutch
            | Block::Gearshift
            | Block::GantryShaft
            | Block::AdjustableChainGearshift
            | Block::SmartChute
            | Block::TrackSignal
            | Block::SteamWhistle
            | Block::CartAssembler
            // `AbstractFunnelBlock.neighborChanged` → `tick`: `powered` follows the signal.
            | Block::AndesiteFunnel
            | Block::BrassFunnel
            | Block::AndesiteBeltFunnel
            | Block::BrassBeltFunnel
    )
}

/// Blocks whose speed setting is scrolled or picked on a board (`KineticScrollValueBehaviour`):
/// the creative motor's and the rotation speed controller's.
fn has_speed_setting(block: Block) -> bool {
    matches!(block, Block::CreativeMotor | Block::RotationSpeedController)
}

fn is_chain_drive(block: Block) -> bool {
    matches!(
        block,
        Block::EncasedChainDrive | Block::AdjustableChainGearshift
    )
}

/// Create's state for the open world: three vanilla dimensions, by index.
pub struct CreateWorld {
    pub table: StateTable,
    /// One per level, by the glue's level index (`dimension_index` for the vanilla three, then
    /// mod dimensions in the order they open).
    pub dims: Vec<Dimension>,
    /// Postboxes whose menus players have open.
    pub postboxes: crate::postbox::Postboxes,
}

/// `minecraft:overworld` 0, `minecraft:the_nether` 1, `minecraft:the_end` 2.
pub fn dimension_index(id: &str) -> Option<usize> {
    match id {
        "minecraft:overworld" => Some(0),
        "minecraft:the_nether" => Some(1),
        "minecraft:the_end" => Some(2),
        _ => None,
    }
}

impl CreateWorld {
    pub fn new(table: StateTable) -> CreateWorld {
        CreateWorld {
            table,
            dims: (0..3).map(|_| Dimension::default()).collect(),
            postboxes: Default::default(),
        }
    }

    /// Level entity registration survives state changes and same-entity recolouring.
    fn register_block_entity(&mut self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let entities = &mut self.dims[dim].block_entities;
        let retains = self.table.has_block_entity(new)
            && self
                .table
                .get(old)
                .zip(self.table.get(new))
                .is_some_and(|((old, _), (new, _))| keeps_block_entity(old, new));
        if !retains {
            entities.remove(pos);
        }
        if self.table.has_block_entity(new) {
            let view = entity_view(self.table.get(new).unwrap().0);
            entities.register(pos, view);
        }
    }

    fn place(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        let Some((block, &info)) = self.table.get(state) else {
            return;
        };
        let info = &info;
        let d = &mut self.dims[dim];
        d.lifts.placed(pos, block, info);
        d.displays.placed(pos, block);
        match block {
            Block::AnalogLever => d.levers.placed(pos),
            Block::ContentObserver | Block::StockpileSwitch => {
                if let (Some(t), Some(f)) = (info.target, info.facing) {
                    let target = crate::observe::target_pos(pos, t, f);
                    d.detectors
                        .placed(pos, block == Block::StockpileSwitch, target);
                }
            }
            _ => {}
        }
        if block == Block::Millstone {
            d.millstones.entry(pos).or_default();
        }
        if block == Block::Basin {
            d.basins.insert(pos);
        }
        if matches!(block, Block::CopperBacktank | Block::NetheriteBacktank)
            && !d.backtanks.contains_key(&pos)
        {
            let saved = d.backtank_saved.remove(&pos).unwrap_or_default();
            d.backtanks.insert(pos, saved);
        }
        if block == Block::SequencedGearshift {
            d.sequencers.entry(pos).or_default();
        }
        if block == Block::AdjustableChainGearshift && !d.settings.contains_key(&pos) {
            // Its signal is read again when it joins (Create saves `Signal`, and its `lazyTick`
            // reads it every 40 ticks).
            d.redstone.push(pos);
        }
        if block == Block::MechanicalPress && !d.presses.contains_key(&pos) {
            d.presses.insert(pos, crate::press::Press::default());
            // A press placed over a basin looks at it (`basinChecker`).
            d.mixer_checks.push(pos);
            // A press placed over a depot looks at what it holds.
            d.depot_work.push([pos[0], pos[1] - 2, pos[2]]);
        }
        if latch_kind(block).is_some() {
            d.latch_checks.push(pos);
        }
        if let Some(kind) = diode_kind(block) {
            // Placed or loaded: `BrassDiodeBlockEntity.read`, or a new one at its default delay.
            // Else its block changed (`powered`, `powering`, `inverted`), which its tick reads.
            if d.diodes.get(&pos).is_none_or(|x| x.kind != kind) {
                let mut x = crate::diode::Diode::new(kind);
                // Another diode replaced here: still listed if it was.
                x.running = d.diodes.get(&pos).is_some_and(|x| x.running);
                if let Some((state, max)) = d.diode_saved.remove(&pos) {
                    x.set_delay(max);
                    x.state = state.clamp(0, x.max);
                }
                d.diodes.insert(pos, x);
                d.diode_checks.push((pos, crate::diode::PLACED_DELAY));
            }
            if let Some(x) = d.diodes.get_mut(&pos) {
                x.powered = info.powered;
                x.powering = info.powering;
                if !x.running && !x.idle() {
                    x.running = true;
                    d.diode_running.push(pos);
                }
            }
        }
        if let Some(kind) = gauge_kind(block)
            && d.gauges.get(&pos).is_none_or(|g| g.kind != kind)
        {
            // Read at the next solve (its part is new, so its network solves).
            d.gauges.insert(pos, crate::gauge::Gauge::new(kind));
            d.kinetics.watch(pos, true);
        }
        if block == Block::Deployer && !d.deployers.contains_key(&pos) {
            let saved = d.deployer_saved.remove(&pos).unwrap_or_default();
            if saved.busy() {
                d.deploying.push(pos);
            }
            d.deployers.insert(pos, saved);
            d.deployer_changes.push(pos);
            // `onPlace` → `redstoneUpdate`.
            d.deployer_power_checks.push(pos);
            // A depot below may have work for it.
            d.depot_work.push([pos[0], pos[1] - 2, pos[2]]);
        }
        if block == Block::AndesiteTunnel || block == Block::BrassTunnel {
            d.tunnel_fresh.push(pos);
        }
        if block == Block::BrassTunnel && !d.brass.contains_key(&pos) {
            // `read`: what it held and its mode, saved with the column.
            let b = d.brass_saved.remove(&pos).unwrap_or_default();
            if b.stack.is_some() {
                d.brass_running.push(pos);
            }
            d.brass.insert(pos, b);
        }
        match block {
            Block::TrackSignal => {
                let kind = if info.cross_signal {
                    crate::network::SignalType::Cross
                } else {
                    crate::network::SignalType::Entry
                };
                // `SignalBlock.getStateForPlacement`: its power is read when it is placed.
                if !d.railway.signals.contains_key(&pos) {
                    d.redstone.push(pos);
                }
                d.railway.add_signal(pos, kind, info.powered);
            }
            Block::TrackStation => d.railway.add_station(pos),
            Block::TrackObserver => {
                d.railway.add_observer(pos, info.powered);
                let filtered = d.observer_filters.contains_key(&pos);
                d.railway.set_observer_filtered(pos, filtered);
            }
            // `initialize`: an interface or contraption controls read their power when they join
            // (their own states change only as they are placed).
            Block::PortableStorageInterface | Block::PortableFluidInterface => {
                d.psis.entry(pos).or_default();
                d.psi_checks.push(pos);
            }
            Block::ContraptionControls => {
                d.controls.entry(pos).or_default();
                d.controls_checks.push(pos);
            }
            Block::MechanicalRoller => {
                d.rollers.entry(pos).or_default();
            }
            // `onPlace` → `updateState`: its power.
            Block::ControllerRail => d.rail_checks.push(pos),
            Block::Track => d
                .railway
                .set_track(pos, info.track.unwrap_or(crate::track::TrackShape::Zo)),
            _ => {}
        }
        if block == Block::GantryShaft {
            d.gantry_checks.push(pos);
        }
        if block == Block::MechanicalCrafter && !d.crafters.contains_key(&pos) {
            d.crafters.insert(pos, crate::crafter::Crafter::default());
            d.crafter_checks.push(pos);
            // Its first tick finds out whether it is powered.
            d.crafter_power_checks.push(pos);
        }
        if block == Block::GantryCarriage {
            // `checkValidGantryShaft` when it is placed.
            d.gantry_assemble.push(pos);
        }
        if block == Block::RedstoneLink {
            if d.links.links.contains_key(&pos) {
                // Toggled (`receiver` changed) or its `powered` set: the same frequencies.
                let changed = d.links.set_receiver(pos, info.receiver);
                d.link_changes.extend(changed);
            } else {
                // `LinkBehaviour.initialize`, with what was saved (`read`).
                let (key, transmit) = d.link_saved.remove(&pos).unwrap_or_default();
                let link = crate::link::Link {
                    key,
                    receiver: info.receiver,
                    transmit,
                    received: 0,
                };
                if link.key != crate::link::Key::default() {
                    d.link_shown.push(pos);
                }
                let changed = d.links.add(pos, link);
                d.link_changes.extend(changed);
            }
            // `onPlace` / the scheduled tick: its input is read.
            d.link_checks.push(pos);
        }
        if block == Block::CrushingWheel {
            d.crusher_checks.push(pos);
        }
        if is_plumbing(block) {
            self.plumbing_placed(dim, pos, block, info);
        }
        let d = &mut self.dims[dim];
        if is_fluid_block(block) {
            // Scan geometry on load; actual block changes queue pressure invalidation
            // in block_changed. Saved Connections remain authoritative on cold load.
            d.pipe_checks.push(pos);
            for dir in crate::kinetics::Dir::ALL {
                let o = dir.offset();
                d.pipe_checks
                    .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
            }
        }
        if matches!(block, Block::Chute | Block::SmartChute) {
            // Placed, loaded or changed (powered): it may take from the inventory above it, and
            // a stack it holds (a powered smart chute's, kept in the middle) moves on.
            d.chute_pulls.wake(pos);
            d.chutes.entry(pos).or_insert(0.5);
        }
        if block == Block::ItemVault
            && let Some(axis) = info.axis
        {
            d.vaults.placed(pos, axis, info.large);
        }
        if matches!(
            block,
            Block::AndesiteFunnel
                | Block::BrassFunnel
                | Block::AndesiteBeltFunnel
                | Block::BrassBeltFunnel
        ) {
            // Placed or changed (extracting, powered, shape): looked at again.
            d.funnels.insert(pos, 0);
            d.funnel_timers.at(0, pos);
            // `FunnelItem.getPlacementState`: a funnel placed over a belt is its belt funnel.
            if matches!(block, Block::AndesiteFunnel | Block::BrassFunnel) {
                d.funnel_shapes.push(pos);
            }
        }
        if block == Block::MechanicalArm {
            d.arms.placed(pos, info.ceiling);
            d.arm_signals.push(pos);
        }
        if block == Block::WeightedEjector {
            if !d.ejectors.contains_key(&pos) {
                let e = d.ejector_saved.remove(&pos).unwrap_or_default();
                d.ejectors.insert(pos, e);
                d.ejector_changes.push(pos);
                // `initialize`: `updateSignal`.
                d.arm_signals.push(pos);
            }
            if !d.ejecting.contains(&pos) {
                d.ejecting.push(pos);
            }
            // Its stacks are a depot's.
            if let std::collections::hash_map::Entry::Vacant(v) = d.depots.entry(pos) {
                v.insert(None);
                d.depot_changes.push(pos);
            }
        }
        if block == Block::ItemDrain {
            d.drains.entry(pos).or_default();
        }
        if block == Block::Spout {
            d.spouts.entry(pos).or_default();
        }
        // Block breakers: the drill, and the saw facing sideways (`SawBlockEntity` cuts only then).
        if block == Block::MechanicalDrill
            || (block == Block::MechanicalSaw
                && info
                    .facing
                    .is_some_and(|f| f.axis() != crate::kinetics::Axis::Y))
        {
            d.drills.entry(pos).or_default();
            // A breaker loaded already turning gets no speed change: it looks at its speed
            // itself until one comes (a stopped one does nothing).
            if !d.drilling.contains(&pos) {
                d.drilling.push(pos);
            }
        }
        if block == Block::MechanicalMixer && !d.mixers.contains_key(&pos) {
            d.mixers.insert(pos, Default::default());
            d.mixer_checks.push(pos);
        }
        if block == Block::Basin {
            d.mixer_checks.push([pos[0], pos[1] + 2, pos[2]]);
        }
        if block == Block::BlazeBurner && !d.burners.contains_key(&pos) {
            let b = d.burner_saved.remove(&pos).unwrap_or_default();
            if b.burning() {
                d.burning.push(pos);
            }
            d.burners.insert(pos, b);
        }
        if block == Block::CrushingWheelController && !d.crushers.contains_key(&pos) {
            let c = d.crusher_saved.remove(&pos).unwrap_or_default();
            d.crushers.insert(pos, (c, 0.0));
        }
        if block == Block::Nozzle
            && let Some(facing) = info.facing
        {
            d.nozzles
                .entry(pos)
                .or_insert_with(|| crate::nozzle::Nozzle::new(facing))
                .facing = facing;
        }
        if block == Block::EncasedFan {
            // Placed, loaded or turned by a wrench: its current is looked at again.
            let fan = d.fans.entry(pos).or_default();
            fan.facing = info.facing;
            fan.rebuild = true;
        }
        if block == Block::MechanicalBearing
            && !d.windmills.contains_key(&pos)
            && let Some(w) = d.windmill_saved.remove(&pos)
        {
            d.windmills.insert(pos, w);
            d.windmill_changes.push(pos);
            // Its speed is looked at again (`onSpeedChanged`).
            d.bearing_checks.push(pos);
        }
        if block == Block::WindmillBearing
            && !d.windmills.contains_key(&pos)
            && let Some(w) = d.windmill_saved.remove(&pos)
        {
            let speed = crate::contraption::windmill_speed(w.sails, w.clockwise);
            d.windmills.insert(pos, w);
            d.windmill_changes.push(pos);
            let old = d.settings.get(&pos).copied().unwrap_or_default();
            d.settings.insert(
                pos,
                Settings {
                    windmill: speed,
                    ..old
                },
            );
        }
        let engine = (block == Block::SteamEngine)
            .then(|| engine_facing(info))
            .flatten();
        // An engine turned or replaced leaves its old tank and shaft first.
        if let Some(old) = self.engine_of(self.dims[dim].states.get(&pos).copied())
            && Some(old) != engine
        {
            self.engine_removed(dim, pos, old);
        }
        if let Some(c) = engine {
            // `onPlace`: its tank's boiler counts it, its shaft is powered.
            let d = &mut self.dims[dim];
            let o = c.opposite().offset();
            d.boiler_checks
                .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
            d.engine_shafts.push(crate::steam::shaft_pos(pos, c));
        }
        if block == Block::BlazeBurner {
            // A burner's heat changed under a boiler (`FluidTankBlock.updateShape`).
            self.boiler_heat_changed(dim, [pos[0], pos[1] + 1, pos[2]]);
        }
        let d = &mut self.dims[dim];
        if block == Block::FluidTank {
            d.boiler_checks.push(pos);
        }
        if block == Block::FluidTank {
            // Loaded: a saved tank's controller takes its blocks back (`read`).
            let saved = d.tank_saved.remove(&pos);
            if let Some(saved) = saved {
                d.tanks.restore(pos, saved);
            } else if !d.tanks.parts.contains_key(&pos) {
                d.tanks.load(pos, None);
            }
            d.tank_changes.push(pos);
            self.tanks_touched(dim, false);
        }
        let d = &mut self.dims[dim];
        if block == Block::CreativeFluidTank && !d.creative_tanks.parts.contains_key(&pos) {
            let fluid = d.plumbing.creative.get(&pos).copied().flatten();
            let saved = d.creative_tank_saved.remove(&pos).map(|mut m| {
                m.tank.fluid = fluid;
                m
            });
            d.creative_tanks.creative = true;
            d.creative_tanks.load(pos, saved);
            if let Some(m) = d.creative_tanks.multis.get_mut(&pos)
                && saved.is_none()
            {
                m.tank.fluid = fluid;
            }
            self.creative_tanks_touched(dim, false);
        }
        let d = &mut self.dims[dim];
        // A wheel loaded with its column looks at its flow (`lazyTick`); placed ones are listed
        // by `block_changed`.
        if matches!(block, Block::WaterWheel | Block::LargeWaterWheel) && !d.wheels.contains(&pos) {
            d.wheels.push(pos);
            d.wheel_checks.push(pos);
        }
        if block == Block::Belt && !d.belt_of.contains_key(&pos) {
            d.belt_dirty.push(pos);
        }
        if block == Block::Depot && !d.depots.contains_key(&pos) {
            // Placed or loaded: clients draw what its block entity holds.
            d.depots.insert(pos, None);
            d.depot_changes.push(pos);
        }
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        let [a, b] = parts(block, info, &settings);
        let previous = d.states.get(&pos).and_then(|s| self.table.get(*s));
        // The block entity survives (another valve colour too); `KineticBlock.onPlace` still
        // treats a different block as non-equivalent.
        let same = previous.is_some_and(|(b, _)| keeps_block_entity(b, block));
        let equivalent =
            previous.is_some_and(|(b, old)| kinetically_equivalent(b, old, block, info));
        if !same {
            d.kinetics.remove(pos);
        }
        // A block whose state changes often (a diode) is listed in its column once.
        if d.states.insert(pos, state).is_none() {
            d.columns
                .entry((pos[0] >> 4, pos[2] >> 4))
                .or_default()
                .push(pos);
        }
        for (sub, part) in [(0, a), (1, b)] {
            if let Some(part) = part {
                if same {
                    d.kinetics.change_block_state(pos, sub, part, equivalent);
                } else {
                    d.kinetics.set(pos, sub, part);
                }
            }
        }
    }

    /// A `Level.setBlock` with `UPDATE_KNOWN_SHAPE` (`ChainGearshiftBlock.neighborChanged`):
    /// `updateIndirectNeighbourShapes` doesn't run, so the block entity keeps its kinetic
    /// information though the states aren't equivalent. Call it just before committing the
    /// state: the host's `block_changed` that follows then finds the state already taken. (The
    /// hook doesn't say a change's update flags: API_GAPS.)
    pub fn setting_known_shape(&mut self, dim: usize, pos: [i32; 3], new: u32) {
        if let Some(old) = self.dims[dim].states.get_mut(&pos)
            && self.table.get(*old).map(|(b, _)| b) == self.table.get(new).map(|(b, _)| b)
        {
            *old = new;
        }
    }

    /// `GearshiftBlock.neighborChanged`: detach before committing a redstone power change.
    /// Direct state replacement does not run this callback.
    pub fn transmission_power_changing(&mut self, dim: usize, pos: [i32; 3]) {
        if self
            .block_at(dim, pos)
            .is_some_and(|b| matches!(b, Block::Clutch | Block::Gearshift))
        {
            self.dims[dim].kinetics.detach_transmission(pos);
        }
    }

    /// `KineticBlockEntity.switchToBlockState`, before the world commits an interaction state change.
    pub fn switching_block_state(&mut self, dim: usize, pos: [i32; 3], new: u32) {
        let d = &mut self.dims[dim];
        let Some(old) = d.states.get(&pos).copied() else {
            return;
        };
        if old == new {
            return;
        }
        let (Some((a, x)), Some((b, y))) = (self.table.get(old), self.table.get(new)) else {
            return;
        };
        let equivalent = kinetically_equivalent(a, x, b, y);
        d.kinetics
            .switch_block_state(pos, equivalent, is_generating(a));
    }

    /// A host block change outside Create's state table still notifies adjacent
    /// pipes. Admission uses the changed block's current state during preparation.
    pub fn fluid_neighbour_changed(&mut self, dim: usize, pos: [i32; 3]) {
        if crate::kinetics::Dir::ALL.into_iter().any(|side| {
            let o = side.offset();
            let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            self.dims[dim]
                .states
                .get(&p)
                .and_then(|state| self.table.get(*state))
                .is_some_and(|(block, _)| is_fluid_block(block))
        }) {
            self.dims[dim]
                .fluid_transport
                .changed
                .push(crate::fluids::runtime::Change::Block(pos));
        }
    }

    /// A committed block change. The first lines are the hot path: two
    /// range tests that miss for every non-Create block.
    #[inline]
    pub fn block_changed(&mut self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        self.register_block_entity(dim, pos, old, new);
        let affects_fluid = [old, new].into_iter().any(|state| {
            self.table
                .get(state)
                .is_some_and(|(block, _)| is_fluid_block(block))
        }) || crate::kinetics::Dir::ALL.into_iter().any(|side| {
            let o = side.offset();
            let neighbour = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            self.dims[dim]
                .states
                .get(&neighbour)
                .and_then(|state| self.table.get(*state))
                .is_some_and(|(block, _)| is_fluid_block(block))
        });
        if affects_fluid {
            self.dims[dim]
                .fluid_transport
                .changed
                .push(crate::fluids::runtime::Change::Block(pos));
        }

        if self
            .table
            .get(new)
            .is_some_and(|(b, _)| b == Block::BlazeBurner)
        {
            self.dims[dim].burner_checks.push(pos);
        }
        // Another block: what comparators were told of the old one no longer holds.
        if !self.dims[dim].analog_levels.is_empty() {
            self.dims[dim].analog_levels.remove(&pos);
        }
        // A track that is gone takes its curves (`TrackBlockEntity.removeInboundConnections`).
        let track = |s: u32| self.table.get(s).is_some_and(|(b, _)| b == Block::Track);
        if track(old) && !track(new) && self.dims[dim].track_curves.at.contains_key(&pos) {
            self.dims[dim].track_curves.remove_at(pos);
            self.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
        // A sliding door opened or closed (or was placed): Create's rules after the tick.
        if let Some((b, n)) = self.table.get(new)
            && crate::dye::is_sliding_door(b)
        {
            let was = self
                .table
                .get(old)
                .filter(|(o, _)| *o == b)
                .map(|(_, o)| o.open);
            // A door placed closed has nothing to do.
            if was != Some(n.open) && (was.is_some() || n.open) {
                self.dims[dim]
                    .door_changes
                    .push((pos, was.unwrap_or(false)));
            }
        }
        // `AbstractChuteBlock.onPlace` / `onRemove`: the chute a chute leads into, and the
        // diagonal chutes over a chute that went, look at their shape again.
        let chute = |s: u32| {
            self.table
                .get(s)
                .filter(|(b, _)| matches!(b, Block::Chute | Block::SmartChute))
                .map(|(_, i)| i.facing.unwrap_or(crate::kinetics::Dir::Down))
        };
        // `onPlace` runs for any new state of a chute (a diagonal chute turned toward another
        // leads into it now); `onRemove` only when the block changes.
        if let Some(f) = chute(new)
            && old != new
        {
            self.dims[dim]
                .chute_shapes
                .push(crate::chute::target(pos, f));
        }
        let kind_of = |s: u32| self.table.get(s).map(|(b, _)| b);
        if let Some(f) = chute(old)
            && kind_of(old) != kind_of(new)
        {
            let d = &mut self.dims[dim];
            d.chute_shapes.push(crate::chute::target(pos, f));
            for side in [
                crate::kinetics::Dir::South,
                crate::kinetics::Dir::West,
                crate::kinetics::Dir::North,
                crate::kinetics::Dir::East,
            ] {
                let o = side.offset();
                d.chute_shapes
                    .push([pos[0] + o[0], pos[1] + 1, pos[2] + o[2]]);
            }
        }
        // A still hose turning forwards waits for the block below its end (`lazyTick`).
        if !self.dims[dim].plumbing.hose_waits.is_empty()
            && let Some(hose) = self.dims[dim].plumbing.hose_waits.remove(&pos)
        {
            self.dims[dim].plumbing.hose_speeds.push((hose, f32::NAN));
        }
        let (was, is) = (self.table.is_kinetic(old), self.table.is_kinetic(new));
        if !was && !is {
            return;
        }
        if let Some((gone, info)) = self.table.get(old)
            && is_plumbing(gone) | (gone == Block::FluidTank)
            && Some(gone) != self.table.get(new).map(|(b, _)| b)
        {
            let info = *info;
            self.plumbing_removed(dim, pos, Some((gone, info)));
        }
        self.dims[dim]
            .lifts
            .changed(pos, self.table.get(old), self.table.get(new));
        if let Some((gone, _)) = self.table.get(old)
            && Some(gone) != self.table.get(new).map(|(b, _)| b)
        {
            // A nixie tube dyed another colour keeps its block entity (`withColor`).
            let recoloured = crate::display::is_nixie(gone)
                && self
                    .table
                    .get(new)
                    .is_some_and(|(b, _)| crate::display::is_nixie(b));
            let d = &mut self.dims[dim];
            if !recoloured && d.displays.removed(pos, gone) {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            let had = match gone {
                Block::AnalogLever => d.levers.removed(pos),
                Block::ContentObserver | Block::StockpileSwitch => d.detectors.removed(pos),
                // Replaced by another of Create's blocks (which keeps the kinetic block entity
                // type): the drain, spout or saw that was there is gone, as on its removal.
                Block::ItemDrain => d.drains.remove(&pos).is_some(),
                Block::Spout => d.spouts.remove(&pos).is_some(),
                Block::MechanicalSaw => d.saws.remove(&pos).is_some(),
                _ => false,
            };
            if had {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
        }
        // `FluidTankBlock.onPlace` / `onRemove`: tanks form or split (a tank's own state
        // changes don't).
        let tank = |s: u32| {
            self.table
                .get(s)
                .is_some_and(|(b, _)| b == Block::FluidTank)
        };
        // `ShaftBlock.pickCorrectShaftType`: a shaft placed in front of a steam engine is its
        // powered shaft (the swap `shaft_changes` makes; a placement can't answer another block).
        if self.table.get(new).is_some_and(|(b, _)| b == Block::Shaft)
            && self.table.get(old).map(|(b, _)| b) != Some(Block::Shaft)
        {
            self.dims[dim].engine_shafts.push(pos);
        }
        let (was_tank, is_tank) = (tank(old), tank(new));
        if was_tank != is_tank {
            if is_tank {
                self.dims[dim].tanks.place(pos);
            } else {
                self.dims[dim].tanks.remove(pos);
            }
            self.tanks_touched(dim, true);
        }
        let creative = |s: u32| {
            self.table
                .get(s)
                .is_some_and(|(b, _)| b == Block::CreativeFluidTank)
        };
        let (was_creative, is_creative) = (creative(old), creative(new));
        if was_creative != is_creative {
            let d = &mut self.dims[dim];
            d.creative_tanks.creative = true;
            if is_creative {
                d.creative_tanks.place(pos);
            } else {
                d.creative_tanks.remove(pos);
            }
            self.creative_tanks_touched(dim, true);
        }
        // Track, a station or a signal gone (placed ones are added by `place`).
        let rail = |s: u32| {
            self.table.get(s).map(|(b, _)| b).filter(|b| {
                matches!(
                    b,
                    Block::Track | Block::TrackStation | Block::TrackSignal | Block::TrackObserver
                )
            })
        };
        if let Some(b) = rail(old)
            && rail(new) != Some(b)
        {
            self.dims[dim].railway.remove(pos);
        }
        let kind = |s: u32| self.table.get(s).map(|(b, _)| b);
        if kind(old) == Some(Block::ItemVault) && kind(new) != Some(Block::ItemVault) {
            // `ItemVaultBlock.onRemove`: its vault splits.
            self.dims[dim].vaults.removed(pos);
        }
        // The blocks whose filter slot `chute_filters` holds, by the blocks it stays with: a
        // brass funnel's filter goes with it onto a belt and off it again.
        // The basin's, the saw's and the deployer's filters are kept there too.
        let family = |b: Option<Block>| match b {
            Some(Block::Chute | Block::SmartChute) => Some(0),
            Some(Block::BrassFunnel | Block::BrassBeltFunnel) => Some(1),
            Some(Block::Basin) => Some(2),
            Some(Block::MechanicalSaw) => Some(3),
            Some(Block::Deployer) => Some(4),
            _ => None,
        };
        if family(kind(old)).is_some() && family(kind(new)) != family(kind(old)) {
            let d = &mut self.dims[dim];
            d.chute_pulls.forget(pos);
            if d.filter_amounts.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(f) = d.chute_filters.remove(&pos) {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                if f.is_filter_item() {
                    d.dropped_filters.push((pos, f));
                }
            }
        }
        // Blocks that ride contraptions, and track observers: what they hold goes with them (a
        // filter item drops, `FilteringBehaviour.destroy`). A contraption taking them away moves
        // their settings first (`create_actors::carry`).
        if let Some(gone) = kind(old)
            && kind(new) != Some(gone)
        {
            let d = &mut self.dims[dim];
            let had = match gone {
                Block::PortableStorageInterface | Block::PortableFluidInterface => {
                    d.psis.remove(&pos);
                    false
                }
                Block::ContraptionControls => d.controls.remove(&pos).is_some(),
                Block::MechanicalRoller => d.rollers.remove(&pos).is_some(),
                Block::TrackObserver => match d.observer_filters.remove(&pos) {
                    Some(f) => {
                        if f.is_filter_item() {
                            d.dropped_filters.push((pos, f));
                        }
                        true
                    }
                    None => false,
                },
                _ => false,
            };
            if had {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
        }
        if is {
            let (was_block, is_block) = (
                self.table.get(old).map(|(b, _)| b),
                self.table.get(new).map(|(b, _)| b),
            );
            if is_block == Some(Block::RoseQuartzLamp) && was_block != Some(Block::RoseQuartzLamp) {
                self.dims[dim].lamp_placed.push(pos);
            }
            if is_block.and_then(diode_kind).is_none() && was_block.and_then(diode_kind).is_some() {
                self.diode_removed(dim, pos);
            }
            if self.table.get(new).is_some_and(|(b, _)| follows_power(b))
                && self.table.get(old).map(|(b, _)| b) != self.table.get(new).map(|(b, _)| b)
            {
                // ClutchBlock.getStateForPlacement: powered as placed.
                self.dims[dim].redstone.push(pos);
            }
            if self.table.get(new).is_some_and(|(b, _)| is_chain_drive(b))
                && self.table.get(old).map(|(b, _)| b) != self.table.get(new).map(|(b, _)| b)
            {
                // ChainDriveBlock.getStateForPlacement: joined to the drives beside it.
                self.dims[dim].chain_checks.push(pos);
            }
            let tunnel = |s: u32| {
                matches!(
                    self.table.get(s).map(|(b, _)| b),
                    Some(Block::AndesiteTunnel | Block::BrassTunnel)
                )
            };
            if tunnel(new) && !tunnel(old) {
                // `BeltTunnelItem`: the belt below is cased.
                self.dims[dim].tunnels_new.push(pos);
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::Millstone) {
                self.dims[dim].millstones.entry(pos).or_default();
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::Basin) {
                self.dims[dim].basins.insert(pos);
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::WaterWheel)
                && self.table.get(old).map(|(b, _)| b) != Some(Block::WaterWheel)
            {
                self.dims[dim].wheels.push(pos);
                self.dims[dim].wheel_checks.push(pos);
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::LargeWaterWheel)
                && self.table.get(old).map(|(b, _)| b) != Some(Block::LargeWaterWheel)
            {
                let d = &mut self.dims[dim];
                d.wheels.push(pos);
                d.wheel_checks.push(pos);
                d.large_wheel_checks.push(pos);
            }
            self.place(dim, pos, new);
        } else {
            if let Some(c) = self.engine_of(Some(old)) {
                self.engine_removed(dim, pos, c);
            }
            match self.table.get(old) {
                Some((Block::LargeWaterWheel, i)) => {
                    if let Some(a) = i.axis {
                        self.dims[dim].large_wheels_gone.push((pos, a));
                    }
                }
                Some((Block::WaterWheelStructure, i)) => {
                    if let Some(f) = i.facing {
                        self.dims[dim].wheel_structures_gone.push((pos, f));
                    }
                }
                _ => {}
            }
            match self.table.get(old).map(|(b, _)| b) {
                Some(Block::FluidTank) => self.dims[dim].boiler_checks.push(pos),
                Some(Block::BlazeBurner) => {
                    self.boiler_heat_changed(dim, [pos[0], pos[1] + 1, pos[2]])
                }
                _ => {}
            }
            let d = &mut self.dims[dim];
            d.shaft_engine.remove(&pos);
            d.kinetics.remove(pos);
            d.states.remove(&pos);
            d.handles.remove(&pos);
            d.wheels.retain(|p| *p != pos);
            d.millstones.remove(&pos);
            d.basins.remove(&pos);
            d.presses.remove(&pos);
            d.backtanks.remove(&pos);
            self.diode_removed(dim, pos);
            let d = &mut self.dims[dim];
            if d.gauges.remove(&pos).is_some() {
                d.kinetics.watch(pos, false);
            }
            if self.table.get(old).map(|(b, _)| b) == Some(Block::CrushingWheel) {
                d.crusher_checks.push(pos);
            }
            if let Some((b, _)) = self.table.get(old)
                && is_fluid_block(b)
            {
                d.fluid_transport
                    .changed
                    .push(crate::fluids::runtime::Change::Block(pos));
                for dir in crate::kinetics::Dir::ALL {
                    let o = dir.offset();
                    d.pipe_checks
                        .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
                }
            }
            d.fluid_transport.remove(pos);
            d.chutes.remove(&pos);
            if d.sequencers.remove(&pos).is_some() {
                d.sequencing.retain(|p| *p != pos);
            }
            if let Some(s) = d.saws.remove(&pos) {
                d.saw_drops
                    .extend(s.slots.into_iter().map(|(item, n)| (pos, item, n)));
            }
            d.spouts.remove(&pos);
            d.drains.remove(&pos);
            d.funnels.remove(&pos);
            if let Some((item, n)) = d.arms.removed(pos) {
                // `ArmBlockEntity.destroy`: its held stack drops.
                d.arm_drops.push((pos, item, n));
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(mut e) = d.ejectors.remove(&pos) {
                d.ejecting.retain(|p| *p != pos);
                d.ejector_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                // `destroy` -> `dropFlyingItems`.
                if let Some(f) = self.table.get(old).and_then(|(_, i)| i.facing) {
                    for (at, v, fl) in e.drop_flying(pos, f) {
                        d.ejector_drops.push((at, v, fl.item, fl.count));
                    }
                }
            }
            if d.drills.remove(&pos).is_some() {
                d.drilling.retain(|p| *p != pos);
            }
            if d.mixers.remove(&pos).is_some() {
                d.mixing.retain(|p| *p != pos);
            }
            if self.table.get(old).map(|(b, _)| b) == Some(Block::Basin) {
                // `onBasinRemoved`: a mixer over it stops.
                d.mixer_checks.push([pos[0], pos[1] + 2, pos[2]]);
            }
            if d.links.links.contains_key(&pos) {
                let changed = d.links.remove(pos);
                d.link_changes.extend(changed);
                d.link_shown.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.basin_fluids.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.chassis_range.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            // Its side filters go (a filter item drops, `FilteringBehaviour.destroy`).
            for side in crate::kinetics::Dir::ALL {
                if d.tunnel_filters.is_empty() {
                    break;
                }
                if let Some(f) = d.tunnel_filters.remove(&(pos, side)) {
                    d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                    if f.is_filter_item() {
                        d.dropped_filters.push((pos, f));
                    }
                }
            }
            if d.scroll_options.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            // What it held drops with its block entity's slot (`BrassTunnelBlockEntity.destroy`).
            if d.brass.remove(&pos).is_some() {
                d.brass_running.retain(|p| *p != pos);
            }
            if d.crafter_inputs.contains_key(&pos) {
                match self.table.get(old).and_then(|(_, i)| i.facing) {
                    Some(f) => crate::crafter_input::remove(&mut d.crafter_inputs, pos, f),
                    None => {
                        d.crafter_inputs.remove(&pos);
                    }
                }
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            // `onRemove`: its slot cover drops.
            d.crafter_unpowered.remove(&pos);
            if d.crafter_covers.remove(&pos) {
                d.crafter_drops
                    .push((pos, "create:crafter_slot_cover".to_owned()));
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(c) = d.crafters.remove(&pos) {
                d.crafting.retain(|p| *p != pos);
                // `ejectWholeGrid` when it breaks: what it held drops (its slot drops with its
                // block entity).
                d.crafter_drops
                    .extend(c.grid.into_iter().map(|(_, item)| (pos, item)));
            }
            if let Some(dp) = d.deployers.remove(&pos) {
                d.deploying.retain(|p| *p != pos);
                d.deployer_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                if let Some(held) = dp.held {
                    // `IBE.onRemove`: its held stack drops.
                    d.deployer_drops.push((pos, held));
                }
                // `DeployerFakePlayer.remove` (`discardPlayer`): its cracks go.
                if let Some((at, _)) = dp.breaking {
                    d.deployer_cracks.push((pos, at));
                }
            }
            if d.burners.remove(&pos).is_some() {
                d.burning.retain(|p| *p != pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.crushers.remove(&pos).is_some() {
                d.running_crushers.retain(|p| *p != pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.fans.remove(&pos).is_some() {
                d.running_fans.retain(|p| *p != pos);
            }
            d.nozzles.remove(&pos);
            if let Some(m) = d.pistons.remove(&pos) {
                d.piston_orphans.push((pos, m));
            }
            if let Some(w) = d.windmills.remove(&pos) {
                d.windmill_orphans.push((pos, w));
                d.windmill_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            let d = &mut self.dims[dim];
            if d.depots.remove(&pos).is_some() {
                d.sliding.retain(|p| *p != pos);
                d.depot_changes.push(pos);
            }
            if d.settings.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(list) = d.columns.get_mut(&(pos[0] >> 4, pos[2] >> 4)) {
                list.retain(|p| *p != pos);
            }
        }
        // `BeltBlock.onRemove` tests block identity, not whether the replacement is
        // kinetic. A pulley becomes a shaft and still removes its next belt segments.
        if let Some(seg) = self.table.get(old).and_then(|(_, i)| i.belt)
            && self.table.get(new).map(|(b, _)| b) != Some(Block::Belt)
        {
            self.belt_removed(dim, pos, seg);
        }
    }

    /// One section of a column that joined the level: `base` is its lowest corner, `states` its
    /// 4,096 states in vanilla's index order (`y << 8 | z << 4 | x`).
    /// `CuckooClockBlockEntity.tick` on the server at `hours:minutes` of a natural dimension:
    /// turning clocks with nothing running start what is due, running ones go on. Each start
    /// (to tell the clients) and each surprise going off. Nothing is looked at without turning
    /// clocks, nor outside the two times unless one runs.
    pub fn tick_cuckoos(
        &mut self,
        dim: usize,
        hours: i32,
        minutes: i32,
    ) -> Vec<([i32; 3], crate::cuckoo::Animation, bool)> {
        use crate::cuckoo;
        let table = &self.table;
        let d = &mut self.dims[dim];
        if d.cuckoo_turning.is_empty() && d.cuckoos.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if cuckoo::due(hours, minutes, false).is_some() {
            // Gone, broken or replaced clocks leave the list here.
            d.cuckoo_turning.retain(|p| {
                d.states
                    .get(p)
                    .and_then(|&s| table.get(s))
                    .is_some_and(|(b, _)| {
                        matches!(b, Block::CuckooClock | Block::MysteriousCuckooClock)
                    })
            });
            for &pos in &d.cuckoo_turning {
                if d.cuckoos.contains_key(&pos) {
                    continue;
                }
                let mysterious = d
                    .states
                    .get(&pos)
                    .and_then(|&s| table.get(s))
                    .is_some_and(|(b, _)| b == Block::MysteriousCuckooClock);
                if let Some(a) = cuckoo::due(hours, minutes, mysterious) {
                    d.cuckoos.insert(pos, (a, 0));
                    out.push((pos, a, false));
                }
            }
        }
        // Started this tick, they run from the next (`startAnimation` is in the else's
        // sibling branch).
        let started: Vec<[i32; 3]> = out.iter().map(|o| o.0).collect();
        let turning = &d.cuckoo_turning;
        let states = &d.states;
        d.cuckoos.retain(|pos, (kind, progress)| {
            let clock = states
                .get(pos)
                .and_then(|&s| table.get(s))
                .is_some_and(|(b, _)| {
                    matches!(b, Block::CuckooClock | Block::MysteriousCuckooClock)
                });
            if !clock {
                return false;
            }
            if started.contains(pos) {
                return true;
            }
            // `getSpeed() == 0` returns before the animation goes on; a clock that stopped is
            // left as it is until it turns again.
            if !turning.contains(pos) {
                return true;
            }
            let (running, boom) = cuckoo::step(*kind, progress);
            if boom {
                out.push((*pos, *kind, true));
            }
            running
        });
        out
    }

    /// Makes sure level `dim` has its state (a mod dimension opening).
    pub fn ensure_dimension(&mut self, dim: usize) {
        while self.dims.len() <= dim {
            self.dims.push(Dimension::default());
        }
    }

    /// One block the world keeps came into a loaded level (its block entity loaded), as
    /// `section_loaded` finds them in a section.
    pub fn block_loaded(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        if self.table.has_block_entity(state) {
            let view = entity_view(self.table.get(state).unwrap().0);
            self.dims[dim].block_entities.register(pos, view);
        }
        if self.table.is_kinetic(state) {
            self.place(dim, pos, state);
        }
    }

    pub fn section_loaded(&mut self, dim: usize, base: [i32; 3], states: &[u32; 4096]) {
        for (i, &s) in states.iter().enumerate() {
            if self.table.is_kinetic(s) || self.table.has_block_entity(s) {
                let pos = [
                    base[0] + (i & 15) as i32,
                    base[1] + (i >> 8) as i32,
                    base[2] + ((i >> 4) & 15) as i32,
                ];
                self.block_loaded(dim, pos, s);
            }
        }
    }

    /// A column left the level: invalidate entities without detaching their networks.
    pub fn column_unloaded(&mut self, dim: usize, cx: i32, cz: i32) {
        let d = &mut self.dims[dim];
        // Its track leaves the graph; trains standing in it leave with it (saved with it).
        d.railway.column_unloaded(cx, cz);
        d.lifts.column_unloaded(cx, cz);
        d.displays.column_unloaded(cx, cz);
        d.levers.column_unloaded(cx, cz);
        d.detectors.column_unloaded(cx, cz);
        if !d.analog_levels.is_empty() {
            d.analog_levels
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
        }
        if let Some(list) = d.columns.remove(&(cx, cz)) {
            for pos in list {
                d.kinetics.unload(pos);
                d.states.remove(&pos);
                d.handles.remove(&pos);
            }
            d.wheels.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.millstones
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.basins.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.backtanks
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.backtank_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.depots.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.presses.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.gauges.retain(|p, _| {
                let keep = (p[0] >> 4, p[2] >> 4) != (cx, cz);
                if !keep {
                    d.kinetics.watch(*p, false);
                }
                keep
            });
            d.fans.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.nozzles.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crushers.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.burners.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.mixers.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.chutes.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.saws.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.spouts.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.drains.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.funnels.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.arms.column_unloaded(cx, cz);
            d.ejectors.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.ejecting.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.chute_pulls.column_unloaded(cx, cz);
            d.chute_filters
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.filter_amounts
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (cx, cz);
            d.psis.retain(|p, _| out(p));
            d.psi_running.retain(out);
            d.controls.retain(|p, _| out(p));
            d.rollers.retain(|p, _| out(p));
            d.observer_filters.retain(|p, _| out(p));
            d.vaults.column_unloaded(cx, cz);
            d.drills.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.drilling.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.fluid_transport
                .pipes
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.fluid_transport
                .pumps
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sequencers
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sequencing.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sequencer_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sequencer_steps
                .retain(|(p, _)| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.chain_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            // FluidTransportBehaviour.read restores pressure. Unloading a column
            // does not call FluidPropagator.propagateChangedPipe on surviving pipes.
            d.plumbing.column_unloaded(cx, cz);
            d.mixing.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.mixer_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.burning.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.burner_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.link_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            // `LinkBehaviour.unload`: its links leave their networks.
            let gone: Vec<[i32; 3]> = d
                .links
                .links
                .keys()
                .filter(|p| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .copied()
                .collect();
            for p in gone {
                let changed = d.links.remove(p);
                d.link_changes.extend(changed);
            }
            d.link_checks.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.diodes.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.diode_running
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.diode_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.diode_checks
                .retain(|p| (p.0[0] >> 4, p.0[2] >> 4) != (cx, cz));
            d.diode_ticks
                .retain(|p| (p.0[0] >> 4, p.0[2] >> 4) != (cx, cz));
            d.lamp_placed.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.lamp_checks.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.lamp_ticks
                .retain(|p| (p.0[0] >> 4, p.0[2] >> 4) != (cx, cz));
            d.link_changes
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.link_shown.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.deployers
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.deploying.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafters.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafter_inputs
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafter_covers
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafter_unpowered
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafter_power_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.deployer_power_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafting.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crafter_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.bearing_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.brass_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.deployer_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.tunnel_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.tunnels_new.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.tunnel_fresh
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.brass.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.basin_fluids
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.chassis_range
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.scroll_options
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.brass_running
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.running_crushers
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crusher_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.running_fans
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.tanks.unload_column(cx, cz);
            d.tank_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.creative_tanks.unload_column(cx, cz);
            d.creative_tank_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.boilers.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.boiler_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.boiler_heat_checks
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.engine_shafts
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.shaft_engine
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.windmills
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            // A belt reaching into the column stops being whole until it is back.
            let gone: Vec<[i32; 3]> = d
                .belt_of
                .iter()
                .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .map(|(_, c)| *c)
                .collect();
            for c in gone {
                if let Some(b) = d.belts.remove(&c) {
                    for p in &b.chain {
                        d.belt_of.remove(p);
                        if (p[0] >> 4, p[2] >> 4) != (cx, cz) {
                            d.belt_dirty.push(*p);
                        }
                    }
                }
            }
            d.belt_dirty.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.depot_work.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sliding.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
        }
        d.block_entities.unload(cx, cz);
        d.settings.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
    }

    /// A block entity setting that changes a part (a motor's speed), then re-places the block.
    pub fn set_settings(&mut self, dim: usize, pos: [i32; 3], state: u32, settings: Settings) {
        self.dims[dim].settings.insert(pos, settings);
        self.dims[dim].generator_changes.push(pos);
        if !self.table.is_kinetic(state) {
            return;
        }
        // A block entity's setting (a motor's speed, a crank turned) changes its parts in place:
        // Create's block entity keeps its speed, source and network (`updateGeneratedRotation`),
        // where a new block state starts them again (`KineticBlock.updateIndirectNeighbourShapes`).
        let d = &mut self.dims[dim];
        if d.states.get(&pos) == Some(&state)
            && let Some((block, info)) = self.table.get(state)
        {
            let [a, b] = parts(block, info, &settings);
            if a.is_some() == d.kinetics.has_part(pos, 0)
                && b.is_some() == d.kinetics.has_part(pos, 1)
            {
                for (sub, part) in [(0, a), (1, b)] {
                    if let Some(part) = part {
                        d.kinetics.set(pos, sub, part);
                    }
                }
                return;
            }
        }
        self.place(dim, pos, state);
    }

    /// `SequencedGearshiftBlockEntity.tick`: the base kinetic tick attaches first;
    /// only afterwards can the instruction advance and detach again. Commit the returned
    /// block-state changes after this call, without another solve until the next tick.
    pub fn tick(
        &mut self,
        dim: usize,
        events: &mut Vec<Event>,
    ) -> (SolveStats, Vec<([i32; 3], crate::sequencer::Step)>) {
        let stats = self.solve(dim, events);
        let steps = self.tick_sequencers(dim);
        (stats, steps)
    }

    /// Re-solves what changed in a dimension: nothing at all when nothing did.
    #[inline]
    fn solve(&mut self, dim: usize, events: &mut Vec<Event>) -> SolveStats {
        let d = &mut self.dims[dim];
        d.kinetics.advance();
        let from = events.len();
        let stats = d.kinetics.solve(events);
        self.flush_kinetic_callbacks(dim);
        let d = &mut self.dims[dim];
        // Sequenced gearshifts follow their speed (`onSpeedChanged`).
        if !d.sequencers.is_empty() {
            let turned: Vec<[i32; 3]> = events[from..]
                .iter()
                .filter_map(|e| match *e {
                    Event::Speed { pos, .. } if d.sequencers.contains_key(&pos) => Some(pos),
                    _ => None,
                })
                .collect();
            for pos in turned {
                self.sequencer_speed(dim, pos);
            }
        }
        let d = &mut self.dims[dim];
        // Fans follow their speed (`onSpeedChanged`: the current is rebuilt); crushing wheels
        // look at their controllers again.
        for e in &events[from..] {
            if let Event::Speed { pos, .. } = *e
                && d.states
                    .get(&pos)
                    .and_then(|&s| self.table.get(s))
                    .is_some_and(|(b, _)| b == Block::CrushingWheel)
            {
                d.crusher_checks.push(pos);
            }
            if let Event::Speed { pos, .. } = *e
                && d.states
                    .get(&pos)
                    .and_then(|&s| self.table.get(s))
                    .is_some_and(|(b, _)| {
                        matches!(b, Block::MechanicalPiston | Block::StickyMechanicalPiston)
                    })
            {
                d.piston_checks.push(pos);
            }
            // A new block's first report (still) is no speed change for a clock that isn't
            // running: Create's `onSpeedChanged` doesn't come, so it doesn't assemble.
            if let Event::Speed { pos, speed, .. } = *e
                && let Some(b) = d.states.get(&pos).and_then(|&s| self.table.get(s))
                && matches!(
                    b.0,
                    Block::ClockworkBearing | Block::RopePulley | Block::ElevatorPulley
                )
                && !(b.0 == Block::ClockworkBearing
                    && speed == 0.0
                    && !d.lifts.clocks.contains_key(&pos))
            {
                d.lifts.speed_changed(pos, b.0);
            }
            if let Event::Speed { pos, speed, .. } = *e
                && d.states
                    .get(&pos)
                    .and_then(|&s| self.table.get(s))
                    .is_some_and(|(b, _)| {
                        matches!(b, Block::CuckooClock | Block::MysteriousCuckooClock)
                    })
            {
                d.cuckoo_turning.retain(|p| *p != pos);
                if speed != 0.0 {
                    d.cuckoo_turning.push(pos);
                }
            }
            if let Event::Speed { pos, speed, .. } = *e
                && d.drills.contains_key(&pos)
            {
                d.drilling.retain(|p| *p != pos);
                if speed != 0.0 {
                    d.drilling.push(pos);
                }
            }
            // Hoses move when their speed changes.
            if let Event::Speed { pos, speed, .. } = *e
                && d.plumbing.hoses.contains_key(&pos)
            {
                d.plumbing.hose_speeds.push((pos, speed));
            }
            if let Event::Speed { pos, .. } = *e
                && (d.mixers.contains_key(&pos) || d.presses.contains_key(&pos))
            {
                d.mixer_checks.push(pos);
            }
            if let Event::Speed { pos, speed, .. } = *e
                && let Some(fan) = d.fans.get_mut(&pos)
                && fan.speed != speed
            {
                fan.speed = speed;
                fan.rebuild = true;
                if !d.running_fans.contains(&pos) {
                    d.running_fans.push(pos);
                }
            }
        }
        // Gauges read their network after each solve of it (`onSpeedChanged`,
        // `updateFromNetwork`): a network's stress can change while their speed does not. Only
        // those in a solved network are looked at.
        let mut touched = std::mem::take(&mut d.gauges_touched);
        d.kinetics.take_touched(&mut touched);
        for pos in touched.drain(..) {
            if let Some(g) = d.gauges.get_mut(&pos)
                && let Some((speed, stress, capacity, over)) = d.kinetics.network(pos, 0)
                && g.update(speed, stress, capacity, over)
            {
                d.gauge_changes.push(pos);
            }
        }
        d.gauges_touched = touched;
        stats
    }

    /// Gauges whose needle clients must see again.
    pub fn take_gauge_changes(&mut self, dim: usize) -> Vec<[i32; 3]> {
        std::mem::take(&mut self.dims[dim].gauge_changes)
    }

    /// Fans to tick: each turning one, and one that just stopped (its current goes). Those whose
    /// current is due to be looked at again (`fanBlockCheckRate`) are marked to rebuild.
    pub fn fans_to_tick(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let d = &mut self.dims[dim];
        let mut out = Vec::with_capacity(d.running_fans.len());
        d.running_fans.retain(|pos| {
            let Some(fan) = d.fans.get_mut(pos) else {
                return false;
            };
            if fan.cooldown == 0 {
                fan.cooldown = crate::fan::CHECK_RATE;
                fan.rebuild = true;
            } else {
                fan.cooldown -= 1;
            }
            out.push(*pos);
            fan.speed != 0.0
        });
        out
    }

    /// A player scrolls on the creative motor at `pos`: its speed moves by one per step, between
    /// -256 and 256, never 0 (Create 5's scroll value; Create 6 sets it on a value board).
    /// Returns the new speed, or None when there is no motor there.
    pub fn scroll_motor(&mut self, dim: usize, pos: [i32; 3], steps: i32) -> Option<f32> {
        if steps == 0 {
            return None;
        }
        let mut new = self.motor_setting(dim, pos)? as i32;
        for _ in 0..steps.unsigned_abs().min(512) {
            new += steps.signum();
            if new == 0 {
                new += steps.signum();
            }
        }
        self.set_motor(dim, pos, new.clamp(-256, 256) as f32)
    }

    /// The creative motor or rotation speed controller at `pos`'s speed setting, None when there
    /// is neither there.
    pub fn motor_setting(&self, dim: usize, pos: [i32; 3]) -> Option<f32> {
        let state = *self.dims[dim].states.get(&pos)?;
        let (block, _) = self.table.get(state)?;
        has_speed_setting(block).then(|| {
            self.dims[dim]
                .settings
                .get(&pos)
                .copied()
                .unwrap_or_default()
                .motor_rpm
        })
    }

    /// A player picks `s` on the creative motor's value board (`KineticScrollValueBehaviour`
    /// with `between(-256, 256)`). Returns the new speed, or None when there is no motor there.
    pub fn board_motor(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        s: crate::board::Settings,
    ) -> Option<f32> {
        self.motor_setting(dim, pos)?;
        self.set_motor(dim, pos, crate::board::kinetic_value(s, 256) as f32)
    }

    fn set_motor(&mut self, dim: usize, pos: [i32; 3], rpm: f32) -> Option<f32> {
        let state = *self.dims[dim].states.get(&pos)?;
        let d = &mut self.dims[dim];
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        // ScrollValueBehaviour only invokes its callback when the value changes.
        if old.motor_rpm == rpm {
            return Some(rpm);
        }
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                motor_rpm: rpm,
                ..old
            },
        );
        Some(rpm)
    }

    /// The settings of a column's blocks, to save with it: `(position, motor speed)`.
    pub fn column_settings(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], i32)> {
        let mut out: Vec<([i32; 3], i32)> = self.dims[dim]
            .settings
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .filter(|(p, _)| {
                self.dims[dim]
                    .states
                    .get(*p)
                    .and_then(|s| self.table.get(*s))
                    .is_some_and(|(b, _)| has_speed_setting(b))
            })
            .map(|(p, s)| (*p, s.motor_rpm as i32))
            .collect();
        out.sort();
        out
    }

    /// A player turns the hand crank at `pos` (`HandCrankBlockEntity.turn`): it generates for
    /// the next 10 ticks, backwards when `back` (sneaking). False when there is no crank there.
    pub fn turn_crank(&mut self, dim: usize, pos: [i32; 3], back: bool) -> bool {
        let Some(&state) = self.dims[dim].states.get(&pos) else {
            return false;
        };
        if self.table.get(state).map(|(b, _)| b) != Some(Block::HandCrank) {
            return false;
        }
        let d = &mut self.dims[dim];
        let handle = d.handles.entry(pos).or_default();
        handle.saved.in_use = 10;
        handle.saved.backwards = back;
        d.handle_changes.insert(pos);
        let crank = if back { -1 } else { 1 };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.crank != crank {
            self.set_settings(dim, pos, state, Settings { crank, ..old });
        }
        true
    }

    /// `ValueSettingsBehaviour.isActive`: evaluate the behaviour's `onlyActiveWhen`
    /// supplier against current block-entity state, independently of its hit transform.
    pub fn value_box_active(&self, dim: usize, pos: [i32; 3]) -> bool {
        let d = &self.dims[dim];
        let Some((block, info)) = d.states.get(&pos).and_then(|s| self.table.get(*s)) else {
            return true;
        };
        match crate::board::Activity::of(block) {
            crate::board::Activity::Always => true,
            crate::board::Activity::IdleValve => {
                d.handles.get(&pos).is_none_or(|h| h.saved.in_use == 0)
            }
            crate::board::Activity::UndrivenSteamShaft => {
                let Some(connected) = engine_facing(info) else {
                    return true;
                };
                let shaft = crate::steam::shaft_pos(pos, connected);
                let available = self.block_at(dim, shaft) == Some(Block::PoweredShaft)
                    && d.shaft_engine.get(&shaft).is_none_or(|e| *e == pos);
                !available || !d.kinetics.driven_elsewhere(shaft, 0)
            }
            crate::board::Activity::DistributingTunnel => match (info.axis, info.tunnel) {
                (Some(axis), Some(shape)) => crate::tunnel::distributes(axis, shape, |side| {
                    let o = side.offset();
                    matches!(
                        self.block_at(dim, [pos[0] + o[0], pos[1], pos[2] + o[2]]),
                        Some(Block::AndesiteTunnel | Block::BrassTunnel)
                    )
                }),
                _ => false,
            },
            crate::board::Activity::BrassFilter => {
                matches!(block, Block::BrassFunnel | Block::BrassBeltFunnel)
            }
        }
    }

    /// A player uses the valve handle at `pos` (`ValveHandleBlockEntity.activate`): not while
    /// something else turns it (`getTheoreticalSpeed`), while it turns or for its cooldown;
    /// else it turns at 32 RPM, for as many ticks as its angle `value` (its value box's,
    /// negative counter-clockwise) takes and two more, backwards when `back` (sneaking), then
    /// waits 4 ticks. False when it doesn't turn or there is no valve handle there.
    pub fn use_valve(&mut self, dim: usize, pos: [i32; 3], back: bool, value: i32) -> bool {
        let Some(&state) = self.dims[dim].states.get(&pos) else {
            return false;
        };
        if !self
            .table
            .get(state)
            .is_some_and(|(b, _)| crate::dye::is_valve_handle(b))
        {
            return false;
        }
        let d = &mut self.dims[dim];
        let turning = d
            .kinetics
            .theoretical(pos, 0)
            .is_some_and(|(speed, _)| speed != 0.0);
        if turning
            || d.handles
                .get(&pos)
                .is_some_and(|h| h.saved.in_use > 0 || h.cooldown > 0)
        {
            return false;
        }
        d.handle_changes.insert(pos);
        d.handles.insert(
            pos,
            crate::valve::Handle {
                saved: crate::valve::SavedHandle {
                    in_use: crate::valve::use_ticks(value),
                    backwards: back,
                    angle: value,
                    total_use_ticks: crate::valve::use_ticks(value),
                    start_angle: 0,
                    target_angle: crate::valve::target_angle(value),
                },
                cooldown: crate::valve::COOLDOWN,
            },
        );
        let crank = if crate::valve::clockwise(value, back) {
            -1
        } else {
            1
        };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        // `SequenceContext.fromGearshift(TURN_ANGLE, rotationSpeed, target)`.
        let context = crate::kinetics::Context {
            distance: false,
            value: value.unsigned_abs() as f32,
            run: 0,
        };
        d.kinetics
            .set_sequence_context(pos, 0, Some((context, context.value / 32.0)));
        self.set_settings(dim, pos, state, Settings { crank, ..old });
        true
    }

    /// One tick of the cranks being turned: those whose time ran out stop; valve handles that
    /// stopped count their cooldown down (`ValveHandleBlockEntity.tick`, after the crank's).
    pub fn tick_cranks(&mut self, dim: usize) {
        let mut stopped = Vec::new();
        let d = &mut self.dims[dim];
        let mut positions: Vec<_> = d.handles.keys().copied().collect();
        positions.sort_by_key(|&pos| d.block_entities.registration_order(pos).unwrap_or(u64::MAX));
        for pos in &positions {
            let handle = d.handles.get_mut(pos).unwrap();
            d.kinetics.tick_base_through(*pos);
            if handle.saved.in_use > 0 {
                handle.saved.in_use -= 1;
                d.handle_changes.insert(*pos);
                if handle.saved.in_use == 0 {
                    stopped.push(*pos);
                }
            }
            if handle.saved.in_use == 0 && handle.cooldown > 0 {
                handle.cooldown -= 1;
            }
        }
        for pos in stopped {
            let d = &mut self.dims[dim];
            if let (Some(&state), Some(old)) = (d.states.get(&pos), d.settings.get(&pos).copied()) {
                // `HandCrankBlockEntity.tick`: the context goes when it stops.
                d.kinetics.set_sequence_context(pos, 0, None);
                self.set_settings(dim, pos, state, Settings { crank: 0, ..old });
            }
        }
    }

    /// Loaded crank/valve positions whose use data the host writes each tick.
    /// Handles whose saved use state changed since the last call (`HandCrankBlockEntity.write`
    /// runs when the entity is saved; the glue writes only what changed).
    pub fn take_handle_changes(&mut self, dim: usize) -> Vec<[i32; 3]> {
        self.dims[dim].handle_changes.drain().collect()
    }

    /// `HandCrankBlockEntity.write`, extended by `ValveHandleBlockEntity.write`.
    pub fn saved_handle(&self, dim: usize, pos: [i32; 3]) -> Option<crate::valve::SavedHandle> {
        let block = self.block_at(dim, pos)?;
        (block == Block::HandCrank || crate::dye::is_valve_handle(block)).then(|| {
            self.dims[dim]
                .handles
                .get(&pos)
                .map(|h| h.saved)
                .unwrap_or_default()
        })
    }

    /// Java `read`: restore generator inputs before restoring the kinetic snapshot.
    /// `cooldown` is not saved by Java and starts at zero.
    pub fn restore_handle(&mut self, dim: usize, pos: [i32; 3], saved: crate::valve::SavedHandle) {
        if self.saved_handle(dim, pos).is_none() {
            return;
        }
        let state = self.dims[dim].states[&pos];
        let old = self.dims[dim]
            .settings
            .get(&pos)
            .copied()
            .unwrap_or_default();
        let clockwise = if self.block_at(dim, pos) == Some(Block::HandCrank) {
            saved.backwards
        } else {
            crate::valve::clockwise(saved.angle, saved.backwards)
        };
        let crank = if saved.in_use == 0 {
            0
        } else if clockwise {
            -1
        } else {
            1
        };
        self.dims[dim]
            .handles
            .insert(pos, crate::valve::Handle { saved, cooldown: 0 });
        // HandCrankBlockEntity.read restores fields without updateGeneratedRotation.
        let settings = Settings { crank, ..old };
        self.dims[dim].settings.insert(pos, settings);
        if let Some((block, info)) = self.table.get(state) {
            let [a, b] = parts(block, info, &settings);
            for (sub, part) in [(0, a), (1, b)] {
                if let Some(part) = part {
                    self.dims[dim]
                        .kinetics
                        .change_block_state(pos, sub, part, true);
                }
            }
        }
    }

    /// Settings saved with a column, before its blocks are scanned.
    /// A creative motor's or speed controller's speed setting, taken from it as a moving
    /// structure carries it away (`ScrollValueBehaviour.write`); none for another block.
    pub fn take_motor_setting(&mut self, dim: usize, pos: [i32; 3]) -> Option<i32> {
        let rpm = self.motor_setting(dim, pos)?;
        self.dims[dim].settings.remove(&pos);
        Some(rpm as i32)
    }

    pub fn load_settings(&mut self, dim: usize, saved: impl Iterator<Item = ([i32; 3], i32)>) {
        for (pos, rpm) in saved {
            self.dims[dim].settings.insert(
                pos,
                Settings {
                    motor_rpm: rpm as f32,
                    ..Settings::default()
                },
            );
        }
    }

    /// A neighbour of the block `state` at `pos` changed: a block that follows redstone is
    /// checked after the tick. The first line is the hot path for every other block.
    #[inline]
    pub fn neighbor_changed(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        if !self.table.is_kinetic(state) {
            return;
        }
        match self.table.get(state).map(|(b, _)| b) {
            Some(b) if follows_power(b) => {
                self.dims[dim].redstone.push(pos);
                if b == Block::SmartChute {
                    // Something above it may have become an inventory.
                    self.dims[dim].chute_pulls.wake(pos);
                }
                if b == Block::GantryShaft {
                    self.dims[dim].gantry_checks.push(pos);
                }
                if b == Block::AdjustableChainGearshift {
                    self.dims[dim].chain_checks.push(pos);
                }
                if b == Block::SteamWhistle {
                    // `updateShape`: without its tank it breaks.
                    self.dims[dim].plumbing.whistle_support.push(pos);
                }
            }
            // `WhistleExtenderBlock.updateShape`: it joins the one above, or falls.
            Some(Block::SteamWhistleExtension) => {
                self.dims[dim].plumbing.extension_checks.push(pos)
            }
            Some(Block::EncasedChainDrive) => self.dims[dim].chain_checks.push(pos),
            // A crafter's power may have risen (`wasPoweredBefore`).
            Some(Block::MechanicalCrafter) => self.dims[dim].crafter_power_checks.push(pos),
            // `DeployerBlock.neighborChanged` → `redstoneUpdate`.
            Some(Block::Deployer) => self.dims[dim].deployer_power_checks.push(pos),
            Some(Block::SequencedGearshift) => self.dims[dim].sequencer_checks.push(pos),
            Some(Block::WaterWheel) => self.dims[dim].wheel_checks.push(pos),
            Some(Block::LargeWaterWheel) => self.dims[dim].large_wheel_checks.push(pos),
            // `WaterWheelStructuralBlock.updateShape`: its wheel looks at its flow again.
            Some(Block::WaterWheelStructure) => {
                if let Some(master) = self.wheel_master(dim, pos, state) {
                    self.dims[dim].wheel_checks.push(master);
                }
            }
            Some(b) if latch_kind(b).is_some() => self.dims[dim].latch_checks.push(pos),
            Some(b) if diode_kind(b).is_some() => self.dims[dim]
                .diode_checks
                .push((pos, crate::diode::BLOCK_DELAY)),
            Some(Block::RoseQuartzLamp) => self.dims[dim].lamp_checks.push(pos),
            Some(Block::Nozzle) => self.dims[dim].nozzle_checks.push(pos),
            Some(Block::Sticker) => self.dims[dim].sticker_checks.push(pos),
            Some(Block::PortableStorageInterface | Block::PortableFluidInterface) => {
                self.dims[dim].psi_checks.push(pos)
            }
            Some(Block::ContraptionControls) => self.dims[dim].controls_checks.push(pos),
            Some(Block::ControllerRail) => self.dims[dim].rail_checks.push(pos),
            Some(Block::ElevatorContact | Block::RedstoneContact) => {
                self.dims[dim].lifts.contact_checks.push(pos)
            }
            Some(Block::RedstoneLink) => self.dims[dim].link_checks.push(pos),
            Some(Block::DisplayLink) => self.dims[dim].displays.checks.push(pos),
            Some(b) if crate::display::is_nixie(b) => {
                self.dims[dim].displays.nixie_checks.push(pos)
            }
            Some(Block::ContentObserver | Block::StockpileSwitch) => {
                self.dims[dim].detectors.neighbor_changed(pos)
            }
            Some(Block::AndesiteTunnel | Block::BrassTunnel) => {
                self.dims[dim].tunnel_checks.push(pos)
            }
            Some(Block::GantryShaft) => self.dims[dim].gantry_checks.push(pos),
            Some(Block::Chute) => self.dims[dim].chute_pulls.wake(pos),
            Some(Block::MechanicalArm | Block::WeightedEjector) => {
                self.dims[dim].arm_signals.push(pos)
            }
            Some(
                Block::AndesiteFunnel
                | Block::BrassFunnel
                | Block::AndesiteBeltFunnel
                | Block::BrassBeltFunnel,
            ) => self.dims[dim].funnel_shapes.push(pos),
            // `FluidTankBlock.updateShape`: what lies under a boiler may heat it differently.
            Some(Block::FluidTank) => self.boiler_heat_changed(dim, pos),
            // `BeltBlock.updateShape`: the tunnel over a belt follows its neighbours.
            Some(Block::Belt) => self.dims[dim]
                .tunnel_fresh
                .push([pos[0], pos[1] + 1, pos[2]]),
            _ => {}
        }
    }

    /// The water wheels to look at in this tick (`tick` counts ticks): those whose neighbour
    /// changed, and one in 60 of the others (Create's `lazyTick` every 60 ticks), with their axis.
    pub fn take_wheel_checks(
        &mut self,
        dim: usize,
        tick: u64,
    ) -> Vec<([i32; 3], crate::kinetics::Axis, bool)> {
        let d = &mut self.dims[dim];
        let mut list = std::mem::take(&mut d.wheel_checks);
        list.extend(d.wheels.iter().filter(|p| {
            (p[0].wrapping_mul(31) ^ p[2].wrapping_mul(17) ^ p[1]).rem_euclid(60) as u64
                == tick % 60
        }));
        list.sort();
        list.dedup();
        list.into_iter()
            .filter_map(|p| {
                let state = *self.dims[dim].states.get(&p)?;
                let (block, info) = self.table.get(state)?;
                match block {
                    Block::WaterWheel => Some((p, info.facing?.axis(), false)),
                    Block::LargeWaterWheel => Some((p, info.axis?, true)),
                    _ => None,
                }
            })
            .collect()
    }

    /// `WaterWheelStructuralBlock.getMaster` of the structure block `state` at `pos`: the block
    /// it points at, through another structure block; Some only when that is a large wheel.
    pub fn wheel_master(&self, dim: usize, pos: [i32; 3], state: u32) -> Option<[i32; 3]> {
        let (mut at, mut state) = (pos, state);
        for _ in 0..2 {
            let (block, info) = self.table.get(state)?;
            if block != Block::WaterWheelStructure {
                return None;
            }
            let o = info.facing?.offset();
            at = [at[0] + o[0], at[1] + o[1], at[2] + o[2]];
            state = *self.dims[dim].states.get(&at)?;
            if self.table.get(state).map(|(b, _)| b) == Some(Block::LargeWaterWheel) {
                return Some(at);
            }
        }
        None
    }

    /// Millstones that turn: the only ones that do anything this tick (Create's tick returns at
    /// once without speed), with their speed.
    pub fn turning_millstones(&self, dim: usize) -> Vec<([i32; 3], f32)> {
        let d = &self.dims[dim];
        d.millstones
            .keys()
            .filter_map(|p| Some((*p, d.kinetics.speed(*p, 0).filter(|s| *s != 0.0)?)))
            .collect()
    }

    /// A water wheel's new flow score: it turns again when the score changed.
    pub fn set_wheel_flow(&mut self, dim: usize, pos: [i32; 3], score: i32) {
        let flow = score.clamp(-1, 1) as i8;
        let d = &mut self.dims[dim];
        let Some(&state) = d.states.get(&pos) else {
            return;
        };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.flow != flow {
            self.set_settings(dim, pos, state, Settings { flow, ..old });
        }
    }

    /// Blocks to check for redstone power, with whether each is powered now.
    pub fn take_redstone(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].redstone);
        v.sort();
        v.dedup();
        v
    }

    /// Whether the block `state` is powered (`powered`), for a block that follows redstone.
    pub fn powered(&self, state: u32) -> Option<bool> {
        let (block, info) = self.table.get(state)?;
        follows_power(block).then_some(info.powered)
    }

    /// Whether the block `state` reads how strong its signal is (an adjustable chain gearshift),
    /// besides whether it is powered.
    pub fn reads_signal(&self, state: u32) -> bool {
        self.table
            .get(state)
            .is_some_and(|(b, _)| b == Block::AdjustableChainGearshift)
    }

    /// `ChainGearshiftBlockEntity.analogSignalChanged`: the strongest signal into the adjustable
    /// chain gearshift at `pos` (`getBestNeighborSignal`); its chain turns again when it changed.
    pub fn set_chain_signal(&mut self, dim: usize, pos: [i32; 3], signal: i32) {
        let d = &mut self.dims[dim];
        if !d.states.contains_key(&pos) {
            return;
        }
        let signal = signal.clamp(0, 15) as i8;
        let known = d.settings.get(&pos).copied();
        if known.unwrap_or_default().signal != signal {
            let old = known.unwrap_or_default();
            d.settings.insert(pos, Settings { signal, ..old });
            d.kinetics
                .chain_signal_changed(pos, crate::chain::modifier_for_signal(i32::from(signal)));
        }
    }

    /// `ChainGearshiftBlockEntity.write`: the entity's persisted analog input.
    pub fn saved_chain_signal(&self, dim: usize, pos: [i32; 3]) -> Option<i32> {
        (self.block_at(dim, pos) == Some(Block::AdjustableChainGearshift)).then(|| {
            i32::from(
                self.dims[dim]
                    .settings
                    .get(&pos)
                    .copied()
                    .unwrap_or_default()
                    .signal,
            )
        })
    }

    /// `ChainGearshiftBlockEntity.read`: restore Signal before base kinetic attachment.
    pub fn restore_chain_signal(&mut self, dim: usize, pos: [i32; 3], signal: i32) {
        if self.block_at(dim, pos) != Some(Block::AdjustableChainGearshift) {
            return;
        }
        let state = self.dims[dim].states[&pos];
        let old = self.dims[dim]
            .settings
            .get(&pos)
            .copied()
            .unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                signal: signal.clamp(0, 15) as i8,
                ..old
            },
        );
    }

    /// The axis of the large cog at `pos` (`ICogWheel.isLargeCog`), for a speed controller placed
    /// below it.
    pub fn large_cog_axis(&self, dim: usize, pos: [i32; 3]) -> Option<crate::kinetics::Axis> {
        let state = *self.dims[dim].states.get(&pos)?;
        let (block, info) = self.table.get(state)?;
        matches!(
            block,
            Block::LargeCogwheel
                | Block::AndesiteEncasedLargeCogwheel
                | Block::BrassEncasedLargeCogwheel
        )
        .then_some(info.axis)?
    }

    /// The encased chain drive at `pos` as its chain rules read it; None for any other block.
    fn chain_drive(&self, dim: usize, pos: [i32; 3]) -> Option<crate::chain::Drive> {
        let state = *self.dims[dim].states.get(&pos)?;
        let (block, info) = self.table.get(state)?;
        if !is_chain_drive(block) {
            return None;
        }
        Some(crate::chain::Drive {
            axis: info.axis?,
            part: info.chain.unwrap_or_default(),
            along_first: info.along_first,
        })
    }

    /// Chain drives placed or beside a change, shaped by their neighbours
    /// (`ChainDriveBlock.updateShape` toward each side off their axis): those whose `part` or
    /// `axis_along_first` must change, with what they become. Hosts also drain this immediately
    /// after a kinetic break, before publishing the completed tick.
    pub fn take_chain_shapes(&mut self, dim: usize) -> Vec<([i32; 3], crate::chain::Drive)> {
        if self.dims[dim].chain_checks.is_empty() {
            return Vec::new();
        }
        let mut checks = std::mem::take(&mut self.dims[dim].chain_checks);
        checks.sort();
        checks.dedup();
        checks
            .into_iter()
            .filter_map(|pos| {
                let now = self.chain_drive(dim, pos)?;
                let shaped = now.shaped(|d| {
                    let o = d.offset();
                    self.chain_drive(dim, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
                });
                (shaped != now).then_some((pos, shaped))
            })
            .collect()
    }

    /// Sequenced gearshifts whose neighbour changed, with whether each runs (its `state` is
    /// not 0).
    pub fn take_sequencer_checks(&mut self, dim: usize) -> Vec<([i32; 3], bool)> {
        if self.dims[dim].sequencer_checks.is_empty() {
            return Vec::new();
        }
        let mut checks = std::mem::take(&mut self.dims[dim].sequencer_checks);
        checks.sort();
        checks.dedup();
        checks
            .into_iter()
            .filter_map(|pos| {
                let state = *self.dims[dim].states.get(&pos)?;
                let (block, info) = self.table.get(state)?;
                (block == Block::SequencedGearshift).then_some((pos, info.sequence != 0))
            })
            .collect()
    }

    /// `SequencedGearshiftBlock.tick`: the sequenced gearshift at `pos` is `powered` or not, its
    /// block `running` (`state` not 0). Returns what its block's `state` becomes.
    pub fn sequencer_redstone(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        powered: bool,
        running: bool,
    ) -> Option<crate::sequencer::Step> {
        let step = self.dims[dim]
            .sequencers
            .get_mut(&pos)?
            .redstone(powered, running);
        if step.is_some() {
            self.sequencer_stepped(dim, pos);
        }
        step
    }

    /// Where the sequence of the sequenced gearshift at `pos` is, to save with it.
    pub fn sequencer_run(&self, dim: usize, pos: [i32; 3]) -> Option<crate::sequencer::Run> {
        Some(self.dims[dim].sequencers.get(&pos)?.saved_run())
    }

    /// The sequenced gearshift at `pos` loaded mid-sequence: it goes on from `run`, its far half
    /// at the running instruction's modifier. Restore the saved kinetic state first,
    /// as the block entity load callback does; its speed and stress are read without
    /// a speed-change notification.
    pub fn restore_sequencer(&mut self, dim: usize, pos: [i32; 3], run: crate::sequencer::Run) {
        // SequencedGearshiftBlockEntity.read calls KineticBlockEntity.read: restore
        // speed and overStressed directly, without synthesizing onSpeedChanged.
        let (speed, overstressed) = self.dims[dim]
            .kinetics
            .theoretical(pos, 0)
            .unwrap_or((0.0, false));
        let Some(s) = self.dims[dim].sequencers.get_mut(&pos) else {
            return;
        };
        s.restore(run);
        s.speed = speed;
        s.overstressed = overstressed;
        // read restores fields; it does not run an instruction or detach kinetics.
        let modifier = s.modifier() as i8;
        let ticks = s.ticks();
        let d = &mut self.dims[dim];
        d.sequencing.retain(|p| *p != pos);
        if ticks {
            d.sequencing.push(pos);
        }
        let settings = d.settings.entry(pos).or_default();
        settings.sequence = modifier;
        if let Some((block, info)) = d.states.get(&pos).and_then(|state| self.table.get(*state)) {
            for (sub, part) in parts(block, info, settings).into_iter().enumerate() {
                if let Some(part) = part {
                    d.kinetics.read_part(pos, sub as u8, part);
                }
            }
        }
    }

    /// The instructions of the sequenced gearshift at `pos`.
    pub fn sequencer_instructions(
        &self,
        dim: usize,
        pos: [i32; 3],
    ) -> Option<Vec<crate::sequencer::Instruction>> {
        Some(self.dims[dim].sequencers.get(&pos)?.instructions.clone())
    }

    /// `ConfigureSequencedGearshiftPacket.applySettings`: the running sequence stops
    /// (`run(-1)`, its block's `state` back to 0 unless powered) and the new instructions are
    /// its own. Return the stop's state change immediately: the caller must commit it in
    /// the packet phase, before the next kinetic tick (not with that tick's instruction steps).
    /// With `stop` false (its block entity loading them) nothing else changes or is returned.
    pub fn set_sequencer_instructions(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        instructions: Vec<crate::sequencer::Instruction>,
        stop: bool,
    ) -> crate::sequencer::State {
        let s = self.dims[dim].sequencers.get_mut(&pos)?;
        let step = if stop { s.run(-1) } else { None };
        s.instructions = if instructions.is_empty() {
            crate::sequencer::default_instructions()
        } else {
            instructions
        };
        if stop {
            self.sequencer_stepped(dim, pos);
        }
        step
    }

    /// One tick of the sequenced gearshifts whose instruction runs (`tick`), and the steps they
    /// took as their speed changed: what their blocks' `state` becomes. Nothing without one.
    fn tick_sequencers(&mut self, dim: usize) -> Vec<([i32; 3], crate::sequencer::Step)> {
        let d = &mut self.dims[dim];
        if d.sequencing.is_empty() && d.sequencer_steps.is_empty() {
            return Vec::new();
        }
        let mut steps = std::mem::take(&mut d.sequencer_steps);
        for pos in d.sequencing.clone() {
            let step = self.dims[dim]
                .sequencers
                .get_mut(&pos)
                .and_then(|s| s.tick());
            if let Some(step) = step {
                self.sequencer_stepped(dim, pos);
                steps.push((pos, step));
            }
        }
        steps
    }

    /// The speed the sequenced gearshift at `pos` is turned at (its block entity's theoretical
    /// `speed`: the half turned, which the other turns at the modifier times) and whether it is
    /// overstressed, after a solve changed either.
    fn sequencer_speed(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &self.dims[dim];
        let (a, overstressed) = d.kinetics.theoretical(pos, 0).unwrap_or((0.0, false));
        let b = d.kinetics.theoretical(pos, 1).map_or(0.0, |(b, _)| b);
        let m = f32::from(d.settings.get(&pos).map_or(0, |s| s.sequence));
        let speed = if m == 0.0 {
            if a != 0.0 { a } else { b }
        } else if (b - a * m).abs() <= 1e-4 * b.abs().max(1.0) {
            a
        } else {
            b
        };
        let Some(s) = self.dims[dim].sequencers.get_mut(&pos) else {
            return;
        };
        if let Some(step) = s.speed_changed(speed, overstressed) {
            self.dims[dim].sequencer_steps.push((pos, step));
            // onSpeedChanged only assigns a new context when it actually runs
            // an instruction. In particular, load notifications preserve read's Sequence.
            self.sequencer_stepped(dim, pos);
        }
    }

    /// After a sequenced gearshift's step: it ticks while an instruction runs, and its far half
    /// turns at the instruction's modifier (`detachKinetics`, then `attachKinetics`).
    fn sequencer_stepped(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        let Some(s) = d.sequencers.get(&pos) else {
            return;
        };
        let (modifier, ticks, context) = (s.modifier() as i8, s.ticks(), s.context());
        d.sequencing.retain(|p| *p != pos);
        if ticks {
            d.sequencing.push(pos);
        }
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        let previous = d
            .kinetics
            .saved_state(pos, 1)
            .and_then(|s| s.sequence)
            .map(|s| s.0);
        if (old.sequence != modifier || previous != context)
            && let Some(&state) = d.states.get(&pos)
        {
            let speed = d.kinetics.theoretical(pos, 0).map_or(0.0, |s| s.0) * f32::from(modifier);
            let sequence = context.map(|c| (c, if speed == 0.0 { 0.0 } else { c.value / speed }));
            if old.sequence == modifier {
                d.kinetics.restart_sequence_output(pos);
            }
            self.set_settings(
                dim,
                pos,
                state,
                Settings {
                    sequence: modifier,
                    ..old
                },
            );
            self.dims[dim]
                .kinetics
                .set_sequence_context(pos, 1, sequence);
        }
    }

    /// `DepotBehaviour.write`: slots retain item identity; this saves its transport state.
    pub fn saved_depot(&self, dim: usize, pos: [i32; 3]) -> Option<crate::depot::SavedDepot> {
        Some(crate::depot::SavedDepot {
            held: *self.dims.get(dim)?.depots.get(&pos)?,
        })
    }

    /// `DepotBehaviour.read`, after block_loaded and before the first depot tick.
    pub fn restore_depot(&mut self, dim: usize, pos: [i32; 3], saved: crate::depot::SavedDepot) {
        self.depot_put(dim, pos, saved.held);
    }

    /// A stack was put on the depot at `pos` with this motion (it slides from now on), or its
    /// held stack left (None).
    pub fn depot_put(&mut self, dim: usize, pos: [i32; 3], held: Option<crate::depot::Held>) {
        let d = &mut self.dims[dim];
        if let Some(h) = d.depots.get_mut(&pos) {
            *h = held;
            if held.is_some_and(|h| !h.sliding()) && !d.depot_work.contains(&pos) {
                d.depot_work.push(pos);
            }
            if held.is_some_and(|h| h.sliding()) && !d.sliding.contains(&pos) {
                d.sliding.push(pos);
            }
            d.depot_changes.push(pos);
        }
    }

    /// A belt segment left (`BeltBlock.onRemove`): its belt drops its items, and the segments
    /// next to it break in turn (a pulley leaves its shaft).
    fn belt_removed(&mut self, dim: usize, pos: [i32; 3], seg: crate::belt::Segment) {
        let d = &mut self.dims[dim];
        if d.belt_quiet.remove(&pos) {
            return;
        }
        if let Some(c) = d.belt_of.remove(&pos)
            && let Some(belt) = d.belts.remove(&c)
        {
            let length = belt.chain.len() as i32;
            for item in belt.items.items {
                d.belt_drops.push((c, belt.segment, length, item));
            }
            for p in &belt.chain {
                d.belt_of.remove(p);
            }
            d.belt_changes.push(c);
            d.belt_saved_changes.insert(c);
        }
        for forward in [true, false] {
            let Some(next) = seg.next(pos, forward) else {
                continue;
            };
            let Some(n) = d
                .states
                .get(&next)
                .and_then(|s| self.table.get(*s))
                .and_then(|(_, i)| i.belt)
            else {
                continue;
            };
            d.belt_breaks.push(BeltBreak {
                pos: next,
                shaft: n.has_pulley().then(|| n.rotation_axis()),
            });
        }
    }

    /// Belts whose segments were placed or loaded: their chains, once whole (Create's
    /// `initBelt`), become belts whose segments turn together. Nothing to do when no belt
    /// changed.
    pub fn settle_belts(&mut self, dim: usize) {
        if self.dims[dim].belt_dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.dims[dim].belt_dirty);
        for pos in dirty {
            self.initialize_belt(dim, pos);
        }
    }

    /// BeltBlockEntity.tick initializes its own chain before the base kinetic tick.
    fn initialize_belt(&mut self, dim: usize, pos: [i32; 3]) {
        if self.dims[dim].belt_of.contains_key(&pos) {
            return;
        }
        let table = &self.table;
        let states = &self.dims[dim].states;
        let segment_at = |p: [i32; 3]| {
            states
                .get(&p)
                .and_then(|s| table.get(*s))
                .and_then(|(b, i)| (b == Block::Belt).then_some(i.belt).flatten())
        };
        let Some(controller) = crate::belt::controller_of(pos, segment_at) else {
            return;
        };
        let chain = crate::belt::chain(controller, segment_at);
        let whole = chain.len() >= 2
            && chain
                .last()
                .and_then(|p| segment_at(*p))
                .is_some_and(|s| s.part == crate::belt::Part::End);
        if !whole {
            return;
        }
        let Some(segment) = segment_at(controller) else {
            return;
        };
        let d = &mut self.dims[dim];
        for p in &chain {
            d.belt_of.insert(*p, controller);
        }
        let items = d.belt_saved.remove(&controller).unwrap_or_default();
        d.belts.insert(
            controller,
            Belt {
                chain: chain.clone(),
                segment,
                items,
            },
        );
        d.belt_changes.push(controller);
        d.belt_saved_changes.insert(controller);
        for p in chain {
            let Some(&state) = self.dims[dim].states.get(&p) else {
                continue;
            };
            let old = self.dims[dim].settings.get(&p).copied().unwrap_or_default();
            self.dims[dim].settings.insert(
                p,
                Settings {
                    belt: Some(controller),
                    ..old
                },
            );
            self.place(dim, p, state);
            // BeltBlock.initBelt attaches each member before inventory.tick.
            self.dims[dim].kinetics.attach_belt(p);
        }
    }

    /// `BeltSlicer`'s first step (`detachKinetics`, `beltLength = 0` on every segment): the belt
    /// with segment `pos` is taken apart, its chain and items handed back; its segments form
    /// belts again after the tick, from their states then (`settle_belts`).
    pub fn take_belt(&mut self, dim: usize, pos: [i32; 3]) -> Option<([i32; 3], Belt)> {
        let d = &mut self.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let b = d.belts.remove(&c)?;
        for p in &b.chain {
            d.belt_of.remove(p);
            d.belt_dirty.push(*p);
        }
        d.belt_changes.push(c);
        d.belt_saved_changes.insert(c);
        Some((c, b))
    }

    /// The belt segment at `pos` leaves without breaking its neighbours or dropping anything
    /// (the slicer's `UPDATE_MOVE_BY_PISTON`).
    pub fn remove_belt_quietly(&mut self, dim: usize, pos: [i32; 3]) {
        self.dims[dim].belt_quiet.insert(pos);
    }

    /// The belt whose controller will be `controller` takes `items` once its chain forms; its
    /// segments `chain` look for their chain after the tick.
    pub fn reform_belt(
        &mut self,
        dim: usize,
        controller: [i32; 3],
        chain: &[[i32; 3]],
        items: crate::belt::Inventory,
    ) {
        let d = &mut self.dims[dim];
        if !items.is_empty() {
            d.belt_saved.insert(controller, items);
        }
        d.belt_dirty.extend_from_slice(chain);
    }

    /// `BeltBlockEntity.write`: save membership even for empty inventories.
    pub fn saved_belt_segment(
        &self,
        dim: usize,
        pos: [i32; 3],
    ) -> Option<crate::belt::SavedSegment> {
        let d = &self.dims[dim];
        let controller = *d.belt_of.get(&pos)?;
        let b = d.belts.get(&controller)?;
        Some(crate::belt::SavedSegment {
            controller,
            index: b.chain.iter().position(|p| *p == pos)? as i32,
            length: b.chain.len() as i32,
        })
    }

    /// `BeltBlockEntity.read`: restore membership before restoring kinetic state.
    /// A loaded belt has a nonzero length and does not run `initBelt`/attachKinetics.
    pub fn restore_belt_segment(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        saved: crate::belt::SavedSegment,
    ) {
        if saved.length < 2 || saved.index < 0 || saved.index >= saved.length {
            return;
        }
        let Some(state) = self.dims[dim].states.get(&pos).copied() else {
            return;
        };
        let Some(mut segment) = self.table.get(state).and_then(|(_, i)| i.belt) else {
            return;
        };
        let d = &mut self.dims[dim];
        d.belt_of.insert(pos, saved.controller);
        d.settings.entry(pos).or_default().belt = Some(saved.controller);
        if pos == saved.controller {
            let mut chain = vec![pos];
            segment.part = crate::belt::Part::Middle;
            for _ in 1..saved.length {
                let Some(next) = segment.next(*chain.last().unwrap(), true) else {
                    return;
                };
                chain.push(next);
            }
            segment.part = crate::belt::Part::Start;
            let items = d.belt_saved.remove(&pos).unwrap_or_default();
            d.belts.insert(
                pos,
                Belt {
                    chain,
                    segment,
                    items,
                },
            );
        }
        self.place(dim, pos, state);
    }

    /// Items saved with belts in a column (`chunk_data_save`), for when their chains form.
    pub fn load_belt_items(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::belt::Inventory)>,
    ) {
        self.dims[dim].belt_saved.extend(saved);
    }

    /// Journal a mutation of `BeltInventory.write`'s items or PositiveOrder.
    /// Unlike client notifications, this includes ordinary transport each tick.
    pub fn belt_inventory_changed(&mut self, dim: usize, controller: [i32; 3]) {
        self.dims[dim].belt_saved_changes.insert(controller);
    }

    /// Consume inventory publications at the completed-tick boundary, before flushing columns.
    pub fn take_belt_saved_changes(&mut self, dim: usize) -> std::collections::HashSet<[i32; 3]> {
        std::mem::take(&mut self.dims[dim].belt_saved_changes)
    }

    /// The belts whose controller is in a column, with their items, to save.
    pub fn column_belts(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::belt::Inventory)> {
        let mut out: Vec<_> = self.dims[dim]
            .belts
            .iter()
            .filter(|(c, _)| (c[0] >> 4, c[2] >> 4) == (cx, cz))
            .map(|(c, b)| (*c, b.items.to_save()))
            .collect();
        out.sort_by_key(|(c, _)| *c);
        out
    }

    /// A windmill's sails were assembled (`assemble`): it generates from the next solve on.
    /// A mechanical bearing assembled what is in front of it: it turns at the network's speed,
    /// generating nothing.
    pub fn bearing_assembled(&mut self, dim: usize, pos: [i32; 3], w: Windmill) {
        let d = &mut self.dims[dim];
        d.windmills.insert(pos, w);
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    pub fn windmill_assembled(&mut self, dim: usize, pos: [i32; 3], state: u32, w: Windmill) {
        let speed = crate::contraption::windmill_speed(w.sails, w.clockwise);
        let d = &mut self.dims[dim];
        d.windmills.insert(pos, w);
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                windmill: speed,
                ..old
            },
        );
    }

    /// A running windmill's direction was set (`onDirectionChanged` → `updateGeneratedRotation`):
    /// it generates its speed the other way. Nothing for a windmill not running.
    pub fn windmill_direction(&mut self, dim: usize, pos: [i32; 3], state: u32, clockwise: bool) {
        let d = &mut self.dims[dim];
        let Some(w) = d.windmills.get_mut(&pos).filter(|w| w.bearing.is_none()) else {
            return;
        };
        if w.clockwise == clockwise {
            return;
        }
        w.clockwise = clockwise;
        let speed = crate::contraption::windmill_speed(w.sails, clockwise);
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                windmill: speed,
                ..old
            },
        );
    }

    /// A windmill was taken apart (`disassemble`): its structure, to put back; it stops.
    pub fn windmill_disassembled(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        state: u32,
    ) -> Option<Windmill> {
        let d = &mut self.dims[dim];
        let w = d.windmills.remove(&pos)?;
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        if w.bearing.is_some() {
            return Some(w);
        }
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                windmill: 0.0,
                ..old
            },
        );
        Some(w)
    }

    /// Windmills saved with a column, for when their bearings are found.
    pub fn load_windmills(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], Windmill)>,
    ) {
        self.dims[dim].windmill_saved.extend(saved);
    }

    /// The assembled windmills of a column, to save.
    pub fn column_windmills(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], Windmill)> {
        let mut out: Vec<_> = self.dims[dim]
            .windmills
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, w)| (*p, w.clone()))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// Fuel put in the burner at `pos` (`tryUpdateFuel`); true when it took it.
    pub fn fuel_burner(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        fuel: crate::burner::Fuel,
        time: i32,
        overflow: bool,
    ) -> bool {
        let d = &mut self.dims[dim];
        let Some(b) = d.burners.get_mut(&pos) else {
            return false;
        };
        if !b.try_fuel(fuel, time, overflow) {
            return false;
        }
        if !d.burning.contains(&pos) {
            d.burning.push(pos);
        }
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        true
    }

    /// One tick of the burning burners: those whose heat changed, with the new heat. A burner
    /// whose fuel ran out leaves the list.
    pub fn tick_burners(&mut self, dim: usize) -> Vec<([i32; 3], crate::fan::Heat)> {
        let d = &mut self.dims[dim];
        let mut changed = Vec::new();
        let burners = &mut d.burners;
        d.burning.retain(|pos| {
            let Some(b) = burners.get_mut(pos) else {
                return false;
            };
            let before = b.heat();
            b.tick();
            if b.heat() != before {
                changed.push((*pos, b.heat()));
            }
            b.burning()
        });
        for (p, _) in &changed {
            d.unsaved.push((p[0] >> 4, p[2] >> 4));
        }
        changed
    }

    /// A transmitter's input read (`transmit`): its receivers' new signals are queued, the column
    /// saved when it changed.
    /// The slots of the block at `pos` changed: the detectors looking at it (or at another
    /// block of its vault) look again. One test when no detector is in the dimension.
    pub fn detectors_contents_changed(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        if d.detectors.watching.is_empty() {
            return;
        }
        match d.vaults.inventory(pos) {
            Some(parts) => d.detectors.contents_changed(parts),
            None => d.detectors.contents_changed([pos]),
        }
    }

    /// Tanks whose contents changed this tick (`tank_changes`, read, not taken): the detectors
    /// looking at any of their blocks look again.
    pub fn detectors_tanks_changed(&mut self, dim: usize) {
        let d = &mut self.dims[dim];
        if d.detectors.watching.is_empty() || d.tank_changes.is_empty() {
            return;
        }
        for c in &d.tank_changes {
            match d.tanks.multis.get(c) {
                Some(m) => d
                    .detectors
                    .contents_changed(crate::tank::blocks(*c, m.width, m.height)),
                None => d.detectors.contents_changed([*c]),
            }
        }
    }

    pub fn link_transmit(&mut self, dim: usize, pos: [i32; 3], power: i32) {
        let d = &mut self.dims[dim];
        if d.links.links.get(&pos).is_some_and(|l| l.transmit != power) {
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
        let changed = d.links.transmit(pos, power);
        d.link_changes.extend(changed);
    }

    /// A player's controller buttons' frequencies pressed or released at `pos`
    /// (`receivePressed`): receivers whose signal changed update in the level's tick.
    pub fn controller_input(
        &mut self,
        dim: usize,
        player: u128,
        pos: [i32; 3],
        keys: &[crate::link::Key],
        pressed: bool,
    ) -> bool {
        let d = &mut self.dims[dim];
        // A newly pressed frequency heard by a receiver (`LINKED_CONTROLLER`).
        let heard = pressed
            && keys.iter().any(|k| {
                !d.controllers
                    .held
                    .iter()
                    .any(|h| h.player == player && h.key == *k)
                    && d.links.listened(k)
            });
        let changed = d
            .controllers
            .receive(&mut d.links, player, pos, keys, pressed);
        d.link_changes.extend(changed);
        heard
    }

    /// `LinkedControllerServerHandler.tick` of a level; whether any frequency is still held.
    pub fn tick_controllers(&mut self, dim: usize) -> bool {
        let d = &mut self.dims[dim];
        if d.controllers.is_empty() {
            return false;
        }
        let changed = d.controllers.tick(&mut d.links);
        d.link_changes.extend(changed);
        !d.controllers.is_empty()
    }

    /// A slot tuned (`setFrequency`).
    pub fn link_frequency(&mut self, dim: usize, pos: [i32; 3], first: bool, item: &str) {
        let d = &mut self.dims[dim];
        let changed = d.links.set_frequency(pos, first, item);
        d.link_changes.extend(changed);
        d.link_shown.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// A deployer's held stack or stroke changed: clients see it, its column is saved.
    pub fn set_deployer(&mut self, dim: usize, pos: [i32; 3], d: crate::deployer::Deployer) {
        let dd = &mut self.dims[dim];
        dd.deployers.insert(pos, d);
        dd.deployer_changes.push(pos);
        dd.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    pub fn load_brass(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::tunnel::Brass)>,
    ) {
        self.dims[dim].brass_saved.extend(saved);
    }

    /// The brass tunnels of a column holding a stack or not splitting (`stackToDistribute`,
    /// `selectionMode`), to save.
    pub fn column_brass(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::tunnel::Brass)> {
        let mut out: Vec<_> = self.dims[dim]
            .brass
            .iter()
            .filter(|(p, b)| {
                (p[0] >> 4, p[2] >> 4) == (cx, cz)
                    && (b.stack.is_some() || b.mode != crate::tunnel::Mode::Split)
            })
            .map(|(p, b)| (*p, b.clone()))
            .collect();
        out.sort_by_key(|e| e.0);
        out
    }

    pub fn load_deployers(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::deployer::Deployer)>,
    ) {
        self.dims[dim].deployer_saved.extend(saved);
    }

    /// The deployers of a column (`State`, `Timer`, `Reach`, the held stack), to save.
    pub fn column_deployers(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::deployer::Deployer)> {
        let mut out: Vec<_> = self.dims[dim]
            .deployers
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, d)| (*p, d.clone()))
            .collect();
        out.sort_by_key(|e| e.0);
        out
    }

    /// Chassis ranges set for `group`, saved with their columns.
    pub fn set_chassis_range(&mut self, dim: usize, group: &[[i32; 3]], range: i32) {
        let d = &mut self.dims[dim];
        for p in group {
            d.chassis_range.insert(*p, range);
            d.unsaved.push((p[0] >> 4, p[2] >> 4));
        }
    }

    /// A basin's tanks changed: its mixer or press looks again, its clients are told, its
    /// column is saved.
    pub fn set_basin_fluids(&mut self, dim: usize, pos: [i32; 3], t: crate::basin::Tanks) {
        let d = &mut self.dims[dim];
        if t == crate::basin::Tanks::default() {
            d.basin_fluids.remove(&pos);
        } else {
            d.basin_fluids.insert(pos, t);
        }
        d.mixer_checks.push([pos[0], pos[1] + 2, pos[2]]);
        d.basin_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// A diode left the world: its block entity goes; a set delay or count is saved no more.
    fn diode_removed(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        if let Some(x) = d.diodes.remove(&pos) {
            if x.running {
                d.diode_running.retain(|p| *p != pos);
            }
            if x.state != 0 || x.max != x.kind.default_delay() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
        }
    }

    /// `BrassDiodeBlockEntity.tick` for the diodes whose tick changes something: the ones whose
    /// block must change, with their new `powering` (`setBlockAndUpdate`). Diodes with nothing
    /// left to do leave the list.
    pub fn tick_diodes(&mut self, dim: usize) -> Vec<([i32; 3], bool)> {
        let d = &mut self.dims[dim];
        let mut out = Vec::new();
        let diodes = &mut d.diodes;
        d.diode_running.retain(|pos| {
            let Some(x) = diodes.get_mut(pos) else {
                return false;
            };
            if let Some(p) = x.tick() {
                out.push((*pos, p));
            }
            x.running = !x.idle();
            x.running
        });
        out
    }

    /// The delay set on a diode's value box (`ScrollValueBehaviour.setValue`); its column is
    /// saved.
    pub fn set_diode_delay(&mut self, dim: usize, pos: [i32; 3], value: i32) -> Option<i32> {
        let d = &mut self.dims[dim];
        let x = d.diodes.get_mut(&pos)?;
        if x.set_delay(value) {
            if !x.running && !x.idle() {
                x.running = true;
                d.diode_running.push(pos);
            }
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
        Some(x.max)
    }

    pub fn load_diodes(&mut self, dim: usize, saved: impl Iterator<Item = ([i32; 3], (i32, i32))>) {
        self.dims[dim].diode_saved.extend(saved);
    }

    /// The diodes of a column not at rest at their default delay (`BrassDiodeBlockEntity.write`:
    /// `State`, and the delay), to save.
    pub fn column_diodes(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], i32, i32)> {
        let mut out: Vec<_> = self.dims[dim]
            .diodes
            .iter()
            .filter(|(p, x)| {
                (p[0] >> 4, p[2] >> 4) == (cx, cz)
                    && (x.state != 0 || x.max != x.kind.default_delay())
            })
            .map(|(p, x)| (*p, x.state, x.max))
            .collect();
        out.sort_by_key(|e| e.0);
        out
    }

    /// The Create block at `pos` as this world last saw it, for blocks it keeps.
    pub fn block_at(&self, dim: usize, pos: [i32; 3]) -> Option<Block> {
        let state = *self.dims.get(dim)?.states.get(&pos)?;
        self.table.get(state).map(|(b, _)| b)
    }

    /// The Create state at `pos` as this world last saw it, for blocks it keeps.
    pub fn state_at(&self, dim: usize, pos: [i32; 3]) -> Option<u32> {
        self.dims.get(dim)?.states.get(&pos).copied()
    }

    pub fn load_links(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], (crate::link::Key, i32))>,
    ) {
        self.dims[dim].link_saved.extend(saved);
    }

    /// The links of a column (`LinkBehaviour.write`: frequencies; `Transmit`), to save.
    pub fn column_links(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::link::Key, i32)> {
        let mut out: Vec<_> = self.dims[dim]
            .links
            .links
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, l)| (*p, l.key.clone(), l.transmit))
            .collect();
        out.sort_by_key(|e| e.0);
        out
    }

    pub fn load_burners(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::burner::Burner)>,
    ) {
        self.dims[dim].burner_saved.extend(saved);
    }

    /// The burners of a column with fuel or a creative heat, to save.
    pub fn column_burners(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::burner::Burner)> {
        let mut out: Vec<_> = self.dims[dim]
            .burners
            .iter()
            .filter(|(p, b)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && b.saved())
            .map(|(p, b)| (*p, *b))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// A controller's crushing speed (`updateSpeed`): it ticks while not 0.
    pub fn set_crusher_speed(&mut self, dim: usize, pos: [i32; 3], speed: f32) {
        let d = &mut self.dims[dim];
        let Some(c) = d.crushers.get_mut(&pos) else {
            return;
        };
        c.1 = speed;
        d.running_crushers.retain(|p| *p != pos);
        if speed != 0.0 {
            d.running_crushers.push(pos);
        }
    }

    /// Controller contents saved with a column, for when their controllers are found.
    pub fn load_crushers(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::crusher::Crusher)>,
    ) {
        self.dims[dim].crusher_saved.extend(saved);
    }

    /// The controllers of a column that hold something, to save.
    pub fn column_crushers(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::crusher::Crusher)> {
        let mut out: Vec<_> = self.dims[dim]
            .crushers
            .iter()
            .filter(|(p, c)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && !c.0.slots.is_empty())
            .map(|(p, c)| (*p, c.0.clone()))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// The trains in a column now (where their pivot bogey stands), to save with it: where
    /// their points are by the graph's nodes.
    pub fn column_trains(&mut self, dim: usize, cx: i32, cz: i32) -> Vec<TrainOnTrack> {
        let r = &mut self.dims[dim].railway;
        let mut out = Vec::new();
        for t in r.trains.iter_mut().filter(|t| t.column == (cx, cz)) {
            t.train.remember(&r.net);
            out.push(t.clone());
        }
        out
    }

    /// Trains saved with a column that joined: each gets a fresh id.
    pub fn load_trains(&mut self, dim: usize, saved: Vec<TrainOnTrack>) -> Vec<u32> {
        self.dims[dim].railway.load_trains(saved)
    }

    /// FluidTankBlockEntity.read state staged until each column member is found.
    pub fn load_tanks(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::tank::SavedTank)>,
    ) {
        self.dims[dim].tank_saved.extend(saved);
    }

    /// FluidTankBlockEntity.write for every column member, including pending connectivity.
    pub fn column_tanks(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::tank::SavedTank)> {
        let mut out: Vec<_> = self.dims[dim]
            .tanks
            .parts
            .keys()
            .filter(|p| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .filter_map(|p| self.dims[dim].tanks.saved(*p).map(|saved| (*p, saved)))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// Creative tanks' sizes saved with a column (by controller).
    pub fn load_creative_tanks(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::tank::Multi)>,
    ) {
        self.dims[dim].creative_tank_saved.extend(saved);
    }

    /// The controllers of a column's creative tanks bigger than a block or without windows, to
    /// save (their fluid is saved with each block).
    pub fn column_creative_tanks(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::tank::Multi)> {
        let mut out: Vec<_> = self.dims[dim]
            .creative_tanks
            .multis
            .iter()
            .filter(|(p, m)| {
                (p[0] >> 4, p[2] >> 4) == (cx, cz) && (m.width, m.height, m.window) != (1, 1, true)
            })
            .map(|(p, m)| (*p, *m))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// Creative tank blocks whose part changed: each takes its tank's fluid, its state and
    /// drawing follow, its column is saved again unless just loaded.
    fn creative_tanks_touched(&mut self, dim: usize, save: bool) {
        let d = &mut self.dims[dim];
        d.creative_tanks.cleared.clear();
        for p in std::mem::take(&mut d.creative_tanks.touched) {
            if let Some((_, m)) = d.creative_tanks.get(p) {
                let fluid = m.tank.fluid;
                d.plumbing.creative.insert(p, fluid);
            }
            d.plumbing.creative_changes.push(p);
            d.tank_states.push(p);
            if save {
                d.unsaved.push((p[0] >> 4, p[2] >> 4));
            }
        }
    }

    /// The creative tank the block at `pos` is part of: its controller and tank.
    pub fn creative_tank(
        &self,
        dim: usize,
        pos: [i32; 3],
    ) -> Option<([i32; 3], crate::tank::Multi)> {
        self.dims[dim].creative_tanks.get(pos).map(|(c, m)| (c, *m))
    }

    /// `CreativeSmartFluidTank.setContainedFluid` on the creative tank of the block at `pos`:
    /// each of its blocks gives `fluid` (none: empty). Whether it changed.
    pub fn set_creative_fluid(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        fluid: Option<crate::tank::Fluid>,
    ) -> bool {
        let d = &mut self.dims[dim];
        let Some((c, m)) = d.creative_tanks.get_mut(pos) else {
            return false;
        };
        if m.tank.fluid == fluid {
            return false;
        }
        m.tank.fluid = fluid;
        let (w, h) = (m.width, m.height);
        for p in crate::tank::blocks(c, w, h) {
            if let Some(f) = d.plumbing.creative.get_mut(&p) {
                *f = fluid;
                d.plumbing.creative_changes.push(p);
                d.analog_checks.push(p);
            }
            d.unsaved.push((p[0] >> 4, p[2] >> 4));
        }
        true
    }

    /// Tank blocks whose part changed (formed, split or loaded): their states, drawing and
    /// boilers follow, and their columns are saved again unless just loaded
    /// (`removeController`'s `boiler.clear()` for blocks split off).
    fn tanks_touched(&mut self, dim: usize, save: bool) {
        let d = &mut self.dims[dim];
        for p in d.tanks.cleared.drain(..) {
            if let Some(b) = d.boilers.get_mut(&p) {
                b.data.clear();
            }
        }
        for p in d.tanks.touched.drain(..) {
            d.tank_changes.push(p);
            d.tank_states.push(p);
            d.boiler_checks.push(p);
            if save {
                d.unsaved.push((p[0] >> 4, p[2] >> 4));
            }
        }
    }

    /// The tank the block at `pos` is part of: its controller and tank.
    pub fn tank(&self, dim: usize, pos: [i32; 3]) -> Option<([i32; 3], crate::tank::Multi)> {
        self.dims[dim].tanks.get(pos).map(|(c, m)| (c, *m))
    }

    /// `FluidTankBlock.onWrenched` (`toggleWindows`): the tank at `pos` shows its windows or hides
    /// them, unless it is an active boiler. Whether it changed.
    pub fn toggle_tank_windows(&mut self, dim: usize, pos: [i32; 3]) -> bool {
        let d = &mut self.dims[dim];
        if let Some((c, m)) = d.creative_tanks.get_mut(pos) {
            m.window = !m.window;
            let (width, height) = (m.width, m.height);
            d.tank_states.extend(crate::tank::blocks(c, width, height));
            d.plumbing.creative_changes.push(c);
            d.unsaved.push((c[0] >> 4, c[2] >> 4));
            return true;
        }
        let Some((c, m)) = d.tanks.get_mut(pos) else {
            return false;
        };
        if d.boilers.get(&c).is_some_and(|b| b.data.is_active()) {
            return false;
        }
        m.window = !m.window;
        let (width, height) = (m.width, m.height);
        d.tank_states.extend(crate::tank::blocks(c, width, height));
        d.tank_changes.push(c);
        d.unsaved.push((c[0] >> 4, c[2] >> 4));
        true
    }

    /// The `top`, `bottom` and `shape` the tank block at `pos` should have.
    pub fn tank_shape(&self, dim: usize, pos: [i32; 3]) -> Option<crate::tank::BlockShape> {
        let d = &self.dims[dim];
        d.tanks
            .block_shape(pos)
            .or_else(|| d.creative_tanks.block_shape(pos))
    }

    /// Tank blocks whose states must follow their tank, each once.
    pub fn take_tank_states(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let d = &mut self.dims[dim];
        if d.tank_states.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut d.tank_states);
        v.sort();
        v.dedup();
        v
    }

    /// The tank of the block at `pos` set to `tank` (a bucket used on it).
    pub fn set_tank(&mut self, dim: usize, pos: [i32; 3], tank: crate::tank::Tank) {
        let d = &mut self.dims[dim];
        let Some((c, m)) = d.tanks.get_mut(pos) else {
            return;
        };
        m.tank = tank;
        d.tank_changes.push(c);
        d.unsaved.push((c[0] >> 4, c[2] >> 4));
    }

    /// Fluid poured into the tank of the block at `pos` (`FluidTank.fill`): what it took.
    pub fn fill_tank(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        fluid: crate::tank::Fluid,
        amount: u32,
    ) -> u32 {
        let d = &mut self.dims[dim];
        let Some((c, m)) = d.tanks.get_mut(pos) else {
            return 0;
        };
        let capacity = m.capacity();
        let took = m.tank.fill_amount(fluid, amount, capacity);
        if took > 0 {
            d.tank_changes.push(c);
            d.unsaved.push((c[0] >> 4, c[2] >> 4));
        }
        took
    }

    /// Fluid drawn from the tank of the block at `pos` (`FluidTank.drain`): what came out.
    pub fn drain_tank(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        amount: u32,
    ) -> Option<(crate::tank::Fluid, u32)> {
        let d = &mut self.dims[dim];
        let (c, m) = d.tanks.get_mut(pos)?;
        let out = m.tank.drain_amount(amount);
        if out.is_some() {
            d.tank_changes.push(c);
            d.unsaved.push((c[0] >> 4, c[2] >> 4));
        }
        out
    }

    /// The way the steam engine of `state` points (`SteamEngineBlock.getFacing`), if it is one.
    fn engine_of(&self, state: Option<u32>) -> Option<crate::kinetics::Dir> {
        match self.table.get(state?)? {
            (Block::SteamEngine, info) => engine_facing(info),
            _ => None,
        }
    }

    /// The way the steam engine at `pos` points, if there is one.
    pub fn engine_at(&self, dim: usize, pos: [i32; 3]) -> Option<crate::kinetics::Dir> {
        self.engine_of(self.dims[dim].states.get(&pos).copied())
    }

    /// `SteamEngineBlock.onRemove` (or turned away): its tank's boiler counts again, its shaft is
    /// looked at, and it stops driving it (`PoweredShaftBlockEntity.remove`).
    fn engine_removed(&mut self, dim: usize, pos: [i32; 3], connected: crate::kinetics::Dir) {
        let d = &mut self.dims[dim];
        let o = connected.opposite().offset();
        d.boiler_checks
            .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
        d.engine_shafts
            .push(crate::steam::shaft_pos(pos, connected));
        self.stop_engine(dim, pos);
    }

    /// The shaft the engine at `engine` drives, if any, is let go and stops.
    fn stop_engine(&mut self, dim: usize, engine: [i32; 3]) {
        let d = &mut self.dims[dim];
        let Some(shaft) = d
            .shaft_engine
            .iter()
            .find(|(_, e)| **e == engine)
            .map(|(s, _)| *s)
        else {
            return;
        };
        d.shaft_engine.remove(&shaft);
        let stopped = crate::steam::Output {
            direction: 0,
            efficiency: 0.0,
        };
        self.set_shaft_output(dim, shaft, stopped);
    }

    /// What drives the powered shaft at `pos` (its engine's direction and efficiency).
    pub fn shaft_steam(&self, dim: usize, pos: [i32; 3]) -> crate::steam::Output {
        self.dims[dim]
            .settings
            .get(&pos)
            .map(|s| s.steam)
            .unwrap_or_default()
    }

    /// A powered shaft's engine output, re-placed when it changed (`PoweredShaftBlockEntity
    /// .update`: `updateGeneratedRotation`).
    fn set_shaft_output(&mut self, dim: usize, shaft: [i32; 3], out: crate::steam::Output) {
        let d = &mut self.dims[dim];
        let Some(&state) = d.states.get(&shaft) else {
            return;
        };
        let old = d.settings.get(&shaft).copied().unwrap_or_default();
        if old.steam.differs(&out) {
            self.set_settings(dim, shaft, state, Settings { steam: out, ..old });
        }
    }

    /// Something under the boiler at `tank` changed: its heat is read again
    /// (`updateBoilerTemperature`, only for an active boiler).
    fn boiler_heat_changed(&mut self, dim: usize, tank: [i32; 3]) {
        let d = &mut self.dims[dim];
        let Some(tank) = d.tanks.controller(tank) else {
            return;
        };
        if let Some(b) = d.boilers.get_mut(&tank) {
            b.data.needs_heat_level_update = true;
            d.boiler_heat_checks.push(tank);
        }
    }

    /// `updateBoilerState` for the tank blocks to look at, by their controller: the engines on
    /// every block of the tank counted (`BoilerData.evaluate`); a tank without engines is no
    /// boiler, a new boiler loses its windows (`setWindows(false)`), and engines no longer on a
    /// boiler stop their shafts.
    pub fn evaluate_boilers(&mut self, dim: usize) {
        if self.dims[dim].boiler_checks.is_empty() {
            return;
        }
        let mut checks = std::mem::take(&mut self.dims[dim].boiler_checks);
        checks.sort();
        checks.dedup();
        let mut stopped = Vec::new();
        for p in checks {
            let controller = self.dims[dim].tanks.controller(p);
            // A boiler whose tank went or joined another (`BoilerData.clear`).
            if controller != Some(p)
                && let Some(b) = self.dims[dim].boilers.remove(&p)
            {
                stopped.extend(b.engines);
                self.dims[dim]
                    .plumbing
                    .sound_checks
                    .extend_from_slice(&b.whistles);
            }
            let Some(c) = controller else {
                continue;
            };
            let Some(m) = self.dims[dim].tanks.multis.get(&c).copied() else {
                continue;
            };
            let parts = &self.dims[dim].tanks.parts;
            let engines = crate::steam::attached_engines(
                crate::tank::blocks(c, m.width, m.height).filter(|q| parts.contains_key(q)),
                |q| self.engine_at(dim, q),
            );
            let whistles = crate::steam::attached_whistles(
                crate::tank::blocks(c, m.width, m.height).filter(|q| parts.contains_key(q)),
                |q| self.whistle_at(dim, q),
            );
            let d = &mut self.dims[dim];
            if engines.is_empty() && whistles.is_empty() {
                if let Some(b) = d.boilers.remove(&c) {
                    stopped.extend(b.engines);
                    d.plumbing.sound_checks.extend_from_slice(&b.whistles);
                }
                continue;
            }
            let b = d.boilers.entry(c).or_default();
            let was_active = b.data.is_active();
            stopped.extend(b.engines.iter().filter(|e| !engines.contains(e)).copied());
            d.plumbing.sound_checks.extend_from_slice(&b.whistles);
            d.plumbing.sound_checks.extend_from_slice(&whistles);
            b.size = m.blocks();
            b.data.attached_engines = engines.len() as i32;
            b.data.attached_whistles = whistles.len() as i32;
            b.data.needs_heat_level_update = true;
            b.engines = engines;
            b.whistles = whistles;
            d.boiler_heat_checks.push(c);
            if !was_active && m.window {
                if let Some(m) = d.tanks.multis.get_mut(&c) {
                    m.window = false;
                }
                d.tank_states
                    .extend(crate::tank::blocks(c, m.width, m.height));
                d.tank_changes.push(c);
                d.unsaved.push((c[0] >> 4, c[2] >> 4));
            }
        }
        // Engines on no boiler now stop (an engine moved to another boiler keeps going).
        stopped.retain(|e| {
            !self.dims[dim]
                .boilers
                .values()
                .any(|b| b.engines.contains(e))
        });
        for e in stopped {
            self.stop_engine(dim, e);
        }
    }

    /// Boilers whose heat must be read again: their controllers and widths (the heaters are
    /// the blocks under the tank's bottom layer).
    pub fn take_heat_checks(&mut self, dim: usize) -> Vec<([i32; 3], u8)> {
        let d = &mut self.dims[dim];
        if d.boiler_heat_checks.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut d.boiler_heat_checks);
        v.sort();
        v.dedup();
        v.into_iter()
            .filter(|p| {
                d.boilers
                    .get(p)
                    .is_some_and(|b| b.data.needs_heat_level_update)
            })
            .filter_map(|p| Some((p, d.tanks.multis.get(&p)?.width)))
            .collect()
    }

    /// The heat of each block under the boiler at `tank` (`updateTemperature`); its engines
    /// follow.
    pub fn set_boiler_heat(&mut self, dim: usize, tank: [i32; 3], heats: &[i32]) {
        let Some(b) = self.dims[dim].boilers.get_mut(&tank) else {
            return;
        };
        if b.data.update_temperature(heats.iter().copied()) {
            // Its whistles sound only while it is heated.
            self.boiler_whistles_changed(dim, tank);
        }
        self.drive_engines(dim, tank);
    }

    /// One tick of the boilers (`BoilerData.tick`): each samples its water supply every 5 ticks,
    /// and its engines follow. Tanks without engines are not here.
    pub fn tick_boilers(&mut self, dim: usize) {
        let d = &mut self.dims[dim];
        if d.boilers.is_empty() {
            return;
        }
        let mut sampled = std::mem::take(&mut d.boiler_sampled);
        for (tank, b) in d.boilers.iter_mut() {
            let capacity = d.tanks.multis.get(tank).map_or(0, |m| m.capacity());
            if b.data.tick(capacity, false) {
                sampled.push(*tank);
            }
        }
        for &tank in &sampled {
            self.drive_engines(dim, tank);
        }
        sampled.clear();
        self.dims[dim].boiler_sampled = sampled;
    }

    /// Water pumped into the tank at `tank`: a boiler takes it all as supply
    /// (`BoilerFluidHandler.fill`); None when the tank is no boiler.
    pub fn fill_boiler(
        &mut self,
        dim: usize,
        tank: [i32; 3],
        fluid: crate::tank::Fluid,
        amount: u32,
    ) -> Option<u32> {
        let d = &mut self.dims[dim];
        let c = d.tanks.controller(tank)?;
        let b = d.boilers.get_mut(&c)?;
        Some(b.data.fill(fluid == crate::tank::Fluid::Water, amount))
    }

    /// BoilerFluidHandler.fill(SIMULATE) validates the same resource without
    /// contributing to gatheredSupply.
    pub fn simulate_fill_boiler(
        &self,
        dim: usize,
        tank: [i32; 3],
        fluid: crate::tank::Fluid,
        amount: u32,
    ) -> Option<u32> {
        let d = &self.dims[dim];
        let controller = d.tanks.controller(tank)?;
        let mut data = d.boilers.get(&controller)?.data.clone();
        Some(data.fill(fluid == crate::tank::Fluid::Water, amount))
    }

    /// Whether the tank at `tank` is a boiler (its fluid handler is the boiler's: nothing can be
    /// drawn from it).
    pub fn is_boiler(&self, dim: usize, tank: [i32; 3]) -> bool {
        let d = &self.dims[dim];
        d.tanks
            .controller(tank)
            .is_some_and(|c| d.boilers.contains_key(&c))
    }

    /// `SteamEngineBlockEntity.tick` for the engines of the boiler at `tank`: each drives the
    /// powered shaft in front of it, unless another engine does (`canBePoweredBy`), at the
    /// boiler's efficiency.
    fn drive_engines(&mut self, dim: usize, tank: [i32; 3]) {
        let Some(b) = self.dims[dim].boilers.get(&tank) else {
            return;
        };
        let efficiency = b.data.engine_efficiency(b.size).clamp(0.0, 1.0);
        for i in 0..b.engines.len() {
            // By index: driving a shaft does not touch the boilers, and nothing is cloned.
            let Some(&engine) = self.dims[dim]
                .boilers
                .get(&tank)
                .and_then(|b| b.engines.get(i))
            else {
                break;
            };
            let Some((Block::SteamEngine, info)) = self.dims[dim]
                .states
                .get(&engine)
                .and_then(|s| self.table.get(*s))
            else {
                continue;
            };
            let (Some(connected), Some(facing)) = (engine_facing(info), info.facing) else {
                continue;
            };
            let shaft = crate::steam::shaft_pos(engine, connected);
            let Some((Block::PoweredShaft, shaft_info)) = self.dims[dim]
                .states
                .get(&shaft)
                .and_then(|s| self.table.get(*s))
            else {
                continue;
            };
            let Some(axis) = shaft_info.axis else {
                continue;
            };
            let d = &mut self.dims[dim];
            if d.shaft_engine.get(&shaft).is_some_and(|e| *e != engine) {
                continue;
            }
            d.shaft_engine.insert(shaft, engine);
            // `movementDirection` (its value box: 0 clockwise, 1 counter-clockwise).
            let counter = d.scroll_options.get(&engine) == Some(&1);
            let mut direction =
                crate::steam::conveyed_direction(efficiency, connected, facing, axis, counter);
            // A shaft another source turns the other way turns the engine round to it
            // (`hasSource`, `setValue(1 - ordinal)`).
            let shaft_speed = d.kinetics.theoretical(shaft, 0).map_or(0.0, |t| t.0);
            if shaft_speed != 0.0
                && (shaft_speed > 0.0) != (direction > 0)
                && d.kinetics.driven_elsewhere(shaft, 0)
            {
                if counter {
                    d.scroll_options.remove(&engine);
                } else {
                    d.scroll_options.insert(engine, 1);
                }
                d.unsaved.push((engine[0] >> 4, engine[2] >> 4));
                direction = -direction;
            }
            self.set_shaft_output(
                dim,
                shaft,
                crate::steam::Output {
                    direction,
                    efficiency,
                },
            );
        }
    }

    /// Shafts in front of engines that changed: `(position, powered)` for each that becomes a
    /// powered shaft (`SteamEngineBlock.onPlace`: `PoweredShaftBlock.getEquivalent`) or a plain
    /// one again (`PoweredShaftBlock.tick` when not `stillValid`).
    pub fn shaft_changes(&mut self, dim: usize) -> Vec<([i32; 3], bool)> {
        if self.dims[dim].engine_shafts.is_empty() {
            return Vec::new();
        }
        let mut checks = std::mem::take(&mut self.dims[dim].engine_shafts);
        checks.sort();
        checks.dedup();
        let mut out = Vec::new();
        for p in checks {
            let Some((block, info)) = self.dims[dim]
                .states
                .get(&p)
                .and_then(|s| self.table.get(*s))
            else {
                continue;
            };
            let Some(axis) = info.axis else {
                continue;
            };
            let valid = crate::steam::still_valid(p, axis, |q| self.engine_at(dim, q));
            match block {
                Block::Shaft if valid => out.push((p, true)),
                Block::PoweredShaft if !valid => {
                    let d = &mut self.dims[dim];
                    d.shaft_engine.remove(&p);
                    if let Some(s) = d.settings.get_mut(&p) {
                        s.steam = crate::steam::Output::default();
                    }
                    out.push((p, false));
                }
                _ => {}
            }
        }
        out
    }

    /// The column of `pos` must be saved again (its belt's items moved).
    pub fn mark_unsaved(&mut self, dim: usize, pos: [i32; 3]) {
        self.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// Level block-entity registration order shared by DepotBehaviour.tick,
    /// BeltBlockEntity.tick, SpoutBlockEntity.tick, ItemDrainBlockEntity.tick and
    /// PumpBlockEntity.tick. Call next only after applying the previous entity's transfers.
    pub fn transport_ticks(&mut self, dim: usize) -> TransportTicks {
        TransportTicks {
            dim,
            entities: self.dims[dim].block_entities.walk(View::Transport),
        }
    }

    /// One DepotBehaviour.tick, including a stack inserted by an earlier belt this tick.
    /// Returns its arrival test, before movement, so received/held callbacks run on this turn.
    pub fn tick_depot(&mut self, dim: usize, pos: [i32; 3], upright: bool) -> bool {
        let d = &mut self.dims[dim];
        let Some(Some(h)) = d.depots.get_mut(&pos) else {
            return false;
        };
        let arrived = h.slide(upright);
        if arrived && !d.depot_work.contains(&pos) {
            d.depot_work.push(pos);
        }
        if !h.sliding() {
            d.sliding.retain(|p| *p != pos);
        }
        arrived
    }

    /// Belts currently holding items, for observation only. Use `transport_ticks` to tick.
    pub fn busy_belts(&self, dim: usize) -> Vec<[i32; 3]> {
        self.dims[dim]
            .belts
            .iter()
            .filter(|(_, b)| !b.items.is_empty())
            .map(|(c, _)| *c)
            .collect()
    }

    /// The depot at `pos` changed in a way clients must see (`notifyUpdate`).
    pub fn depot_changed(&mut self, dim: usize, pos: [i32; 3]) {
        if self.dims[dim].depots.contains_key(&pos) {
            self.dims[dim].depot_changes.push(pos);
        }
    }

    /// The depot at `pos` asks the press above it from the next tick on (its stack arrived or
    /// changed, the press finished or started turning).
    pub fn wake_depot(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        if d.depots.contains_key(&pos) && !d.depot_work.contains(&pos) {
            d.depot_work.push(pos);
        }
    }

    /// Depots whose contents changed since the last call, once each.
    pub fn take_depot_changes(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].depot_changes);
        v.sort();
        v.dedup();
        v
    }

    pub fn take_unsaved(&mut self, dim: usize) -> Vec<(i32, i32)> {
        let mut v = std::mem::take(&mut self.dims[dim].unsaved);
        v.sort();
        v.dedup();
        v
    }

    /// `IRotate.hasShaftTowards`: whether the kinetic block at `pos` has a shaft on `face`.
    pub fn has_shaft_towards(&self, dim: usize, pos: [i32; 3], face: crate::kinetics::Dir) -> bool {
        let d = &self.dims[dim];
        let Some((block, info)) = d.states.get(&pos).and_then(|&s| self.table.get(s)) else {
            return false;
        };
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        parts(block, info, &settings)
            .iter()
            .flatten()
            .any(|p| p.shafts & face.bit() != 0)
    }

    /// The cog at `pos` (`ICogWheel.isSmallCog` / `isLargeCog`): whether it is large, and its
    /// axis; None for a block without one.
    pub fn cog_at(&self, dim: usize, pos: [i32; 3]) -> Option<(bool, crate::kinetics::Axis)> {
        let d = &self.dims[dim];
        let (block, info) = d.states.get(&pos).and_then(|&s| self.table.get(s))?;
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        parts(block, info, &settings)
            .iter()
            .flatten()
            .find_map(|p| match p.cog {
                crate::kinetics::Cog::Small => Some((false, p.axis)),
                crate::kinetics::Cog::Large => Some((true, p.axis)),
                crate::kinetics::Cog::None => None,
            })
    }

    /// `CogWheelBlock.isValidCogwheelPosition`: no large cog (nor, for a large one, a small cog)
    /// beside `pos` off `axis`, unless that neighbour's own axis points at it.
    pub fn valid_cog_position(
        &self,
        dim: usize,
        pos: [i32; 3],
        large: bool,
        axis: crate::kinetics::Axis,
    ) -> bool {
        crate::kinetics::Dir::ALL
            .into_iter()
            .filter(|d| d.axis() != axis)
            .all(|d| {
                let o = d.offset();
                let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                let along = self.dims[dim]
                    .states
                    .get(&at)
                    .and_then(|&s| self.table.get(s))
                    .is_some_and(|(_, i)| i.axis == Some(d.axis()));
                along
                    || match self.cog_at(dim, at) {
                        Some((other_large, _)) => !other_large && !large,
                        None => true,
                    }
            })
    }

    pub fn speed(&self, dim: usize, pos: [i32; 3]) -> Option<f32> {
        self.dims[dim].kinetics.speed(pos, 0)
    }

    /// Backtanks saved with a column (`BacktankBlockEntity.read`): those already placed take
    /// their air now, the others when their block is found.
    pub fn load_backtanks(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::equipment::Backtank)>,
    ) {
        let d = &mut self.dims[dim];
        for (pos, b) in saved {
            match d.backtanks.get_mut(&pos) {
                Some(placed) => *placed = b,
                None => {
                    d.backtank_saved.insert(pos, b);
                }
            }
        }
    }

    /// The backtanks of a column holding air or waiting, to save, sorted by position.
    pub fn column_backtanks(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::equipment::Backtank)> {
        let mut v: Vec<_> = self.dims[dim]
            .backtanks
            .iter()
            .filter(|(p, b)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && **b != Default::default())
            .map(|(p, b)| (*p, *b))
            .collect();
        v.sort_by_key(|e| e.0);
        v
    }

    /// The backtanks whose tick does something (turning, dry, waiting or not full), with their
    /// speed: the only ones ticked.
    pub fn filling_backtanks(&self, dim: usize) -> Vec<([i32; 3], f32)> {
        let d = &self.dims[dim];
        d.backtanks
            .iter()
            .filter_map(|(p, b)| {
                let speed = d.kinetics.speed(*p, 0)?;
                let state = *d.states.get(p)?;
                let waterlogged = self.table.get(state)?.1.waterlogged;
                b.ticks(speed, waterlogged).then_some((*p, speed))
            })
            .collect()
    }

    /// A backtank's air changed: its column saves again.
    pub fn backtank_changed(&mut self, dim: usize, pos: [i32; 3]) {
        self.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// The blocks comparators read the same output from as the block at `pos`: a vault's
    /// (`ItemVaultBlockEntity.updateComparators`) or a fluid tank's blocks, else the block.
    pub fn analog_unit(&self, dim: usize, pos: [i32; 3]) -> Vec<[i32; 3]> {
        let d = &self.dims[dim];
        if let Some(parts) = d.vaults.inventory(pos) {
            return parts;
        }
        if let Some((c, m)) = d.tanks.get(pos) {
            return crate::tank::blocks(c, m.width, m.height).collect();
        }
        vec![pos]
    }
}

/// Valves, smart pipes, hose pulleys, creative tanks and whistles.
impl CreateWorld {
    /// Deliver KineticBlockEntity subclass onSpeedChanged callbacks before publishing
    /// entity data. Java invokes these synchronously during rotation propagation;
    /// pointer.chase changes the saved target without advancing the pointer.
    pub fn flush_kinetic_callbacks(&mut self, dim: usize) {
        let d = &mut self.dims[dim];
        for (pos, speed) in d.kinetics.take_speed_notifications() {
            if let Some(pointer) = d.plumbing.valves.get_mut(&pos) {
                pointer.speed_changed(speed);
                d.kinetics.mark_saved_changed(pos);
            }
        }
    }

    /// FluidValveBlockEntity.write / LerpedFloat.writeNBT: value, chase target and speed.
    pub fn saved_valve_pointer(&self, dim: usize, pos: [i32; 3]) -> Option<crate::fluids::Pointer> {
        self.dims[dim].plumbing.valves.get(&pos).copied()
    }

    /// FluidValveBlockEntity.read / LerpedFloat.readNBT, before the first entity tick.
    pub fn restore_valve_pointer(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        saved: crate::fluids::Pointer,
    ) {
        if let Some(pointer) = self.dims[dim].plumbing.valves.get_mut(&pos) {
            *pointer = saved;
            self.dims[dim].kinetics.mark_saved_changed(pos);
        }
    }

    /// Fluid tank connectivity and FluidValveBlockEntity.tick in shared registration order.
    /// The kinetic base tick and onSpeedChanged run before the valve pointer advances.
    pub fn tick_fluid_entities(
        &mut self,
        dim: usize,
        open: impl Fn([i32; 3]) -> Option<bool>,
    ) -> Vec<([i32; 3], bool)> {
        let mut positions = self.dims[dim].block_entities.walk(View::Transport);
        let mut out = Vec::new();
        while let Some(pos) = positions.next(&self.dims[dim].block_entities) {
            if matches!(
                self.block_at(dim, pos),
                Some(Block::FluidValve | Block::FluidTank | Block::CreativeFluidTank)
            ) && let Some(next) = self.tick_fluid_entity(dim, pos, open(pos))
            {
                out.push((pos, next));
            }
        }
        out
    }

    /// One FluidValveBlockEntity/FluidTankBlockEntity turn, interleaved with transfers.
    pub fn tick_fluid_entity(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        open: Option<bool>,
    ) -> Option<bool> {
        let d = &mut self.dims[dim];
        if d.plumbing.valves.contains_key(&pos) {
            d.kinetics.tick_base_through(pos);
        }
        self.flush_kinetic_callbacks(dim);
        let d = &mut self.dims[dim];
        d.tanks.tick(pos);
        d.creative_tanks.tick(pos);
        let mut out = None;
        if let Some(pointer) = d.plumbing.valves.get_mut(&pos)
            && let Some(enabled) = open
        {
            let before = *pointer;
            out = pointer.tick(enabled);
            if *pointer != before {
                d.kinetics.mark_saved_changed(pos);
            }
        }
        self.tanks_touched(dim, true);
        self.creative_tanks_touched(dim, true);
        out
    }

    /// KineticBlockEntity.tick precedes its FluidTransportBehaviour tick.
    pub fn fluid_transport_base_tick(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        let mut prefix = d.block_entities.walk_through(View::All, pos);
        while let Some(p) = prefix.next(&d.block_entities) {
            d.kinetics.tick_registered_base(p);
        }
    }

    /// Pipe behaviours tick in Level.blockEntityTickers registration order.
    pub fn fluid_transport_positions(&self, dim: usize) -> Vec<[i32; 3]> {
        let d = &self.dims[dim];
        let mut walk = d.block_entities.walk(View::All);
        let mut out = Vec::new();
        while let Some(pos) = walk.next(&d.block_entities) {
            if self.block_at(dim, pos).is_some_and(|b| {
                matches!(
                    b,
                    Block::FluidPipe
                        | Block::EncasedFluidPipe
                        | Block::GlassFluidPipe
                        | Block::SmartFluidPipe
                        | Block::FluidValve
                        | Block::MechanicalPump
                )
            }) {
                out.push(pos);
            }
        }
        out
    }

    /// A valve, smart pipe, hose pulley, creative tank, whistle or extension placed, loaded or
    /// changed.
    fn plumbing_placed(&mut self, dim: usize, pos: [i32; 3], block: Block, info: &StateInfo) {
        let d = &mut self.dims[dim];
        let p = &mut d.plumbing;
        match block {
            Block::FluidValve => p.valve_placed(pos),
            Block::HosePulley if !p.hoses.contains_key(&pos) => {
                let (offset, tank) = p.hose_saved.remove(&pos).unwrap_or_default();
                p.hoses.insert(
                    pos,
                    crate::hose::Hose {
                        offset,
                        tank,
                        ..Default::default()
                    },
                );
                p.hose_changes.push(pos);
            }
            Block::CreativeFluidTank if !p.creative.contains_key(&pos) => {
                let fluid = p.creative_saved.remove(&pos).flatten();
                p.creative.insert(pos, fluid);
                p.creative_changes.push(pos);
            }
            Block::SteamWhistle => {
                p.whistles.entry(pos).or_default();
                // `onPlace`: its tank counts it (`updateBoilerState`), its pitch is counted;
                // placed or powered, it may sound.
                p.whistle_checks.push(pos);
                p.sound_checks.push(pos);
                if let Some(a) = whistle_attached(info) {
                    let o = a.offset();
                    d.boiler_checks
                        .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
                }
            }
            Block::SteamWhistleExtension => p.extension_checks.push(pos),
            _ => {}
        }
    }

    /// A whistle or extension left `pos`: its tank counts again, the stack's root counts its
    /// pitch again, the extension above may fall.
    fn plumbing_removed(&mut self, dim: usize, pos: [i32; 3], old: Option<(Block, StateInfo)>) {
        let d = &mut self.dims[dim];
        if d.plumbing.removed(pos) {
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
        match old {
            Some((Block::SteamWhistle, info)) => {
                if let Some(a) = whistle_attached(&info) {
                    let o = a.offset();
                    d.boiler_checks
                        .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
                }
                d.plumbing
                    .extension_checks
                    .push([pos[0], pos[1] + 1, pos[2]]);
            }
            Some((Block::SteamWhistleExtension, _)) => {
                // `onRemove`: the root below counts again; above, an extension may fall.
                d.plumbing.extension_checks.push(pos);
                d.plumbing
                    .extension_checks
                    .push([pos[0], pos[1] + 1, pos[2]]);
            }
            Some((Block::FluidTank, _)) => {
                // Whistles on a tank that went break (`updateShape`, `canSurvive`).
                for dir in crate::kinetics::Dir::ALL {
                    let o = dir.offset();
                    d.plumbing
                        .whistle_support
                        .push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
                }
            }
            _ => {}
        }
    }

    /// The steam whistle at `pos` is attached this way, if it is one.
    pub fn whistle_at(&self, dim: usize, pos: [i32; 3]) -> Option<crate::kinetics::Dir> {
        match self.table.get(*self.dims[dim].states.get(&pos)?)? {
            (Block::SteamWhistle, info) => whistle_attached(info),
            _ => None,
        }
    }

    /// Whistles whose pitch is counted again (`queuePitchUpdate`), each once.
    pub fn take_whistle_checks(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let p = &mut self.dims[dim].plumbing;
        if p.whistle_checks.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut p.whistle_checks);
        v.sort();
        v.dedup();
        v
    }

    /// The boiler of the tank the whistle at `pos` is on, if that tank is one.
    pub fn whistle_boiler(&self, dim: usize, pos: [i32; 3]) -> Option<&crate::steam::Boiler> {
        let a = self.whistle_at(dim, pos)?.offset();
        let tank = [pos[0] + a[0], pos[1] + a[1], pos[2] + a[2]];
        let d = &self.dims[dim];
        let c = d.tanks.controller(tank)?;
        d.boilers.get(&c).map(|b| &b.data)
    }

    /// Whistles that may start or stop sounding (placed, powered, their boiler's heat or
    /// engines changed), each once.
    pub fn take_sound_checks(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let p = &mut self.dims[dim].plumbing;
        if p.sound_checks.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut p.sound_checks);
        v.sort();
        v.dedup();
        v
    }

    /// The whistles of the boiler at `tank` look again at whether they sound.
    fn boiler_whistles_changed(&mut self, dim: usize, tank: [i32; 3]) {
        let d = &mut self.dims[dim];
        if let Some(b) = d.boilers.get(&tank) {
            d.plumbing.sound_checks.extend_from_slice(&b.whistles);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny registry: two vanilla states, then Create's shaft (x, y, z) and creative motor
    /// facing east and west.
    fn world() -> CreateWorld {
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:shaft", vec![("axis", "x")]),
            ("create:shaft", vec![("axis", "y")]),
            ("create:shaft", vec![("axis", "z")]),
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:creative_motor", vec![("facing", "west")]),
            ("create:andesite_casing", vec![]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        CreateWorld::new(table)
    }

    fn item_belt_world(rpm: f32) -> CreateWorld {
        let belt = |part| {
            (
                "create:belt",
                vec![("facing", "south"), ("slope", "horizontal"), ("part", part)],
            )
        };
        let names = [
            ("create:creative_motor", vec![("facing", "east")]),
            belt("start"),
            belt("middle"),
            belt("end"),
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        for (start, length) in [(0, 5), (-4, 4)] {
            for i in 0..length {
                w.block_changed(
                    0,
                    [0, 8, start + i],
                    0,
                    if i == 0 {
                        11
                    } else if i == length - 1 {
                        13
                    } else {
                        12
                    },
                );
            }
            w.block_changed(0, [-1, 8, start], 0, 10);
            w.set_motor(0, [-1, 8, start], rpm);
        }
        w.settle_belts(0);
        w.tick(0, &mut Vec::new());
        w
    }

    #[test]
    fn registrations_include_only_create_block_entities() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            ("create:depot", vec![]),
            ("create:depot", vec![("waterlogged", "true")]),
            ("create:shaft", vec![("axis", "x")]),
            ("create:andesite_casing", vec![]),
            ("create:redstone_contact", vec![]),
            ("create:steam_whistle_extension", vec![]),
            ("create:clipboard", vec![]),
            ("create:track", vec![("has_be", "false")]),
            ("create:track", vec![("has_be", "true")]),
            ("create:train_door", vec![("half", "upper")]),
            ("create:train_door", vec![("half", "lower")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names
                .iter()
                .map(|(name, props)| (*name, props.iter().copied())),
        ));
        for (i, _) in names.iter().enumerate() {
            let state = 10 + i as u32;
            let pos = [i as i32, 0, 0];
            w.block_loaded(0, pos, state);
            assert_eq!(
                w.dims[0].block_entities.registration_order(pos).is_some(),
                matches!(state, 10 | 11 | 12 | 16 | 18 | 20)
            );
        }
        let a = [0, 0, 0];
        let original = w.dims[0].block_entities.registration_order(a);
        w.block_changed(0, a, 10, 11);
        assert_eq!(w.dims[0].block_entities.registration_order(a), original);
        w.block_changed(0, a, 11, 12);
        assert!(w.dims[0].block_entities.registration_order(a) > original);
        w.block_changed(0, [6, 0, 0], 16, 13);
        assert_eq!(w.dims[0].block_entities.registration_order([6, 0, 0]), None);
        states::clear_range();
    }

    #[test]
    fn first_empty_direction_follows_init_attachment() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(32.0);
        let c = [0, 8, 0];
        // Remove only the belt nodes; the already ticking motor remains available.
        for i in 0..5 {
            w.dims[0].kinetics.remove([0, 8, i]);
        }
        w.take_belt(0, c);
        w.settle_belts(0);
        assert_eq!(w.transport_ticks(0).next(&mut w), None);
        assert!(
            w.column_belts(0, 0, 0)
                .iter()
                .find(|(p, _)| *p == c)
                .unwrap()
                .1
                .positive
        );
    }

    #[test]
    fn drain_tests_room_at_its_registration_turn() {
        // ItemDrainBlockEntity.continueProcessing keeps 20 while the tank cannot
        // accept a bucket; once a pump makes room an admitted turn advances to 19.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for pump_first in [false, true] {
            states::set_range(10, 2);
            let mut w = CreateWorld::new(StateTable::new(
                10,
                [
                    ("create:item_drain", std::iter::empty()),
                    ("create:mechanical_pump", std::iter::empty()),
                ]
                .into_iter(),
            ));
            let (drain, pump) = ([0, 4, 0], [2, 4, 0]);
            for (p, state) in if pump_first {
                [(pump, 11), (drain, 10)]
            } else {
                [(drain, 10), (pump, 11)]
            } {
                w.block_changed(0, p, 0, state);
            }
            let d = w.dims[0].drains.get_mut(&drain).unwrap();
            crate::drain::insert(
                d,
                "minecraft:water_bucket",
                1,
                crate::kinetics::Dir::North,
                &[],
            );
            d.processing = 20;
            d.fluid = Some((crate::tank::Fluid::Water, 1000));
            let mut turns = w.transport_ticks(0);
            while let Some(turn) = turns.next(&mut w) {
                let d = w.dims[0].drains.get_mut(&drain).unwrap();
                match turn {
                    TransportTick::Drain(_) => {
                        crate::drain::tick(d, &[]);
                    }
                    TransportTick::Pump(_) => d.fluid = None,
                    _ => panic!("unexpected turn"),
                }
            }
            assert_eq!(
                w.dims[0].drains[&drain].processing,
                if pump_first { 19 } else { 20 }
            );
        }
    }

    #[test]
    fn spout_timer_advances_after_an_earlier_depot_callback() {
        // Java #66: an earlier transported-item callback starts at 20, then the
        // later SpoutBlockEntity.tick decrements to 19 on that same level tick.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for spout_first in [true, false] {
            states::set_range(10, 2);
            let mut w = CreateWorld::new(StateTable::new(
                10,
                [
                    ("create:spout", std::iter::empty()),
                    ("create:depot", std::iter::empty()),
                ]
                .into_iter(),
            ));
            let (spout, depot) = ([0, 6, 0], [0, 4, 0]);
            for (p, state) in if spout_first {
                [(spout, 10), (depot, 11)]
            } else {
                [(depot, 11), (spout, 10)]
            } {
                w.block_changed(0, p, 0, state);
            }
            let mut held = crate::depot::Held::new(0, false);
            held.locked = true;
            w.depot_put(0, depot, Some(held));
            crate::spout::fill(
                w.dims[0].spouts.get_mut(&spout).unwrap(),
                crate::tank::Fluid::Water,
                1000,
            );
            let mut turns = w.transport_ticks(0);
            while let Some(turn) = turns.next(&mut w) {
                match turn {
                    TransportTick::Spout(p) => {
                        crate::spout::tick(w.dims[0].spouts.get_mut(&p).unwrap())
                    }
                    TransportTick::Depot(p) => {
                        assert!(w.tick_depot(0, p, false));
                        let d = &mut w.dims[0];
                        crate::spout::on_depot(
                            d.spouts.get_mut(&spout).unwrap(),
                            d.depots.get_mut(&p).unwrap().as_mut().unwrap(),
                            "minecraft:bucket",
                            &[],
                        );
                    }
                    _ => panic!("unexpected turn"),
                }
            }
            assert_eq!(
                w.dims[0].spouts[&spout].processing,
                if spout_first { 20 } else { 19 }
            );
        }
    }

    #[test]
    fn fluid_machines_share_transport_registration_order() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            "create:spout",
            "create:item_drain",
            "create:mechanical_pump",
            "create:depot",
            "create:fluid_tank",
            "create:fluid_valve",
        ];
        states::set_range(10, 6);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|n| (*n, std::iter::empty())),
        ));
        for n in 0..6 {
            w.block_changed(0, [n, 8, 0], 0, 10 + n as u32);
        }
        let mut turns = w.transport_ticks(0);
        assert_eq!(turns.next(&mut w), Some(TransportTick::Spout([0, 8, 0])));
        assert_eq!(turns.next(&mut w), Some(TransportTick::Drain([1, 8, 0])));
        assert_eq!(turns.next(&mut w), Some(TransportTick::Pump([2, 8, 0])));
        assert_eq!(turns.next(&mut w), Some(TransportTick::Depot([3, 8, 0])));
        assert_eq!(turns.next(&mut w), Some(TransportTick::Fluid([4, 8, 0])));
        assert_eq!(turns.next(&mut w), Some(TransportTick::Fluid([5, 8, 0])));
        assert_eq!(turns.next(&mut w), None);
    }

    #[test]
    fn depot_slides_at_its_registration_turn() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for depot_first in [false, true] {
            let names = [
                (
                    "create:belt",
                    vec![
                        ("facing", "south"),
                        ("slope", "horizontal"),
                        ("part", "start"),
                    ],
                ),
                (
                    "create:belt",
                    vec![
                        ("facing", "south"),
                        ("slope", "horizontal"),
                        ("part", "end"),
                    ],
                ),
                ("create:depot", vec![]),
            ];
            states::set_range(10, 3);
            let mut w = CreateWorld::new(StateTable::new(
                10,
                names.iter().map(|(n, p)| (*n, p.iter().copied())),
            ));
            let p = [0, 8, 2];
            if depot_first {
                w.block_loaded(0, p, 12);
            }
            w.block_loaded(0, [0, 8, 0], 10);
            w.block_loaded(0, [0, 8, 1], 11);
            if !depot_first {
                w.block_loaded(0, p, 12);
            }
            w.settle_belts(0);
            // A queued belt item makes the controller require a full tick.
            w.dims[0]
                .belts
                .get_mut(&[0, 8, 0])
                .unwrap()
                .items
                .add(crate::belt::Item::new("minecraft:iron_ingot", 16, 0, false));
            let mut turns = w.transport_ticks(0);
            while let Some(turn) = turns.next(&mut w) {
                match turn {
                    TransportTick::Belt(_) => {
                        let mut h = crate::depot::Held::new(0, false);
                        h.position = 0.0;
                        w.depot_put(0, p, Some(h));
                    }
                    TransportTick::Depot(p) => {
                        w.tick_depot(0, p, false);
                    }
                    _ => panic!("unexpected machine"),
                }
            }
            assert_eq!(
                w.saved_depot(0, p).unwrap().held.unwrap().position,
                if depot_first { 0.0 } else { 0.125 }
            );
        }
    }

    struct ItemBeltSide;
    impl crate::belt::Surroundings for ItemBeltSide {
        fn ending(&mut self) -> crate::belt::Ending {
            crate::belt::Ending::Blocked
        }
        fn insert(&mut self, _: &crate::belt::Item) -> Option<u8> {
            None
        }
        fn process(
            &mut self,
            _: i32,
            _: &mut crate::belt::Item,
            _: bool,
        ) -> Option<crate::belt::Process> {
            None
        }
    }

    #[test]
    fn empty_belt_inventory_direction_is_saved() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(32.0);
        assert_eq!(w.transport_ticks(0).next(&mut w), None);
        let saved = w.column_belts(0, 0, 0);
        assert_eq!(saved.len(), 1);
        assert!(saved[0].1.positive);
        assert!(saved[0].1.items.is_empty());
    }

    #[test]
    fn empty_belt_direction_is_journaled_once() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(32.0);
        w.take_belt_saved_changes(0);
        assert_eq!(w.transport_ticks(0).next(&mut w), None);
        assert_eq!(w.take_belt_saved_changes(0), [[0, 8, 0], [0, 8, -4]].into());
        assert_eq!(w.transport_ticks(0).next(&mut w), None);
        assert!(w.take_belt_saved_changes(0).is_empty());
    }

    #[test]
    fn moving_belt_journals_current_saved_positions() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(-32.0);
        let c = [0, 8, 0];
        w.take_belt_saved_changes(0);
        let mut item = crate::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        item.position = 1.4958333;
        w.dims[0].belts.get_mut(&c).unwrap().items =
            crate::belt::Inventory::from_saved(vec![item], false);
        // Java c36 ticks 42-44. No insertion or client notification is needed.
        for expected in [1.4291666, 1.3624998, 1.2958331] {
            let out = w.dims[0].belts.get_mut(&c).unwrap().items.tick(
                5,
                -32.0 / 480.0,
                true,
                &mut ItemBeltSide,
            );
            assert!(!out.changed);
            if out.saved_changed {
                w.belt_inventory_changed(0, c);
            }
            assert_eq!(w.take_belt_saved_changes(0), [c].into());
            assert!(w.take_belt_saved_changes(0).is_empty());
            let saved = w.column_belts(0, 0, 0);
            assert!((saved[0].1.items[0].position - expected).abs() < 0.000001);
        }
        let out = w.dims[0]
            .belts
            .get_mut(&c)
            .unwrap()
            .items
            .tick(5, 0.0, true, &mut ItemBeltSide);
        assert!(!out.saved_changed);
    }

    #[test]
    fn loaded_belt_moves_on_its_first_inventory_tick() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(-32.0);
        let c = [0, 8, 0];
        let mut item = crate::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        // Java belt.min.json ticks 41 -> 42: 1.4958333 -> 1.4291666 after reload.
        item.position = 1.4958333;
        item.inserted_at = 1;
        item.inserted_from = Some(crate::kinetics::Dir::Up);
        w.dims[0].belts.get_mut(&c).unwrap().items.add(item);
        let members: Vec<_> = (0..5)
            .map(|z| {
                let p = [0, 8, z];
                (
                    p,
                    w.saved_belt_segment(0, p).unwrap(),
                    w.dims[0].kinetics.saved_state(p, 0).unwrap(),
                )
            })
            .collect();
        let inventory = w.column_belts(0, 0, 0);
        w.column_unloaded(0, 0, 0);
        w.load_belt_items(0, inventory.into_iter());
        for (p, saved, kinetic) in members {
            let state = if saved.index == 0 {
                11
            } else if saved.index == 4 {
                13
            } else {
                12
            };
            w.block_loaded(0, p, state);
            w.restore_belt_segment(0, p, saved);
            assert!(w.dims[0].kinetics.restore_state(p, 0, kinetic));
        }
        assert_eq!(
            w.transport_ticks(0).next(&mut w),
            Some(TransportTick::Belt(c))
        );
        let rpm = w.speed(0, c).unwrap();
        assert_eq!(rpm, -32.0);
        let b = w.dims[0].belts.get_mut(&c).unwrap();
        let speed = crate::belt::direction_aware_speed(rpm, b.segment.facing);
        b.items.tick(5, speed, true, &mut ItemBeltSide);
        assert!((b.items.items[0].position - 1.4291666).abs() < 0.000001);
        assert_eq!(b.items.items[0].inserted_at, 1);
        assert_eq!(
            b.items.items[0].inserted_from,
            Some(crate::kinetics::Dir::Up)
        );
    }

    #[test]
    fn empty_belt_tick_sets_first_stack_order() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(32.0);
        assert_eq!(w.transport_ticks(0).next(&mut w), None);
        let b = w.dims[0].belts.get_mut(&[0, 8, 0]).unwrap();
        let mut item = crate::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        item.position = 1.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
        b.items.add(item);
        let mut phase = w.transport_ticks(0);
        assert_eq!(phase.next(&mut w), Some(TransportTick::Belt([0, 8, 0])));
        let b = w.dims[0].belts.get_mut(&[0, 8, 0]).unwrap();
        b.items.tick(
            5,
            crate::belt::direction_aware_speed(32.0, b.segment.facing),
            true,
            &mut ItemBeltSide,
        );
        assert!((b.items.items[0].position - 1.5041667).abs() < 1e-6);
        states::clear_range();
    }

    #[test]
    fn empty_belt_receives_before_its_turn_in_the_same_tick() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(-32.0);
        let sender = [0, 8, 0];
        let receiver = [0, 8, -4];
        let mut item = crate::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        item.position = 0.01;
        w.dims[0]
            .belts
            .get_mut(&sender)
            .unwrap()
            .items
            .add(item.clone());
        let mut phase = w.transport_ticks(0);
        assert_eq!(phase.next(&mut w), Some(TransportTick::Belt(sender)));
        item.position = 4.26; // DirectBeltInputBehaviour's start at the receiving end.
        w.dims[0].belts.get_mut(&receiver).unwrap().items.add(item);
        assert_eq!(phase.next(&mut w), Some(TransportTick::Belt(receiver)));
        let b = w.dims[0].belts.get_mut(&receiver).unwrap();
        b.items.tick(
            4,
            crate::belt::direction_aware_speed(-32.0, b.segment.facing),
            true,
            &mut ItemBeltSide,
        );
        assert!((b.items.items[0].position - 4.1933336).abs() < 1e-6);
        b.items.tick(
            4,
            crate::belt::direction_aware_speed(-32.0, b.segment.facing),
            true,
            &mut ItemBeltSide,
        );
        assert!((b.items.items[0].position - 4.126667).abs() < 1e-6);
        states::clear_range();
    }

    #[test]
    fn belt_receiving_after_its_turn_waits_until_next_tick() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = item_belt_world(-32.0);
        let receiver = [0, 8, -4];
        let mut phase = w.transport_ticks(0);
        // Both empty controllers already took their turns.
        assert_eq!(phase.next(&mut w), None);
        let mut item = crate::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        item.position = 4.26;
        w.dims[0].belts.get_mut(&receiver).unwrap().items.add(item);
        assert_eq!(phase.next(&mut w), None);
        assert!(w.dims[0].belts[&receiver].items.items.is_empty());
        let mut phase = w.transport_ticks(0);
        assert_eq!(phase.next(&mut w), Some(TransportTick::Belt(receiver)));
        let b = w.dims[0].belts.get_mut(&receiver).unwrap();
        b.items.tick(4, -32.0 / 480.0, true, &mut ItemBeltSide);
        assert!((b.items.items[0].position - 4.1933336).abs() < 1e-6);
        states::clear_range();
    }

    #[test]
    fn pulley_switch_keeps_driven_network_capacity() {
        // BeltBlock.useItemOn -> KineticBlockEntity.switchToBlockState.
        // Issue #24 Java: middle -> pulley keeps 32 rpm and 524288 capacity.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let belt = |part| {
            (
                "create:belt",
                vec![("facing", "south"), ("slope", "horizontal"), ("part", part)],
            )
        };
        let names = [
            ("create:creative_motor", vec![("facing", "east")]),
            belt("start"),
            belt("middle"),
            belt("end"),
            belt("pulley"),
        ];
        states::set_range(10, 5);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let mut events = Vec::new();
        let middle = [0, 8, 1];
        for (p, id) in [
            ([-1, 8, 0], 10),
            ([0, 8, 0], 11),
            (middle, 12),
            ([0, 8, 2], 13),
        ] {
            w.block_changed(0, p, 0, id);
        }
        w.settle_belts(0);
        w.tick(0, &mut events);
        w.set_motor(0, [-1, 8, 0], 32.0);
        for _ in 0..40 {
            w.tick(0, &mut events);
        }
        w.switching_block_state(0, middle, 14);
        w.block_changed(0, middle, 12, 14);
        for _ in 0..100 {
            w.tick(0, &mut events);
            assert_eq!(w.speed(0, middle), Some(32.0));
            assert_eq!(w.dims[0].kinetics.network(middle, 0).unwrap().2, 524288.0);
        }
        states::clear_range();
    }

    #[test]
    fn motor_same_block_state_lifecycle() {
        // Java issue #13: north 32 -> south 32 keeps -32; changing axes clears
        // kinetic information without reinitializing the surviving motor entity.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = ["north", "south", "west"];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names
                .iter()
                .map(|f| ("create:creative_motor", [("facing", *f)].into_iter())),
        ));
        let p = [0, 8, 1];
        let mut events = Vec::new();
        w.block_changed(0, p, 0, 10);
        w.tick(0, &mut events);
        w.set_motor(0, p, 32.0);
        w.tick(0, &mut events);
        assert_eq!(w.speed(0, p), Some(-32.0));
        w.block_changed(0, p, 10, 11);
        w.set_motor(0, p, 32.0);
        w.tick(0, &mut events);
        assert_eq!(w.speed(0, p), Some(-32.0));
        w.block_changed(0, p, 11, 12);
        w.tick(0, &mut events);
        assert_eq!(w.speed(0, p), Some(0.0));
        assert_eq!(w.dims[0].kinetics.network(p, 0).unwrap().2, 0.0);
        states::clear_range();
    }

    #[test]
    fn wrenched_shaft_leaves_network_immediately() {
        // Issue #14's Java checkpoints: size 3 at 73, size 2 at 75/77/79.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            ("create:creative_motor", vec![("facing", "east")]),
            (
                "create:gearshift",
                vec![("axis", "x"), ("powered", "false")],
            ),
            ("create:shaft", vec![("axis", "x")]),
            ("create:shaft", vec![("axis", "z")]),
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let motor = [0, 8, 0];
        let shaft = [2, 8, 0];
        let mut events = Vec::new();
        for tick in 0..160 {
            match tick {
                0 => w.block_changed(0, motor, 0, 10),
                1 => {
                    w.set_motor(0, motor, 32.0);
                }
                3 => w.block_changed(0, [1, 8, 0], 0, 11),
                4 => w.block_changed(0, shaft, 0, 12),
                74 => {
                    w.switching_block_state(0, shaft, 13);
                    assert_eq!(
                        w.dims[0].kinetics.network_membership(motor, 0).unwrap().1,
                        2,
                        "immediately after switch"
                    );
                    w.block_changed(0, shaft, 12, 13);
                }
                _ => {}
            }
            w.tick(0, &mut events);
            if tick >= 4 {
                let expected = if tick < 74 { 3 } else { 2 };
                assert_eq!(
                    w.dims[0].kinetics.network_membership(motor, 0).unwrap().1,
                    expected,
                    "after tick {tick}"
                );
            }
        }
        states::clear_range();
    }

    #[test]
    fn wrench_reactivates_surviving_motor() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            ["north", "south"]
                .into_iter()
                .map(|f| ("create:creative_motor", [("facing", f)].into_iter())),
        ));
        let p = [0, 8, 1];
        let mut events = Vec::new();
        w.block_changed(0, p, 0, 10);
        w.tick(0, &mut events);
        w.set_motor(0, p, 32.0);
        w.tick(0, &mut events);
        w.switching_block_state(0, p, 11);
        w.block_changed(0, p, 10, 11);
        assert_eq!(w.speed(0, p), Some(-32.0));
        w.tick(0, &mut events);
        assert_eq!(w.speed(0, p), Some(32.0));
        assert_eq!(w.motor_setting(0, p), Some(32.0));
        states::clear_range();
    }

    #[test]
    fn a_drain_replaced_by_another_block_of_create_s_is_gone() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:item_drain", vec![]),
            ("create:andesite_funnel", vec![("facing", "up")]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        let mut w = CreateWorld::new(table);
        let (drain, funnel) = (2, 3);
        let pos = [0, 64, 0];
        w.block_changed(0, pos, 0, drain);
        assert!(w.dims[0].drains.contains_key(&pos));
        w.block_changed(0, pos, drain, funnel);
        assert!(!w.dims[0].drains.contains_key(&pos));
        states::clear_range();
    }

    #[test]
    fn gauges_change_only_when_their_network_does() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:creative_motor", vec![("facing", "east")]),
            (
                "create:stressometer",
                vec![("facing", "up"), ("axis_along_first", "true")],
            ),
            (
                "create:speedometer",
                vec![("facing", "up"), ("axis_along_first", "true")],
            ),
            ("create:mechanical_press", vec![("facing", "east")]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        let mut w = CreateWorld::new(table);
        let (motor, stress, speed, press) = (2, 3, 4, 5);
        w.block_changed(0, [0, 64, 0], 0, motor);
        w.block_changed(0, [1, 64, 0], 0, stress);
        w.block_changed(0, [2, 64, 0], 0, speed);
        w.tick(0, &mut Vec::new());
        let mut changed = w.take_gauge_changes(0);
        changed.sort();
        assert_eq!(changed, [[1, 64, 0], [2, 64, 0]]);
        assert!(crate::gauge::speed_dial(16.0) == w.dims[0].gauges[&[2, 64, 0]].shown.dial);
        assert_eq!(w.dims[0].gauges[&[1, 64, 0]].shown.dial, 0.0);
        // A press joins: the stressometer reads its stress, the speedometer is unchanged.
        w.block_changed(0, [3, 64, 0], 0, press);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.take_gauge_changes(0), [[1, 64, 0]]);
        assert!(w.dims[0].gauges[&[1, 64, 0]].shown.dial > 0.0);
        // Steady: nothing.
        w.tick(0, &mut Vec::new());
        assert!(w.take_gauge_changes(0).is_empty());
        w.block_changed(0, [1, 64, 0], stress, 0);
        assert!(!w.dims[0].gauges.contains_key(&[1, 64, 0]));
    }

    #[test]
    fn gauges_reset_comparators_when_their_source_stops() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            ("create:creative_motor", vec![("facing", "east")]),
            (
                "create:speedometer",
                vec![("facing", "up"), ("axis_along_first", "true")],
            ),
            (
                "create:stressometer",
                vec![("facing", "up"), ("axis_along_first", "true")],
            ),
            ("create:mechanical_press", vec![("facing", "east")]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        let mut w = CreateWorld::new(table);
        for (x, state) in [2, 3, 4, 5].into_iter().enumerate() {
            w.block_changed(0, [x as i32, 64, 0], 0, state);
            w.tick(0, &mut Vec::new());
        }
        // Java: 16 rpm gives ceil(.24 * 14) = 4; press stress / motor capacity
        // is 128 / 262144, giving comparator 1.
        for (x, expected) in [(1, 4), (2, 1)] {
            assert_eq!(
                crate::gauge::analog_output(w.dims[0].gauges[&[x, 64, 0]].shown.dial),
                expected
            );
        }
        w.take_gauge_changes(0);
        w.block_changed(0, [0, 64, 0], 2, 0);
        w.tick(0, &mut Vec::new());
        for x in [1, 2] {
            assert_eq!(
                crate::gauge::analog_output(w.dims[0].gauges[&[x, 64, 0]].shown.dial),
                0
            );
        }
        let mut changed = w.take_gauge_changes(0);
        changed.sort();
        assert_eq!(changed, [[1, 64, 0], [2, 64, 0]]);
    }

    #[test]
    fn worlds_solve_on_change_and_pay_nothing_when_steady() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let (shaft_x, motor_east, casing) = (2, 5, 7);
        assert!(!w.table.is_kinetic(0) && !w.table.is_kinetic(casing));
        // Vanilla changes miss at once.
        w.block_changed(0, [5, 5, 5], 0, 1);
        assert_eq!(w.tick(0, &mut Vec::new()).0, SolveStats::default());
        w.block_changed(0, [0, 64, 0], 0, motor_east);
        w.block_changed(0, [1, 64, 0], 0, shaft_x);
        let mut e = Vec::new();
        assert_eq!(w.tick(0, &mut e).0.components, 1);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(16.0));
        assert_eq!(
            w.tick(0, &mut e).0,
            SolveStats::default(),
            "steady: no work"
        );
        // The motor's speed setting re-solves it.
        w.set_settings(
            0,
            [0, 64, 0],
            motor_east,
            Settings {
                motor_rpm: 64.0,
                ..Settings::default()
            },
        );
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(64.0));
        // Breaking the motor stops the shaft.
        w.block_changed(0, [0, 64, 0], motor_east, 0);
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(0.0));
        states::clear_range();
    }

    #[test]
    fn scrolling_a_motor_skips_zero_and_saves_with_its_column() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let motor_east = 5;
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        assert_eq!(
            w.scroll_motor(0, [1, 64, 1], -16),
            Some(-1.0),
            "16 down 16 skips 0"
        );
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1), Some(1.0));
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1000), Some(256.0));
        assert_eq!(w.scroll_motor(0, [2, 64, 1], 1), None, "no motor there");
        // The value board: row 0 turns it counter to its axis, column 0 is speed 1.
        use crate::board::Settings as Pick;
        let pick = |row, value| Pick { row, value };
        assert_eq!(w.board_motor(0, [1, 64, 1], pick(0, 0)), Some(-1.0));
        assert_eq!(w.motor_setting(0, [1, 64, 1]), Some(-1.0));
        assert_eq!(w.board_motor(0, [1, 64, 1], pick(1, 256)), Some(256.0));
        assert_eq!(
            w.board_motor(0, [2, 64, 1], pick(1, 1)),
            None,
            "no motor there"
        );
        assert_eq!(w.take_unsaved(0), [(0, 0)]);
        assert_eq!(w.column_settings(0, 0, 0), [([1, 64, 1], 256)]);
        // A reload brings the setting back before the motor is found.
        w.column_unloaded(0, 0, 0);
        assert!(w.column_settings(0, 0, 0).is_empty());
        w.load_settings(0, [([1, 64, 1], 256)].into_iter());
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        w.block_changed(0, [2, 64, 1], 0, 2);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [2, 64, 1]), Some(256.0));
        states::clear_range();
    }

    #[test]
    fn a_removed_deployer_clears_its_cracks() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![(
            "create:deployer",
            vec![("axis_along_first", "false"), ("facing", "down")],
        )];
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let (pos, at) = ([0, 64, 0], [0, 62, 0]);
        w.block_changed(0, pos, 0, 10);
        let mut d = w.dims[0].deployers[&pos].clone();
        d.breaking = Some((at, 0.5));
        w.set_deployer(0, pos, d);
        w.block_changed(0, pos, 10, 0);
        assert_eq!(w.dims[0].deployer_cracks, [(pos, at)]);
        // One that broke nothing leaves none.
        w.dims[0].deployer_cracks.clear();
        w.block_changed(0, pos, 0, 10);
        w.block_changed(0, pos, 10, 0);
        assert!(w.dims[0].deployer_cracks.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_motor_s_new_speed_keeps_its_place_on_the_network() {
        // A motor, a gearbox, and a second motor placed facing the first: through the gearbox
        // it turns its own way, so it joins as a member (create-fuzz seed 1294). Scrolled to 156,
        // faster, it takes the network over (`applyNewSpeed`), which needs it to stay the block
        // entity it was, a member, not a block just placed: then the first turns at 156 too.
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:gearbox", vec![("axis", "y")]),
            ("create:creative_motor", vec![("facing", "west")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let mut e = Vec::new();
        for (x, state) in [(0, 10), (1, 11), (2, 12)] {
            w.block_changed(0, [x, 0, 0], 0, state);
            w.tick(0, &mut e);
        }
        assert_eq!(w.speed(0, [0, 0, 0]), Some(16.0));
        let fast = Settings {
            motor_rpm: 156.0,
            ..Settings::default()
        };
        w.set_settings(0, [2, 0, 0], 12, fast);
        e.clear();
        w.tick(0, &mut e);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        assert_eq!(w.speed(0, [0, 0, 0]), Some(156.0));
        states::clear_range();
    }

    #[test]
    fn split_shafts_are_checked_for_power_when_placed_or_their_neighbour_changes() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:clutch", vec![("axis", "x"), ("powered", "false")]),
            ("create:clutch", vec![("axis", "x"), ("powered", "true")]),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(w.take_redstone(0), [[0, 0, 0]], "placed: checked once");
        w.block_changed(0, [0, 0, 0], 10, 11);
        assert!(
            w.take_redstone(0).is_empty(),
            "its own power change is no new check"
        );
        w.neighbor_changed(0, [0, 0, 0], 11);
        w.neighbor_changed(0, [1, 0, 0], 12);
        w.neighbor_changed(0, [2, 0, 0], 3);
        assert_eq!(
            w.take_redstone(0),
            [[0, 0, 0]],
            "shafts and vanilla blocks are not"
        );
        assert_eq!(w.powered(11), Some(true));
        assert_eq!(w.powered(12), None);
        states::clear_range();
    }

    #[test]
    fn train_signals_follow_redstone_and_their_type() {
        use crate::network::SignalType;
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:track_signal",
                vec![("powered", "false"), ("type", "entry_signal")],
            ),
            (
                "create:track_signal",
                vec![("powered", "true"), ("type", "entry_signal")],
            ),
            (
                "create:track_signal",
                vec![("powered", "false"), ("type", "cross_signal")],
            ),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(w.take_redstone(0), [[0, 0, 0]], "placed: its power is read");
        let s = w.dims[0].railway.signals[&[0, 0, 0]];
        assert_eq!((s.kind, s.powered), (SignalType::Entry, false));
        w.block_changed(0, [0, 0, 0], 10, 11);
        assert!(w.take_redstone(0).is_empty());
        assert!(w.dims[0].railway.signals[&[0, 0, 0]].powered);
        w.neighbor_changed(0, [0, 0, 0], 11);
        assert_eq!(w.take_redstone(0), [[0, 0, 0]], "a neighbour changed");
        assert_eq!(w.powered(11), Some(true));
        w.block_changed(0, [0, 0, 0], 11, 12);
        assert_eq!(
            w.dims[0].railway.signals[&[0, 0, 0]].kind,
            SignalType::Cross
        );
        w.block_changed(0, [0, 0, 0], 12, 0);
        assert!(w.dims[0].railway.signals.is_empty());
        states::clear_range();
    }

    #[test]
    fn depot_motion_survives_cold_load() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [("create:depot", std::iter::empty())].into_iter(),
        ));
        let pos = [0, 8, 5];
        w.block_changed(0, pos, 0, 10);
        w.depot_put(
            0,
            pos,
            Some(crate::depot::Held::put_by_player(
                90,
                false,
                crate::depot::Side::West,
            )),
        );
        for _ in 0..9 {
            let mut turns = w.transport_ticks(0);
            while let Some(TransportTick::Depot(p)) = turns.next(&mut w) {
                w.tick_depot(0, p, false);
            }
        }
        let saved = w.saved_depot(0, pos).unwrap();
        w.column_unloaded(0, 0, 0);
        w.block_loaded(0, pos, 10);
        w.restore_depot(0, pos, saved);
        assert_eq!(w.saved_depot(0, pos), Some(saved));
        for expected in [0.48592162, 0.48944122, 0.49208093] {
            let mut turns = w.transport_ticks(0);
            while let Some(TransportTick::Depot(p)) = turns.next(&mut w) {
                w.tick_depot(0, p, false);
            }
            assert!(
                (w.saved_depot(0, pos).unwrap().held.unwrap().position - expected).abs()
                    < 0.0000001
            );
        }
        w.restore_depot(0, pos, crate::depot::SavedDepot { held: None });
        assert_eq!(w.saved_depot(0, pos).unwrap().held, None);
        states::clear_range();
    }

    #[test]
    fn a_depot_ticks_only_while_its_stack_slides() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![("create:depot", vec![])];
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(
            w.take_depot_changes(0),
            [[0, 0, 0]],
            "placed: clients see it"
        );
        assert!(w.dims[0].sliding.is_empty(), "an empty depot does nothing");
        let held = crate::depot::Held::put_by_player(0, false, crate::depot::Side::East);
        w.depot_put(0, [0, 0, 0], Some(held));
        assert_eq!(w.take_depot_changes(0), [[0, 0, 0]]);
        let mut ticks = 0;
        while !w.dims[0].sliding.is_empty() {
            let mut turns = w.transport_ticks(0);
            while let Some(TransportTick::Depot(p)) = turns.next(&mut w) {
                w.tick_depot(0, p, false);
            }
            ticks += 1;
        }
        assert!((5..30).contains(&ticks), "{ticks}");
        let at = w.dims[0].depots[&[0, 0, 0]].unwrap();
        assert!(0.5 - at.position <= 1.0 / 512.0);
        assert!(
            w.take_depot_changes(0).is_empty(),
            "arriving is no change to send"
        );
        w.column_unloaded(0, 0, 0);
        assert!(w.dims[0].depots.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_belt_turns_as_one_and_breaks_as_create_s_does() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let belt = |part: &'static str| {
            (
                "create:belt",
                vec![
                    ("facing", "east"),
                    ("slope", "horizontal"),
                    ("part", part),
                    ("casing", "false"),
                    ("waterlogged", "false"),
                ],
            )
        };
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            belt("start"),
            belt("middle"),
            belt("end"),
            ("create:creative_motor", vec![("facing", "south")]),
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // A belt east along x from 0 to 2; its pulleys turn on z. A motor north of the start
        // faces south into it.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.block_changed(0, [2, 0, 0], 0, 12);
        w.block_changed(0, [0, 0, -1], 0, 13);
        w.settle_belts(0);
        assert_eq!(w.dims[0].belts[&[0, 0, 0]].chain.len(), 3);
        assert_eq!(w.dims[0].belt_of[&[2, 0, 0]], [0, 0, 0]);
        w.tick(0, &mut Vec::new());
        // The motor turns the start's pulley; the segments carry it to the end.
        let start = w.speed(0, [0, 0, 0]).unwrap();
        assert!(start != 0.0);
        assert_eq!(w.speed(0, [1, 0, 0]), Some(start));
        assert_eq!(w.speed(0, [2, 0, 0]), Some(start));
        // Breaking the middle: the start and end become shafts (they are pulleys).
        w.block_changed(0, [1, 0, 0], 11, 0);
        assert!(w.dims[0].belts.is_empty());
        let breaks = std::mem::take(&mut w.dims[0].belt_breaks);
        assert_eq!(breaks.len(), 2);
        assert!(
            breaks
                .iter()
                .all(|b| b.shaft == Some(crate::kinetics::Axis::Z))
        );
        states::clear_range();
    }

    #[test]
    fn belt_break_cascade_reaches_far_pulley_before_returning() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = ["start", "middle", "end"].map(|part| {
            (
                "create:belt",
                vec![
                    ("facing", "south"),
                    ("slope", "horizontal"),
                    ("part", part),
                    ("casing", "false"),
                    ("waterlogged", "false"),
                ],
            )
        });
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names
                .iter()
                .map(|(n, p)| (*n, p.iter().copied()))
                .chain(std::iter::once((
                    "create:shaft",
                    [("axis", "x")].iter().copied(),
                ))),
        ));
        for z in 0..3 {
            w.block_changed(0, [0, 8, z], 0, 10 + z as u32);
        }
        w.settle_belts(0);
        w.block_changed(0, [0, 8, 0], 10, 0);
        struct Ground<'a>(&'a mut CreateWorld);
        impl crate::belt::BreakWorld for Ground<'_> {
            fn take_breaks(&mut self) -> Vec<BeltBreak> {
                std::mem::take(&mut self.0.dims[0].belt_breaks)
            }
            fn replace(&mut self, b: BeltBreak) {
                let old = self.0.dims[0].states[&b.pos];
                assert_eq!(b.shaft, (b.pos[2] == 2).then_some(crate::kinetics::Axis::X));
                self.0
                    .block_changed(0, b.pos, old, if b.shaft.is_some() { 13 } else { 0 });
            }
        }
        crate::belt::break_cascade(&mut Ground(&mut w));
        // Java cascade.min.json: tick 101 has air at z=1, shaft(axis=x) at z=2.
        assert!(!w.dims[0].states.contains_key(&[0, 8, 1]));
        assert_eq!(w.dims[0].states.get(&[0, 8, 2]), Some(&13));
        assert!(w.dims[0].belt_breaks.is_empty());
        states::clear_range();
    }

    #[test]
    fn belt_break_cascade_continues_through_an_intermediate_pulley() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = ["start", "pulley", "middle", "end"].map(|part| {
            (
                "create:belt",
                vec![
                    ("facing", "south"),
                    ("slope", "horizontal"),
                    ("part", part),
                    ("casing", "false"),
                    ("waterlogged", "false"),
                ],
            )
        });
        states::set_range(10, 6);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names
                .iter()
                .map(|(n, p)| (*n, p.iter().copied()))
                .chain(std::iter::once((
                    "create:shaft",
                    [("axis", "x")].iter().copied(),
                )))
                .chain(std::iter::once((
                    "create:creative_motor",
                    [("facing", "east")].iter().copied(),
                ))),
        ));
        for z in 0..4 {
            w.block_changed(0, [0, 8, z], 0, if z == 1 { 12 } else { 10 + z as u32 });
        }
        w.block_changed(0, [-1, 8, 0], 0, 15);
        w.settle_belts(0);
        w.tick(0, &mut Vec::new());
        // Putting a shaft into the middle changes its part, without removing the belt.
        w.switching_block_state(0, [0, 8, 1], 11);
        w.block_changed(0, [0, 8, 1], 12, 11);
        assert!(w.dims[0].belt_breaks.is_empty());
        w.block_changed(0, [0, 8, 0], 10, 0);
        struct Ground<'a>(&'a mut CreateWorld);
        impl crate::belt::BreakWorld for Ground<'_> {
            fn take_breaks(&mut self) -> Vec<BeltBreak> {
                std::mem::take(&mut self.0.dims[0].belt_breaks)
            }
            fn replace(&mut self, b: BeltBreak) {
                let old = self.0.dims[0].states[&b.pos];
                assert_eq!(
                    b.shaft,
                    matches!(b.pos[2], 1 | 3).then_some(crate::kinetics::Axis::X)
                );
                self.0
                    .block_changed(0, b.pos, old, if b.shaft.is_some() { 14 } else { 0 });
            }
        }
        crate::belt::break_cascade(&mut Ground(&mut w));
        // Java c23pulley.json, tick 101: air, shaft(axis=x), air, shaft(axis=x).
        assert!(!w.dims[0].states.contains_key(&[0, 8, 0]));
        assert_eq!(w.dims[0].states.get(&[0, 8, 1]), Some(&14));
        assert!(!w.dims[0].states.contains_key(&[0, 8, 2]));
        assert_eq!(w.dims[0].states.get(&[0, 8, 3]), Some(&14));
        assert!(w.dims[0].belt_breaks.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_sloped_belt_turns_as_one_across_its_diagonal_segments() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let belt = |part: &'static str| {
            (
                "create:belt",
                vec![
                    ("facing", "east"),
                    ("slope", "upward"),
                    ("part", part),
                    ("casing", "false"),
                    ("waterlogged", "false"),
                ],
            )
        };
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            belt("start"),
            belt("middle"),
            belt("end"),
            ("create:creative_motor", vec![("facing", "south")]),
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // Up east: each segment one over and one up from the last.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 1, 0], 0, 11);
        w.block_changed(0, [2, 2, 0], 0, 12);
        w.block_changed(0, [0, 0, -1], 0, 13);
        w.settle_belts(0);
        assert_eq!(w.dims[0].belts[&[0, 0, 0]].chain.len(), 3);
        w.tick(0, &mut Vec::new());
        let start = w.speed(0, [0, 0, 0]).unwrap();
        assert!(start != 0.0);
        assert_eq!(w.speed(0, [1, 1, 0]), Some(start));
        assert_eq!(w.speed(0, [2, 2, 0]), Some(start));
        states::clear_range();
    }

    #[test]
    fn ghost_only_network_keeps_cached_size_after_expiry() {
        loaded_handle_cache(false);
    }

    #[test]
    fn loaded_valve_first_tick_expiry_clears_capacity() {
        loaded_handle_cache(true);
    }

    fn loaded_handle_cache(valve: bool) {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 1);
        let name = if valve {
            "create:copper_valve_handle"
        } else {
            "create:hand_crank"
        };
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [(name, [("facing", "up")].into_iter())].into_iter(),
        ));
        let pos = [0, 8, 0];
        w.block_changed(0, pos, 0, 10);
        w.tick(0, &mut Vec::new());
        if valve {
            assert!(w.use_valve(0, pos, false, 1));
        } else {
            assert!(w.turn_crank(0, pos, false));
        }
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        while valve && w.saved_handle(0, pos).unwrap().in_use > 1 {
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
        }
        let handle = w.saved_handle(0, pos).unwrap();
        let kinetic = w.dims[0].kinetics.saved_state(pos, 0).unwrap();
        w.column_unloaded(0, 0, 0);
        w.block_loaded(0, pos, 10);
        w.restore_handle(0, pos, handle);
        assert!(w.dims[0].kinetics.restore_state(pos, 0, kinetic));
        for _ in 0..handle.in_use {
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
        }
        // Create issue #45: super.tick enrolls before expiry, so remove resets the cache.
        assert_eq!(w.dims[0].kinetics.network(pos, 0).unwrap().2, 0.0);
        if !valve {
            assert!(w.turn_crank(0, pos, true));
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
            // Create issue #43: no ghost tick recalculates the retained 256 capacity.
            let net = w.dims[0]
                .kinetics
                .saved_state(pos, 0)
                .unwrap()
                .network
                .unwrap();
            assert_eq!((net.capacity, net.size), (256.0, 2));
            assert_eq!(w.dims[0].kinetics.network_membership(pos, 0).unwrap().1, 1);
        }
        states::clear_range();
    }

    #[test]
    fn crank_use_survives_cold_load() {
        cold_handle_load(false);
    }

    #[test]
    fn valve_use_survives_cold_load() {
        cold_handle_load(true);
    }

    fn cold_handle_load(valve: bool) {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let table = || {
            StateTable::new(
                10,
                [
                    ("create:hand_crank", [("facing", "up")].into_iter()),
                    ("create:copper_valve_handle", [("facing", "up")].into_iter()),
                ]
                .into_iter(),
            )
        };
        let id = if valve { 11 } else { 10 };
        {
            for back in [false, true] {
                let pos = [0, 8, 0];
                let mut w = CreateWorld::new(table());
                w.block_loaded(0, pos, id);
                if valve {
                    assert!(w.use_valve(0, pos, back, -45));
                } else {
                    assert!(w.turn_crank(0, pos, back));
                }
                w.tick_cranks(0);
                w.tick(0, &mut Vec::new());
                let saved = w.saved_handle(0, pos).unwrap();
                assert_eq!(saved.in_use, if valve { 6 } else { 9 });
                let kinetic = w.dims[0].kinetics.saved_state(pos, 0).unwrap();
                let rpm = if back ^ valve { -32.0 } else { 32.0 };
                assert_eq!(w.dims[0].kinetics.generated(pos, 0), Some(rpm));
                w.column_unloaded(0, 0, 0);
                w.block_loaded(0, pos, id);
                w.restore_handle(0, pos, saved);
                w.dims[0].kinetics.restore_state(pos, 0, kinetic);
                assert_eq!(w.dims[0].kinetics.generated(pos, 0), Some(rpm));
                assert_eq!(w.saved_handle(0, pos), Some(saved));
                w.tick(0, &mut Vec::new());
                assert_eq!(w.speed(0, pos), Some(rpm));
                let network = w.dims[0]
                    .kinetics
                    .saved_state(pos, 0)
                    .unwrap()
                    .network
                    .unwrap();
                assert_eq!(network.capacity, 256.0);
                // Java getSize counts the old object and the fresh object; the
                // cached size remains 1 until a changed network total causes sync.
                assert_eq!(network.size, 1);
                assert_eq!(w.dims[0].kinetics.network_membership(pos, 0).unwrap().1, 2);
                for _ in 1..saved.in_use {
                    w.tick_cranks(0);
                }
                assert_eq!(w.dims[0].kinetics.generated(pos, 0), Some(rpm));
                w.tick_cranks(0);
                w.tick(0, &mut Vec::new());
                assert_eq!(w.dims[0].kinetics.generated(pos, 0), Some(0.0));
                if valve {
                    assert!(
                        w.use_valve(0, pos, false, 45),
                        "Java does not save cooldown"
                    );
                }
            }
        }
        states::clear_range();
    }

    #[test]
    fn a_turned_crank_drives_for_ten_ticks() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:hand_crank",
                vec![("facing", "west"), ("waterlogged", "false")],
            ),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // Facing west, hanging from the block to its east: the shaft at x + 1.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0));
        assert!(w.turn_crank(0, [0, 0, 0], false));
        w.tick(0, &mut Vec::new());
        // convertToDirection(32, west): -32 along x.
        assert_eq!(w.speed(0, [1, 0, 0]), Some(-32.0));
        for _ in 0..9 {
            w.tick_cranks(0);
        }
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.speed(0, [1, 0, 0]),
            Some(-32.0),
            "still turning at tick 9"
        );
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0), "stopped after 10 ticks");
        assert!(!w.turn_crank(0, [1, 0, 0], false), "a shaft is no crank");
        states::clear_range();
    }

    #[test]
    fn interaction_journals_include_immediate_handle_rotation() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:copper_valve_handle", [("facing", "up")].into_iter()),
                ("create:shaft", [("axis", "y")].into_iter()),
            ]
            .into_iter(),
        ));
        let (handle, shaft) = ([0, 8, 0], [0, 7, 0]);
        w.block_changed(0, shaft, 0, 11);
        w.block_changed(0, handle, 0, 10);
        w.tick(0, &mut Vec::new());
        w.take_handle_changes(0);
        w.dims[0].kinetics.take_saved_changes();
        // An unrelated interaction can publish a newly placed, stationary shaft.
        let idle = [8, 8, 0];
        w.block_changed(0, idle, 0, 11);
        assert!(
            w.dims[0]
                .kinetics
                .saved_state(idle, 0)
                .unwrap()
                .needs_speed_update
        );
        w.dims[0].kinetics.take_saved_changes();
        w.tick(0, &mut Vec::new());
        assert!(
            !w.dims[0]
                .kinetics
                .saved_state(idle, 0)
                .unwrap()
                .needs_speed_update
        );
        assert!(w.dims[0].kinetics.take_saved_changes().contains(&idle));
        // ValveHandleBlockEntity.activate calls updateGeneratedRotation before returning.
        assert!(w.use_valve(0, handle, false, 90));
        assert_eq!(w.saved_handle(0, handle).unwrap().in_use, 12);
        assert_eq!(w.speed(0, shaft), Some(32.0));
        assert!(w.take_handle_changes(0).contains(&handle));
        let changed = w.dims[0].kinetics.take_saved_changes();
        assert!(changed.contains(&handle) && changed.contains(&shaft));
        states::clear_range();
    }

    #[test]
    fn recoloured_valve_retains_entity_and_driven_shaft() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            "create:copper_valve_handle",
            "create:red_valve_handle",
            "create:white_valve_handle",
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names
                .iter()
                .map(|n| (*n, [("facing", "up")].into_iter()))
                .chain(std::iter::once((
                    "create:shaft",
                    [("axis", "y")].into_iter(),
                ))),
        ));
        let (handle, shaft) = ([0, 8, 0], [0, 7, 0]);
        w.block_changed(0, shaft, 0, 13);
        w.tick(0, &mut Vec::new());
        w.block_changed(0, handle, 0, 10);
        w.tick(0, &mut Vec::new());
        // Issue #21 Java trace: sneaking use of 41 degrees generates -32 RPM.
        assert!(w.use_valve(0, handle, true, 41));
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(-32.0));
        w.block_changed(0, handle, 10, 11);
        for _ in 0..38 {
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
        }
        w.block_changed(0, handle, 11, 12);
        for _ in 0..70 {
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
        }
        // HandCrankBlockEntity.tick expires, but clearKineticInformation did not
        // detach the old shaft. Java retains its speed, source and sequence.
        assert_eq!(w.dims[0].handles[&handle].saved.in_use, 0);
        assert_eq!(w.speed(0, shaft), Some(-32.0));
        assert_eq!(
            w.dims[0].kinetics.sequence_limit(shaft, 0, false),
            Some(41.0)
        );
        states::clear_range();
    }

    fn assert_recoloured_valve_reload(backwards: bool) {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:shaft", [("axis", "y")].into_iter()),
                ("create:copper_valve_handle", [("facing", "up")].into_iter()),
                ("create:red_valve_handle", [("facing", "up")].into_iter()),
            ]
            .into_iter(),
        ));
        let positions = [[0, 7, 0], [0, 8, 0], [0, 6, 0]];
        for (p, state) in positions.into_iter().zip([10, 11, 10]) {
            w.block_changed(0, p, 0, state);
            w.tick(0, &mut Vec::new());
        }
        assert!(w.use_valve(0, positions[1], backwards, -180));
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        w.block_changed(0, positions[1], 11, 12);
        let handle = w.saved_handle(0, positions[1]).unwrap();
        let saved = positions.map(|p| w.dims[0].kinetics.saved_state(p, 0).unwrap());
        // KineticBlockEntity.write after clearKineticInformation keeps Sequence,
        // but writes Speed:0 and no Source/Network on the recoloured entity.
        assert_eq!(saved[1].speed, 0.0);
        assert_eq!(saved[1].source, None);
        assert_eq!(saved[1].network, None);
        assert!(saved[1].sequence.is_some());
        w.column_unloaded(0, 0, 0);
        for ((p, state), kinetic) in positions.into_iter().zip([10, 12, 10]).zip(saved) {
            w.block_loaded(0, p, state);
            if p == positions[1] {
                w.restore_handle(0, p, handle);
            }
            assert!(w.dims[0].kinetics.restore_state(p, 0, kinetic));
        }
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.dims[0].kinetics.sequence_limit(positions[2], 0, false),
            Some(180.0)
        );
        // Java issue #69 checkpoint 223: the handle is the root at +/-32 RPM.
        assert_eq!(
            w.speed(0, positions[0]),
            Some(if backwards { 32.0 } else { -32.0 })
        );
        assert_eq!(
            w.dims[0]
                .kinetics
                .saved_state(positions[1], 0)
                .unwrap()
                .source,
            None
        );
        assert_eq!(
            w.dims[0]
                .kinetics
                .saved_state(positions[0], 0)
                .unwrap()
                .source
                .unwrap()
                .pos,
            positions[1]
        );
        states::clear_range();
    }

    #[test]
    fn recoloured_valve_reload_restores_forward_source_tree() {
        assert_recoloured_valve_reload(false);
    }

    #[test]
    fn recoloured_valve_reload_restores_reverse_source_tree() {
        assert_recoloured_valve_reload(true);
    }

    #[test]
    fn valve_same_tick_reload_keeps_owned_sequence() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [("create:copper_valve_handle", [("facing", "up")].into_iter())].into_iter(),
        ));
        let p = [0, 8, 0];
        w.block_changed(0, p, 0, 10);
        w.tick(0, &mut Vec::new());
        assert!(w.use_valve(0, p, false, -180));
        let handle = w.saved_handle(0, p).unwrap();
        let kinetic = w.dims[0].kinetics.saved_state(p, 0).unwrap();
        w.column_unloaded(0, 0, 0);
        w.block_loaded(0, p, 10);
        w.restore_handle(0, p, handle);
        w.dims[0].kinetics.restore_state(p, 0, kinetic);
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.dims[0]
                .kinetics
                .saved_state(p, 0)
                .unwrap()
                .sequence
                .unwrap()
                .0
                .value,
            180.0
        );
        states::clear_range();
    }

    #[test]
    fn valve_load_uses_serialized_sequence_even_when_it_disagrees_with_angle() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:copper_valve_handle", [("facing", "up")].into_iter()),
                ("create:red_valve_handle", [("facing", "up")].into_iter()),
                ("create:creative_motor", [("facing", "up")].into_iter()),
            ]
            .into_iter(),
        ));
        let p = [0, 8, 0];
        w.block_changed(0, p, 0, 10);
        w.tick(0, &mut Vec::new());
        assert!(w.use_valve(0, p, false, 180));
        let handle = w.saved_handle(0, p).unwrap();
        let mut kinetic = w.dims[0].kinetics.saved_state(p, 0).unwrap();
        // KineticBlockEntity.read reads Sequence independently of angleInput/InUse.
        for sequence in [
            Some((
                crate::kinetics::Context {
                    distance: true,
                    value: 73.0,
                    run: 9,
                },
                -2.5,
            )),
            None,
        ] {
            kinetic.sequence = sequence;
            w.column_unloaded(0, 0, 0);
            w.block_loaded(0, p, 10);
            w.restore_handle(0, p, handle);
            assert!(w.dims[0].kinetics.restore_state(p, 0, kinetic));
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
            w.block_changed(0, p, 10, 11);
            w.tick_cranks(0);
            w.tick(0, &mut Vec::new());
            // An external source changes this owner's speed; Java setSpeed does
            // not recalculate the saved Sequence from the valve's angleInput.
            w.block_changed(0, [0, 7, 0], 0, 12);
            w.set_settings(
                0,
                [0, 7, 0],
                12,
                Settings {
                    motor_rpm: 64.0,
                    ..Default::default()
                },
            );
            w.tick(0, &mut Vec::new());
            assert_eq!(w.speed(0, p), Some(64.0));
            assert_eq!(
                w.dims[0].kinetics.saved_state(p, 0).unwrap().sequence,
                sequence
            );
        }
        states::clear_range();
    }

    #[test]
    fn a_valve_handle_turns_by_its_angle_then_cools_down() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:red_valve_handle",
                vec![("facing", "west"), ("waterlogged", "false")],
            ),
            ("create:shaft", vec![("axis", "x")]),
            ("create:creative_motor", vec![("facing", "west")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.tick(0, &mut Vec::new());
        assert!(w.value_box_active(0, [0, 0, 0]));
        assert!(w.use_valve(0, [0, 0, 0], false, 45));
        assert!(!w.value_box_active(0, [0, 0, 0]));
        assert!(!w.use_valve(0, [0, 0, 0], false, 45), "not while it turns");
        w.tick(0, &mut Vec::new());
        // HandCrankBlockEntity.getGeneratedSpeed: 32 in its facing's direction.
        assert_eq!(w.speed(0, [1, 0, 0]), Some(-32.0));
        // Every valve handle has the copper one's capacity (8 su/RPM).
        assert_eq!(
            w.dims[0].kinetics.network([1, 0, 0], 0).map(|n| n.2),
            Some(8.0 * 32.0)
        );
        for _ in 0..6 {
            w.tick_cranks(0);
        }
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(-32.0), "45° takes 5 ticks, +2");
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0), "stopped after 7");
        assert!(
            w.value_box_active(0, [0, 0, 0]),
            "cooldown does not hide the box"
        );
        // HandCrankBlockEntity.tick clears the valve's Sequence at expiry (issue #19).
        assert_eq!(w.dims[0].kinetics.sequence_limit([0, 0, 0], 0, false), None);
        assert_eq!(
            w.dims[0]
                .kinetics
                .saved_state([0, 0, 0], 0)
                .unwrap()
                .sequence,
            None
        );
        // The cooldown: 4, counted from the tick it stopped.
        for _ in 0..3 {
            assert!(!w.use_valve(0, [0, 0, 0], true, 45), "cooling down");
            w.tick_cranks(0);
        }
        assert!(
            w.use_valve(0, [0, 0, 0], true, 45),
            "turns again, backwards"
        );
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(32.0));
        for _ in 0..7 {
            w.tick_cranks(0);
        }
        // Something else turning it: a motor at the shaft's far end.
        for _ in 0..4 {
            w.tick_cranks(0);
        }
        w.block_changed(0, [2, 0, 0], 0, 12);
        w.tick(0, &mut Vec::new());
        assert_ne!(w.speed(0, [1, 0, 0]), Some(0.0));
        assert!(
            !w.use_valve(0, [0, 0, 0], false, 45),
            "driven by another source"
        );
        assert!(!w.use_valve(0, [1, 0, 0], false, 45), "a shaft is no valve");
        states::clear_range();
    }

    #[test]
    fn sliding_doors_that_open_or_close_are_noted() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let door = |open: &'static str| {
            (
                "create:train_door",
                vec![
                    ("facing", "north"),
                    ("half", "lower"),
                    ("hinge", "left"),
                    ("open", open),
                    ("powered", "false"),
                    ("visible", "true"),
                ],
            )
        };
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![door("false"), door("true")];
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [0, 0, 0], 10, 11);
        w.block_changed(0, [0, 0, 0], 11, 11);
        w.block_changed(0, [0, 0, 0], 11, 0);
        w.block_changed(0, [5, 0, 0], 0, 11);
        assert_eq!(
            w.dims[0].door_changes,
            vec![([0, 0, 0], false), ([5, 0, 0], false)],
            "opened, and placed open (powered); placed closed, the same state and removal \
             are not changes"
        );
        assert!(w.dims[0].kinetics.is_empty(), "a door is no kinetic block");
        states::clear_range();
    }

    #[test]
    fn an_engine_on_a_boiler_powers_the_shaft_in_front() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:fluid_tank",
                vec![("bottom", "true"), ("top", "true")],
            ),
            (
                "create:steam_engine",
                vec![
                    ("face", "wall"),
                    ("facing", "south"),
                    ("waterlogged", "false"),
                ],
            ),
            ("create:shaft", vec![("axis", "x")]),
            ("create:powered_shaft", vec![("axis", "x")]),
            ("create:blaze_burner", vec![("blaze", "seething")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let (tank, engine, shaft) = ([0, 0, 0], [0, 0, 1], [0, 0, 3]);
        w.block_changed(0, [0, -1, 0], 0, 14);
        w.block_changed(0, tank, 0, 10);
        w.block_changed(0, shaft, 0, 12);
        // On the tank's south side, pointing south: its shaft two blocks on.
        w.block_changed(0, engine, 0, 11);
        assert_eq!(w.engine_at(0, engine), Some(crate::kinetics::Dir::South));
        w.evaluate_boilers(0);
        assert_eq!(w.dims[0].boilers[&tank].engines, vec![engine]);
        assert_eq!(w.shaft_changes(0), vec![(shaft, true)]);
        w.block_changed(0, shaft, 12, 13);
        assert_eq!(w.take_heat_checks(0), vec![(tank, 1)]);
        w.set_boiler_heat(0, tank, &[2]);
        assert!(w.take_heat_checks(0).is_empty());
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.speed(0, shaft),
            Some(0.0),
            "a one-block tank is too small to boil"
        );
        assert_eq!(w.dims[0].shaft_engine.get(&shaft), Some(&engine));
        // As a 2x2x2 tank, a seething burner and 20 mB a tick: level 2, full efficiency.
        w.dims[0].boilers.get_mut(&tank).unwrap().size = 8;
        let supply_before = w.dims[0].boilers[&tank].data.gathered_supply;
        assert_eq!(
            w.simulate_fill_boiler(0, tank, crate::tank::Fluid::Water, 100),
            Some(100)
        );
        assert_eq!(
            w.simulate_fill_boiler(0, tank, crate::tank::Fluid::Lava, 100),
            Some(0)
        );
        assert_eq!(w.dims[0].boilers[&tank].data.gathered_supply, supply_before);

        assert_eq!(
            w.fill_boiler(0, tank, crate::tank::Fluid::Water, 100),
            Some(100)
        );
        assert_eq!(
            w.fill_boiler(0, tank, crate::tank::Fluid::Lava, 100),
            Some(0)
        );
        assert_eq!(
            w.fill_boiler(0, [5, 0, 0], crate::tank::Fluid::Water, 1),
            None
        );
        w.tick_boilers(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(64.0));
        // The burner goes out: the engine stops.
        w.block_changed(0, [0, -1, 0], 14, 0);
        assert_eq!(w.take_heat_checks(0), vec![(tank, 1)]);
        w.set_boiler_heat(0, tank, &[]);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(0.0));
        w.set_boiler_heat(0, tank, &[2]);
        // Broken: the boiler goes, the shaft is a plain one again.
        w.block_changed(0, engine, 11, 0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(0.0));
        w.evaluate_boilers(0);
        assert!(w.dims[0].boilers.is_empty());
        assert_eq!(w.shaft_changes(0), vec![(shaft, false)]);
        assert!(w.dims[0].shaft_engine.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_whistle_makes_its_tank_a_boiler_and_sounds_while_it_is_heated() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:fluid_tank",
                vec![("bottom", "true"), ("top", "true")],
            ),
            (
                "create:steam_whistle",
                vec![
                    ("facing", "north"),
                    ("powered", "true"),
                    ("size", "small"),
                    ("wall", "false"),
                ],
            ),
            (
                "create:steam_whistle_extension",
                vec![("shape", "single"), ("size", "small")],
            ),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let (tank, whistle) = ([0, 0, 0], [0, 1, 0]);
        w.block_changed(0, tank, 0, 10);
        w.block_changed(0, whistle, 0, 11);
        assert_eq!(w.whistle_at(0, whistle), Some(crate::kinetics::Dir::Down));
        w.evaluate_boilers(0);
        let b = &w.dims[0].boilers[&tank];
        assert_eq!(
            (b.whistles.clone(), b.data.attached_whistles),
            (vec![whistle], 1)
        );
        assert!(b.data.is_active() && b.engines.is_empty());
        assert!(w.take_sound_checks(0).contains(&whistle));
        // Unheated: silent. A passive heater under it: it sounds; its heat changing tells it.
        assert!(!crate::whistle::sounds(true, w.whistle_boiler(0, whistle)));
        w.set_boiler_heat(0, tank, &[crate::steam::PASSIVE_HEAT]);
        assert_eq!(w.take_sound_checks(0), vec![whistle]);
        assert!(crate::whistle::sounds(true, w.whistle_boiler(0, whistle)));
        // An extension placed on it: its pitch is counted again.
        w.block_changed(0, [0, 2, 0], 0, 12);
        assert_eq!(w.dims[0].plumbing.extension_checks, vec![[0, 2, 0]]);
        // The whistle goes: the tank is no boiler, the extension above may fall.
        w.block_changed(0, whistle, 11, 0);
        w.evaluate_boilers(0);
        assert!(w.dims[0].boilers.is_empty());
        assert!(w.dims[0].plumbing.whistles.is_empty());
        assert!(w.dims[0].plumbing.extension_checks.contains(&[0, 2, 0]));
        states::clear_range();
    }

    #[test]
    fn valve_pointer_cold_load_and_saved_change_journal() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [(
                "create:fluid_valve",
                [("facing", "up"), ("enabled", "true")].into_iter(),
            )]
            .into_iter(),
        ));
        let pos = [0, 8, 0];
        w.block_changed(0, pos, 0, 10);
        let saved = crate::fluids::Pointer {
            value: 0.3,
            target: 1.0,
            speed: 0.1,
        };
        w.restore_valve_pointer(0, pos, saved);
        assert_eq!(w.saved_valve_pointer(0, pos), Some(saved));
        w.dims[0].kinetics.take_saved_changes();
        w.column_unloaded(0, 0, 0);
        w.block_loaded(0, pos, 10);
        w.restore_valve_pointer(0, pos, saved);
        w.dims[0].kinetics.take_saved_changes();
        assert!(w.tick_fluid_entities(0, |_| Some(true)).is_empty());
        assert_eq!(w.saved_valve_pointer(0, pos).unwrap().value, 0.4);
        assert_eq!(w.dims[0].kinetics.take_saved_changes(), [pos].into());
        assert!(w.dims[0].kinetics.take_saved_changes().is_empty());
        // onSpeedChanged(0) freezes a fully open pointer while targeting zero.
        let stopped = crate::fluids::Pointer {
            value: 1.0,
            target: 0.0,
            speed: 0.0,
        };
        w.restore_valve_pointer(0, pos, stopped);
        w.dims[0].kinetics.take_saved_changes();
        w.column_unloaded(0, 0, 0);
        w.block_loaded(0, pos, 10);
        w.restore_valve_pointer(0, pos, stopped);
        w.dims[0].kinetics.take_saved_changes();
        assert!(w.tick_fluid_entities(0, |_| Some(true)).is_empty());
        assert_eq!(w.saved_valve_pointer(0, pos), Some(stopped));
        // The base kinetic tick clears NeedsSpeedUpdate on its first loaded turn.
        assert_eq!(w.dims[0].kinetics.take_saved_changes(), [pos].into());
        assert!(w.tick_fluid_entities(0, |_| Some(true)).is_empty());
        assert!(w.dims[0].kinetics.take_saved_changes().is_empty());
        states::clear_range();
    }

    #[test]
    fn ordinary_pipe_turn_runs_the_earlier_kinetic_base_prefix() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:creative_motor", vec![("facing", "east")]),
                ("create:shaft", vec![("axis", "x")]),
                (
                    "create:fluid_pipe",
                    vec![("east", "true"), ("west", "true")],
                ),
            ]
            .into_iter()
            .map(|(n, p)| (n, p.into_iter())),
        ));
        let drive = [1, 0, 0];
        let pipe = [9, 0, 0];
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, drive, 0, 11);
        w.block_changed(0, pipe, 0, 12);
        w.set_settings(
            0,
            [0, 0, 0],
            10,
            Settings {
                motor_rpm: 64.,
                ..Settings::default()
            },
        );
        w.tick(0, &mut Vec::new());
        w.dims[0].kinetics.change_block_state(
            drive,
            0,
            crate::kinetics::Part::shaft(crate::kinetics::Axis::X),
            false,
        );
        assert_eq!(w.speed(0, drive), Some(0.));
        w.fluid_transport_base_tick(0, pipe);
        assert_eq!(w.speed(0, drive), Some(64.));
        states::clear_range();
    }

    // Java's reload cases: chase speed at 32 rpm is 0.1, with targets 1/0.
    // Publication after an interaction must include onSpeedChanged's chase, even
    // though the next FluidValveBlockEntity.tick has not moved its value yet.
    fn valve_interaction_reload(rpm: f32, value: f32, target: f32) {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:creative_motor", vec![("facing", "south")]),
                (
                    "create:fluid_valve",
                    vec![
                        ("facing", "down"),
                        ("axis_along_first", "false"),
                        ("enabled", "true"),
                    ],
                ),
            ]
            .into_iter()
            .map(|(n, p)| (n, p.into_iter())),
        ));
        let (motor, valve) = ([0, 8, 1], [0, 8, 2]);
        w.block_changed(0, motor, 0, 10);
        w.block_changed(0, valve, 0, 11);
        w.tick_fluid_entities(0, |_| Some(true));
        w.tick(0, &mut Vec::new());
        w.restore_valve_pointer(
            0,
            valve,
            crate::fluids::Pointer {
                value,
                target: 1.0 - target,
                speed: 0.0,
            },
        );
        w.board_motor(
            0,
            motor,
            crate::board::Settings {
                row: i32::from(rpm >= 0.0),
                value: rpm.abs() as i32,
            },
        );
        w.flush_kinetic_callbacks(0);
        let saved = w.saved_valve_pointer(0, valve).unwrap();
        assert_eq!(
            saved,
            crate::fluids::Pointer {
                value,
                target,
                speed: 0.1
            }
        );
        let kinetics: Vec<_> = [motor, valve]
            .into_iter()
            .map(|p| (p, w.dims[0].kinetics.saved_state(p, 0).unwrap()))
            .collect();
        w.column_unloaded(0, 0, 0);
        w.load_settings(0, [(motor, rpm as i32)].into_iter());
        for (p, state) in [(motor, 10), (valve, 11)] {
            w.block_loaded(0, p, state);
        }
        w.restore_valve_pointer(0, valve, saved);
        for (p, saved) in kinetics {
            w.dims[0].kinetics.restore_state(p, 0, saved);
        }
        assert!(w.tick_fluid_entities(0, |_| Some(true)).is_empty());
        assert!(
            (w.saved_valve_pointer(0, valve).unwrap().value
                - if target == 1.0 { 0.1 } else { 0.9 })
            .abs()
                < 1e-6
        );
        states::clear_range();
    }

    #[test]
    fn valve_placement_interaction_publishes_chase_before_reload() {
        valve_interaction_reload(32.0, 0.0, 1.0);
    }

    #[test]
    fn valve_reversal_interaction_publishes_chase_before_reload() {
        valve_interaction_reload(-32.0, 1.0, 0.0);
    }

    #[test]
    fn valve_chases_on_its_first_kinetic_tick() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:creative_motor", vec![("facing", "south")]),
                (
                    "create:fluid_valve",
                    vec![
                        ("facing", "down"),
                        ("axis_along_first", "false"),
                        ("enabled", "false"),
                    ],
                ),
            ]
            .into_iter()
            .map(|(n, p)| (n, p.into_iter())),
        ));
        let (motor, valve) = ([0, 8, 1], [0, 8, 2]);
        w.block_changed(0, motor, 0, 10);
        w.tick_fluid_entities(0, |_| Some(false));
        w.tick(0, &mut Vec::new());
        w.block_changed(0, valve, 0, 11);
        let setting = Settings {
            motor_rpm: 32.0,
            ..Settings::default()
        };
        w.set_settings(0, motor, 10, setting);
        for tick in 14..=23 {
            let changes = w.tick_fluid_entities(0, |_| Some(false));
            if tick < 23 {
                assert!(changes.is_empty());
            } else {
                assert_eq!(changes, [(valve, true)]);
            }
            w.tick(0, &mut Vec::new());
        }
        states::clear_range();
    }

    #[test]
    fn valves_open_with_their_speed_and_pipes_look_again() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:creative_motor", vec![("facing", "west")]),
            (
                "create:fluid_valve",
                vec![
                    ("axis_along_first", "false"),
                    ("enabled", "false"),
                    ("facing", "up"),
                ],
            ),
            ("create:hose_pulley", vec![("facing", "north")]),
            ("create:creative_fluid_tank", vec![]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let (valve, hose, tank) = ([0, 0, 0], [5, 0, 0], [9, 0, 0]);
        w.block_changed(0, valve, 0, 11);
        assert!(
            !w.dims[0].fluid_transport.changed.is_empty(),
            "a valve is a pipe"
        );
        assert!(w.dims[0].plumbing.valves.contains_key(&valve));
        w.dims[0].plumbing.valve_speed(valve, 32.0);
        assert!(w.dims[0].plumbing.valves[&valve].moving());
        // A hose pulley turns on x (its facing north turned clockwise), its shaft out east.
        w.block_changed(0, hose, 0, 12);
        assert!(w.dims[0].plumbing.hoses.contains_key(&hose));
        let [p, _] = crate::parts::parts(
            Block::HosePulley,
            &StateInfo::read([("facing", "north")].into_iter()),
            &Settings::default(),
        );
        assert_eq!(p.map(|p| p.axis), Some(crate::kinetics::Axis::X));
        // A motor east of the hose, facing west into its shaft: the hose is given its speed.
        w.block_changed(0, [6, 0, 0], 0, 10);
        w.tick(0, &mut Vec::new());
        let speeds = std::mem::take(&mut w.dims[0].plumbing.hose_speeds);
        assert!(
            speeds.iter().any(|(p, s)| *p == hose && s.abs() == 16.0),
            "{speeds:?}"
        );
        w.block_changed(0, tank, 0, 13);
        assert_eq!(w.dims[0].plumbing.creative.get(&tank), Some(&None));
        w.block_changed(0, tank, 13, 0);
        assert!(w.dims[0].plumbing.creative.is_empty());
        states::clear_range();
    }

    #[test]
    fn creative_tanks_join_share_their_fluid_and_split() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:creative_fluid_tank", vec![]),
            ("create:fluid_tank", vec![]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let water = Some(crate::tank::Fluid::Water);
        w.block_changed(0, [1, 0, 0], 0, 10);
        assert!(w.set_creative_fluid(0, [1, 0, 0], water));
        // A fluid tank beside it stays a tank of its own kind.
        w.block_changed(0, [2, 0, 0], 0, 11);
        assert_eq!(w.creative_tank(0, [2, 0, 0]), None);
        for p in [
            [0, 0, 0],
            [0, 0, 1],
            [1, 0, 1],
            [0, 1, 0],
            [1, 1, 0],
            [0, 1, 1],
            [1, 1, 1],
        ] {
            w.block_changed(0, p, 0, 10);
        }
        w.tick_fluid_entities(0, |_| None);
        let (c, m) = w.creative_tank(0, [1, 1, 1]).unwrap();
        assert_eq!((c, m.width, m.height), ([0, 0, 0], 2, 2));
        // The new controller had none: it takes the fluid, and every block gives it.
        assert_eq!(m.tank.fluid, water);
        assert!(w.dims[0].plumbing.creative.values().all(|f| *f == water));
        assert_eq!(w.dims[0].plumbing.creative.len(), 8);
        let saved = w.column_creative_tanks(0, 0, 0);
        assert_eq!(saved.len(), 1);
        // Its controller broken: the rest split and form again, each keeping the fluid.
        w.block_changed(0, [0, 0, 0], 10, 0);
        w.tick_fluid_entities(0, |_| None);
        assert_eq!(w.dims[0].plumbing.creative.len(), 7);
        assert!(w.dims[0].plumbing.creative.values().all(|f| *f == water));
        let shape = w.tank_shape(0, [1, 1, 1]).unwrap();
        assert!(shape.top);
        states::clear_range();
    }

    #[test]
    fn a_two_by_two_boiler_over_kindled_burners_drives_its_engine() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:fluid_tank",
                vec![("bottom", "true"), ("top", "true"), ("shape", "window")],
            ),
            (
                "create:steam_engine",
                vec![
                    ("face", "wall"),
                    ("facing", "south"),
                    ("waterlogged", "false"),
                ],
            ),
            ("create:shaft", vec![("axis", "x")]),
            ("create:powered_shaft", vec![("axis", "x")]),
            ("create:blaze_burner", vec![("blaze", "kindled")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let square = [[0, 0, 0], [1, 0, 0], [0, 0, 1], [1, 0, 1]];
        for p in square {
            w.block_changed(0, [p[0], -1, p[2]], 0, 14);
        }
        for p in square {
            w.block_changed(0, p, 0, 10);
        }
        w.tick_fluid_entities(0, |_| None);
        // One tank, its controller the north-west corner, each block its corner window.
        let c = [0, 0, 0];
        let (controller, m) = w.tank(0, [1, 0, 1]).unwrap();
        assert_eq!((controller, m.width, m.height), (c, 2, 1));
        assert_eq!(w.take_tank_states(0), {
            let mut v = square.to_vec();
            v.sort();
            v
        });
        assert_eq!(
            w.tank_shape(0, [1, 0, 1]).unwrap().shape,
            crate::tank::Shape::WindowSe
        );
        // An engine on the south side of the south-east block, its shaft two blocks on.
        let (engine, shaft) = ([1, 0, 2], [1, 0, 4]);
        w.block_changed(0, shaft, 0, 12);
        w.block_changed(0, engine, 0, 11);
        w.evaluate_boilers(0);
        assert_eq!(w.dims[0].boilers[&c].engines, vec![engine]);
        assert_eq!(w.dims[0].boilers[&c].size, 4);
        assert!(w.is_boiler(0, [1, 0, 1]) && w.is_boiler(0, c));
        // A new boiler loses its windows.
        assert_eq!(
            w.tank_shape(0, [1, 0, 1]).unwrap().shape,
            crate::tank::Shape::Plain
        );
        assert_eq!(w.shaft_changes(0), vec![(shaft, true)]);
        w.block_changed(0, shaft, 12, 13);
        // Heat from the four burners under it.
        assert_eq!(w.take_heat_checks(0), vec![(c, 2)]);
        w.set_boiler_heat(0, c, &[1, 1, 1, 1]);
        // 50 mB of water into any of its blocks: 10 mB a tick, level 1 (a 4-block tank's most).
        assert_eq!(
            w.fill_boiler(0, [1, 0, 0], crate::tank::Fluid::Water, 50),
            Some(50)
        );
        w.tick_boilers(0);
        let b = &w.dims[0].boilers[&c];
        assert_eq!((b.data.active_heat, b.data.level(b.size)), (4, 1));
        assert_eq!(b.data.engine_efficiency(b.size), 1.0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(64.0));
        // A block broken: the tank splits, the engine's block is a boiler of its own, too small.
        w.block_changed(0, [1, 0, 0], 10, 0);
        assert_eq!(w.tank(0, [1, 0, 1]).unwrap().0, [1, 0, 1]);
        w.evaluate_boilers(0);
        assert_eq!(
            w.dims[0].boilers.keys().copied().collect::<Vec<_>>(),
            vec![[1, 0, 1]]
        );
        assert_eq!(w.dims[0].boilers[&[1, 0, 1]].size, 1);
        assert_eq!(w.take_heat_checks(0), vec![([1, 0, 1], 1)]);
        w.set_boiler_heat(0, [1, 0, 1], &[1]);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, shaft), Some(0.0));
        assert_eq!(w.dims[0].shaft_engine.get(&shaft), Some(&engine));
        // Saved by controller: the three tanks left are plain one-block tanks without windows.
        let saved = w.column_tanks(0, 0, 0);
        assert_eq!(saved.len(), 3);
        assert!(
            saved
                .iter()
                .all(|(_, m)| m.multi.is_some_and(|m| m.blocks() == 1 && !m.window))
        );
        states::clear_range();
    }

    #[test]
    fn loaded_sections_join_and_unloaded_columns_leave() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let mut states = [0u32; 4096];
        // x 0..3 at y 0, z 0 of a section at (16, 64, 32): a motor then two shafts.
        states[0] = 5;
        states[1] = 2;
        states[2] = 2;
        w.section_loaded(0, [16, 64, 32], &states);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [18, 64, 32]), Some(16.0));
        w.column_unloaded(0, 1, 2);
        assert_eq!(w.speed(0, [18, 64, 32]), None);
        assert!(w.dims[0].kinetics.is_empty());
        states::clear_range();
    }

    #[test]
    fn analog_chain_sweep_breaks_on_tick_118() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            ("create:creative_motor", vec![("facing", "east")]),
            (
                "create:adjustable_chain_gearshift",
                vec![("axis", "x"), ("powered", "false")],
            ),
            (
                "create:adjustable_chain_gearshift",
                vec![("axis", "x"), ("powered", "true")],
            ),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let pos = [0, 8, 0];
        let strengths = [
            7, 5, 0, 5, 13, 1, 14, 0, 10, 12, 1, 6, 8, 5, 7, 5, 10, 14, 5, 3, 11,
        ];
        let mut state = 11;
        for tick in 0..120 {
            if tick == 1 {
                w.block_changed(0, [-1, 8, 0], 0, 10);
            }
            if tick == 2 {
                w.set_settings(
                    0,
                    [-1, 8, 0],
                    10,
                    Settings {
                        motor_rpm: 32.0,
                        ..Settings::default()
                    },
                );
            }
            if tick == 10 {
                w.block_changed(0, pos, 0, state);
            }
            if (38..=118).contains(&tick) && (tick - 38) % 4 == 0 {
                let signal = strengths[(tick - 38) / 4];
                let next = if signal == 0 { 11 } else { 12 };
                if next != state {
                    w.setting_known_shape(0, pos, next);
                    w.block_changed(0, pos, state, next);
                    state = next;
                }
                w.set_chain_signal(0, pos, signal);
            }
            let mut events = Vec::new();
            w.tick(0, &mut events);
            let broke = events
                .iter()
                .any(|e| matches!(e, Event::Break {pos: p} if *p == pos));
            assert_eq!(
                broke,
                tick == 118,
                "Java stays at 32 rpm until signal 11 on tick 118 (checkpoint 119 is air), tick {tick}"
            );
            if (10..118).contains(&tick) {
                assert_eq!(w.speed(0, pos), Some(32.0));
            }
        }
        states::clear_range();
    }

    #[test]
    fn chain_drives_shape_their_chain_and_follow_the_signal() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let drive = |part| vec![("axis", "x"), ("axis_along_first", "false"), ("part", part)];
        let gearshift = |part, powered| {
            vec![
                ("axis", "x"),
                ("axis_along_first", "false"),
                ("part", part),
                ("powered", powered),
            ]
        };
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:encased_chain_drive", drive("none")),
            ("create:encased_chain_drive", drive("end")),
            (
                "create:adjustable_chain_gearshift",
                gearshift("none", "false"),
            ),
            (
                "create:adjustable_chain_gearshift",
                gearshift("start", "false"),
            ),
            ("create:creative_motor", vec![("facing", "east")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [-1, 0, 0], 0, 14);
        w.block_changed(0, [0, 0, 0], 0, 12);
        w.block_changed(0, [0, 0, 1], 0, 10);
        // Placed: both are shaped (the gearshift starts the chain south, the drive ends it),
        // and the gearshift's signal is read.
        let shapes = w.take_chain_shapes(0);
        let part = |p: [i32; 3]| shapes.iter().find(|s| s.0 == p).map(|s| s.1.part);
        assert_eq!(part([0, 0, 0]), Some(crate::chain::Part::Start));
        assert_eq!(part([0, 0, 1]), Some(crate::chain::Part::End));
        assert_eq!(w.take_redstone(0), [[0, 0, 0]]);
        assert!(w.reads_signal(12) && !w.reads_signal(10));
        // The glue sets them; nothing more to shape.
        w.block_changed(0, [0, 0, 0], 12, 13);
        w.block_changed(0, [0, 0, 1], 10, 11);
        assert!(w.take_chain_shapes(0).is_empty());
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [0, 0, 1]), Some(16.0));
        // A signal of 7: the chain turns at 1.5 times the gearshift.
        w.set_chain_signal(0, [0, 0, 0], 7);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [0, 0, 0]), Some(16.0));
        assert_eq!(w.speed(0, [0, 0, 1]), Some(24.0));
        // The drive's neighbour changes: it is shaped again (no change here).
        w.neighbor_changed(0, [0, 0, 1], 11);
        assert!(w.take_chain_shapes(0).is_empty());
        states::clear_range();
    }

    #[test]
    fn loaded_chain_signal_precedes_base_attachment() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            ("create:creative_motor", vec![("facing", "south")]),
            (
                "create:adjustable_chain_gearshift",
                vec![
                    ("axis", "z"),
                    ("axis_along_first", "true"),
                    ("part", "start"),
                    ("powered", "true"),
                ],
            ),
            (
                "create:encased_chain_drive",
                vec![("axis", "z"), ("axis_along_first", "true"), ("part", "end")],
            ),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        let positions = [[0, 8, -1], [0, 8, 0], [1, 8, 0]];
        for (i, p) in positions.into_iter().enumerate() {
            w.block_changed(0, p, 0, 10 + i as u32);
        }
        w.set_settings(
            0,
            positions[0],
            10,
            Settings {
                motor_rpm: 32.0,
                ..Settings::default()
            },
        );
        w.tick(0, &mut Vec::new());
        w.set_chain_signal(0, positions[1], 3);
        w.tick(0, &mut Vec::new());
        let saved = positions.map(|p| w.dims[0].kinetics.saved_state(p, 0).unwrap());
        let signal = w.saved_chain_signal(0, positions[1]).unwrap();
        w.column_unloaded(0, 0, -1);
        w.column_unloaded(0, 0, 0);
        w.load_settings(0, [(positions[0], 32)].into_iter());
        for (i, p) in positions.into_iter().enumerate() {
            w.block_loaded(0, p, 10 + i as u32);
            w.restore_chain_signal(0, p, signal);
            assert!(w.dims[0].kinetics.restore_state(p, 0, saved[i]));
        }
        w.set_chain_signal(0, positions[1], 3);
        w.tick(0, &mut Vec::new());
        // chain-active-tiny: completed tick 8, Java keeps 32 and 40 RPM.
        assert_eq!(w.saved_chain_signal(0, positions[1]), Some(3));
        assert_eq!(w.speed(0, positions[1]), Some(32.0));
        assert_eq!(w.speed(0, positions[2]), Some(40.0));
        assert_eq!(w.dims[0].kinetics.flicker_score(positions[1]), 0);
        states::clear_range();
    }

    #[test]
    fn initial_zero_chain_poll_does_not_detach() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [
                ("create:creative_motor", vec![("facing", "east")]),
                ("create:adjustable_chain_gearshift", vec![("axis", "x")]),
            ]
            .into_iter()
            .map(|(n, p)| (n, p.into_iter())),
        ));
        w.block_loaded(0, [-1, 8, 0], 10);
        w.block_loaded(0, [0, 8, 0], 11);
        w.tick(0, &mut Vec::new());
        let before = w.dims[0].kinetics.flicker_score([0, 8, 0]);
        w.set_chain_signal(0, [0, 8, 0], 0);
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.dims[0].kinetics.flicker_score([0, 8, 0]),
            (before - 1).max(0)
        );
        states::clear_range();
    }

    #[test]
    fn loaded_sequencer_continues_saved_timer() {
        use crate::sequencer::{Instruction, Kind, Run, Speed};
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            [(
                "create:sequenced_gearshift",
                [("axis", "x"), ("state", "1"), ("vertical", "false")].into_iter(),
            )]
            .into_iter(),
        ));
        let pos = [0, 8, 0];
        w.block_loaded(0, pos, 10);
        w.set_sequencer_instructions(
            0,
            pos,
            vec![Instruction {
                kind: Kind::TurnAngle,
                speed: Speed::ForwardFast,
                value: 90,
            }],
            false,
        );
        let mut kinetic = w.dims[0].kinetics.saved_state(pos, 0).unwrap();
        kinetic.speed = 32.0;
        w.dims[0].kinetics.restore_state(pos, 0, kinetic);
        kinetic.speed = 64.0;
        kinetic.source = Some(crate::kinetics::PartLocation { pos, sub: 0 });
        w.dims[0].kinetics.restore_state(pos, 1, kinetic);
        let far_read_state = w.dims[0].kinetics.saved_state(pos, 1).unwrap();
        let read_state = w.dims[0].kinetics.saved_state(pos, 0).unwrap();
        // Java c37: completed tick 22 saved these fields; read does not call
        // onSpeedChanged. The unchanged 32 RPM report must leave the timer alone.
        w.restore_sequencer(
            0,
            pos,
            Run {
                current: 0,
                duration: 7,
                progress: 19.2,
                timer: 2,
                powered_before: true,
            },
        );
        assert_eq!(w.dims[0].kinetics.saved_state(pos, 1), Some(far_read_state));
        assert_eq!(w.dims[0].kinetics.saved_state(pos, 0), Some(read_state));
        let s = w.dims[0].sequencers.get_mut(&pos).unwrap();
        assert_eq!(s.speed_changed(32.0, false), None);
        assert_eq!(s.tick(), None);
        assert_eq!(s.duration, 7);
        assert_eq!(s.timer, 3);
        assert_eq!(s.progress, 28.800001);
        states::clear_range();
    }

    #[test]
    fn replacing_a_sequencer_program_commits_before_the_next_kinetic_tick() {
        use crate::sequencer::{Instruction, Kind, Run, Step};
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let seq = |state| vec![("axis", "x"), ("state", state), ("vertical", "false")];
        let names = [
            ("create:sequenced_gearshift", seq("0")),
            ("create:sequenced_gearshift", seq("3")),
            ("create:creative_motor", vec![("facing", "east")]),
        ];
        states::set_range(10, names.len() as u32);
        // ConfigureSequencedGearshiftPacket.applySettings calls run(-1) even while
        // AWAIT's modifier is zero (no detachKinetics), then commits state 0 now.
        for kind in [Kind::Await, Kind::TurnDistance] {
            let mut w = CreateWorld::new(StateTable::new(
                10,
                names.iter().map(|(n, p)| (*n, p.iter().copied())),
            ));
            let pos = [0, 8, 0];
            w.block_changed(0, [-1, 8, 0], 0, 12);
            w.set_motor(0, [-1, 8, 0], 32.0);
            w.block_changed(0, pos, 0, 11);
            let instructions = vec![
                Instruction::new(Kind::TurnAngle, 90),
                Instruction::new(Kind::Delay, 3),
                Instruction::new(kind, 1),
            ];
            w.set_sequencer_instructions(0, pos, instructions.clone(), false);
            w.restore_sequencer(
                0,
                pos,
                Run {
                    current: 2,
                    duration: if kind == Kind::Await { -1 } else { 18 },
                    progress: 0.0,
                    timer: 0,
                    powered_before: false,
                },
            );
            w.tick(0, &mut Vec::new());
            assert_eq!(w.speed(0, pos), Some(32.0));
            let commit = |w: &mut CreateWorld, step| {
                if step == Step::Ended {
                    let old = w.state_at(0, pos).unwrap();
                    w.block_changed(0, pos, old, 10);
                }
            };
            // Java transmission-sequencer tick 100: the unpowered packet stops it.
            if let Some(step) = w.set_sequencer_instructions(0, pos, instructions, true) {
                commit(&mut w, step);
            }
            // Tick 101: attach happens after the packet's state commit, so its source
            // is present. Replaying the old deferred commit leaves it cleared here.
            let (_, steps) = w.tick(0, &mut Vec::new());
            for (_, step) in steps {
                commit(&mut w, step);
            }
            assert_eq!(w.state_at(0, pos), Some(10));
            assert_eq!(w.speed(0, pos), Some(32.0));
            assert!(
                w.dims[0]
                    .kinetics
                    .saved_state(pos, 0)
                    .unwrap()
                    .source
                    .is_some()
            );
            assert_eq!(w.sequencer_run(0, pos).unwrap().current, -1);
        }
        states::clear_range();
    }

    #[test]
    fn sequencer_instruction_change_attaches_on_the_following_tick() {
        use crate::sequencer::{Instruction, Kind, Step};
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let seq = |state| vec![("axis", "x"), ("state", state), ("vertical", "false")];
        let names = [
            ("create:sequenced_gearshift", seq("0")),
            ("create:sequenced_gearshift", seq("1")),
            ("create:sequenced_gearshift", seq("2")),
            ("create:creative_motor", vec![("facing", "east")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        fn level_tick(w: &mut CreateWorld) -> Vec<([i32; 3], Step)> {
            let (_, steps) = w.tick(0, &mut Vec::new());
            for &(pos, step) in &steps {
                if let Step::Run(n) = step {
                    let old = w.state_at(0, pos).unwrap();
                    w.block_changed(0, pos, old, 10 + u32::from(n));
                }
            }
            steps
        }
        let pos = [0, 8, 0];
        w.block_changed(0, [-1, 8, 0], 0, 13);
        w.set_motor(0, [-1, 8, 0], 32.0);
        w.block_changed(0, pos, 0, 10);
        w.tick(0, &mut Vec::new());
        w.set_sequencer_instructions(
            0,
            pos,
            vec![
                Instruction::new(Kind::TurnAngle, 90),
                Instruction::new(Kind::Delay, 3),
                Instruction::new(Kind::Await, -1),
            ],
            false,
        );
        assert_eq!(
            w.sequencer_redstone(0, pos, true, false),
            Some(Step::Run(1))
        );
        w.block_changed(0, pos, 10, 11);
        // Java at 32 rpm: ceil(90 / 9.6) + 2 = 12 ticks, then run(1).
        for _ in 0..12 {
            assert!(level_tick(&mut w).is_empty());
        }
        assert_eq!(level_tick(&mut w), [(pos, Step::Run(2))]);
        // Java issue #20 tick 36: DELAY starts with no source or speed.
        let saved = w.dims[0].kinetics.saved_state(pos, 0).unwrap();
        assert_eq!(saved.speed, 0.0);
        assert!(saved.source.is_none());
        let run = w.sequencer_run(0, pos).unwrap();
        assert_eq!(
            (run.current, run.duration, run.progress, run.timer),
            (1, 3, 0.0, 0)
        );
        // Tick 37: super.tick attaches before the delay counts its first tick.
        assert!(level_tick(&mut w).is_empty());
        assert_eq!(w.speed(0, pos), Some(32.0));
        assert!(
            w.dims[0]
                .kinetics
                .saved_state(pos, 0)
                .unwrap()
                .source
                .is_some()
        );
        assert_eq!(w.sequencer_run(0, pos).unwrap().progress, 1.0);
        states::clear_range();
    }

    #[test]
    fn a_sequenced_gearshift_turns_its_far_half_through_its_sequence() {
        use crate::sequencer::Step;
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let seq = |state| vec![("axis", "x"), ("state", state), ("vertical", "false")];
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:sequenced_gearshift", seq("0")),
            ("create:sequenced_gearshift", seq("1")),
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [-1, 0, 0], 0, 12);
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 13);
        assert!(w.tick(0, &mut Vec::new()).1.is_empty());
        assert_eq!(
            w.speed(0, [1, 0, 0]),
            Some(0.0),
            "idle: the far half is still"
        );
        // Powered: its neighbour changed, its redstone is looked at; the 90° turn starts.
        w.neighbor_changed(0, [0, 0, 0], 10);
        assert_eq!(w.take_sequencer_checks(0), [([0, 0, 0], false)]);
        assert_eq!(
            w.sequencer_redstone(0, [0, 0, 0], true, false),
            Some(Step::Run(1))
        );
        w.block_changed(0, [0, 0, 0], 10, 11);
        // 21 ticks at 16 RPM, then it ends and the far half stops.
        for i in 0..21 {
            assert!(w.tick(0, &mut Vec::new()).1.is_empty());
            if i == 0 {
                assert_eq!(w.speed(0, [1, 0, 0]), Some(16.0));
            }
        }
        assert_eq!(w.tick(0, &mut Vec::new()).1, [([0, 0, 0], Step::Ended)]);
        assert!(w.tick(0, &mut Vec::new()).1.is_empty());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0));
        // SequencedGearshiftBlockEntity.run(-1): Sequence is absent, including on disk.
        assert_eq!(w.dims[0].kinetics.sequence_limit([0, 0, 0], 1, false), None);
        assert_eq!(
            w.dims[0]
                .kinetics
                .saved_state([0, 0, 0], 1)
                .unwrap()
                .sequence,
            None
        );
        assert!(
            w.tick(0, &mut Vec::new()).1.is_empty(),
            "idle: nothing ticks"
        );
        states::clear_range();
    }

    #[test]
    fn a_sequenced_gearshift_loaded_at_an_await_goes_on_at_the_next_pulse() {
        use crate::sequencer::{Instruction, Kind, Run, Step};
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let seq = |state| vec![("axis", "x"), ("state", state), ("vertical", "false")];
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:sequenced_gearshift", seq("2")),
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [-1, 0, 0], 0, 11);
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 12);
        let list = vec![
            Instruction::new(Kind::Await, -1),
            Instruction::new(Kind::TurnAngle, 90),
            Instruction::new(Kind::End, 1),
        ];
        w.set_sequencer_instructions(0, [0, 0, 0], list, false);
        // Saved waiting at the await (index 0), power gone since.
        w.restore_sequencer(
            0,
            [0, 0, 0],
            Run {
                current: 0,
                duration: -1,
                progress: 0.0,
                timer: 0,
                powered_before: false,
            },
        );
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0), "waiting: still");
        assert_eq!(w.sequencer_run(0, [0, 0, 0]).map(|r| r.current), Some(0));
        // The pulse moves past the await to the turn.
        assert_eq!(
            w.sequencer_redstone(0, [0, 0, 0], true, true),
            Some(Step::Run(2))
        );
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(16.0));
        states::clear_range();
    }

    #[test]
    fn a_speed_controller_is_set_like_a_motor() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:rotation_speed_controller", vec![("axis", "x")]),
            ("create:large_cogwheel", vec![("axis", "z")]),
            ("create:creative_motor", vec![("facing", "east")]),
        ];
        states::set_range(10, names.len() as u32);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [-1, 0, 0], 0, 12);
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [0, 1, 0], 0, 11);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.motor_setting(0, [0, 0, 0]), Some(16.0));
        assert_eq!(w.speed(0, [0, 1, 0]), Some(16.0));
        assert_eq!(w.scroll_motor(0, [0, 0, 0], 48), Some(64.0));
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [0, 1, 0]), Some(64.0));
        assert_eq!(w.column_settings(0, 0, 0), [([0, 0, 0], 64)]);
        states::clear_range();
    }

    #[test]
    fn diodes_tick_only_while_they_have_work_and_lamps_are_tracked() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:pulse_extender", vec![("facing", "north")]),
            (
                "create:pulse_extender",
                vec![("facing", "north"), ("powered", "true")],
            ),
            (
                "create:pulse_extender",
                vec![
                    ("facing", "north"),
                    ("powered", "true"),
                    ("powering", "true"),
                ],
            ),
            ("create:pulse_timer", vec![("facing", "north")]),
            ("create:rose_quartz_lamp", vec![]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        let mut w = CreateWorld::new(table);
        let (off, powered, on, timer, lamp) = (2, 3, 4, 5, 6);
        let p = [0, 64, 0];
        // Placed at rest: no tick, a check of its input one tick later.
        w.block_changed(0, p, 0, off);
        assert!(w.dims[0].diode_running.is_empty());
        assert_eq!(w.dims[0].diode_checks, [(p, crate::diode::PLACED_DELAY)]);
        // Its input on: `powering` at the next tick, then held, idle.
        w.block_changed(0, p, off, powered);
        assert_eq!(w.tick_diodes(0), [(p, true)]);
        w.block_changed(0, p, powered, on);
        assert!(w.tick_diodes(0).is_empty());
        assert!(w.dims[0].diode_running.is_empty());
        assert_eq!(w.dims[0].columns[&(0, 0)], [p], "listed once");
        // A delay set on it is saved with its column.
        assert_eq!(w.set_diode_delay(0, p, 1), Some(2));
        assert_eq!(w.set_diode_delay(0, p, 40), Some(40));
        assert_eq!(w.column_diodes(0, 0, 0), [(p, 2, 40)]);
        // Neighbours: a check two ticks out.
        w.neighbor_changed(0, p, on);
        assert_eq!(
            w.dims[0].diode_checks.last(),
            Some(&(p, crate::diode::BLOCK_DELAY))
        );
        // Broken: gone.
        w.block_changed(0, p, on, 0);
        assert!(w.dims[0].diodes.is_empty());
        // A timer at rest still runs.
        w.block_changed(0, p, 0, timer);
        assert_eq!(w.dims[0].diode_running, [p]);
        // Listed once however often its block changes; broken, unlisted.
        w.block_changed(0, p, timer, timer);
        assert_eq!(w.set_diode_delay(0, p, 7), Some(7));
        assert_eq!(w.dims[0].diode_running, [p]);
        assert!(w.dims[0].diodes[&p].running);
        w.block_changed(0, p, timer, 0);
        assert!(w.dims[0].diode_running.is_empty());
        // A lamp placed is looked at; a lamp is known by position for the signal hook.
        w.block_changed(0, [1, 64, 0], 0, lamp);
        assert_eq!(w.dims[0].lamp_placed, [[1, 64, 0]]);
        assert_eq!(w.block_at(0, [1, 64, 0]), Some(Block::RoseQuartzLamp));
        w.neighbor_changed(0, [1, 64, 0], lamp);
        assert_eq!(w.dims[0].lamp_checks, [[1, 64, 0]]);
        states::clear_range();
    }

    #[test]
    fn levers_and_detectors_are_kept_by_position_and_told_of_their_targets() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:analog_lever",
                vec![("face", "floor"), ("facing", "north")],
            ),
            (
                "create:content_observer",
                vec![("facing", "east"), ("powered", "false"), ("target", "wall")],
            ),
            (
                "create:content_observer",
                vec![("facing", "east"), ("powered", "true"), ("target", "wall")],
            ),
            (
                "create:stockpile_switch",
                vec![("facing", "north"), ("level", "0"), ("target", "floor")],
            ),
            ("create:fluid_tank", vec![]),
            ("create:cuckoo_clock", vec![("facing", "north")]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        let mut w = CreateWorld::new(table);
        let (lever, observer, powered, switch, tank, clock) = (2, 3, 4, 5, 6, 7);
        let (l, o, s) = ([0, 64, 0], [3, 64, 0], [8, 65, 0]);
        w.block_changed(0, l, 0, lever);
        assert_eq!(w.dims[0].levers.click(l, false, 0), Some(1));
        w.block_changed(0, o, 0, observer);
        w.block_changed(0, s, 0, switch);
        let d = &mut w.dims[0].detectors;
        assert_eq!(d.target_of[&o], [4, 64, 0]);
        assert_eq!(d.target_of[&s], [8, 64, 0]);
        assert_eq!(d.take_checks(0), [o, s]);
        // Its own `powered` changing is no reason to look again.
        w.block_changed(0, o, observer, powered);
        assert!(w.dims[0].detectors.take_checks(1).is_empty());
        // A tank under the switch: its contents changing reaches the switch.
        w.block_changed(0, [8, 64, 0], 0, tank);
        w.dims[0].tank_changes.push([8, 64, 0]);
        w.detectors_tanks_changed(0);
        w.detectors_contents_changed(0, [4, 64, 0]);
        assert_eq!(w.dims[0].detectors.take_checks(2), [o, s]);
        w.neighbor_changed(0, o, powered);
        assert_eq!(w.dims[0].detectors.take_checks(3), [o]);
        // A cuckoo clock turns on a shaft out of its back.
        w.block_changed(0, [20, 64, 0], 0, clock);
        assert!(w.has_shaft_towards(0, [20, 64, 0], crate::kinetics::Dir::South));
        assert!(!w.has_shaft_towards(0, [20, 64, 0], crate::kinetics::Dir::North));
        // Gone: their state goes, the lever with a signal updating its neighbours.
        w.block_changed(0, l, lever, 0);
        w.block_changed(0, o, powered, 0);
        assert_eq!(w.dims[0].levers.removed, [l]);
        assert!(!w.dims[0].detectors.target_of.contains_key(&o));
        assert!(w.dims[0].detectors.target_of.contains_key(&s));
        states::clear_range();
    }
}
