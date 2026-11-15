//! Create's decorative and simple blocks: girders and their encased shafts, brackets on shafts,
//! cogs and pipes, the large water wheel's frame, placards, the desk bell and the peculiar bell's
//! haunting, the creative crate, the item hatch, clipboards, table cloths and cardboard, the
//! vertical gearbox and empty blaze burner items, and Create's copper shingles and tiles
//! weathering, scraped and waxed. The rules are `create_engine::decor`'s.
//!
//! What blocks hold (brackets, placards' items, crates' supplies, hatches' filters, clipboards)
//! is kept per level and saved with its column as `create_decor`, in the legacy layout. Only
//! placards, bells and hatches with a timer running, and blocks that changed, are looked at each
//! tick. Clients see a bracket through its shaft's, cog's or pipe's block entity (`Bracket` in
//! the kinetic block entity's data) and a placard's item through the placard's own block entity
//! (`create:placard`), each drawn by its renderer.
//!
//! Left out, for want of API (each in `HORIZON_API.md`):
//! - The clipboard's pages screen on a placed clipboard (legacy `create_clipboard::open`, and
//!   `set_clipboard_stack` after an edit): the clipboard's screen is the clipboard port's; the
//!   click here is taken so the lever template does not flip.
//! - The weathering neighbours of Create's copper (`WeatheringCopper` and its age, legacy
//!   `behaviours().data(state)`): no API says which blocks are weathering copper nor their age;
//!   Create's own are known to the engine and vanilla's are read from their ids (`exposed_`,
//!   `weathered_`, `oxidized_`, never `waxed_`). A state fact for it would make it exact.
//! - Lightning clearing Create's copper (`LightningBolt.clearCopperOnLightningStrike`): the
//!   `lightning_strike` hook names no level, and the game's own clearing steps copper by the
//!   `neoforge:oxidizables` data map or `minecraft:` names. Create's `neoforge:oxidizables` and
//!   `neoforge:waxables` maps in its data (the data generator's) would close it with no glue.
//! - What follows a placement from an item (placards and clipboards turned, a clipboard's pages,
//!   girders joining, a large water wheel's room, a peculiar bell) runs in the item's
//!   `on_block_placed_by`, which names no level: a dispenser's placement, with no placer, is left
//!   as the template made it. A level in `OnBlockPlacedByEvent` would close it.
//! - A placard shows a 3D item at half size (`PlacardRenderer`: `isGui3d`): no API reads an
//!   item model's `gui3d`, so an item that places a block stands for it.

mod brackets;
mod carried;
mod clicks;
mod copper;
mod crates;
mod doors;
mod girders;
mod items;
mod placard;
mod place;
mod saved;
mod scaffolding;
mod tick;

use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::decor::Bracket;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use mod_api::{
    BlockFace, BlockState, Compound, Direction, Flow, Hand, Host, HostBlockStates, ItemStack,
    LeftClickBlockEvent, PlayerRef, Registrar, RegistrationContext, ServerHost, SoundSource,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of a column's decorative state in its saved data.
const KEY: &str = "create_decor";

/// The kinetic block entity's data key a bracket is shown under (its state text).
pub const BRACKET: &str = "Bracket";

/// A timer running on a block: due at a game time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timer {
    /// `PlacardBlockEntity.poweredTicks` ran out.
    Placard,
    /// `DeskBellBlockEntity.blockStateTimer` ran out.
    Bell,
    /// `ItemHatchBlock.tick`: it closes.
    Hatch,
}

/// One level's decorative state.
#[derive(Default)]
struct Dim {
    brackets: HashMap<[i32; 3], Bracket>,
    placards: HashMap<[i32; 3], ItemStack>,
    supplies: HashMap<[i32; 3], String>,
    hatch_filters: HashMap<[i32; 3], String>,
    clipboards: HashMap<[i32; 3], ItemStack>,
    timers: Vec<([i32; 3], i64, Timer)>,
    /// Blocks of ours placed, changed or removed: position, old and new engine state.
    changes: Vec<([i32; 3], u32, u32)>,
    /// Girders and encased shafts whose neighbour changed (`updateShape`), peculiar bells to
    /// look under.
    girder_checks: Vec<[i32; 3]>,
    /// Metal scaffolding whose neighbour changed: whether it is a bottom one.
    scaffold_checks: Vec<[i32; 3]>,
    bell_checks: Vec<[i32; 3]>,
    /// Blocks whose bracket or placard item clients must see again.
    shown: Vec<[i32; 3]>,
}

thread_local! {
    static DECOR: RefCell<HashMap<usize, Dim>> = RefCell::new(HashMap::new());
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    DECOR.with_borrow_mut(|d| f(d.entry(dim).or_default()))
}

/// The block and state facts of engine state `id` (server side).
fn block_of(id: u32) -> Option<(Block, StateInfo)> {
    with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
}

/// The Create block `state` is, from its key alone (either side).
fn create_block(host: &dyn Host, state: BlockState) -> Option<Block> {
    state
        .block()
        .id(host)
        .strip_prefix("create:")
        .and_then(Block::from_name)
}

/// Blocks a bracket goes on (`BracketedBlockEntityBehaviour`: shafts and cogs,
/// `AbstractSimpleShaftBlock`; fluid pipes).
fn bracketed(b: Block) -> bool {
    matches!(
        b,
        Block::Shaft | Block::Cogwheel | Block::LargeCogwheel | Block::FluidPipe
    )
}

/// The blocks whose changes this module looks at (besides bracketed ones holding a bracket).
fn ours(b: Block) -> bool {
    matches!(
        b,
        Block::MetalGirder
            | Block::MetalGirderEncasedShaft
            | Block::Placard
            | Block::Clipboard
            | Block::CreativeCrate
            | Block::ItemHatch
            | Block::PeculiarBell
    )
}

fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

fn dir(d: Direction) -> Dir {
    Dir::ALL[d as usize]
}

fn face_of(d: Dir) -> BlockFace {
    match d {
        Dir::Down => BlockFace::Down,
        Dir::Up => BlockFace::Up,
        Dir::North => BlockFace::North,
        Dir::South => BlockFace::South,
        Dir::West => BlockFace::West,
        Dir::East => BlockFace::East,
    }
}

fn flag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn dir_named(v: &str) -> Option<Dir> {
    Dir::ALL
        .into_iter()
        .find(|d| super::kinetics::dir_name(*d) == v)
}

/// One property of `state`.
fn prop(host: &dyn Host, state: BlockState, key: &str) -> Option<String> {
    state
        .property(&host.block_states(), key)
        .ok()
        .flatten()
        .map(str::to_owned)
}

/// `state` with `props` set; None when it lacks one or a value.
fn with_props(host: &dyn Host, state: BlockState, props: &[(&str, &str)]) -> Option<BlockState> {
    let facts = host.block_states();
    props
        .iter()
        .try_fold(state, |s, (k, v)| s.with(&facts, k, v).ok())
}

/// The state of block `block` with `props` (`BlockHelper.copyProperties` onto its default).
fn state_with(host: &dyn Host, block: Block, props: &[(&str, &str)]) -> Option<BlockState> {
    let base = host.block_state_default(states::key(block))?;
    with_props(host, base, props)
}

/// The state text and name of `state`: `create:shaft[axis=y]`.
fn state_text(host: &dyn Host, state: BlockState) -> String {
    host.block_state_string(state).unwrap_or_default()
}

impl ServerLevel<'_> {
    /// Plays `sound` at the centre of `pos`, as a block's (`SoundSource.BLOCKS`).
    fn block_sound(&mut self, sound: &str, pos: [i32; 3], volume: f32, pitch: f32) {
        self.level.play_sound(
            &mut *self.host,
            sound,
            center(pos),
            SoundSource::Blocks,
            volume,
            pitch,
        );
    }

    /// `Block.popResource`: `stack` dropped at `pos`.
    fn drop_at(&mut self, pos: [i32; 3], stack: ItemStack) {
        let _ = self.level.pop_resource(&mut *self.host, pos, stack);
    }

    /// Sets the state at `pos` with every update (`Block.UPDATE_ALL`).
    fn put(&mut self, pos: [i32; 3], state: BlockState) -> bool {
        self.set_state(pos, state, super::server::update_all())
    }
}

/// Whether `player` plays in creative (`getAbilities().instabuild`).
fn creative(host: &dyn Host, player: PlayerRef) -> bool {
    player.is_creative(host) == Some(true)
}

/// Whether `player` is sneaking (`isShiftKeyDown`).
fn sneaking(host: &dyn Host, player: PlayerRef) -> bool {
    player.entity().is_shift_key_down(host) == Some(true)
}

/// One of the stack in `player`'s `hand` used up (not in creative).
fn use_one(host: &mut dyn ServerHost, player: PlayerRef, hand: Hand, stack: &ItemStack) {
    if creative(&*host, player) {
        return;
    }
    if let Some(living) = host.living(player.entity()) {
        let _ = living.set_hand_item(host, hand, stack.clone().shrink(1));
    }
}

/// `count` of item `id`, if the game has it.
fn stack_of(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    let key = host.item_key(&mod_api::Identifier::parse(id).ok()?)?;
    ItemStack::new(key, count)
}

/// Gives `player` one of item `id` (`native_give_items`: what doesn't fit drops at them).
fn give(host: &mut dyn ServerHost, player: PlayerRef, id: &str) {
    if let Some(stack) = stack_of(&*host, id, 1) {
        let _ = player.give_stack(host, stack);
    }
}

/// The id of the level `player` is in.
fn player_dimension(host: &dyn Host, player: PlayerRef) -> Option<String> {
    player.level(host)?.dimension().id(host)
}

pub struct Decor;

impl Subsystem for Decor {
    fn synced_keys(&self) -> &'static [&'static str] {
        &[BRACKET]
    }

    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], b: Block, out: &mut Compound) {
        carried::carry_out(lv, pos, b, out);
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        b: Block,
        data: &Compound,
        turn: create_engine::contraption::Transform,
    ) {
        carried::carry_in(lv, pos, b, data, turn);
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        clicks::register(r);
        place::register(r);
        items::register(r);
        brackets::register(r);
        placard::register(r);
        doors::register(r);
        crates::register(r);
        copper::register(r);
        scaffolding::register(r);
    }

    fn world_closed(&self) {
        DECOR.with_borrow_mut(HashMap::clear);
    }

    fn client_reset(&self) {
        doors::forget();
    }

    /// A Create block changed: ours are looked at after the level's tick. Shafts, cogs and pipes
    /// only when they hold a bracket.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (o, n) = (block_of(old).map(|x| x.0), block_of(new).map(|x| x.0));
        let mine = |b: Option<Block>| b.is_some_and(ours);
        let bracket = o.is_some_and(bracketed)
            && DECOR.with_borrow(|d| d.get(&dim).is_some_and(|d| d.brackets.contains_key(&pos)));
        if mine(o) || mine(n) || bracket {
            with(dim, |d| d.changes.push((pos, old, new)));
        }
    }

    /// Girders and encased shafts shape themselves again (`updateShape`), a peculiar bell looks
    /// at what is under it, metal scaffolding at what holds it.
    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        match block_of(state).map(|(b, _)| b) {
            Some(Block::MetalGirder | Block::MetalGirderEncasedShaft) => {
                with(dim, |d| d.girder_checks.push(pos))
            }
            Some(Block::PeculiarBell) => with(dim, |d| d.bell_checks.push(pos)),
            Some(b) if scaffolding::BLOCKS.contains(&b) => {
                with(dim, |d| d.scaffold_checks.push(pos))
            }
            _ => {}
        }
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        saved::load(lv, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        saved::save(lv, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        let outside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] != chunk;
        with(dim, |d| {
            d.brackets.retain(|p, _| outside(p));
            d.placards.retain(|p, _| outside(p));
            d.clipboards.retain(|p, _| outside(p));
            d.supplies.retain(|p, _| outside(p));
            d.hatch_filters.retain(|p, _| outside(p));
            d.timers.retain(|t| outside(&t.0));
        });
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick::tick(lv);
    }

    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        clicks::left_click(host, event)
    }
}

/// `BottomlessItemHandler` of a creative crate at `pos`: Some(supply) when one is there.
pub fn crate_supply(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Option<String>> {
    let (b, _) = block_of(lv.block(pos))?;
    (b == Block::CreativeCrate).then(|| with(lv.dim, |d| d.supplies.get(&pos).cloned()))
}
