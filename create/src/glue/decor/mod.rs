//! Create's decorative and simple blocks: girders and their encased shafts, brackets on shafts,
//! cogs and pipes, the large water wheel's frame, placards, the desk bell (`bell`), the peculiar
//! and haunted bells ringing and their haunting (`bells`), the creative crate, the item hatch, clipboards, table cloths (their corners, `cloth`) and cardboard, the
//! vertical gearbox and empty blaze burner items, and Create's copper shingles and tiles
//! weathering, scraped and waxed. The rules are `create_engine::decor`'s.
//!
//! A placed clipboard opens its pages' screen (the clipboard port's, `display::clipboard`), which
//! writes them back onto it and its `written` state; sneaking or a left click picks it up.
//!
//! What blocks hold (brackets, placards' items, crates' supplies, hatches' filters, clipboards)
//! is kept per level and saved with its column as `create_decor`. Only
//! placards, bells and hatches with a timer running, and blocks that changed, are looked at each
//! tick. Clients see a bracket through its shaft's, cog's or pipe's block entity (`Bracket` in
//! the kinetic block entity's data), a placard's item through the placard's own block entity
//! (`create:placard`) and a hatch's filter through the hatch's (`create:item_hatch`), each drawn
//! by its renderer. An item hatch lets through what its filter passes (`items::filters`: a list
//! filter's items, an attribute filter's attributes, or an item). A girder item clicked on a
//! girder extends its line (`GirderPlacementHelper`). Table cloths' items and shops are the stock
//! logistics' (`stock::shop`); their placement helper is here.
//!
//! The placement helpers' targets (cloths, sails, girders, metal ladders) are offered to the
//! indicator round the crosshair (`interaction::placement`); their ghost of the block a click
//! would place is left out for want of API (`API_GAPS.md`, Client). A haunted bell's souls are
//! `souls`'.
//!
//! Lightning clears Create's copper as vanilla's: the glue gives the server's data Create's
//! chains as NeoForge's `neoforge:oxidizables` and `neoforge:waxables` block data maps
//! (`data_pack_files`), which the game's own `clearCopperOnLightningStrike` reads.
//!
//! Left out, for want of API (each in `API_GAPS.md`):
//! - The weathering neighbours of Create's copper (`WeatheringCopper` and its age): no API says
//!   which blocks are weathering copper nor their age; Create's own are known to the
//!   engine and vanilla's are read from their ids (`exposed_`, `weathered_`, `oxidized_`, never
//!   `waxed_`). A state fact for it would make it exact.
//! - What follows a placement from an item (placards and clipboards turned, a clipboard's pages,
//!   girders joining, a large water wheel's room, a peculiar bell) runs in the item's
//!   `on_block_placed_by`, which names no level: a dispenser's placement, with no placer, is left
//!   as the template made it. A level in `OnBlockPlacedByEvent` would close it.
//! - A placard shows a 3D item at half size (`PlacardRenderer`: `isGui3d`): no API reads an
//!   item model's `gui3d`, so an item that places a block stands for it.

mod bell;
mod bells;
mod brackets;
mod carried;
mod clicks;
pub(crate) mod cloth;
mod copper;
mod crates;
mod doors;
mod girders;
mod hatch;
mod items;
mod ladders;
mod placard;
mod place;
mod saved;
mod scaffolding;
mod souls;
mod tick;

/// The decor helpers' target for the placement indicator (cloths, sails, girders, metal
/// ladders): where `item` clicked at `point` on the block at `pos` would go.
pub(crate) fn helper_target(
    lv: &super::server::ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    point: [f64; 3],
    item: mod_api::ItemKey,
) -> Option<crate::glue::interaction::HelperTarget> {
    let id = item.id(&*lv.host);
    clicks::helper_target(lv, player, pos, point, &id)
        .or_else(|| ladders::helper_target(lv, player, pos, item))
}

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
    /// `ItemHatchBlockEntity.filtering`: a filter item's list and attributes, or a plain item.
    hatch_filters: HashMap<[i32; 3], create_engine::filter::Filter>,
    clipboards: HashMap<[i32; 3], ItemStack>,
    timers: Vec<([i32; 3], i64, Timer)>,
    /// Blocks of ours placed, changed or removed: position, old and new engine state.
    changes: Vec<([i32; 3], u32, u32)>,
    /// Girders and encased shafts whose neighbour changed (`updateShape`), peculiar bells to
    /// look under.
    girder_checks: Vec<[i32; 3]>,
    /// Metal scaffolding whose neighbour changed: whether it is a bottom one.
    scaffold_checks: Vec<[i32; 3]>,
    /// Metal ladders whose neighbour changed: whether anything still holds them.
    ladder_checks: Vec<[i32; 3]>,
    bell_checks: Vec<[i32; 3]>,
    /// Table cloths placed or whose neighbour changed: which sides are culled.
    cloth_checks: Vec<[i32; 3]>,
    /// Blocks whose bracket or placard item clients must see again.
    shown: Vec<[i32; 3]>,
}

thread_local! {
    static DECOR: RefCell<HashMap<usize, Dim>> = RefCell::new(HashMap::new());
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    DECOR.with_borrow_mut(|d| f(d.entry(dim).or_default()))
}

/// The clipboards placed within `range` blocks of `at` in level `dim` (their centres), with
/// their pages.
pub(crate) fn clipboards_near(dim: usize, at: [f64; 3], range: f64) -> Vec<ItemStack> {
    with(dim, |d| {
        d.clipboards
            .iter()
            .filter(|(p, _)| {
                (0..3)
                    .map(|i| (f64::from(p[i]) + 0.5 - at[i]).powi(2))
                    .sum::<f64>()
                    <= range * range
            })
            .map(|(_, s)| s.clone())
            .collect()
    })
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

pub(crate) fn face_of(d: Dir) -> BlockFace {
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

/// Gives `player` one of item `id` (what doesn't fit drops at them).
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
        &[BRACKET, bell::DING, bells::RINGING]
    }

    fn animated(&self, block: Block, pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
        (block == Block::DeskBell && bell::animated(pos, entity))
            || (bells::is_bell(block) && bells::animated(pos, entity))
    }

    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], b: Block, out: &mut Compound) {
        carried::carry_out(lv, pos, b, out);
    }

    fn draw_carried(
        &self,
        host: &dyn mod_api::ClientHost,
        block: Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        if block == Block::Placard {
            placard::draw_carried(host, event);
        } else if bracketed(block) {
            brackets::draw_carried(host, event);
        }
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
        hatch::register(r);
        doors::register(r);
        crates::register(r);
        copper::register(r);
        scaffolding::register(r);
        ladders::register(r);
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, bell::BellDraw)
        {
            crate::create_log!("[create] the desk bell's drawing part was refused: {e:?}");
        }
        // On the cloths' own block entity type (`stock::shop`), beside the shop's drawing.
        let cloth_kind = r.block_entity_type_key(crate::glue::stock::shop::TYPE);
        if let Err(e) = r.block_entity_part(&cloth_kind, cloth::ClothDraw) {
            crate::create_log!("[create] the table cloths' drawing part was refused: {e:?}");
        }
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, bells::BellDraw)
        {
            crate::create_log!("[create] the bells' drawing part was refused: {e:?}");
        }
        bells::register(r);
        souls::register(r);
    }

    fn world_closed(&self) {
        DECOR.with_borrow_mut(HashMap::clear);
        bells::forget();
        souls::forget();
    }

    /// Create's copper as NeoForge's block data maps (see the module's doc).
    fn data_pack_files(&self, files: &mut Vec<(String, Vec<u8>)>) {
        let (oxidizables, waxables) = create_engine::decor::copper_data_maps();
        files.push((
            "data_maps/neoforge:block/oxidizables".into(),
            oxidizables.into_bytes(),
        ));
        files.push((
            "data_maps/neoforge:block/waxables".into(),
            waxables.into_bytes(),
        ));
    }

    /// A held haunted bell's pulses (`HauntedBellPulser`).
    fn player_tick(&self, host: mod_api::HostSide, event: &mut mod_api::PlayerTickPostEvent) {
        if let mod_api::HostSide::Server(host) = host {
            souls::player_tick(host, event.player);
        }
    }

    fn particle_providers(&self, providers: &mut dyn mod_api::ParticleProviders) {
        providers.register(souls::SOUL.to_owned(), std::sync::Arc::new(souls::Soul));
        providers.register(
            souls::SOUL_BASE.to_owned(),
            std::sync::Arc::new(souls::SoulBase),
        );
    }

    fn client_reset(&self) {
        doors::forget();
        bell::forget();
        bells::client_forget();
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
        if n.is_some_and(cloth::is_cloth) {
            with(dim, |d| d.cloth_checks.push(pos));
        }
        if let Some((b, info)) = block_of(new)
            && bells::is_bell(b)
        {
            let was = block_of(old)
                .filter(|(o, _)| *o == b)
                .map(|(_, i)| i.powered);
            bells::changed(dim, pos, was, info.powered);
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
            Some(b) if cloth::is_cloth(b) => with(dim, |d| d.cloth_checks.push(pos)),
            _ if ladders::is_ladder(state) => with(dim, |d| d.ladder_checks.push(pos)),
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
        bells::tick(lv);
        souls::tick(lv);
    }

    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        clicks::left_click(host, event)
    }

    /// A girder takes the wrench as its own (`GirderBlock.useItemOn`: its top or bottom bracket,
    /// never turned), and an encased shaft wrenched turns back into its girder.
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        player: PlayerRef,
        pos: [i32; 3],
        block: Block,
        _clicked: Dir,
        point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if sneaking {
            return false;
        }
        match block {
            Block::MetalGirder => {
                girders::wrench(lv, pos, point);
                true
            }
            Block::MetalGirderEncasedShaft => {
                girders::unencase(lv, player, pos);
                true
            }
            _ => false,
        }
    }
}

/// `ItemHatchBlockEntity.filtering`: the filter of the hatch at `pos`, if one.
pub(crate) fn hatch_filter(dim: usize, pos: [i32; 3]) -> Option<create_engine::filter::Filter> {
    with(dim, |d| d.hatch_filters.get(&pos).cloned())
}

/// `FilteringBehaviour.setFilter` on the hatch at `pos`.
pub(crate) fn set_hatch_filter(
    dim: usize,
    pos: [i32; 3],
    filter: Option<create_engine::filter::Filter>,
) {
    with(dim, |d| {
        d.shown.push(pos);
        match filter {
            Some(f) => d.hatch_filters.insert(pos, f),
            None => d.hatch_filters.remove(&pos),
        }
    });
    super::persist::mark_pos_unsaved(dim, pos);
}

/// `FilteringBehaviour.setFilter` on the creative crate at `pos`: its supply.
pub(crate) fn set_crate_supply(dim: usize, pos: [i32; 3], item: Option<String>) {
    with(dim, |d| match item {
        Some(item) => d.supplies.insert(pos, item),
        None => d.supplies.remove(&pos),
    });
    super::persist::mark_pos_unsaved(dim, pos);
}

/// The creative crate's supply at `pos` (by level).
pub(crate) fn crate_supply_of(dim: usize, pos: [i32; 3]) -> Option<String> {
    with(dim, |d| d.supplies.get(&pos).cloned())
}

/// A clipboard of no pages.
pub(crate) fn clipboard_stack(host: &dyn Host) -> Option<ItemStack> {
    mod_api::Identifier::parse("create:clipboard")
        .ok()
        .and_then(|i| host.item_key(&i))
        .map(ItemStack::of)
}

/// The clipboard placed at `pos` (`ClipboardBlockEntity.components()` on its stack).
pub(crate) fn placed_clipboard(dim: usize, pos: [i32; 3]) -> Option<ItemStack> {
    with(dim, |d| d.clipboards.get(&pos).cloned())
}

/// `ClipboardEditPacket` on a placed clipboard (`setComponents`, `onEditedBy`): its stack, and
/// its `written` state following whether it has pages (`updateWrittenState`).
pub(crate) fn set_placed_clipboard(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: ItemStack,
    written: bool,
) {
    let dim = lv.dim;
    with(dim, |d| d.clipboards.insert(pos, stack));
    super::persist::mark_pos_unsaved(dim, pos);
    let value = if written { "true" } else { "false" };
    if lv.property(pos, "written").as_deref() != Some(value) {
        lv.set_property(pos, "written", value, super::server::update_all());
    }
}

/// `BottomlessItemHandler` of a creative crate at `pos`: Some(supply) when one is there.
pub fn crate_supply(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Option<String>> {
    let (b, _) = block_of(lv.block(pos))?;
    (b == Block::CreativeCrate).then(|| with(lv.dim, |d| d.supplies.get(&pos).cloned()))
}
