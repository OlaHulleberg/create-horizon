//! Create's package logistics: packagers, frogports, postboxes' packages and chain conveyors.
//! The rules are `create_engine`'s `package`, `packager`, `port` and `chain_conveyor`; their
//! state is typed, per level, on the server thread, saved with its column as
//! `create_packages`; only packagers and frogports with something to do and conveyors
//! carrying packages tick. A package's address, contents and order data are kept in its
//! stack's `minecraft:custom_data` (`body`). Clients draw the conveyors' packages from the
//! `create:chain` messages, round the wheel and gliding along each chain, every frame.
//!
//! The inventories a packager packs from and unpacks into, and a frogport's below it, are any
//! block's `ITEMS` capability through the face toward it (`inv`).
//!
//! Conveyors draw their wheel, shaft, guards and chains (`ChainConveyorRenderer`; a turning
//! conveyor's chains scroll a pixel at a time, through pre-shifted chain models; Create's plainer
//! chain past `MIP_DISTANCE` is not drawn, as a kept block entity is not drawn again when only the
//! camera moves) and are joined with
//! chains (`ChainConveyorConnectionHandler`, the first conveyor kept on the server). A frogport is
//! aimed at a chain point with its item (`frog_target`) and draws its body, head, tongue and
//! catch (`frog_draw`).
//!
//! Other blocks' automation (hoppers, funnels, chutes) reaches a packager's box and a port's
//! packages through block entity slots mirroring them (`automation`).
//!
//! Left out, for want of API:
//! - A frogport is not told when the inventory below it changes (`contents_changed` is a Part
//!   of the changed block only, with no hook for every container's `setChanged`): a frogport
//!   with an inventory below looks at it on Create's own lazy tick, every 10 ticks, instead of
//!   at once.
//! - A dropped package is an item, not Create's package entity (`PackageEntity`, API_GAPS.md).

mod automation;
pub mod body;
mod box_draw;
mod frog_draw;
mod frog_target;
mod inv;
pub(crate) mod port_screen;
mod station_ports;

use super::server::{ServerLevel, UPDATE_CLIENTS};
use super::states;
use super::subsystem::Subsystem;
use super::world::{level_index, with_world};
use super::{net, persist, stock};
use create_engine::Block;
use create_engine::chain_conveyor::{self as chain, Drawn, Parcel};
use create_engine::kinetics::Dir;
use create_engine::package;
use create_engine::port::{Out, Packaging, Target};
use inv::Inv;
use mod_api::{
    AnalogOutputEvent, BlockEntityPart, BlockEntityRenderBoundingBoxEvent, BlockEntitySubmitEvent,
    BlockPart, ClientHost, Command, CommandArgumentValue, CommandSource, Compound, Direction, Flow,
    Hand, HasAnalogOutputEvent, InteractionResult, ItemDisplayContext, ItemKey, ItemStack,
    LevelHost, NeighbourUpdate, PlayerRef, RegisterCommandsEvent, Registrar, RegistrationContext,
    RunModCommandEvent, ServerHost, SoundSource, Tag, Text, UseItemOnEvent,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub type Pk = Packaging<ItemStack>;

/// A block of ours placed or removed: level, position, was, is.
type Change = (usize, [i32; 3], Option<Block>, Option<Block>);

/// The key the column's data keeps this under.
const KEY: &str = "create_packages";

/// What the client draws of one conveyor: when it got them (on the animation clock, set at the
/// first frame) and the packages.
struct Shown {
    since: Option<f64>,
    drawn: Vec<(Drawn, Option<ItemStack>)>,
}

thread_local! {
    /// Postboxes players placed, joined to their chosen station at the next tick.
    static POSTBOX_PLACED: RefCell<Vec<(usize, [i32; 3], PlayerRef)>> = const { RefCell::new(Vec::new()) };
    /// Each level's package logistics, by level index, made when first needed.
    static PACKAGES: RefCell<Option<Vec<Pk>>> = const { RefCell::new(None) };
    /// Blocks of ours placed or removed, handled at the level's tick.
    static CHANGES: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    /// Conveyors removed since the last send: clients clear what they drew there.
    static GONE: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    /// The largest stack of each item a port held, for what comparators read of it.
    static MAXES: RefCell<HashMap<ItemKey, u32>> = RefCell::new(HashMap::new());
    /// The client's conveyors' packages, by conveyor.
    static SHOWN: RefCell<HashMap<[i32; 3], Shown>> = RefCell::new(HashMap::new());
    /// Conveyors whose packages are all gone: drawn once more without them.
    static CLEARED: RefCell<Vec<[i32; 3]>> = const { RefCell::new(Vec::new()) };
    /// Whether this client asked the server for what it draws (`sync`).
    static SYNCED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` on level `dim`'s package logistics.
pub fn with<R>(dim: usize, f: impl FnOnce(&mut Pk) -> R) -> R {
    PACKAGES.with_borrow_mut(|p| {
        let levels = p.get_or_insert_with(Vec::new);
        if levels.len() <= dim {
            levels.resize_with(dim + 1, Pk::default);
        }
        f(&mut levels[dim])
    })
}

fn present() -> bool {
    PACKAGES.with_borrow(Option::is_some)
}

/// The packages going round the chain conveyor at `pos` (`getLoopingPackages`), none for any
/// other block.
pub(crate) fn looping_packages(dim: usize, pos: [i32; 3]) -> Vec<ItemStack> {
    if !present() {
        return Vec::new();
    }
    with(dim, |pk| {
        pk.chains.conveyors.get(&pos).map_or_else(Vec::new, |c| {
            c.looping.iter().map(|p| p.parcel.stack.clone()).collect()
        })
    })
}

fn ours(b: Block) -> bool {
    matches!(
        b,
        Block::Packager | Block::Repackager | Block::PackageFrogport | Block::ChainConveyor
    ) || create_engine::dye::is_postbox(b)
}

/// A packager or a re-packager.
fn is_packager(b: Block) -> bool {
    matches!(b, Block::Packager | Block::Repackager)
}

/// A frogport or a postbox.
fn is_port(b: Block) -> bool {
    b == Block::PackageFrogport || create_engine::dye::is_postbox(b)
}

pub fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

pub fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

pub fn direction(d: Dir) -> Direction {
    Direction::ALL[d as usize]
}

fn now(lv: &ServerLevel<'_>) -> u64 {
    lv.game_time().max(0) as u64
}

/// A package as the rules carry it: its stack and its address.
pub fn parcel(stack: ItemStack) -> Parcel<ItemStack> {
    Parcel {
        address: body::address(&stack),
        stack,
    }
}

/// `Block.popResource` / `PackageEntity.fromItemStack`: the stack drops as an item at `at`.
fn drop_stack(lv: &mut ServerLevel<'_>, at: [f64; 3], stack: ItemStack) {
    let _ = lv
        .level
        .spawn_item(&mut *lv.host, at[0], at[1], at[2], stack, 10);
}

/// The kinetic network's chains of the conveyor at `pos`: the conveyors it joins turn with it.
fn sync_far(dim: usize, pos: [i32; 3]) {
    let to: Vec<[i32; 3]> = with(dim, |pk| {
        pk.chains
            .conveyors
            .get(&pos)
            .map(|c| c.connections.iter().map(|k| add(pos, *k)).collect())
            .unwrap_or_default()
    });
    with_world(|w| w.dims[dim].kinetics.set_far(pos, to));
}

fn speed_of(dim: usize, pos: [i32; 3]) -> f32 {
    with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
}

/// `minecraft:happy_villager`, for a postbox trading with a train.
static HAPPY: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

/// A postbox used on the station at `station` by `player` (`onUse`): chosen for the next
/// postbox placed.
pub(crate) fn choose_station(player: PlayerRef, dim: usize, station: [i32; 3]) {
    station_ports::choose(player, dim, station);
}

/// The stations a train's package instruction may go to (`station_ports::package_stations`).
pub(crate) fn package_stations(
    lv: &ServerLevel<'_>,
    fetch: Option<&str>,
    aboard: &[ItemStack],
) -> Vec<[i32; 3]> {
    station_ports::package_stations(lv, fetch, aboard)
}

/// `minecraft:block`, for the rope's dust as a frogport catches.
static BLOCK_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

/// `minecraft:dust`, for a frogport's line to its point (`animateConnection`).
static DUST_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

pub struct Packages;

impl Subsystem for Packages {
    /// A chain conveyor carrying packages is drawn every frame, as they move along it (its
    /// drawing is otherwise kept, and drawn anew only when its data changes).
    fn animated(&self, block: Block, pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
        if block == Block::PackageFrogport {
            return frog_draw::animated(&entity.nbt);
        }
        if block != Block::ChainConveyor {
            return box_draw::animated(block, pos, entity);
        }
        let carrying = matches!(
            super::kinetics::synced(&entity.nbt, CARRYING),
            Some(Tag::Byte(b)) if *b != 0
        );
        // A turning conveyor's chains scroll (`renderChain`'s `animation`).
        let turning = super::kinetics::speeds_of(&entity.nbt)[0] != 0.0
            && matches!(entity.nbt.get(&CHAINS.into()), Some(Tag::IntArray(v)) if v.len() > 1);
        carrying
            || turning
            || SHOWN.with_borrow(|s| s.contains_key(&pos))
            || CLEARED.with_borrow_mut(|c| {
                let was = c.contains(&pos);
                c.retain(|p| *p != pos);
                was
            })
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = BLOCK_PARTICLE.set(r.particle_key("minecraft:block"));
        let _ = DUST_PARTICLE.set(r.particle_key("minecraft:dust"));
        let _ = HAPPY.set(r.particle_key("minecraft:happy_villager"));
        automation::register(r);
        for &b in Block::ALL.iter().filter(|&&b| is_packager(b) || is_port(b)) {
            r.block_part(states::key(b), PackageBlock);
        }
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, box_draw::BoxDraw)
        {
            crate::create_log!("[create] the packagers' drawing part was refused: {e:?}");
        }
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, frog_draw::FrogDraw)
        {
            crate::create_log!("[create] the frogport's drawing part was refused: {e:?}");
        }
        let frogport = r.item_key("create:package_frogport");
        r.item_part(frogport, FrogportItem);
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, ConveyorDraw)
        {
            crate::create_log!("[create] the chain conveyor's drawing part was refused: {e:?}");
        }
    }

    fn standalone_models(&self, models: &mut Vec<String>) {
        models.extend(CONVEYOR_MODELS.iter().map(|m| (*m).to_owned()));
        models.extend((1..16).map(|k| format!("create:block/chain_conveyor/chain_{k}")));
        models.extend(RIGGING_MODELS.iter().map(|m| (*m).to_owned()));
        models.extend(frog_draw::models());
    }

    /// `ChainConveyorConnectionHandler.onItemUsedOnBlock`: a chain conveyor clicked with chains
    /// (taken on both sides; the server keeps the first conveyor chosen).
    fn right_click_block(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        let click = event.click;
        let id = |h: &dyn mod_api::Host| click.stack.map(|s| s.item().id(h));
        let chains = match &host {
            mod_api::HostSide::Client(h) => id(&**h),
            mod_api::HostSide::Server(h) => id(&**h),
        };
        // `PackagePortTargetSelectionHandler.onUse`: a postbox used on a station chooses it.
        if chains
            .as_deref()
            .is_some_and(|i| i.starts_with("create:") && i.ends_with("_postbox"))
        {
            if let mod_api::HostSide::Client(h) = host
                && click.hand == Hand::Main
                && frog_target::choose_station(h, click.pos)
            {
                *event.cancellation_result = InteractionResult::Success;
                return Flow::Cancel;
            }
            return Flow::Continue;
        }
        if chains.as_deref() == Some("create:package_frogport") {
            // `ChainConveyorInteractionHandler.onUse`: a chain point under the crosshair.
            if let mod_api::HostSide::Client(h) = host
                && click.hand == Hand::Main
                && frog_target::use_on_chain(h)
            {
                *event.cancellation_result = InteractionResult::Success;
                return Flow::Cancel;
            }
            return Flow::Continue;
        }
        if chains.as_deref() != Some("minecraft:iron_chain") {
            return Flow::Continue;
        }
        match host {
            mod_api::HostSide::Client(host) => {
                let Some(level) = host.level(host.dimension()) else {
                    return Flow::Continue;
                };
                let conveyor = level
                    .block_state(&*host, click.pos)
                    .and_then(states::id)
                    .and_then(block_of)
                    == Some(Block::ChainConveyor);
                let may = host
                    .player(click.player)
                    .is_some_and(|p| p.may_build(&*host) == Some(true));
                if !conveyor || !may {
                    return Flow::Continue;
                }
            }
            mod_api::HostSide::Server(host) => {
                let Some(player) = host.player(click.player) else {
                    return Flow::Continue;
                };
                let dimension = click.dimension.key().id().to_string();
                let Some(mut lv) = ServerLevel::new(host, &dimension) else {
                    return Flow::Continue;
                };
                if block_of(lv.block(click.pos)) != Some(Block::ChainConveyor)
                    || player.may_build(&*lv.host) != Some(true)
                {
                    return Flow::Continue;
                }
                if click.hand == Hand::Main {
                    chain_clicked(&mut lv, player, click.pos);
                }
            }
        }
        *event.cancellation_result = InteractionResult::Consume;
        Flow::Cancel
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            CHAINS,
            CARRYING,
            frog_draw::FROG,
            box_draw::TRAY,
            box_draw::FLAG,
        ]
    }

    fn world_closed(&self) {
        frog_target::reset_server();
        automation::reset();
        station_ports::reset();
        frog_draw::reset();
        box_draw::reset();
        PACKAGES.with_borrow_mut(|p| *p = None);
        CHANGES.with_borrow_mut(Vec::clear);
        GONE.with_borrow_mut(Vec::clear);
        MAXES.with_borrow_mut(HashMap::clear);
        CHAINS_SENT.with_borrow_mut(HashMap::clear);
    }

    /// Ours placed or gone are handled at the level's tick.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (o, n) = (
            block_of(old).filter(|b| ours(*b)),
            block_of(new).filter(|b| ours(*b)),
        );
        if o == n {
            return;
        }
        CHANGES.with_borrow_mut(|c| c.push((dim, pos, o, n)));
    }

    /// A packager reads its power at the level's tick (`PackagerBlock.neighborChanged`); a
    /// frogport looks at the inventory below.
    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        match block_of(state) {
            Some(Block::Packager | Block::Repackager) => with(dim, |pk| pk.redstone.push(pos)),
            Some(Block::PackageFrogport) => with(dim, |pk| pk.wake(pos)),
            _ => {}
        }
    }

    /// A chain conveyor's packages move at its new speed, and the frogports on it register
    /// again (a chain running the other way moves them).
    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], speed: f32) {
        if !present() {
            return;
        }
        with(lv.dim, |pk| {
            if pk.chains.conveyors.contains_key(&pos) {
                pk.chains.set_speed(pos, speed);
                pk.conveyor_changed(pos);
            }
        });
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        level_tick(lv);
        let dim = lv.dim;
        let placed: Vec<([i32; 3], PlayerRef)> = POSTBOX_PLACED.with_borrow_mut(|p| {
            let (mine, rest): (Vec<_>, Vec<_>) =
                std::mem::take(p).into_iter().partition(|x| x.0 == dim);
            *p = rest;
            mine.into_iter().map(|x| (x.1, x.2)).collect()
        });
        for (pos, player) in placed {
            // The client's choice ends (as a frogport's).
            net::send_to_player(lv.host, player, frog_target::CHANNEL, &[]);
            if !station_ports::placed(lv, player, pos) {
                let _ = player.action_bar(
                    lv.host,
                    Text::translate("create.gui.package_port.not_targeting_anything", Vec::new()),
                );
            }
        }
        station_ports::level_tick(lv);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        if let Some(Tag::Compound(own)) = data.get(&KEY.into()) {
            chunk_load(lv, own);
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        chunk_data_save(lv, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        chunk_unload(dim, chunk);
    }

    /// A client joining asks for what it draws (`sync`): every conveyor carrying packages in
    /// its level. Not this subsystem's channel alone: the others answer it too.
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel == port_screen::CHANNEL {
            port_screen::client_message(host, player, data);
            return true;
        }
        if channel == frog_target::CHANNEL {
            frog_target::client_message(host, player, data);
            return true;
        }
        if channel != create_engine::client::CHANNEL || data != b"sync" || !present() {
            return false;
        }
        let Some(dim) = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()))
        else {
            return false;
        };
        let conveyors: Vec<[i32; 3]> = with(dim, |pk| {
            pk.chains
                .conveyors
                .iter()
                .filter(|(_, c)| !c.is_empty())
                .map(|(p, _)| *p)
                .collect()
        });
        send_chains(host, &[player], dim, &conveyors);
        false
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        if port_screen::server_message(host, channel, data) || frog_target::server_message(channel)
        {
            return true;
        }
        if channel != chain::CHANNEL {
            return false;
        }
        if let Some((pos, drawn)) = chain::decode(data) {
            SHOWN.with_borrow_mut(|s| {
                if drawn.is_empty() {
                    // Drawn once more without them, then kept.
                    if s.remove(&pos).is_some() {
                        CLEARED.with_borrow_mut(|c| c.push(pos));
                    }
                } else {
                    let drawn = drawn.into_iter().map(|d| (d, None)).collect();
                    s.insert(pos, Shown { since: None, drawn });
                }
            });
        }
        true
    }

    /// Once in each world, the client asks for what it draws.
    fn client_tick(&self, host: &mut dyn ClientHost) {
        port_screen::tick(host);
        frog_target::client_tick(host);
        frog_draw::client_tick(host);
        if !SYNCED.get() && net::send_to_server(host, create_engine::client::CHANNEL, b"sync") {
            SYNCED.set(true);
        }
    }

    fn screen_render_post(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        port_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        port_screen::screen_mouse_pressed(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        port_screen::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        port_screen::screen_char_typed(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenClosingEvent<'_>) {
        port_screen::screen_closing(host, event);
    }

    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        frog_target::render_level_stage(host, event);
    }

    /// `PackagePortPlacementPacket`: a frogport placed by a player takes the point chosen.
    fn entity_place(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::EntityPlaceEvent,
    ) -> Flow {
        let block = states::id(event.state).and_then(block_of);
        // `flushSettings` for a postbox: the station chosen, or nothing chosen.
        if block.is_some_and(create_engine::dye::is_postbox)
            && let Some(placer) = event.placer
            && let Some(player) = host
                .players()
                .server_players()
                .into_iter()
                .find(|p| p.entity() == placer)
            && let Some(dimension) = player.dimension(&*host).map(|d| d.to_string())
            && let Some(mut lv) = ServerLevel::new(host, &dimension)
        {
            let pos = [event.pos.x, event.pos.y, event.pos.z];
            // The port is known at the level's next tick (`block_moved`): joined then.
            POSTBOX_PLACED.with_borrow_mut(|p| p.push((lv.dim, pos, player)));
            let _ = &mut lv;
            return Flow::Continue;
        }
        let frog = block == Some(Block::PackageFrogport);
        if let (true, Some(placer)) = (frog, event.placer)
            && let Some(player) = host
                .players()
                .server_players()
                .into_iter()
                .find(|p| p.entity() == placer)
            && let Some(dim) = player
                .dimension(&*host)
                .and_then(|d| level_index(&d.to_string()))
        {
            let pos = [event.pos.x, event.pos.y, event.pos.z];
            frog_target::placed(host, player, dim, pos);
        }
        Flow::Continue
    }

    fn client_reset(&self) {
        frog_target::reset_client();
        port_screen::reset();
        SHOWN.with_borrow_mut(HashMap::clear);
        CLEARED.with_borrow_mut(Vec::clear);
        SYNCED.set(false);
        box_draw::client_reset();
    }

    /// Our debug commands, standing in for the clipboard (`address`; signs are read), the frogport
    /// item's selection (`frogport`) and the chain item (`chain`, `unchain`).
    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        for usage in [
            "create_debug address <pos:block_pos> <address:text>",
            "create_debug frogport <pos:block_pos> <target:text>",
            "create_debug chain <from:block_pos> <to:block_pos>",
            "create_debug unchain <from:block_pos> <to:block_pos>",
            "create_debug packages <pos:block_pos>",
        ] {
            event.commands.push(Command::op(usage, 2));
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let Some(verb) = debug_verb(
            event,
            &["address", "frogport", "chain", "unchain", "packages"],
        ) else {
            return Flow::Continue;
        };
        let here = event.command.position;
        let Some(dimension) = event.command.dimension.id(&*host) else {
            return Flow::Cancel;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Cancel;
        };
        let dim = lv.dim;
        let pos = |name: &str| match event.command.argument(name) {
            Some(CommandArgumentValue::BlockPos(p)) => Some(*p),
            _ => None,
        };
        let text = |name: &str| match event.command.argument(name) {
            Some(CommandArgumentValue::String(s)) => s.clone(),
            _ => String::new(),
        };
        let line = match (verb, pos("pos")) {
            ("address", Some(p)) => set_address(dim, p, text("address").trim()),
            ("frogport", Some(p)) => {
                let target = text("target");
                let words: Vec<&str> = target.split_whitespace().collect();
                set_frogport(dim, p, &words, here)
            }
            ("packages", Some(p)) => describe(&lv, dim, p),
            ("chain" | "unchain", _) => match (pos("from"), pos("to")) {
                (Some(a), Some(b)) => chain_command(&mut lv, a, b, verb == "chain"),
                _ => format!("usage: create_debug {verb} <x y z> <x y z>"),
            },
            _ => format!("usage: create_debug {verb} <x y z> ..."),
        };
        feedback(lv.host, line);
        Flow::Cancel
    }
}

/// The `ITEMS` handler of the packager `side` asks, if it is one (`automation`).
pub(crate) fn packager_items(
    host: &mut dyn ServerHost,
    side: mod_api::BlockSide,
    dim: usize,
) -> Option<Box<dyn mod_api::ResourceHandler<mod_api::ItemResource>>> {
    if !present() {
        return None;
    }
    automation::provide(host, side, dim)
}

/// The verb of a `create_debug` command when it is one of `verbs`.
pub fn debug_verb(event: &RunModCommandEvent<'_>, verbs: &[&'static str]) -> Option<&'static str> {
    let l = &event.command.literals;
    if l.first().map(String::as_str) != Some("create_debug") {
        return None;
    }
    let verb = l.get(1)?;
    verbs.iter().copied().find(|v| v == verb)
}

/// A command's answer to whoever ran it.
pub fn feedback(host: &mut dyn ServerHost, line: String) {
    let _ = host.commands().feedback(Text::literal(line));
}

/// The player who ran a command, if a player did.
pub fn command_player(source: &CommandSource) -> Option<PlayerRef> {
    match source {
        CommandSource::Player { player, .. } => Some(*player),
        _ => None,
    }
}

/// Coordinates from words (`~` from `here`).
pub fn coords(words: &[&str], here: Option<[f64; 3]>) -> Option<[i32; 3]> {
    let mut out = [0; 3];
    for (i, v) in out.iter_mut().enumerate() {
        let t = words.get(i)?;
        *v = match t.strip_prefix('~') {
            Some(off) => here?[i].floor() as i32 + off.parse::<i32>().unwrap_or(0),
            None => t.parse().ok()?,
        };
    }
    Some(out)
}

/// The level's tick: blocks placed and removed, packagers' power, the packagers, chains and
/// frogports, then what clients draw. Nothing runs in a level with nothing to do.
fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if !present() && CHANGES.with_borrow(Vec::is_empty) {
        return;
    }
    let changes: Vec<Change> = CHANGES.with_borrow_mut(|c| {
        let mine = c.iter().filter(|x| x.0 == dim).copied().collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (_, pos, was, is) in changes {
        block_moved(lv, dim, pos, was, is);
    }
    automation::reconcile(lv);
    // What comparators read of packagers and ports that changed (since the last tick too: a
    // menu took or gave a package) is looked at after this tick.
    let mut touched = with(dim, |pk| std::mem::take(&mut pk.changed));
    let now = now(lv);
    if with(dim, |pk| pk.is_idle()) && GONE.with_borrow(Vec::is_empty) {
        frog_draw::sync(lv);
        box_draw::sync(lv);
        automation::sync(lv);
        analog_touched(lv, &touched);
        mark_unsaved(dim, &[]);
        return;
    }
    // PackagerBlock.neighborChanged: `powered` follows the signal; a new one activates it.
    for pos in with(dim, |pk| std::mem::take(&mut pk.redstone)) {
        let state = lv.block(pos);
        let Some((Block::Packager | Block::Repackager, was)) =
            with_world(|w| w.table.get(state).map(|(b, i)| (b, i.powered))).flatten()
        else {
            continue;
        };
        let now_powered = lv.has_neighbor_signal(pos);
        if was == now_powered {
            continue;
        }
        lv.set_property(
            pos,
            "powered",
            if now_powered { "true" } else { "false" },
            UPDATE_CLIENTS,
        );
        if !was {
            activate(lv, dim, pos);
        }
    }
    let mut out = Vec::new();
    with(dim, |pk| pk.tick(&mut out));
    for o in out {
        match o {
            Out::Recheck(pos) => {
                // `lazyTick`: `redstonePowered` read back from the block, then a try to send.
                let state = lv.block(pos);
                let powered = with_world(|w| w.table.get(state).map(|(_, i)| i.powered))
                    .flatten()
                    .unwrap_or(false);
                with(dim, |pk| pk.packager(pos).powered = powered);
                if redstone_mode(lv, dim, pos) && with(dim, |pk| pk.packager(pos).can_send()) {
                    attempt_send(lv, dim, pos);
                }
            }
            Out::PackagerDone(_) => {}
            Out::Drop(at, p) => drop_stack(lv, at, p.stack),
            Out::Caught(pos) => crate::glue::advancements::award_placer(lv, pos, "frogport"),
        }
    }
    for pos in with(dim, |pk| pk.due(now)) {
        frogport(lv, dim, pos, now);
    }
    // Clients redraw the conveyors whose packages changed.
    let mut changed = with(dim, |pk| pk.chains.take_changed());
    changed.extend(GONE.with_borrow_mut(|g| {
        let mine: Vec<_> = g.iter().filter(|x| x.0 == dim).map(|x| x.1).collect();
        g.retain(|x| x.0 != dim);
        mine
    }));
    if !changed.is_empty() {
        let players = lv.players();
        send_chains(lv.host, &players, dim, &changed);
        sync_chains(lv, &changed);
    }
    // Frogports placed with a chain point, now known; what clients draw of frogports.
    frog_target::level_tick(lv);
    frog_draw::sync(lv);
    box_draw::sync(lv);
    automation::sync(lv);
    touched.extend(with(dim, |pk| std::mem::take(&mut pk.changed)));
    analog_touched(lv, &touched);
    mark_unsaved(dim, &changed);
}

/// The columns whose packagers, ports or conveyors changed (and those holding `also`) are
/// written at the end of this tick.
fn mark_unsaved(dim: usize, also: &[[i32; 3]]) {
    let mut unsaved = with(dim, |pk| std::mem::take(&mut pk.unsaved));
    unsaved.extend(also.iter().map(|p| (p[0] >> 4, p[2] >> 4)));
    unsaved.sort_unstable();
    unsaved.dedup();
    for (cx, cz) in unsaved {
        persist::mark_unsaved(dim, [cx, cz]);
    }
}

/// Comparators beside packagers and ports whose output may have changed read them again
/// (`updateNeighbourForOutputSignal`); the items their ports hold are noted for that reading.
fn analog_touched(lv: &mut ServerLevel<'_>, touched: &[[i32; 3]]) {
    if touched.is_empty() {
        return;
    }
    let mut seen: Vec<[i32; 3]> = Vec::with_capacity(touched.len());
    for &pos in touched {
        if seen.contains(&pos) {
            continue;
        }
        seen.push(pos);
        note_maxes(lv, pos);
        if let Some(state) = lv.block_state(pos) {
            let _ =
                lv.level
                    .update_neighbors(&mut *lv.host, pos, state, NeighbourUpdate::OutputSignal);
        }
    }
}

/// Notes the largest stack of each item the port at `pos` holds.
fn note_maxes(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    let items: Vec<ItemKey> = with(lv.dim, |pk| {
        pk.ports
            .get(&pos)
            .map(|p| p.items.iter().flatten().map(|b| b.stack.item()).collect())
            .unwrap_or_default()
    });
    MAXES.with_borrow_mut(|m| {
        for item in items {
            m.entry(item)
                .or_insert_with(|| lv.host.max_stack_size(item).unwrap_or(64));
        }
    });
}

/// `getAnalogOutputSignal` of a packager (`PackagerBlock`: 15 while it holds a box or animates)
/// or a frogport or postbox (`PackagePortBlockEntity.getComparatorOutput`: its 18 slots of
/// packages, as `calcRedstoneFromInventory` counts them); None for anything else.
fn analog(dim: usize, pos: [i32; 3]) -> Option<i32> {
    PACKAGES.with(|p| {
        let p = p.try_borrow().ok()?;
        let pk = p.as_ref()?.get(dim)?;
        if let Some(k) = pk.packagers.get(&pos) {
            return Some(i32::from(k.comparator()));
        }
        let port = pk.ports.get(&pos)?;
        Some(MAXES.with_borrow(|m| {
            create_engine::comparator::inventory(
                port.items.iter().map(|s| {
                    s.as_ref().map(|p| {
                        let max = m.get(&p.stack.item()).copied().unwrap_or(64);
                        (p.stack.count(), max)
                    })
                }),
                64,
            )
        }))
    })
}

/// One of our blocks placed or removed.
fn block_moved(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    was: Option<Block>,
    is: Option<Block>,
) {
    let mut drops: Vec<([f64; 3], ItemStack)> = Vec::new();
    match was {
        // `PackagerBlockEntity.destroy`: its box and queued boxes drop.
        Some(Block::Packager | Block::Repackager) => with(dim, |pk| {
            if let Some(p) = pk.packagers.remove(&pos) {
                // A box in its slot drops with the block (`automation`).
                let held = p.held.filter(|_| !automation::mirrored(dim, pos));
                automation::forget(dim, pos);
                for b in held.into_iter().chain(p.sliding) {
                    drops.push((center(pos), b.stack));
                }
                for (b, n) in p.queue {
                    for _ in 0..n.max(1) {
                        drops.push((center(pos), b.stack.clone()));
                    }
                }
            }
            pk.active.retain(|x| *x != pos);
        }),
        // `ChainConveyorBlockEntity.destroy`: its packages drop, the conveyors it joined let go.
        Some(Block::ChainConveyor) => {
            let joined: Vec<[i32; 3]> = with(dim, |pk| {
                let joined = pk
                    .chains
                    .conveyors
                    .get(&pos)
                    .map(|c| c.connections.iter().map(|k| add(pos, *k)).collect())
                    .unwrap_or_default();
                let mut ev = Vec::new();
                pk.chains.remove(pos, &mut ev);
                for e in ev {
                    if let chain::Event::Drop(at, p) = e {
                        drops.push((at, p.stack));
                    }
                }
                joined
            });
            with_world(|w| w.dims[dim].kinetics.set_far(pos, Vec::new()));
            for j in joined {
                sync_far(dim, j);
                with(dim, |pk| pk.conveyor_changed(j));
            }
            GONE.with_borrow_mut(|g| g.push((dim, pos)));
        }
        // `PackagePortBlockEntity.destroy`: its packages drop.
        // Those in its slots drop with the block (`automation`); one joined to a station leaves
        // it (`removePackagePort`).
        Some(_) => {
            station_ports::detach(dim, pos);
            let removed: Vec<ItemStack> = with(dim, |pk| {
                pk.remove_port(pos).into_iter().map(|p| p.stack).collect()
            });
            for s in automation::port_unmirrored(dim, pos, removed) {
                drops.push((center(pos), s));
            }
        }
        None => {}
    }
    match is {
        Some(b @ (Block::Packager | Block::Repackager)) => {
            with(dim, |pk| {
                pk.packager(pos).repackager = b == Block::Repackager;
                pk.touch(pos);
            });
            // `initialize`: `linked` follows a stock link on it.
            stock::recheck_linked(lv, pos);
        }
        Some(Block::ChainConveyor) => {
            let speed = speed_of(dim, pos);
            with(dim, |pk| {
                pk.chains.add(pos, Vec::new());
                pk.chains.set_speed(pos, speed);
                pk.touch(pos);
            });
        }
        Some(Block::PackageFrogport) => with(dim, |pk| {
            pk.port(pos, true);
            pk.touch(pos);
            pk.wake(pos);
        }),
        Some(_) => with(dim, |pk| {
            pk.port(pos, false);
            pk.touch(pos);
        }),
        None => {}
    }
    for (at, s) in drops {
        drop_stack(lv, at, s);
    }
}

/// The packager's facing and the inventory behind it (`oppositeOfBlockFacing`), reached through
/// the face toward the packager.
fn behind(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<([i32; 3], Direction)> {
    let state = lv.block(pos);
    let facing = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten()?;
    let o = facing.offset();
    Some((
        [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]],
        direction(facing),
    ))
}

/// The inventory behind the packager at `pos`, when there is one.
fn behind_inv(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) -> Option<([i32; 3], Inv)> {
    let (at, face) = behind(lv, pos)?;
    Some((at, inv::at(lv, dim, at, face)?))
}

/// `activate`: a pulse packs the inventory behind into a package (`attemptToSend`), and the
/// button cooldown starts.
fn activate(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) {
    with(dim, |pk| pk.packager(pos).pulse());
    // `recheckIfLinksPresent`, then a linked packager waits for its network's orders.
    stock::recheck_linked(lv, pos);
    if !redstone_mode(lv, dim, pos) {
        with(dim, |pk| pk.activate(pos));
        return;
    }
    if with(dim, |pk| pk.packager(pos).can_send()) {
        attempt_send(lv, dim, pos);
    }
    with(dim, |pk| {
        pk.packager(pos).after_pulse();
        pk.activate(pos);
    });
}

/// A stack compared as Create compares stacks (`isSameItemSameComponents`).
struct Same<'a>(&'a ItemStack);

impl PartialEq for Same<'_> {
    fn eq(&self, o: &Self) -> bool {
        self.0.is_stackable_with(o.0)
    }
}

/// `!canFitInsideContainerItems`: packages and shulker boxes.
fn bulky(item: &str) -> bool {
    package::is_package(item) || item.ends_with("shulker_box")
}

/// The packing view of an inventory's slots.
fn slots_of<'a, K>(
    lv: &ServerLevel<'_>,
    view: &'a [Option<ItemStack>],
    key: impl Fn(&'a ItemStack) -> K,
) -> Vec<Option<package::Slot<K>>> {
    view.iter()
        .map(|s| {
            s.as_ref().map(|s| {
                let id = body::item_id(&*lv.host, s);
                package::Slot {
                    key: key(s),
                    count: s.count(),
                    max: body::stack_limit(&*lv.host, s),
                    bulky: bulky(&id),
                    package: package::is_package(&id),
                }
            })
        })
        .collect()
}

/// What a packing took: the package taken whole, or a new box of the contents.
fn packed_stack(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    inv: &Inv,
    view: &[Option<ItemStack>],
    packed: &package::Packed,
    boxed: Option<&mut Vec<ItemStack>>,
) -> Option<ItemStack> {
    let mut taken: HashMap<usize, Parcel<ItemStack>> = HashMap::new();
    for &(slot, n) in &packed.taken {
        if let Some(p) = inv::take(lv, dim, inv, slot, n) {
            taken.insert(slot, p);
        }
    }
    if let Some(p) = packed.package.and_then(|s| taken.remove(&s)) {
        return Some(p.stack);
    }
    let contents: Vec<Option<ItemStack>> = packed
        .contents
        .iter()
        .map(|c| {
            let (from, n) = (*c)?;
            body::with_count(view.get(from)?.as_ref()?, n)
        })
        .collect();
    if let Some(boxed) = boxed {
        boxed.extend(contents.iter().flatten().cloned());
    }
    new_box(lv, pos, &contents)
}

/// `PackagerBlockEntity.attemptToSend(null)`: the inventory behind packed into one box (a
/// package there goes as it is), addressed with the packager's address, sliding out.
fn attempt_send(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) {
    update_sign_address(lv, dim, pos);
    if with(dim, |pk| pk.packager(pos).repackager) {
        attempt_repackage(lv, dim, pos);
        return;
    }
    let Some((at, inv)) = behind_inv(lv, dim, pos) else {
        return;
    };
    // A packager behind a packager is no inventory to it (`instanceof PackagerItemHandler`).
    if inv.is_packager() {
        return;
    }
    let view = inv::view(lv, dim, &inv);
    let packed = package::pack(&slots_of(lv, &view, Same));
    let Some(packed) = packed else { return };
    let Some(mut stack) = packed_stack(lv, dim, pos, &inv, &view, &packed, None) else {
        return;
    };
    let address = with(dim, |pk| pk.packager(pos).address.clone());
    let address = (!package::is_blank(&address)).then_some(address);
    body::set_address(&mut stack, address.as_deref());
    with(dim, |pk| {
        pk.packager(pos).send(Parcel {
            stack,
            address: address.unwrap_or_default(),
        });
        pk.activate(pos);
    });
    crate::glue::advancements::award_placer(lv, pos, "packager");
    let _ = at;
}

/// `unwrapBox`: whether the package's contents fit the inventory behind (`simulate`), else
/// they go in (`DefaultUnpackingHandler.unpack`) and the tray slides in. An empty package
/// always unwraps, without the tray.
fn unwrap(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
    simulate: bool,
) -> bool {
    if !with(dim, |pk| pk.packager(pos).can_unwrap()) {
        return false;
    }
    if with(dim, |pk| pk.packager(pos).repackager) {
        return unwrap_whole(lv, dim, pos, stack, simulate);
    }
    let items: Vec<ItemStack> = body::contents(&*lv.host, stack)
        .into_iter()
        .flatten()
        .collect();
    if items.is_empty() {
        return true;
    }
    let Some((_, inv)) = behind_inv(lv, dim, pos) else {
        return false;
    };
    if simulate {
        return match &inv {
            Inv::Packager(_) => false,
            Inv::Cap(items_cap) => inv::fits(lv, items_cap, &items),
            // A port takes packages only.
            Inv::Port(_) => items
                .iter()
                .all(|i| package::is_package(&body::item_id(&*lv.host, i))),
        };
    }
    if inv.is_packager() {
        return false;
    }
    for i in items {
        if let Some(left) = inv::insert(lv, dim, &inv, parcel(i)) {
            // `unpack` trusts its simulation: what still did not fit drops.
            drop_stack(lv, center(pos), left.stack);
        }
    }
    box_draw::unwrapped(dim, pos, body::item_id(&*lv.host, stack));
    with(dim, |pk| {
        pk.packager(pos).unwrapped();
        pk.activate(pos);
    });
    true
}

/// `RepackagerBlockEntity.unwrapBox`: the package goes into the inventory behind as it is, when
/// a slot takes it.
fn unwrap_whole(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
    simulate: bool,
) -> bool {
    let Some((_, inv)) = behind_inv(lv, dim, pos) else {
        return false;
    };
    let room = match &inv {
        Inv::Packager(_) => return false,
        Inv::Cap(items) => inv::has_empty(lv, items),
        Inv::Port(p) => with(dim, |pk| {
            pk.ports.get(p).is_some_and(|port| {
                port.automation_takes(&body::address(stack)) && !port.backed_up()
            })
        }),
    };
    if !room || simulate {
        return room;
    }
    if let Some(left) = inv::insert(lv, dim, &inv, parcel(stack.clone())) {
        drop_stack(lv, center(pos), left.stack);
    }
    box_draw::unwrapped(dim, pos, body::item_id(&*lv.host, stack));
    with(dim, |pk| {
        pk.packager(pos).unwrapped();
        pk.activate(pos);
    });
    true
}

/// `RepackagerBlockEntity.attemptToSend`: a package without order data in the inventory behind
/// goes out as it is (with the sign's address); else, once every fragment of an order is there,
/// they are taken and the order goes out repacked (`PackageRepackageHelper`), queued.
/// `PackagerBlockEntity.updateSignAddress`: the text of a sign on any side of the packager (its
/// front, else its back; the lines joined by spaces) is its address, the last side found
/// winning. With no sign written, the address set before (`/create_debug address`) stays.
fn update_sign_address(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) {
    let mut found = None;
    for d in Dir::ALL {
        let o = d.offset();
        let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        if let Some(text) = sign_text(lv, at) {
            found = Some(text);
        }
    }
    if let Some(address) = found {
        with(dim, |pk| pk.packager(pos).address = address);
    }
}

/// The written text of the sign at `pos` (`getSign`): its front's lines, else its back's.
fn sign_text(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let host: &dyn mod_api::Host = &*lv.host;
    let state = lv.block_state(pos)?;
    if host.block_in_tag(state.block(), "minecraft:all_signs") != Some(true) {
        return None;
    }
    let data = lv.host.block_entity(lv.level, pos)?.data(host)?;
    for side in ["front_text", "back_text"] {
        let Some(Tag::Compound(text)) = data.get(&side.into()) else {
            continue;
        };
        let Some(Tag::List(messages)) = text.get(&"messages".into()) else {
            continue;
        };
        let mut address = String::new();
        for m in messages {
            let line = component_text(m);
            if !line.trim().is_empty() {
                address.push_str(line.trim());
                address.push(' ');
            }
        }
        if !address.trim().is_empty() {
            return Some(address.trim().to_owned());
        }
    }
    None
}

/// A text component's plain text, as a sign keeps it: a string (JSON or plain) or a compound
/// with `text` and `extra`.
fn component_text(tag: &Tag) -> String {
    match tag {
        Tag::String(s) => {
            let s = s.to_string_lossy();
            match s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
                None if s.starts_with('{') => s
                    .split("\"text\":\"")
                    .skip(1)
                    .filter_map(|r| r.split('"').next())
                    .collect(),
                None => s,
            }
        }
        Tag::Compound(c) => {
            let mut out = match c.get(&"text".into()) {
                Some(Tag::String(t)) => t.to_string_lossy(),
                _ => String::new(),
            };
            if let Some(Tag::List(extra)) = c.get(&"extra".into()) {
                for e in extra {
                    out.push_str(&component_text(e));
                }
            }
            out
        }
        Tag::List(parts) => parts.iter().map(component_text).collect(),
        _ => String::new(),
    }
}

fn attempt_repackage(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) {
    if !with(dim, |pk| {
        let k = pk.packager(pos);
        k.can_send() && k.queue.is_empty()
    }) {
        return;
    }
    let Some((_, inv)) = behind_inv(lv, dim, pos) else {
        return;
    };
    if inv.is_packager() {
        return;
    }
    let view = inv::view(lv, dim, &inv);
    let mut fragments: Vec<(usize, create_engine::stock::OrderData<ItemStack>)> = Vec::new();
    let mut completed = None;
    for (slot, s) in view.iter().enumerate() {
        let Some(s) = s else { continue };
        if !package::is_package(&body::item_id(&*lv.host, s)) {
            continue;
        }
        let Some(order) = body::order(&*lv.host, s) else {
            // Not fragmented: it goes out as it is.
            let Some(mut p) = inv::take(lv, dim, &inv, slot, 1) else {
                return;
            };
            let address = with(dim, |pk| pk.packager(pos).address.clone());
            if !package::is_blank(&address) {
                body::set_address(&mut p.stack, Some(&address));
                p.address = address;
            }
            with(dim, |pk| {
                pk.packager(pos).send(p);
                pk.activate(pos);
            });
            return;
        };
        let id = order.order_id;
        fragments.push((slot, order));
        let mine: Vec<&create_engine::stock::OrderData<ItemStack>> = fragments
            .iter()
            .filter(|f| f.1.order_id == id)
            .map(|f| &f.1)
            .collect();
        if create_engine::stock::is_order_complete(&mine) {
            completed = Some(id);
            break;
        }
    }
    let Some(id) = completed else { return };
    let mut frags = Vec::new();
    for (slot, order) in fragments.into_iter().filter(|f| f.1.order_id == id) {
        let Some(s) = view[slot].as_ref() else {
            continue;
        };
        frags.push(create_engine::stock::Fragment {
            order: stock::order_data_key(&order),
            address: body::address(s),
            contents: body::contents(&*lv.host, s)
                .into_iter()
                .flatten()
                .map(|i| (stock::Key::of(&i), i.count()))
                .collect(),
        });
    }
    let host: &dyn mod_api::Host = &*lv.host;
    let time = lv.game_time();
    let boxes = create_engine::stock::repack(
        &frags,
        id,
        |k| body::stack_limit(host, &k.0),
        || stock::next_int(time),
    );
    // Every fragment of the order leaves the inventory.
    for (slot, s) in view.iter().enumerate() {
        let fragment = s.as_ref().is_some_and(|s| {
            package::is_package(&body::item_id(&*lv.host, s))
                && body::order(&*lv.host, s).is_some_and(|o| o.order_id == id)
        });
        if fragment {
            inv::take(lv, dim, &inv, slot, 1);
        }
    }
    for b in boxes {
        let contents: Vec<Option<ItemStack>> = b
            .contents
            .iter()
            .map(|(k, n)| body::with_count(&k.0, *n))
            .collect();
        let Some(mut stack) = new_box(lv, pos, &contents) else {
            continue;
        };
        body::set_address(&mut stack, Some(&b.address));
        body::set_order(&*lv.host, &mut stack, &stock::order_data_stack(&b.order));
        with(dim, |pk| {
            pk.packager(pos).queue.push_back((
                Parcel {
                    stack,
                    address: b.address.clone(),
                },
                b.count,
            ))
        });
    }
    with(dim, |pk| pk.activate(pos));
}

/// `PackageItem.containing`: a box of a random style holding `contents`.
fn new_box(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    contents: &[Option<ItemStack>],
) -> Option<ItemStack> {
    let mut seed = (lv.game_time() as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    let next = |n: u32| {
        seed ^= seed >> 31;
        seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        seed ^= seed >> 29;
        (seed % u64::from(n.max(1))) as u32
    };
    let mut b = body::new_stack(&*lv.host, package::random_box(next), 1)?;
    body::set_contents(&*lv.host, &mut b, contents);
    Some(b)
}

/// The packager's inventory as `getAvailableItems` sums it, and which inventory it is
/// (`InventoryIdentifier`: a vault's first block), or None when nothing is behind it. A
/// packager behind it is an inventory of nothing.
pub fn available(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
) -> (create_engine::stock::Summary<stock::Key>, Option<[i32; 3]>) {
    let mut s = create_engine::stock::Summary::default();
    let Some((at, inv)) = behind_inv(lv, dim, pos) else {
        return (s, None);
    };
    let vault = block_of(lv.block(at)) == Some(Block::ItemVault);
    let id = vault
        .then(|| with_world(|w| w.dims[dim].vaults.inventory(at)).flatten())
        .flatten()
        .and_then(|parts| parts.iter().min().copied())
        .unwrap_or(at);
    if inv.is_packager() {
        return (s, Some(id));
    }
    for stack in inv::view(lv, dim, &inv).iter().flatten() {
        s.add(&stock::Key::of(stack), stack.count() as i32);
    }
    (s, Some(id))
}

/// `isTooBusyFor`: a packager with that many boxes queued takes no order.
pub fn too_busy(dim: usize, pos: [i32; 3], kind: create_engine::stock::RequestType) -> bool {
    with(dim, |pk| {
        pk.packagers.get(&pos).is_some_and(|k| k.too_busy(kind))
    })
}

/// `performPackageRequests` for one packager: up to 100 boxes of its requests
/// (`attemptToSend(requests)`), each sliding out or queued. The stacks put in boxes (not
/// packages sent as they are), for `deductFromAccurateSummary`.
pub fn perform_requests(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    queue: &mut std::collections::VecDeque<create_engine::stock::Request<stock::Key>>,
) -> Vec<ItemStack> {
    let mut boxed = Vec::new();
    for _ in 0..create_engine::stock::MAX_PACKAGES {
        if queue.is_empty() {
            break;
        }
        let Some((_, inv)) = behind_inv(lv, dim, pos) else {
            break;
        };
        if inv.is_packager() {
            break;
        }
        let view = inv::view(lv, dim, &inv);
        let mut slots = slots_of(lv, &view, stock::Key::of);
        let Some(r) = create_engine::stock::pack_requests(&mut slots, queue) else {
            continue;
        };
        let Some(mut stack) = packed_stack(lv, dim, pos, &inv, &view, &r.packed, Some(&mut boxed))
        else {
            continue;
        };
        body::set_address(&mut stack, Some(&r.address));
        body::set_order(&*lv.host, &mut stack, &stock::order_data_stack(&r.order));
        if with(dim, |pk| pk.packager(pos).sends_now()) {
            crate::glue::advancements::award_placer(lv, pos, "packager");
        }
        with(dim, |pk| {
            pk.packager(pos).send_or_queue(Parcel {
                stack,
                address: r.address.clone(),
            });
            pk.activate(pos);
        });
    }
    boxed
}

/// `redstoneModeActive`: a packager with a stock link on it packs for its network only, not on
/// a signal; a re-packager always on a signal.
fn redstone_mode(lv: &ServerLevel<'_>, dim: usize, pos: [i32; 3]) -> bool {
    with(dim, |pk| {
        pk.packagers.get(&pos).is_some_and(|k| k.repackager)
    }) || stock::link_of(lv, pos).is_none()
}

/// A frogport looks at its inventories (`lazyTick`): what it caught for its address goes down
/// into the inventory below (`tryPushingToAdjacentInventories`), then a package not for it,
/// its own or from below, is sent onto its chain (`tryPullingFromOwnAndAdjacentInventories`).
/// It looks again in 10 ticks while something waits, and while an inventory is below it (that
/// inventory's changes are not told: Create's lazy tick).
fn frogport(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3], now: u64) {
    let Some(animating) = with(dim, |pk| pk.ports.get(&pos).map(|p| p.is_animating())) else {
        return;
    };
    if animating {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let below_is_frog = with(dim, |pk| pk.ports.get(&below).is_some_and(|p| p.frog));
    let inv = if below_is_frog {
        None
    } else {
        inv::at(lv, dim, below, Direction::Up)
    };
    let mut failed = false;
    if let Some(inv) = &inv {
        for slot in 0..create_engine::port::SLOTS {
            let p = with(dim, |pk| {
                let port = pk.ports.get_mut(&pos)?;
                let gives = port.items[slot]
                    .as_ref()
                    .is_some_and(|b| port.automation_gives(&b.address));
                if gives { port.items[slot].take() } else { None }
            });
            let Some(p) = p else { continue };
            if let Some(back) = inv::insert(lv, dim, inv, p) {
                with(dim, |pk| {
                    if let Some(port) = pk.ports.get_mut(&pos) {
                        port.items[slot] = Some(back);
                    }
                });
                failed = true;
            } else {
                with(dim, |pk| pk.touch(pos));
            }
        }
    }
    with(dim, |pk| {
        if let Some(port) = pk.ports.get_mut(&pos) {
            port.failed_export = failed;
        }
    });
    let mut waiting = failed || inv.is_some();
    if with(dim, |pk| pk.can_send(pos)) {
        if !with(dim, |pk| pk.send_own(pos))
            && let Some(inv) = &inv
        {
            let filter = with(dim, |pk| port_filter(pk, pos));
            let view = inv::view(lv, dim, inv);
            let packager = inv.is_packager();
            let slot = view.iter().position(|s| {
                s.as_ref().is_some_and(|s| {
                    package::is_package(&body::item_id(&*lv.host, s))
                        && (packager
                            || filter
                                .as_deref()
                                .is_none_or(|f| !package::match_address(&body::address(s), f)))
                })
            });
            if let Some(slot) = slot
                && let Some(p) = inv::take(lv, dim, inv, slot, 1)
            {
                with(dim, |pk| pk.start(pos, p, true));
            }
        }
    } else {
        // Held back by its target: it waits only if it has something to send.
        let own = with(dim, |pk| {
            let port = pk.ports.get(&pos);
            port.is_some_and(|p| p.target.is_some() && p.has_outgoing())
        });
        waiting |= own;
    }
    if waiting {
        with(dim, |pk| pk.retry(pos, now));
    }
}

/// The frogport's filter (`getFilterString`).
fn port_filter(pk: &Pk, pos: [i32; 3]) -> Option<String> {
    let port = pk.ports.get(&pos)?;
    port.filter_string().map(str::to_owned)
}

/// A right click on a packager (`PackagerBlock.useItemOn`): a wrench, a frogport or a gauge in
/// hand (or a stock link, unless one is on it) passes; with nothing held, a package in hand is
/// unpacked into the inventory behind; a held box goes to the player.
fn use_packager(
    host: &mut dyn ServerHost,
    event: &UseItemOnEvent<'_>,
) -> Option<InteractionResult> {
    let click = event.click;
    let dimension = click.dimension.key().id();
    let mut lv = ServerLevel::new(host, dimension)?;
    let (dim, pos) = (lv.dim, click.pos);
    let hand = click.stack.map(|s| body::item_id(&*lv.host, s));
    let linked = stock::link_of(&lv, pos).is_some();
    if hand.as_deref().is_some_and(|h| {
        matches!(
            h,
            "create:wrench" | "create:package_frogport" | "create:factory_gauge"
        ) || (h == "create:stock_link" && !linked)
    }) {
        return Some(InteractionResult::TryWithEmptyHand);
    }
    if click.hand != Hand::Main {
        return Some(InteractionResult::Success);
    }
    let (animating, held) = with(dim, |pk| {
        let p = pk.packager(pos);
        (p.animation_ticks > 0, p.held.is_some())
    });
    if animating {
        return Some(InteractionResult::Success);
    }
    let player = lv.host.player(click.player)?;
    if !held {
        let Some(stack) = click.stack.cloned() else {
            return Some(InteractionResult::Success);
        };
        let id = body::item_id(&*lv.host, &stack);
        if !package::is_package(&id) || !unwrap(&mut lv, dim, pos, &stack, true) {
            return Some(InteractionResult::Success);
        }
        unwrap(&mut lv, dim, pos, &stack, false);
        automation::sync(&mut lv);
        stock::stock_check(&mut lv, dim, pos);
        if let Some(slot) = player.selected_slot(&*lv.host) {
            let _ = player.set_inventory_slot(&mut *lv.host, slot as u32, stack.shrink(1));
        }
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "create:depot_plop",
            center(pos),
            SoundSource::Blocks,
            1.0,
            1.0,
        );
        return Some(InteractionResult::Success);
    }
    let Some(b) = with(dim, |pk| inv::take_box(pk, pos)) else {
        return Some(InteractionResult::Success);
    };
    // `placeItemBackInInventory`: into the inventory, else dropped.
    let _ = player.give_stack(&mut *lv.host, b.stack);
    with(dim, |pk| pk.wake([pos[0], pos[1] + 1, pos[2]]));
    automation::sync(&mut lv);
    Some(InteractionResult::Success)
}

/// `PackagerItemHandler.insertItem` of one package from a mechanical arm (its `PackagerType`
/// point): while it holds no box and sends none, a package that unwraps into the inventory
/// behind is unwrapped (only looked at with `simulate`). Whether it took one.
pub fn arm_insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: &ItemStack,
    simulate: bool,
) -> bool {
    let dim = lv.dim;
    let busy = with(dim, |pk| {
        let p = pk.packager(pos);
        p.held.is_some() || !p.queue.is_empty()
    });
    let id = body::item_id(&*lv.host, stack);
    if busy || !package::is_package(&id) || !unwrap(lv, dim, pos, stack, true) {
        return false;
    }
    if !simulate {
        unwrap(lv, dim, pos, stack, false);
        stock::stock_check(lv, dim, pos);
    }
    true
}

/// `PackagerItemHandler.extractItem` for a mechanical arm: the box it holds, while it is not
/// animating (only looked at with `simulate`).
pub fn arm_extract(lv: &mut ServerLevel<'_>, pos: [i32; 3], simulate: bool) -> Option<ItemStack> {
    let dim = lv.dim;
    if simulate {
        return with(dim, |pk| {
            let p = pk.packager(pos);
            if p.animation_ticks != 0 {
                return None;
            }
            p.held.as_ref().map(|b| b.stack.clone())
        });
    }
    let b = with(dim, |pk| inv::take_box(pk, pos))?;
    with(dim, |pk| pk.wake([pos[0], pos[1] + 1, pos[2]]));
    Some(b.stack)
}

/// `PackagePortBlockEntity.use`: the main hand's click, not sneaking, opens the port's screen.
pub(crate) fn use_port(
    host: &mut dyn ServerHost,
    click: &mod_api::BlockClick<'_>,
) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    if player.entity().is_shift_key_down(&*host) == Some(true) {
        return None;
    }
    if click.hand != Hand::Main {
        return Some(InteractionResult::Success);
    }
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    if frog_draw::put_on_goggles(&mut lv, click) {
        return Some(InteractionResult::Success);
    }
    if port_clipboard(&mut lv, player, click.pos) {
        return Some(InteractionResult::Success);
    }
    port_screen::open(&mut lv, player, click.pos, false);
    Some(InteractionResult::Success)
}

/// `PackagePortBlockEntity.use` with a clipboard in the main hand: the port's address is written
/// on it, and the screen does not open. False without a clipboard.
pub(crate) fn port_clipboard(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> bool {
    let held = super::items::held(&*lv.host, player, Hand::Main);
    let Some(clip) = held.filter(|s| s.item().id(&*lv.host) == "create:clipboard") else {
        return false;
    };
    let filter = with(lv.dim, |pk| pk.ports.get(&pos).map(|p| p.filter.clone()));
    if let Some((clip, address)) = filter.and_then(|f| address_to_clipboard(&clip, &f)) {
        super::items::set_held(lv.host, player, Hand::Main, Some(clip));
        let line = Text::translate(
            "create.clipboard.address_added",
            vec![Text::literal(address)],
        );
        let _ = player.action_bar(lv.host, line);
    }
    true
}

/// `PackagePortBlockEntity.addAddressToClipboard`: `#address` added to the first page with room
/// (twelve entries), or a new one, unless the clipboard has it already; it is then written on.
fn address_to_clipboard(clip: &ItemStack, address: &str) -> Option<(ItemStack, String)> {
    use super::display::clipboard;
    if address.trim().is_empty() {
        return None;
    }
    let mut content = clipboard::content(clip);
    let entry = format!("#{address}");
    let spaced = format!("# {address}");
    if content
        .pages
        .iter()
        .flatten()
        .any(|e| e.text == entry || e.text == spaced)
    {
        return None;
    }
    let page = match content.pages.iter().position(|p| p.len() <= 11) {
        Some(i) => i,
        None => {
            content.pages.push(Vec::new());
            content.pages.len() - 1
        }
    };
    content.pages[page].push(create_engine::clipboard::Entry {
        checked: false,
        text: entry,
        icon: None,
    });
    content.kind = create_engine::clipboard::Kind::Written;
    Some((
        clipboard::with_content(clip, Some(&content)),
        address.to_owned(),
    ))
}

/// Packagers', frogports' and postboxes' own moments: a packager's click, and what comparators
/// read of each.
struct PackageBlock;

#[mod_api::horizon_part]
impl BlockPart for PackageBlock {
    /// A packager's slots changed: automation took its box or put a package in (`automation`).
    fn contents_changed(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::ContentsChangedEvent,
    ) {
        let Some(dimension) = event.level.dimension().id(&*host).map(|d| d.to_string()) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let (packager, port) = with(lv.dim, |pk| {
            (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
        });
        if packager {
            automation::contents_changed(&mut lv, pos);
        } else if port {
            automation::port_contents_changed(&mut lv, pos);
        }
        if packager || port {
            automation::sync(&mut lv);
        }
    }

    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let b = block_of(states::id(event.state)?)?;
        if b == Block::PackageFrogport {
            let held = event.click.stack.map(|s| s.item().id(&*host));
            // The wrench and the frogport's own item (choosing a target) pass.
            if matches!(
                held.as_deref(),
                Some("create:wrench" | "create:package_frogport")
            ) {
                return None;
            }
            return use_port(host, event.click);
        }
        if !is_packager(b) {
            return None;
        }
        use_packager(host, event)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        if block_of(states::id(event.state)?)? != Block::PackageFrogport {
            return None;
        }
        use_port(host, event.click)
    }

    fn has_analog_output(&self, _level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        event.output = true;
    }

    fn analog_output(&self, _level: &dyn LevelHost, event: &mut AnalogOutputEvent) {
        let Some(dim) = level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        event.output = analog(dim, pos).unwrap_or(0);
    }
}

/// A packager's address (its sign's) or a port's filter (`addressFilter`, `filterChanged`).
fn set_address(dim: usize, pos: [i32; 3], text: &str) -> String {
    let line = with(dim, |pk| {
        if let Some(p) = pk.packagers.get_mut(&pos) {
            p.address = text.to_owned();
            pk.touch(pos);
            return format!("packager address \"{text}\"");
        }
        if let Some(p) = pk.ports.get_mut(&pos) {
            p.filter = text.to_owned();
            pk.register(pos);
            pk.wake(pos);
            pk.touch(pos);
            return format!("port filter \"{text}\"");
        }
        "no packager or port there".to_owned()
    });
    station_ports::filter_changed(dim, pos);
    line
}

/// A frogport's target: a chain conveyor, round its wheel at an angle or along one of its
/// chains (`ChainConveyorFrogportTarget`, `setup`: flipped when the chain runs backwards).
fn set_frogport(dim: usize, pos: [i32; 3], words: &[&str], here: [f64; 3]) -> String {
    let usage = "usage: create_debug frogport <x y z> <conveyor x y z> <angle | distance dx dy dz>";
    let Some(conveyor) = coords(words, Some(here)) else {
        return usage.into();
    };
    let Some(chain_pos) = words.get(3).and_then(|w| w.parse::<f32>().ok()) else {
        return usage.into();
    };
    let connection = if words.len() >= 7 {
        coords(&words[4..], None)
    } else {
        None
    };
    with(dim, |pk| {
        if !pk.ports.get(&pos).is_some_and(|p| p.frog) {
            return "no frogport there".to_owned();
        }
        let Some(c) = pk.chains.conveyors.get(&conveyor) else {
            return "no chain conveyor there".to_owned();
        };
        if connection.is_some_and(|k| !c.connections.contains(&k)) {
            return "no such chain on that conveyor".to_owned();
        }
        let flipped = c.speed < 0.0;
        if let Some(old) = pk.ports[&pos].target {
            pk.chains.deregister_port(pos, old.conveyor);
        }
        if let Some(port) = pk.ports.get_mut(&pos) {
            port.target = Some(Target {
                conveyor,
                chain_pos,
                connection,
                flipped,
            });
        }
        pk.register(pos);
        pk.wake(pos);
        pk.touch(pos);
        format!("frogport on {conveyor:?} at {chain_pos} {connection:?}")
    })
}

/// `ChainConveyorConnectionHandler`: the first conveyor clicked with chains, by player.
type FirstChain = HashMap<u128, (usize, [i32; 3])>;

thread_local! {
    static FIRST_CHAIN: RefCell<FirstChain> = RefCell::new(HashMap::new());
}

fn chain_status(lv: &mut ServerLevel<'_>, player: PlayerRef, key: &str, red: bool) {
    let mut style = mod_api::text::TextStyle::default();
    if red {
        style.color = Some("red".to_owned());
    }
    let line = if key.is_empty() {
        Text::literal("")
    } else {
        Text::translate(key, Vec::new()).with_style(style)
    };
    let _ = player.action_bar(lv.host, line);
}

/// The chains in a player's inventory (`getChainsFromInventory` counting).
fn chains_held(lv: &ServerLevel<'_>, player: PlayerRef) -> u32 {
    player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .iter()
        .filter(|(_, s)| s.item().id(&*lv.host) == "minecraft:iron_chain")
        .map(|(_, s)| s.count())
        .sum()
}

/// `getChainsFromInventory(.., false)`: `cost` chains out of the inventory, held stack first.
fn take_chains(lv: &mut ServerLevel<'_>, player: PlayerRef, mut cost: u32) {
    let slots = player.inventory_slots(&*lv.host).unwrap_or_default();
    for (i, s) in slots {
        if cost == 0 {
            break;
        }
        if s.item().id(&*lv.host) != "minecraft:iron_chain" {
            continue;
        }
        let n = s.count().min(cost);
        cost -= n;
        let rest = body::with_count(&s, s.count() - n);
        let _ = player.set_inventory_slot(lv.host, i, rest);
    }
}

/// A chain conveyor clicked with chains on the server: the first one is chosen, the second is
/// joined to it (`validateAndConnect`, `ChainConveyorConnectionPacket`), its chains taken
/// outside creative. Sneaking clears the choice.
fn chain_clicked(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let uuid = player.uuid();
    let count = with(dim, |pk| {
        pk.chains
            .conveyors
            .get(&pos)
            .map_or(0, |c| c.connections.len())
    });
    if count >= MAX_CHAIN_CONNECTIONS {
        chain_status(
            lv,
            player,
            "create.chain_conveyor.cannot_add_more_connections",
            true,
        );
        return;
    }
    let first = FIRST_CHAIN.with_borrow(|f| f.get(&uuid).copied());
    let Some((first_dim, first)) = first.filter(|f| f.0 == dim) else {
        FIRST_CHAIN.with_borrow_mut(|f| f.insert(uuid, (dim, pos)));
        chain_status(lv, player, "create.chain_conveyor.select_second", false);
        return;
    };
    let _ = first_dim;
    FIRST_CHAIN.with_borrow_mut(|f| f.remove(&uuid));
    if super::items::sneaking(&*lv.host, player) {
        chain_status(lv, player, "create.chain_conveyor.selection_cleared", false);
        return;
    }
    if pos == first {
        chain_status(lv, player, "create.chain_conveyor.selection_cleared", false);
        return;
    }
    let diff = [0, 1, 2].map(|i| f64::from(pos[i] - first[i]));
    let d2: f64 = diff.iter().map(|v| v * v).sum();
    let fail = |lv: &mut ServerLevel<'_>, key: &str| {
        chain_status(lv, player, key, true);
        let at = pos.map(|v| f64::from(v) + 0.5);
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "create:deny",
            at,
            SoundSource::Blocks,
            1.0,
            1.0,
        );
    };
    if d2 >= MAX_CHAIN_LENGTH * MAX_CHAIN_LENGTH {
        return fail(lv, "create.chain_conveyor.too_far");
    }
    if d2 < 2.5 * 2.5 {
        return fail(lv, "create.chain_conveyor.too_close");
    }
    let horizontal = (diff[0] * diff[0] + diff[2] * diff[2]).sqrt() - 1.5;
    if horizontal <= 0.0 {
        return fail(lv, "create.chain_conveyor.cannot_connect_vertically");
    }
    if diff[1].abs() / horizontal > 1.0 {
        return fail(lv, "create.chain_conveyor.too_steep");
    }
    let (target, joined) = with(dim, |pk| {
        let back = [first[0] - pos[0], first[1] - pos[1], first[2] - pos[2]];
        let t = pk.chains.conveyors.get(&pos);
        (
            t.map_or(0, |c| c.connections.len()),
            t.is_some_and(|c| c.connections.contains(&back)),
        )
    });
    if target >= MAX_CHAIN_CONNECTIONS {
        return fail(lv, "create.chain_conveyor.cannot_add_more_connections");
    }
    if joined {
        return fail(lv, "create.chain_conveyor.already_connected");
    }
    if block_of(lv.block(first)) != Some(Block::ChainConveyor) {
        return fail(lv, "create.chain_conveyor.blocks_invalid");
    }
    let creative = super::items::creative(&*lv.host, player);
    let cost =
        chain::chain_cost([pos[0] - first[0], pos[1] - first[1], pos[2] - first[2]]).max(0) as u32;
    if !creative && chains_held(lv, player) < cost {
        return fail(lv, "create.chain_conveyor.not_enough_chains");
    }
    let line = chain_command(lv, first, pos, true);
    if !line.starts_with("chain ") {
        return fail(lv, "create.chain_conveyor.blocks_invalid");
    }
    if !creative {
        take_chains(lv, player, cost);
    }
    chain_status(lv, player, "", false);
    let at = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.chain.place",
        at,
        SoundSource::Blocks,
        1.0,
        0.8,
    );
}

/// `maxChainConveyorLength` and `maxChainConveyorConnections` (Create's defaults).
const MAX_CHAIN_LENGTH: f64 = 32.0;
const MAX_CHAIN_CONNECTIONS: usize = 4;

/// `ChainConveyorConnectionHandler`'s connection (the chain item's, after its checks; the
/// command's, with none) or its removal.
fn chain_command(lv: &mut ServerLevel<'_>, a: [i32; 3], b: [i32; 3], on: bool) -> String {
    let dim = lv.dim;
    let mut drops = Vec::new();
    let ok = with(dim, |pk| {
        let ok = if on {
            pk.chains.connect(a, b)
        } else {
            let had = pk.chains.conveyors.get(&a).is_some_and(|c| {
                c.connections
                    .contains(&[b[0] - a[0], b[1] - a[1], b[2] - a[2]])
            });
            let mut ev = Vec::new();
            pk.chains.disconnect(a, b, &mut ev);
            for e in ev {
                if let chain::Event::Drop(at, p) = e {
                    drops.push((at, p.stack));
                }
            }
            had
        };
        if ok {
            pk.conveyor_changed(a);
            pk.conveyor_changed(b);
            pk.touch(a);
            pk.touch(b);
        }
        ok
    });
    sync_far(dim, a);
    sync_far(dim, b);
    for (at, s) in drops {
        drop_stack(lv, at, s);
    }
    match (ok, on) {
        (true, true) => format!("chain {a:?} - {b:?}"),
        (true, false) => format!("unchained {a:?} - {b:?}"),
        (false, _) => "no such conveyors or chain".to_owned(),
    }
}

/// A packager's, port's or conveyor's state, for tests.
fn describe(lv: &ServerLevel<'_>, dim: usize, pos: [i32; 3]) -> String {
    let id = |s: &ItemStack| body::item_id(&*lv.host, s);
    with(dim, |pk| {
        if let Some(p) = pk.packagers.get(&pos) {
            let held = p.held.as_ref().or(p.sliding.as_ref());
            let queued: u32 = p.queue.iter().map(|q| q.1.max(1)).sum();
            return format!(
                "{} address=\"{}\" powered={} animation={} inward={} held={} queued={queued}",
                if p.repackager {
                    "repackager"
                } else {
                    "packager"
                },
                p.address,
                p.powered,
                p.animation_ticks,
                p.inward,
                held.map_or("-".to_owned(), |b| format!(
                    "{} to \"{}\"",
                    id(&b.stack),
                    b.address
                )),
            );
        }
        if let Some(p) = pk.ports.get(&pos) {
            let items: Vec<String> = p
                .items
                .iter()
                .flatten()
                .map(|b| format!("{} to \"{}\"", id(&b.stack), b.address))
                .collect();
            return format!(
                "{} filter=\"{}\" target={:?} station={:?} animating={} items=[{}]",
                if p.frog { "frogport" } else { "postbox" },
                p.filter,
                p.target.map(|t| (t.conveyor, t.chain_pos, t.connection)),
                p.station,
                p.is_animating(),
                items.join(", ")
            );
        }
        if let Some(c) = pk.chains.conveyors.get(&pos) {
            let travelling: usize = c.travelling.iter().map(|(_, l)| l.len()).sum();
            let routes: Vec<String> = c
                .routes()
                .iter()
                .map(|(f, d, k)| format!("{f}:{d}->{k:?}"))
                .collect();
            return format!(
                "chain_conveyor speed={} connections={:?} looping={} travelling={} ports={} routes=[{}]",
                c.speed,
                c.connections,
                c.looping.len(),
                travelling,
                c.loop_ports.len() + c.travel_ports.len(),
                routes.join(", ")
            );
        }
        "nothing of the package logistics there".to_owned()
    })
}

/// The conveyors' packages to `players`, each conveyor in one message.
fn send_chains(
    host: &mut dyn ServerHost,
    players: &[PlayerRef],
    dim: usize,
    conveyors: &[[i32; 3]],
) {
    if players.is_empty() {
        return;
    }
    for &pos in conveyors {
        let drawn = with(dim, |pk| pk.chains.drawn(pos, |s| s.item().id(&*host)));
        let data = chain::encode(pos, &drawn);
        net::send_to_players(host, players, chain::CHANNEL, &data);
    }
}

/// `ChainConveyorRenderer.renderBox`: how far a package hangs up from its target, by the
/// style's hook.
fn lift(item: &str) -> f32 {
    package::style(item).map_or(0.0, |s| {
        17.0 / 16.0 - s.rigging / 16.0 + f32::from(s.height) / 32.0
    })
}

/// The key of a conveyor's chains in its block entity's data, for its clients: whether it runs
/// backwards, then each connection's offset.
pub const CHAINS: &str = "Chains";
/// The key of whether a conveyor carries packages in its block entity's data, for its clients.
const CARRYING: &str = "Carrying";

/// The conveyor's partial models (`AllPartialModels.CHAIN_CONVEYOR_*`).
const CONVEYOR_MODELS: [&str; 4] = [
    "create:block/chain_conveyor/wheel",
    "create:block/chain_conveyor/guard",
    "create:block/chain_conveyor/shaft",
    "minecraft:block/iron_chain",
];

/// The packages' riggings (`PackageStyle.getRiggingModel`), by size.
const RIGGING_MODELS: [&str; 4] = [
    "create:item/package/rigging_12x12",
    "create:item/package/rigging_10x12",
    "create:item/package/rigging_12x10",
    "create:item/package/rigging_10x8",
];

/// The rigging a package hangs from on a chain, by its style's size.
fn rigging(item: &str) -> Option<&'static str> {
    let s = package::style(item)?;
    Some(match (s.width, s.height) {
        (12, 12) => RIGGING_MODELS[0],
        (10, 12) => RIGGING_MODELS[1],
        (12, 10) => RIGGING_MODELS[2],
        (10, 8) => RIGGING_MODELS[3],
        _ => return None,
    })
}

/// What each conveyor's block entity was last told of its chains, by level and position.
type SentChains = HashMap<(usize, [i32; 3]), (Vec<i32>, bool)>;

thread_local! {
    static CONVEYOR_MODEL_REFS: RefCell<HashMap<&'static str, mod_api::ModelRef>> =
        RefCell::new(HashMap::new());
    /// What each conveyor's block entity was last told of its chains.
    static CHAINS_SENT: RefCell<SentChains> =
        RefCell::new(HashMap::new());
}

fn conveyor_model(
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    host: &dyn ClientHost,
    file: &'static str,
) {
    for fresh in [false, true] {
        let cached = (!fresh)
            .then(|| CONVEYOR_MODEL_REFS.with_borrow(|m| m.get(file).copied()))
            .flatten();
        let model = cached.or_else(|| {
            let id = mod_api::Identifier::parse(file).ok()?;
            let m = host.render_assets().model(&id).ok()?;
            CONVEYOR_MODEL_REFS.with_borrow_mut(|c| c.insert(file, m));
            Some(m)
        });
        let Some(m) = model else { return };
        // Vanilla's chain model names no render type (its block's layer is cutout): drawn
        // cut out, as `RenderTypes.chain`.
        let mut options = mod_api::TerrainModelOptions::default();
        if file.ends_with("iron_chain") || file.contains("chain_conveyor/chain_") {
            options.material = mod_api::TerrainMaterial::Cutout;
        }
        match draw.terrain_model(m, options) {
            Ok(()) => return,
            Err(mod_api::TerrainModelError::Model(_)) if !fresh => continue,
            Err(_) => return,
        }
    }
}

/// The conveyor's chains in its block entity's data, and whether it carries packages, written
/// when they changed: a conveyor carrying packages is drawn anew every frame (`animated`), its
/// drawing kept again once they are gone.
fn sync_chains(lv: &mut ServerLevel<'_>, conveyors: &[[i32; 3]]) {
    let dim = lv.dim;
    for &pos in conveyors {
        let data = with(dim, |pk| {
            pk.chains.conveyors.get(&pos).map(|c| {
                let mut v = vec![i32::from(c.reversed)];
                v.extend(c.connections.iter().flatten());
                (v, !c.is_empty())
            })
        });
        let Some(data) = data else {
            CHAINS_SENT.with_borrow_mut(|s| s.remove(&(dim, pos)));
            continue;
        };
        if CHAINS_SENT.with_borrow(|s| s.get(&(dim, pos)) == Some(&data)) {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let tag = Compound::from([
            (CHAINS.into(), Tag::IntArray(data.0.clone())),
            (CARRYING.into(), Tag::Byte(i8::from(data.1))),
        ]);
        if entity.merge_data(&mut *lv.host, tag).is_ok() {
            CHAINS_SENT.with_borrow_mut(|s| s.insert((dim, pos), data));
        }
    }
}

/// `ChainConveyorRenderer`/`ChainConveyorVisual`: the wheel, a guard toward each chain, the
/// shaft turning at the conveyor's speed, and each chain running from the wheel to the next
/// (`renderChain`: crossed quads of the chain's sprite, scrolling along at the speed).
fn draw_conveyor(
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    host: &dyn ClientHost,
    pos: [i32; 3],
    nbt: &Compound,
) {
    let speed = super::kinetics::speeds_of(nbt)[0];
    // `renderChains`: the chains' scroll, a full texture per `360 / |speed|` ticks.
    let time = if speed == 0.0 {
        0.0
    } else {
        ((draw.game_time() as f64 + f64::from(draw.partial())) / (360.0 / f64::from(speed.abs())))
            .rem_euclid(1.0)
    };
    let animation = time - 0.5;
    conveyor_model(draw, host, CONVEYOR_MODELS[0]);
    draw.spin([0.0, 1.0, 0.0], [0.5; 3], speed * 0.3, 0.0);
    conveyor_model(draw, host, CONVEYOR_MODELS[2]);
    draw.still();
    let ints = match nbt.get(&CHAINS.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => return,
    };
    let Some((&rev, rest)) = ints.split_first() else {
        return;
    };
    let reversed = rev != 0;
    frog_target::seen(
        pos,
        reversed,
        rest.as_chunks::<3>()
            .0
            .iter()
            .map(|k| [k[0], k[1], k[2]])
            .collect(),
    );
    for k in rest.as_chunks::<3>().0 {
        let stats = chain::connection_stats([0, 0, 0], *k, reversed);
        let d = [0, 1, 2].map(|i| stats.end[i] - stats.start[i]);
        let yaw = d[0].atan2(d[2]).to_degrees() as f32;
        let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
        let pitch = d[1].atan2(flat).to_degrees() as f32;
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.5; 3]);
        conveyor_model(draw, host, CONVEYOR_MODELS[1]);
        draw.pop();
        // The chain: from its start (the block's corner frame), turned along it.
        let start = [0, 1, 2].map(|i| stats.start[i] as f32);
        draw.push();
        draw.translate(start);
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], 90.0 - pitch, [0.0; 3]);
        draw.rotate_about([0.0, 1.0, 0.0], 45.0, [0.0; 3]);
        draw_chain(draw, host, stats.length, animation);
        draw.pop();
    }
}

/// `renderChain`: the chain's sprite in two crossed planes `length` long up the pose's y,
/// each block of it vanilla's chain model (`renderPart`'s quads, the same columns of the
/// sprite), its texture scrolled by `animation` (`minV`), to the pixel: the models
/// `chain_conveyor/chain_<k>` that `create-data prepare` makes are the chain shifted `k` pixels.
fn draw_chain(
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    host: &dyn ClientHost,
    length: f32,
    animation: f64,
) {
    // The top of every whole block of chain starts at `minV` plus the part of a block past
    // the last whole one (`maxV = length + minV` at the bottom).
    let phase = (animation + f64::from(length).fract()).rem_euclid(1.0);
    let k = ((phase * 16.0) as usize) % 16;

    let model = if k == 0 {
        None
    } else {
        CHAIN_MODELS.get(k - 1).copied()
    };
    let mut y = 0.0f32;
    while y < length {
        let piece = (length - y).min(1.0);
        draw.push();
        draw.translate([-0.5, y, -0.5]);
        if piece < 1.0 {
            draw.scale([1.0, piece, 1.0]);
        }
        conveyor_model(draw, host, model.unwrap_or(CONVEYOR_MODELS[3]));
        draw.pop();
        y += 1.0;
    }
}

/// The chain scrolled 1 to 15 pixels (made by `create-data prepare`).
const CHAIN_MODELS: [&str; 15] = [
    "create:block/chain_conveyor/chain_1",
    "create:block/chain_conveyor/chain_2",
    "create:block/chain_conveyor/chain_3",
    "create:block/chain_conveyor/chain_4",
    "create:block/chain_conveyor/chain_5",
    "create:block/chain_conveyor/chain_6",
    "create:block/chain_conveyor/chain_7",
    "create:block/chain_conveyor/chain_8",
    "create:block/chain_conveyor/chain_9",
    "create:block/chain_conveyor/chain_10",
    "create:block/chain_conveyor/chain_11",
    "create:block/chain_conveyor/chain_12",
    "create:block/chain_conveyor/chain_13",
    "create:block/chain_conveyor/chain_14",
    "create:block/chain_conveyor/chain_15",
];

/// The frogport's item: used in the air on a chain point, it chooses it
/// (`ChainConveyorInteractionHandler.onUse`).
struct FrogportItem;

#[mod_api::horizon_part]
impl mod_api::ItemPart for FrogportItem {
    fn use_item(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::UseItemEvent<'_>,
    ) -> Option<InteractionResult> {
        match host {
            mod_api::HostSide::Client(h) if event.hand == Hand::Main => {
                frog_target::use_on_chain(h).then_some(InteractionResult::Success)
            }
            _ => None,
        }
    }
}

/// Draws the chain conveyor's packages from the server's last message, every frame: round the
/// wheel turning about the conveyor at its speed, along a chain gliding at the chain's.
struct ConveyorDraw;

#[mod_api::horizon_part]
impl BlockPart for ConveyorDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for ConveyorDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.block != "create:chain_conveyor" {
            return;
        }
        let pos = event.pos;
        let draw = &mut *event.draw;
        let now = draw.animation_time();
        draw_conveyor(draw, host, pos, &event.entity.nbt);
        SHOWN.with_borrow_mut(|shown| {
            let Some(s) = shown.get_mut(&pos) else {
                return;
            };
            let since = *s.since.get_or_insert(now);
            // The server's speeds are per second; the clock counts ticks.
            let seconds = ((now - since) / 20.0) as f32;
            for (d, stack) in &mut s.drawn {
                if stack.is_none() {
                    *stack = body::new_stack(host, &d.item, 1);
                }
                let Some(stack) = stack.as_ref() else {
                    continue;
                };
                let lift = lift(&d.item);
                draw.push();
                if d.looping {
                    let angle = d.angle + d.degrees * seconds;
                    draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5, 0.0, 0.5]);
                    draw.translate([0.5, 6.0 / 16.0 - 9.0 / 16.0 + lift, 0.5 + 0.875]);
                    draw.rotate_about([0.0, 1.0, 0.0], d.yaw - d.angle, [0.0; 3]);
                } else {
                    let at =
                        [0, 1, 2].map(|i| d.at[i] - pos[i] as f32 - 0.5 + d.velocity[i] * seconds);
                    draw.translate(at);
                    draw.translate([0.5, 0.5 + lift, 0.5]);
                    draw.rotate_about([0.0, 1.0, 0.0], d.yaw, [0.0; 3]);
                }
                // The item model is drawn about its centre.
                draw.translate([0.5; 3]);
                // Its rigging (`PACKAGE_RIGGING`), posed as the box's model is.
                if let Some(rig) = rigging(&d.item) {
                    draw.push();
                    draw.translate([-0.5; 3]);
                    conveyor_model(draw, host, rig);
                    draw.pop();
                }
                draw.item(stack, ItemDisplayContext::None, None, d.id as i32);
                draw.pop();
            }
        });
    }

    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        if event.block != "create:chain_conveyor" {
            return;
        }
        let pos = event.pos;
        SHOWN.with_borrow(|shown| {
            let Some(s) = shown.get(&pos) else {
                return;
            };
            let b = &mut *event.bounds;
            for (d, _) in &s.drawn {
                // A package on a chain glides on until the next message: reach its whole way.
                let reach = if d.looping { 2.0 } else { 34.0 };
                for i in 0..3 {
                    let at = d.at[i] - pos[i] as f32;
                    let (lo, hi) = if d.looping {
                        (-reach, 1.0 + reach)
                    } else if d.velocity[i] >= 0.0 {
                        (at - 1.0, at + 1.0 + d.velocity[i].abs().min(1.0) * reach)
                    } else {
                        (at - 1.0 - d.velocity[i].abs().min(1.0) * reach, at + 1.0)
                    };
                    b[i] = b[i].min(lo);
                    b[i + 3] = b[i + 3].max(hi);
                }
            }
        });
    }
}

/// Packagers, ports and conveyors of a column saved (`write`) under `create_packages`.
fn chunk_data_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    if !present() {
        return;
    }
    let host: &dyn mod_api::Host = &*lv.host;
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let parcel_tag = |p: &Parcel<ItemStack>| {
        Tag::compound([
            ("Item", body::to_nbt(host, &p.stack)),
            ("Address", Tag::String(p.address.as_str().into())),
        ])
    };
    let pos_tag = |p: [i32; 3]| Tag::IntArray(p.to_vec());
    let own = with(lv.dim, |pk| {
        let mut packagers: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk.packagers.iter().filter(|(p, _)| inside(p)).collect();
        list.sort_by_key(|(p, _)| **p);
        for (p, k) in list {
            let mut c = vec![
                ("Pos", pos_tag(*p)),
                ("SignAddress", Tag::String(k.address.as_str().into())),
                (
                    "State",
                    Tag::IntArray(vec![
                        i32::from(k.powered),
                        k.animation_ticks as i32,
                        i32::from(k.inward),
                        k.button_cooldown as i32,
                    ]),
                ),
            ];
            if let Some(b) = &k.held {
                c.push(("HeldBox", parcel_tag(b)));
            }
            if let Some(b) = &k.sliding {
                c.push(("SlidingBox", parcel_tag(b)));
            }
            if k.repackager {
                c.push(("Repackager", Tag::IntArray(vec![1])));
            }
            if !k.queue.is_empty() {
                // `QueuedExitingPackages`: each box with its count.
                let queue = k
                    .queue
                    .iter()
                    .filter_map(|(b, n)| {
                        let Tag::Compound(mut c) = parcel_tag(b) else {
                            return None;
                        };
                        c.insert("Count".into(), Tag::IntArray(vec![*n as i32]));
                        Some(Tag::Compound(c))
                    })
                    .collect();
                c.push(("QueuedExitingPackages", Tag::List(queue)));
            }
            packagers.push(Tag::compound(c));
        }
        let mut ports: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk.ports.iter().filter(|(p, _)| inside(p)).collect();
        list.sort_by_key(|(p, _)| **p);
        for (p, port) in list {
            let items: Vec<Tag> = port
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let b = s.as_ref()?;
                    let Tag::Compound(mut c) = parcel_tag(b) else {
                        return None;
                    };
                    c.insert("Slot".into(), Tag::IntArray(vec![i as i32]));
                    Some(Tag::Compound(c))
                })
                .collect();
            let mut c = vec![
                ("Pos", pos_tag(*p)),
                (
                    "Frog",
                    Tag::IntArray(vec![
                        i32::from(port.frog),
                        i32::from(port.accepts),
                        i32::from(port.goggles),
                    ]),
                ),
                ("AddressFilter", Tag::String(port.filter.as_str().into())),
                ("Inventory", Tag::List(items)),
            ];
            if let Some(s) = port.station {
                c.push(("Station", Tag::IntArray(s.to_vec())));
            }
            if let Some(t) = port.target {
                let k = t.connection.unwrap_or([0; 3]);
                c.push((
                    "Target",
                    Tag::IntArray(vec![
                        t.conveyor[0],
                        t.conveyor[1],
                        t.conveyor[2],
                        i32::from(t.connection.is_some()),
                        k[0],
                        k[1],
                        k[2],
                        i32::from(t.flipped),
                    ]),
                ));
                c.push(("ChainPos", Tag::Double(f64::from(t.chain_pos).to_bits())));
            }
            if let Some(b) = &port.animated {
                c.push(("AnimatedPackage", parcel_tag(b)));
                c.push(("Deposit", Tag::IntArray(vec![i32::from(port.deposit)])));
            }
            ports.push(Tag::compound(c));
        }
        let mut conveyors: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk
            .chains
            .conveyors
            .iter()
            .filter(|(p, _)| inside(p))
            .collect();
        list.sort_by_key(|(p, _)| **p);
        let package_tag = |b: &chain::Package<ItemStack>| {
            let mut c = match parcel_tag(&b.parcel) {
                Tag::Compound(c) => c,
                _ => Compound::new(),
            };
            c.insert(
                "Position".into(),
                Tag::Double(f64::from(b.position).to_bits()),
            );
            Tag::Compound(c)
        };
        for (p, c) in list {
            let travelling: Vec<Tag> = c
                .travelling
                .iter()
                .map(|(k, l)| {
                    Tag::compound([
                        ("Target", Tag::IntArray(k.to_vec())),
                        ("Packages", Tag::List(l.iter().map(package_tag).collect())),
                    ])
                })
                .collect();
            conveyors.push(Tag::compound([
                ("Pos", pos_tag(*p)),
                (
                    "Connections",
                    Tag::IntArray(c.connections.iter().flatten().copied().collect()),
                ),
                (
                    "LoopingPackages",
                    Tag::List(c.looping.iter().map(package_tag).collect()),
                ),
                ("TravellingPackages", Tag::List(travelling)),
            ]));
        }
        (packagers, ports, conveyors)
    });
    let (packagers, ports, conveyors) = own;
    if packagers.is_empty() && ports.is_empty() && conveyors.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    data.insert(
        KEY.into(),
        Tag::compound([
            ("packagers", Tag::List(packagers)),
            ("ports", Tag::List(ports)),
            ("conveyors", Tag::List(conveyors)),
        ]),
    );
}

pub fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

pub fn text(t: Option<&Tag>) -> String {
    match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

fn float(t: Option<&Tag>) -> f32 {
    match t {
        Some(Tag::Double(bits)) => f64::from_bits(*bits) as f32,
        _ => 0.0,
    }
}

pub fn pos_of(c: &Compound) -> Option<[i32; 3]> {
    let v = ints(c.get(&"Pos".into()));
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// The compounds of the list under `key`.
pub fn list(data: &Compound, key: &str) -> Vec<Compound> {
    match data.get(&key.into()) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|t| match t {
                Tag::Compound(c) => Some(c.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parcel_from(host: &dyn mod_api::Host, t: Option<&Tag>) -> Option<Parcel<ItemStack>> {
    let Some(Tag::Compound(c)) = t else {
        return None;
    };
    Some(Parcel {
        stack: body::from_nbt(host, c.get(&"Item".into())?)?,
        address: text(c.get(&"Address".into())),
    })
}

/// A column joined a level: its packagers, ports and conveyors back (`read`).
fn chunk_load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let dim = lv.dim;
    let host: &dyn mod_api::Host = &*lv.host;
    let mut conveyors = Vec::new();
    let mut ports_loaded = Vec::new();
    with(dim, |pk| {
        for c in list(data, "packagers") {
            let Some(pos) = pos_of(&c) else { continue };
            let s = ints(c.get(&"State".into()));
            let k = pk.packager(pos);
            k.address = text(c.get(&"SignAddress".into()));
            k.powered = s.first().is_some_and(|v| *v != 0);
            k.animation_ticks = s.get(1).copied().unwrap_or(0).clamp(0, 20) as u32;
            k.inward = s.get(2).is_none_or(|v| *v != 0);
            k.button_cooldown = s.get(3).copied().unwrap_or(0).clamp(0, 40) as u32;
            k.held = parcel_from(host, c.get(&"HeldBox".into()));
            k.sliding = parcel_from(host, c.get(&"SlidingBox".into()));
            k.repackager = !ints(c.get(&"Repackager".into())).is_empty();
            if let Some(Tag::List(l)) = c.get(&"QueuedExitingPackages".into()) {
                k.queue = l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(e) = t else { return None };
                        let n = ints(e.get(&"Count".into())).first().copied().unwrap_or(1);
                        Some((parcel_from(host, Some(t))?, n.max(0) as u32))
                    })
                    .collect();
            }
            if k.active() {
                pk.activate(pos);
            }
        }
        for c in list(data, "conveyors") {
            let Some(pos) = pos_of(&c) else { continue };
            let connections: Vec<[i32; 3]> = ints(c.get(&"Connections".into()))
                .as_chunks::<3>()
                .0
                .iter()
                .map(|k| [k[0], k[1], k[2]])
                .collect();
            pk.chains.add(pos, connections);
            let packages = |t: Option<&Tag>| -> Vec<(f32, Parcel<ItemStack>)> {
                match t {
                    Some(Tag::List(l)) => l
                        .iter()
                        .filter_map(|t| {
                            let Tag::Compound(c) = t else { return None };
                            Some((
                                float(c.get(&"Position".into())),
                                parcel_from(host, Some(t))?,
                            ))
                        })
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let looping = packages(c.get(&"LoopingPackages".into()));
            let travelling = match c.get(&"TravellingPackages".into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(e) = t else { return None };
                        let k = ints(e.get(&"Target".into()));
                        (k.len() == 3)
                            .then(|| ([k[0], k[1], k[2]], packages(e.get(&"Packages".into()))))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            pk.chains.load_packages(pos, looping, travelling);
            conveyors.push(pos);
        }
        for c in list(data, "ports") {
            let Some(pos) = pos_of(&c) else { continue };
            let f = ints(c.get(&"Frog".into()));
            let frog = f.first().is_some_and(|v| *v != 0);
            let port = pk.port(pos, frog);
            port.accepts = f.get(1).is_none_or(|v| *v != 0);
            port.goggles = f.get(2).is_some_and(|v| *v != 0);
            port.filter = text(c.get(&"AddressFilter".into()));
            if let Some(Tag::List(items)) = c.get(&"Inventory".into()) {
                for t in items {
                    let Tag::Compound(e) = t else { continue };
                    let slot = ints(e.get(&"Slot".into())).first().copied().unwrap_or(-1);
                    if let (Some(s), Some(b)) = (
                        port.items.get_mut(slot.max(0) as usize),
                        parcel_from(host, Some(t)),
                    ) {
                        *s = Some(b);
                    }
                }
            }
            let s = ints(c.get(&"Station".into()));
            if s.len() == 3 {
                port.station = Some([s[0], s[1], s[2]]);
            }
            let t = ints(c.get(&"Target".into()));
            if t.len() == 8 {
                port.target = Some(Target {
                    conveyor: [t[0], t[1], t[2]],
                    chain_pos: float(c.get(&"ChainPos".into())),
                    connection: (t[3] != 0).then_some([t[4], t[5], t[6]]),
                    flipped: t[7] != 0,
                });
            }
            let deposit = ints(c.get(&"Deposit".into()))
                .first()
                .is_some_and(|v| *v != 0);
            if let Some(b) = parcel_from(host, c.get(&"AnimatedPackage".into())) {
                pk.start(pos, b, deposit);
            }
            pk.register(pos);
            pk.wake(pos);
            ports_loaded.push(pos);
        }
    });
    for pos in ports_loaded {
        note_maxes(lv, pos);
        station_ports::attach(dim, pos);
    }
    for pos in conveyors {
        sync_far(dim, pos);
        with(dim, |pk| pk.conveyor_changed(pos));
    }
}

/// A column left a level: what was in it is forgotten (kept in its save).
fn chunk_unload(dim: usize, chunk: [i32; 2]) {
    if !present() {
        return;
    }
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let conveyors = with(dim, |pk| {
        pk.packagers.retain(|p, _| !inside(p));
        pk.active.retain(|p| !inside(p));
        let ports: Vec<[i32; 3]> = pk.ports.keys().filter(|p| inside(p)).copied().collect();
        for p in ports {
            // `saveOfflineBuffer`: a postbox joined to a station leaves its packages there.
            if let Some(port) = pk.ports.get(&p) {
                let items: Vec<Option<ItemStack>> = port
                    .items
                    .iter()
                    .map(|s| s.as_ref().map(|b| b.stack.clone()))
                    .collect();
                station_ports::save_buffer(dim, p, &items);
            }
            pk.remove_port(p);
        }
        let conveyors: Vec<[i32; 3]> = pk
            .chains
            .conveyors
            .keys()
            .filter(|p| inside(p))
            .copied()
            .collect();
        for p in &conveyors {
            pk.chains.unload(*p);
        }
        conveyors
    });
    for p in conveyors {
        with_world(|w| w.dims[dim].kinetics.set_far(p, Vec::new()));
    }
}
