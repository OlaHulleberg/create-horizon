//! Create's logistics: belts, depots, chutes and smart chutes, item vaults, funnels, andesite
//! and brass tunnels (`Logistics`), and mechanical arms and weighted ejectors (`Arms`).
//!
//! Their state is the engine's (`create_engine::world`); their stacks live in their block
//! entities' slots (depots, ejectors, vaults, chutes: `block_container`), which they give and
//! take through `ITEMS` (`handlers`), and they reach every other inventory through `ITEMS` in
//! transactions (`items`). Clients get their contents on Create's channels (`create:depot`,
//! `create:belt`, `create:arm`, `create:ejector`), or as synced block entity keys (belts' dye
//! and casing, funnels' and tunnels' flaps, filters, a windowed chute's stack), and draw them
//! from the kinetic block entity's renderer (`client`): again each frame while they move, else
//! when they change. Belts' segments scroll their texture and wear their casing through standalone
//! models drawn with the terrain's sprites (`standalone_models`, `terrain_model` with a UV shift).
//!
//! Left out, for want of an API or because another subsystem owns it:
//! - `BakedModel.isGui3d` (how a stack lies on a belt or depot) is not in the mod API: an item
//!   with a block stands in for it (`items::is_block_item`).
//! - The machines above a belt or depot (presses, deployers, spouts: `Subsystem::belt_process`),
//!   and items landing on basins, saws and item drains are the processing and fluids
//!   subsystems'. A sequenced assembly's progress on a stack (processing's `stacks`) goes with
//!   it onto and off a belt (`belt_stack`).
//! - Containers that are not Create's (vanilla's, other mods') say nothing to Create when they
//!   change (`contents_changed` reaches only a block's own parts), so a chute under one looks
//!   again every `RETRY` ticks.
//! - An arm's points and an ejector's target are chosen with their items as in Create
//!   (`arms::select_point`, `arms::select_ejector_target`), outlined on their chooser's client,
//!   and dropped by left-clicking them (`arms::left_click`).
//! - Belts outline and collide as Create's `BeltShapes` (declared by `tools/create-data`), but
//!   every entity collides with the shape cut at 19 px, where Create gives its passengers the whole
//!   outline (content shapes select by entity class, not by riding). A belt's casing is drawn by
//!   its block entity (its block is never a terrain model, `ENTITYBLOCK_ANIMATED` in every
//!   state), as `BeltModel` would draw it.
//! - Players riding a belt are pushed by their own client (`push_player`), other entities by
//!   their motion (`EntityRef::set_velocity`): the mod API has no `Entity.move`. Up a slope they
//!   climb the belt's stairs by stepping up, where Create moves them.
//! - Item vaults' connected textures (`ItemVaultCTBehaviour`) are the connected-texture rules
//!   `create-data prepare` writes: a vault joins any vault block of its axis and size beside it,
//!   not only its own vault's.
//! - A filter slot's amount board (`amount`) is opened by a held use as Create's; a smart chute's
//!   client takes its stacks to fall when it decides whether to hold (the server decides by the
//!   fans' real pull), and a portable storage interface gives up to the amount, never exactly.
//! - A brass tunnel's mode value box and the comparators beside a vault's other blocks are the
//!   interaction and redstone subsystems'.
//! - Belts offer no `ITEMS` handler (their items are the engine's, and a handler can't learn its
//!   transaction aborted: API_GAPS.md), so tunnels pass none of their belt's on: a brass
//!   tunnel's handler (`handlers::TunnelItems`) only takes a stack to hand out.
//!
//! The kinetic block entity type is declared by `Kinetics`; `Entities`, after it, adds the
//! handlers and the renderer to it. Belts' segments and pulleys are posed partials that
//! `kinetics::draw_part` leaves to this renderer.

pub(crate) mod amount;
pub mod arms;
mod belt;
pub(crate) mod carried;
pub(crate) mod chute;
pub(crate) mod client;
pub(crate) mod depot;
mod flaps;
pub(crate) mod funnel;
mod handlers;
pub(crate) mod items;
pub(crate) mod placement;
mod slicer;
pub(crate) mod tunnel;

pub use arms::Arms;
// What the processing machines hand their results to beside them: a belt or depot taking a stack
// from a side, and the two facts every stack placed on one needs.
pub(crate) use belt::{
    belt_segment, direct_input_accepts, direct_input_at, insert_into_from, put_on_from_above,
    set_casing,
};
pub(crate) use funnel::{export_up, funnel_at, funnel_passes};
pub(crate) use items::{random_angle, upright};
pub(crate) use tunnel::set_brass_mode;

use super::items::filters;
use super::kinetics;
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use mod_api::{
    BlockPart, ClientHost, Compound, ContentsChangedEvent, Flow, Hand, HostBlockStates, HostSide,
    ITEMS, InteractionResult, ItemAfterVerticalCollisionEvent, ModContainer, PlayerRef, Registrar,
    RegistrationContext, ServerHost, Tag, UseItemOnEvent, UseWithoutItemEvent,
};

pub struct Logistics;

/// The blocks whose stacks are their block entity's slots, with the slot count (a brass
/// tunnel's: the stack it hands out).
const CONTAINERS: [(Block, usize); 6] = [
    (Block::Depot, create_engine::depot::SLOTS),
    (Block::WeightedEjector, create_engine::depot::SLOTS),
    (Block::ItemVault, create_engine::vault::SLOTS),
    (Block::Chute, 1),
    (Block::SmartChute, 1),
    (Block::BrassTunnel, 1),
];

/// The blocks a player uses, items land on, or whose contents a hopper changes.
const PARTS: [Block; 12] = [
    Block::Depot,
    Block::WeightedEjector,
    Block::Belt,
    Block::MechanicalArm,
    Block::Chute,
    Block::SmartChute,
    Block::ItemVault,
    Block::AndesiteFunnel,
    Block::BrassFunnel,
    Block::AndesiteBeltFunnel,
    Block::BrassBeltFunnel,
    Block::BrassTunnel,
];

impl Subsystem for Logistics {
    fn write_entities(&self, lv: &mut ServerLevel<'_>) {
        depot::save_depots(lv);
        belt::save_belts(lv);
    }

    fn draw_carried(
        &self,
        host: &dyn mod_api::ClientHost,
        block: create_engine::Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        client::draw_carried(host, block, event);
    }

    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        out: &mut Compound,
    ) {
        carried::carry_out(lv, pos, block, out);
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        data: &Compound,
        turn: create_engine::contraption::Transform,
    ) {
        carried::carry_in(lv, pos, block, data, turn);
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        for (block, slots) in CONTAINERS {
            let all: Vec<usize> = (0..slots).collect();
            // The game's own handler (hoppers below a chute) reads these; Create's rules are
            // `handlers`', asked first.
            let inputs = if matches!(block, Block::Depot | Block::WeightedEjector) {
                vec![0]
            } else if block == Block::BrassTunnel {
                // Only through its handler (`BrassTunnelItemHandler`, `handlers`).
                Vec::new()
            } else {
                all.clone()
            };
            r.block_container(
                &states::key(block),
                ModContainer {
                    slots,
                    faces: std::array::from_fn(|_| all.clone()),
                    inputs,
                    outputs: if block == Block::BrassTunnel {
                        Vec::new()
                    } else {
                        all
                    },
                    menu: None,
                },
            );
        }
        placement::register(r);
        for block in PARTS {
            r.block_part(states::key(block), LogisticsBlock);
        }
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        let (vaults, filters) = vaults_from_nbt(&*lv.host, data);
        let side_filters = tunnel_filters_from_nbt(&*lv.host, data);
        let belts = belts_from_nbt(data);
        let brass = brass_from_nbt(data);
        let amounts: Vec<([i32; 3], create_engine::board::FilterAmount)> =
            match data.get(&FILTER_AMOUNTS.into()) {
                Some(Tag::IntArray(v)) => v
                    .as_chunks::<5>()
                    .0
                    .iter()
                    .map(|a| {
                        let amount = create_engine::board::FilterAmount::new(
                            a[3].clamp(0, 255) as u8,
                            a[4] != 0,
                        );
                        ([a[0], a[1], a[2]], amount)
                    })
                    .collect(),
                _ => Vec::new(),
            };
        let dim = lv.dim;
        with_world(|w| {
            w.dims[dim].vaults.load(vaults.into_iter());
            w.dims[dim].filter_amounts.extend(amounts);
            w.dims[dim].chute_filters.extend(filters);
            w.dims[dim].tunnel_filters.extend(side_filters);
            w.load_belt_items(dim, belts.into_iter());
            w.load_brass(dim, brass.into_iter());
        });
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let dim = lv.dim;
        let (cx, cz) = (chunk[0], chunk[1]);
        let side_filters = with_world(|w| {
            let mut v: Vec<(([i32; 3], Dir), create_engine::filter::Filter)> = w.dims[dim]
                .tunnel_filters
                .iter()
                .filter(|((p, _), _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .map(|(k, f)| (*k, f.clone()))
                .collect();
            v.sort_by_key(|e| (e.0.0, e.0.1 as u8));
            v
        })
        .unwrap_or_default();
        tunnel_filters_to_nbt(&*lv.host, data, &side_filters);
        let Some((vaults, filters, belts, brass)) = with_world(|w| {
            let mut filters: Vec<([i32; 3], create_engine::filter::Filter)> = w.dims[dim]
                .chute_filters
                .iter()
                .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .map(|(p, f)| (*p, f.clone()))
                .collect();
            filters.sort_by_key(|e| e.0);
            (
                w.dims[dim].vaults.column(cx, cz),
                filters,
                w.column_belts(dim, cx, cz),
                w.column_brass(dim, cx, cz),
            )
        }) else {
            return;
        };
        vaults_to_nbt(&*lv.host, data, &vaults, &filters);
        // `FilterAmount` and `UpTo` of the filter slots set: x, y, z, count, up to.
        let mut amounts: Vec<[i32; 5]> = with_world(|w| {
            w.dims[dim]
                .filter_amounts
                .iter()
                .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .map(|(p, a)| [p[0], p[1], p[2], i32::from(a.count), i32::from(a.up_to)])
                .collect()
        })
        .unwrap_or_default();
        if !amounts.is_empty() {
            amounts.sort();
            let flat = amounts.into_iter().flatten().collect();
            data.insert(FILTER_AMOUNTS.into(), Tag::IntArray(flat));
        }
        if !belts.is_empty() {
            data.insert("belts".into(), belts_to_nbt(&belts));
        }
        if !brass.is_empty() {
            data.insert("brass".into(), brass_to_nbt(&brass));
        }
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        placement::tick_vault_layers(lv);
        chute::tick_vaults(lv);
        chute::tick_chute_shapes(lv);
        chute::tick_chutes(lv);
        funnel::tick_funnel_shapes(lv);
        funnel::tick_funnels(lv);
        tunnel::tick_tunnels(lv);
        tunnel::tick_brass(lv);
        belt::tick_covers(lv);
        belt::tick_transport(lv);
        depot::send_changed_depots(lv);
        belt::transport_passengers(lv);
        funnel::collect_inside(lv);
        flaps::send(lv);
    }

    /// `onWrenched` of Create's logistics blocks (sneaking, they are picked up as any block):
    /// a funnel toggles extracting, a belt funnel its shape, a chute its window or casing, a
    /// tunnel its window; a belt loses its casing or pulley (`belt::wrench_belt`). Each of them
    /// is never turned.
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        player: PlayerRef,
        pos: [i32; 3],
        block: Block,
        clicked: Dir,
        _point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if sneaking {
            return false;
        }
        let prop = |lv: &ServerLevel<'_>, key: &str| -> Option<String> {
            let state = lv.block_state(pos)?;
            let facts = lv.host.block_states();
            state
                .property(&facts, key)
                .ok()
                .flatten()
                .map(str::to_owned)
        };
        let set = |lv: &mut ServerLevel<'_>, key: &str, value: &str| {
            lv.set_property(pos, key, value, super::server::update_all());
        };
        match block {
            Block::Belt => {
                belt::wrench_belt(lv, pos, player, clicked);
            }
            // FunnelBlock.onWrenched: `cycle(EXTRACTING)`.
            Block::AndesiteFunnel | Block::BrassFunnel => {
                let extracting = prop(lv, "extracting").as_deref() == Some("true");
                set(lv, "extracting", if extracting { "false" } else { "true" });
            }
            // BeltFunnelBlock.onWrenched: pulling and pushing swap, extended and retracted
            // swap (a funnel over a sloped belt stays retracted).
            Block::AndesiteBeltFunnel | Block::BrassBeltFunnel => {
                let sloped = belt::belt_segment(lv, [pos[0], pos[1] - 1, pos[2]])
                    .is_some_and(|(_, _, s)| s.slope != create_engine::belt::Slope::Horizontal);
                let next = match prop(lv, "shape").as_deref() {
                    Some("pulling") => "pushing",
                    Some("pushing") => "pulling",
                    Some("extended") => "retracted",
                    Some("retracted") if !sloped => "extended",
                    _ => return true,
                };
                set(lv, "shape", next);
                crate::glue::advancements::funnel_kiss(lv, player, pos);
            }
            // ChuteBlock.onWrenched: an encased chute loses its casing; a vertical one swaps
            // its window; an intersection passes.
            Block::Chute => {
                let down = prop(lv, "facing").as_deref() == Some("down");
                match prop(lv, "shape").as_deref() {
                    Some("intersection") => {}
                    Some("encased") => {
                        set(lv, "shape", "normal");
                        // The casing's particles: the industrial iron block's.
                        if let Some(iron) = lv.host.block_state_id("create:industrial_iron_block") {
                            lv.destroy_effect(pos, iron);
                        }
                    }
                    Some("normal") if down => set(lv, "shape", "window"),
                    Some(_) if down => set(lv, "shape", "normal"),
                    _ => {}
                }
            }
            // BeltTunnelBlock.onWrenched: a straight tunnel's window opens or closes.
            Block::AndesiteTunnel | Block::BrassTunnel => match prop(lv, "shape").as_deref() {
                Some("window") => set(lv, "shape", "closed"),
                Some("closed") => set(lv, "shape", "window"),
                _ => {}
            },
            _ => return false,
        }
        true
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(level) = player.level(&*host) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let dim = lv.dim;
        let depots: Vec<[i32; 3]> =
            with_world(|w| w.dims[dim].depots.keys().copied().collect()).unwrap_or_default();
        depot::send_depots(&mut lv, &[player], &depots);
        let belts: Vec<[i32; 3]> =
            with_world(|w| w.dims[dim].belts.keys().copied().collect()).unwrap_or_default();
        belt::send_belts(&mut lv, &[player], &belts);
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        client::logistics_message(host, channel, data)
    }

    /// The client's prediction of a click a belt takes (`client::predict_click`).
    /// `ArmInteractionPointHandler` and `EjectorTargetHandler.leftClickingBlocksDeselectsThem`.
    fn left_click_block(
        &self,
        host: &mut dyn mod_api::ServerHost,
        event: &mut mod_api::LeftClickBlockEvent,
    ) -> mod_api::Flow {
        arms::left_click(host, event.pos)
    }

    fn right_click_block(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> mod_api::Flow {
        // `FunnelItem.funnelItemAlwaysPlacesWhenUsed`: a funnel is placed, never the clicked
        // block used (a chest opened).
        let id = match &host {
            mod_api::HostSide::Client(h) => event.click.stack.map(|s| s.item().id(&**h)),
            mod_api::HostSide::Server(h) => event.click.stack.map(|s| s.item().id(&**h)),
        };
        if matches!(
            id.as_deref(),
            Some("create:andesite_funnel" | "create:brass_funnel")
        ) {
            *event.use_block = mod_api::TriState::False;
        }
        match host {
            mod_api::HostSide::Client(host) => client::predict_click(host, event),
            mod_api::HostSide::Server(_) => mod_api::Flow::Continue,
        }
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick_client_depots(host);
        client::tick_client_belts(host);
        client::tick_client_flaps();
        belt::carry_local_player(host);
    }

    fn client_reset(&self) {
        client::reset_logistics();
    }

    /// The belt partials, drawn with their texture scrolling, and the model naming the
    /// scrolling sprites (so the block sprites hold them).
    fn standalone_models(&self, models: &mut Vec<String>) {
        models.extend(
            create_engine::belt::PARTIAL_MODELS
                .iter()
                .map(|m| (*m).to_owned()),
        );
        models.extend(
            create_engine::belt::CASING_MODELS
                .iter()
                .map(|m| (*m).to_owned()),
        );
        models.extend(
            create_engine::belt::COVER_MODELS
                .iter()
                .map(|m| (*m).to_owned()),
        );
        models.push("create:block/belt_scroll_sprites".to_owned());
        models.extend(client::FLAP_MODELS.iter().map(|m| (*m).to_owned()));
        models.extend(client::ARM_TINTED.iter().map(|m| (*m).to_owned()));
    }
}

/// What logistics adds to the kinetic block entity type: its `ITEMS` handlers and its
/// renderer. Registered after `Kinetics`, which declares the type; it takes part in nothing
/// else.
pub struct Entities;

/// A block entity key holding the item a brass funnel's or smart chute's filter slot shows
/// (an empty string: none), synced for its clients to draw.
pub(crate) const FILTER: &str = "Filter";

/// A block entity key holding a belt segment's dye (`BeltBlockEntity.color`: a `DyeColor` name,
/// an empty string or none for an undyed belt), synced for its clients to draw.
pub(crate) const BELT_COLOR: &str = "Dye";

/// Block entity keys of a cased belt segment: its casing (`andesite` or `brass`), and 1 while a
/// block covers it (`BeltBlockEntity.casing`, `covered`), synced for its clients to draw.
pub(crate) const BELT_CASING: &str = "Casing";
pub(crate) const BELT_COVERED: &str = "Covered";

/// The key of the filter slots' amounts in a column's saved data.
const FILTER_AMOUNTS: &str = "filter_amounts";

/// Shows filter `filter` in the slot of the block at `pos` (`FILTER`).
pub(crate) fn show_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    filter: Option<&create_engine::filter::Filter>,
) {
    let id = filter
        .and_then(|f| filters::filter_stack(&*lv.host, f))
        .map(|s| items::id_of(&*lv.host, &s))
        .unwrap_or_default();
    if let Some(entity) = lv.host.block_entity(lv.level, pos) {
        let data = Compound::from([(FILTER.into(), Tag::String(id.as_str().into()))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

impl Subsystem for Entities {
    /// Arms and ejectors move their own parts every frame, sliding stacks and belts' items
    /// move (`client::animated`).
    fn animated(
        &self,
        block: create_engine::Block,
        pos: [i32; 3],
        entity: &mod_api::ModBlockEntity,
    ) -> bool {
        client::animated(block, pos, entity)
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            super::drawn::SHOWN,
            FILTER,
            BELT_COLOR,
            BELT_CASING,
            BELT_COVERED,
            flaps::KEY,
            tunnel::SIDE_FILTERS,
            chute::CHUTE_ITEM,
            chute::CHUTE_POSITION,
            chute::CHUTE_MOTION,
        ]
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let Some(kind) = kinetics::kind() else {
            crate::create_log!("[create] no kinetic block entity type: logistics has no renderer");
            return;
        };
        r.block_entity_capability_before_game(&ITEMS, &kind, handlers::provide);
        if let Err(e) = r.block_entity_part(&kind, client::LogisticsEntity) {
            crate::create_log!("[create] the logistics block entity part was refused: {e:?}");
        }
    }
}

/// The Create block at `pos` of `lv` and its state's facts.
/// `BeltBlock.updateEntityAfterFallOn`: an entity other than an item on the ground over a belt
/// is noted to be carried (`belt::stepped_on`).
pub fn block_step_on(event: &mut mod_api::BlockStepOnEvent<'_>) {
    if event.kind == "minecraft:item" {
        return;
    }
    let block = states::id(event.state)
        .and_then(|id| with_world(|w| w.table.get(id).map(|(b, _)| b)))
        .flatten();
    let Some(dim) = super::world::level_index(event.dimension) else {
        return;
    };
    match block {
        Some(Block::Belt) => belt::stepped_on(dim, event.position, event.entity as u32),
        // `EjectorBlock.updateEntityAfterFallOn`: a player's own client triggers it.
        Some(Block::WeightedEjector)
            if !matches!(event.kind, "minecraft:player" | "minecraft:experience_orb") =>
        {
            arms::ejector_stepped_on(dim, event.position, event.entity as u32)
        }
        _ => {}
    }
}

pub fn info_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, StateInfo)> {
    let id = lv.block(pos);
    if id == 0 {
        return None;
    }
    with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
}

/// Whether the block at `pos` has a collision shape (`BeltProcessingBehaviour.isBlocked`,
/// an ejector's flight path).
pub fn has_collision(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let cell = [
        f64::from(pos[0]) + 1e-4,
        f64::from(pos[1]) + 1e-4,
        f64::from(pos[2]) + 1e-4,
        f64::from(pos[0]) + 1.0 - 1e-4,
        f64::from(pos[1]) + 1.0 - 1e-4,
        f64::from(pos[2]) + 1.0 - 1e-4,
    ];
    lv.level
        .collision_boxes(&*lv.host, cell)
        .is_some_and(|b| !b.is_empty())
}

/// Arms using the point `pos` look again.
fn mark_touched(dim: usize, pos: [i32; 3]) {
    with_world(|w| w.dims[dim].arms.touched(pos));
}

/// The level of a server-side event.
fn level_of<'a>(host: &'a mut dyn ServerHost, dimension: &'a str) -> Option<ServerLevel<'a>> {
    ServerLevel::new(host, dimension)
}

/// The block part of Create's logistics blocks: used by players, landed on by items, its slots
/// changed by hoppers.
struct LogisticsBlock;

/// A player's click on the logistics block `state` at `click.pos`, with or without an item
/// (Create handles both in `useItemOn`).
fn use_block(
    host: &mut dyn ServerHost,
    state: mod_api::BlockState,
    click: &mod_api::BlockClick<'_>,
) -> Option<InteractionResult> {
    let id = states::id(state)?;
    let (block, _) = with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()?;
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = level_of(host, &dimension)?;
    let pos = click.pos;
    let face = mod_api::Direction::ALL
        .into_iter()
        .find(|d| Dir::ALL[*d as usize].offset() == click.hit.face)?;
    let main = click.hand == Hand::Main;
    let held = click.stack.map(|s| items::id_of(&*lv.host, s));
    let used = match block {
        Block::Depot => depot::use_depot(&mut lv, pos, face, click.player, main),
        // EjectorBlock.useItemOn: as a depot's (a wrench passes on to the wrench).
        Block::WeightedEjector if held.as_deref() != Some("create:wrench") => {
            depot::use_depot(&mut lv, pos, face, click.player, main)
        }
        Block::MechanicalArm => arms::use_arm(&mut lv, pos, click.player),
        Block::SmartChute => {
            chute::use_smart_chute(&mut lv, pos, click) || chute::use_chute(&mut lv, pos, click)
        }
        Block::Chute => chute::use_chute(&mut lv, pos, click),
        Block::BrassFunnel | Block::BrassBeltFunnel => {
            funnel::use_funnel_filter(&mut lv, pos, click)
        }
        // A filter slot first (`ValueSettingsInputHandler`), then `useWithoutItem`: an empty
        // hand takes the stacks of the tunnel's group.
        Block::BrassTunnel => {
            tunnel::use_tunnel_filter(&mut lv, pos, click)
                || click.stack.is_none() && main && tunnel::grab_stacks(&mut lv, pos, click.player)
        }
        Block::Belt => {
            // A potion named with its contents: a water bottle washes the belt as water does.
            let potion = click
                .stack
                .map(|s| items::key_of(&*lv.host, s))
                .filter(|k| create_engine::tank::split_potion_item(k).is_some());
            belt::use_belt_with(
                &mut lv,
                pos,
                click.player,
                click.hand,
                potion.or(held).as_deref(),
                (Dir::ALL[face as usize], click.hit.point),
            )
        }
        _ => false,
    };
    used.then_some(InteractionResult::Success)
}

#[mod_api::horizon_part]
impl BlockPart for LogisticsBlock {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        use_block(host, event.state, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        // Create's blocks open no menu: an unused click passes, so the held item is used (a
        // block placed against a depot's side), never the barrel template's menu.
        Some(use_block(host, event.state, event.click).unwrap_or(InteractionResult::Pass))
    }

    /// `BeltBlock.updateEntityAfterFallOn` / `entityInside` for an item, and
    /// `SharedDepotBlockMethods.onLanded`: a belt takes a landing stack onto its segment (when
    /// nothing just put there is in the way), a depot takes it when it holds nothing, a chute
    /// while empty, an inserting funnel into its inventory.
    fn item_after_vertical_collision(
        &self,
        host: HostSide,
        event: &mut ItemAfterVerticalCollisionEvent<'_>,
    ) -> Flow {
        let HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        if event.client {
            return Flow::Continue;
        }
        let Some(mut lv) = level_of(host, event.dimension) else {
            return Flow::Continue;
        };
        let thrower = crate::glue::advancements::package_thrower(event.entity, event.stack);
        item_landed(&mut lv, event.block_pos, event.item_pos, event.stack);
        crate::glue::advancements::package_landed(&mut lv, thrower, event.block_pos, event.stack);
        Flow::Continue
    }

    /// `updateShape` from above: a cased belt looks at its cover again (`BeltBlock`), a chute
    /// at its shape (`AbstractChuteBlock`).
    fn update_shape(
        &self,
        _level: &mut dyn mod_api::LevelTicksHost,
        event: &mut mod_api::UpdateShapeEvent,
    ) -> Option<mod_api::BlockState> {
        if event.direction != mod_api::Direction::Up {
            return None;
        }
        let block = states::id(event.state)
            .and_then(|id| with_world(|w| w.table.get(id).map(|(b, _)| b)))
            .flatten();
        let dim = super::world::level_index(event.dimension.key().id())?;
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        match block {
            Some(Block::Belt) => belt::cover_check(dim, pos),
            Some(Block::Chute) => {
                with_world(|w| w.dims[dim].chute_shapes.push(pos));
            }
            _ => {}
        }
        None
    }

    /// `BeltBlock.entityInside`: what stands on a belt is carried at the end of the tick.
    fn entity_inside(
        &self,
        _level: &mut dyn mod_api::LevelRandomHost,
        event: &mut mod_api::EntityInsideEvent<'_>,
    ) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        if event.kind == "minecraft:item" {
            // `FunnelBlock.entityInside`: an item inside a funnel, taken at the level's tick.
            let funnel = with_world(|w| {
                w.table
                    .get(id)
                    .map(|(b, _)| matches!(b, Block::AndesiteFunnel | Block::BrassFunnel))
            })
            .flatten();
            if funnel == Some(true)
                && let Some(dim) = super::world::level_index(event.dimension)
            {
                let pos = [event.pos.x, event.pos.y, event.pos.z];
                funnel::item_inside(dim, pos, event.entity as u32);
            }
            return;
        }
        let belt = with_world(|w| w.table.get(id).map(|(b, _)| b == Block::Belt)).flatten();
        if belt != Some(true) {
            return;
        }
        let Some(dim) = super::world::level_index(event.dimension) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        belt::passenger_inside(dim, pos, event.entity as u32);
    }

    fn contents_changed(&self, host: &mut dyn ServerHost, event: &mut ContentsChangedEvent) {
        let Some(dimension) = event.level.dimension().id(&*host) else {
            return;
        };
        let Some(lv) = level_of(host, &dimension) else {
            return;
        };
        contents_changed(&lv, [event.pos.x, event.pos.y, event.pos.z]);
    }
}

/// A container of Create's at `pos` changed (a hopper, a pipe, any handler's commit): chutes
/// asleep under it (or under its vault) look into it again; arms using it as a point, and an
/// ejector it is, look again; so do smart observers and threshold switches looking at it; a
/// chute given a stack lets it fall from the top; a depot's held stack arrives or leaves.
fn contents_changed(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    let Some((block, _)) = info_at(lv, pos) else {
        return;
    };
    let dim = lv.dim;
    with_world(|w| {
        w.container_changed(dim, pos);
        w.detectors_contents_changed(dim, pos);
        w.dims[dim].arms.touched(pos);
    });
    arms::wake_ejectors(dim, &[pos]);
    match block {
        Block::Chute | Block::SmartChute => {
            if chute::chute_stack(lv, pos).is_some() {
                with_world(|w| {
                    w.dims[dim].chutes.entry(pos).or_insert(1.0);
                });
            }
        }
        Block::Depot | Block::WeightedEjector => depot::depot_contents_changed(lv, pos),
        Block::BrassTunnel => tunnel::slot_changed(lv, pos),
        _ => {}
    }
}

/// An item entity landed at `at` on the block at `pos`: Create's belts, depots, ejectors,
/// chutes and inserting funnels take what they take of `stack`.
fn item_landed(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    at: [f64; 3],
    stack: &mut Option<mod_api::ItemStack>,
) {
    let Some(s) = stack.clone() else {
        return;
    };
    // The entity's own block, else the one under it.
    let own = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    let kind = |lv: &ServerLevel<'_>, p| info_at(lv, p).map(|(b, _)| b);
    let (spot, block) = match (kind(lv, own), kind(lv, pos)) {
        (Some(b @ Block::Belt), _) => (own, b),
        (_, Some(b @ Block::Belt)) => (pos, b),
        (_, Some(b @ Block::Depot)) => (pos, b),
        // EjectorBlock.updateEntityAfterFallOn: an item stack lands as on a depot.
        (_, Some(Block::WeightedEjector)) => (pos, Block::Depot),
        (_, Some(b @ (Block::Chute | Block::SmartChute))) => (pos, b),
        (_, Some(b @ (Block::AndesiteFunnel | Block::BrassFunnel))) => (pos, b),
        _ => return,
    };
    match block {
        Block::Chute | Block::SmartChute => {
            if chute::chute_take(lv, spot, &s) {
                *stack = None;
            }
        }
        Block::AndesiteFunnel | Block::BrassFunnel => {
            // FunnelBlock.entityInside, inserting: into the inventory it is on, as much as fits.
            let Some(f) = funnel::funnel_at(lv, spot) else {
                return;
            };
            if f.extracting || f.powered || !funnel::funnel_passes(lv, spot, &s) {
                return;
            }
            let behind = create_engine::funnel::behind(spot, f.facing);
            let count = s.count();
            let moved = s.clone();
            *stack = items::insert_item(lv, behind, items::face(f.facing), s);
            if stack.as_ref().is_none_or(|l| l.count() != count) {
                flaps::funnel(lv, spot, true);
                crate::glue::redstone::detectors::funnel_transfer(lv, spot, &moved);
            }
        }
        Block::Depot => {
            // DirectBeltInputBehaviour from above: only into an empty depot (an ejector's while
            // charged).
            let dim = lv.dim;
            // An ejector merges it onto what it holds, up to its stack size (`enableMerging`).
            if arms::is_ejector(dim, spot) {
                let key = items::key_of(&*lv.host, &s);
                let count = s.count().min(255) as u8;
                let left = arms::depot_insert(lv, spot, &key, count, false);
                if left < count {
                    *stack = s.shrink(u32::from(count - left));
                }
                return;
            }
            if !depot::depot_items(lv, spot).is_empty() || !arms::ejector_accepts(dim, spot) {
                return;
            }
            let id = items::id_of(&*lv.host, &s);
            let max = u32::from(items::stack_limit(&*lv.host, &id));
            let count = s.count().min(max);
            let Some(put) = items::with_count(&s, count) else {
                return;
            };
            if !items::set_slot(lv, spot, 0, Some(put)) {
                return;
            }
            *stack = s.shrink(count);
            let held = create_engine::depot::Held::new(
                items::random_angle(lv, spot),
                items::upright(&*lv.host, &id),
            );
            with_world(|w| w.depot_put(dim, spot, Some(held)));
        }
        _ => {
            // A belt carries the stack by its name, components and all (a sequenced
            // assembly's progress apart).
            let (id, progress) = belt::belt_name(&*lv.host, &s);
            let count = s.count().min(u32::from(items::stack_limit(&*lv.host, &id))) as u8;
            if let Some(left) = belt::put_on_from_above(lv, spot, &id, count, progress, false)
                && left < count
            {
                *stack = s.shrink(u32::from(count - left));
            }
        }
    }
}

/// Vault blocks' parts (`ItemVaultBlockEntity.write`: `Controller`, `Size`, `Length`,
/// `Uninitialized`) and smart chutes' filters saved with a column: `vaults` holds x, y, z, the
/// controller's x, y, z, width, length and 1 when it still looks for its vault, for each block;
/// `chute_filters` the positions and the filter stacks (an item id in saves from before filter
/// items).
fn vaults_to_nbt(
    host: &dyn mod_api::Host,
    own: &mut Compound,
    vaults: &[([i32; 3], create_engine::vault::Part)],
    filters: &[([i32; 3], create_engine::filter::Filter)],
) {
    if !vaults.is_empty() {
        let v = vaults
            .iter()
            .flat_map(|(p, x)| {
                let c = x.controller.unwrap_or(*p);
                [
                    p[0],
                    p[1],
                    p[2],
                    c[0],
                    c[1],
                    c[2],
                    x.width,
                    x.length,
                    i32::from(x.dirty),
                ]
            })
            .collect();
        own.insert("vaults".into(), Tag::IntArray(v));
    }
    if !filters.is_empty() {
        let mut c = Compound::new();
        let pos = filters.iter().flat_map(|(p, _)| *p).collect();
        let items = filters
            .iter()
            .map(|(_, f)| filters::filter_to_nbt(host, f).unwrap_or_else(|| Tag::String("".into())))
            .collect();
        c.insert("pos".into(), Tag::IntArray(pos));
        c.insert("items".into(), Tag::List(items));
        own.insert("chute_filters".into(), Tag::Compound(c));
    }
}

/// Brass tunnels' side filters saved with a column: `tunnel_filters` holds x, y, z and the side
/// of each, and its filter.
pub(super) fn tunnel_filters_to_nbt(
    host: &dyn mod_api::Host,
    own: &mut Compound,
    filters: &[(([i32; 3], Dir), create_engine::filter::Filter)],
) {
    if filters.is_empty() {
        return;
    }
    let mut c = Compound::new();
    let pos = filters
        .iter()
        .flat_map(|((p, d), _)| [p[0], p[1], p[2], *d as i32])
        .collect();
    let items = filters
        .iter()
        .map(|(_, f)| filters::filter_to_nbt(host, f).unwrap_or_else(|| Tag::String("".into())))
        .collect();
    c.insert("pos".into(), Tag::IntArray(pos));
    c.insert("items".into(), Tag::List(items));
    own.insert("tunnel_filters".into(), Tag::Compound(c));
}

/// What `tunnel_filters_to_nbt` wrote.
pub(super) fn tunnel_filters_from_nbt(
    host: &dyn mod_api::Host,
    data: &Compound,
) -> Vec<(([i32; 3], Dir), create_engine::filter::Filter)> {
    let Some(Tag::Compound(c)) = data.get(&"tunnel_filters".into()) else {
        return Vec::new();
    };
    let (Some(Tag::IntArray(pos)), Some(Tag::List(items))) =
        (c.get(&"pos".into()), c.get(&"items".into()))
    else {
        return Vec::new();
    };
    pos.as_chunks::<4>()
        .0
        .iter()
        .zip(items)
        .filter_map(|(p, i)| {
            let side = *Dir::ALL.get(usize::try_from(p[3]).ok()?)?;
            Some((
                ([p[0], p[1], p[2]], side),
                filters::filter_from_nbt(host, i)?,
            ))
        })
        .collect()
}

/// What `vaults_to_nbt` wrote.
#[allow(clippy::type_complexity)]
fn vaults_from_nbt(
    host: &dyn mod_api::Host,
    data: &Compound,
) -> (
    Vec<([i32; 3], create_engine::vault::Part)>,
    Vec<([i32; 3], create_engine::filter::Filter)>,
) {
    let vaults = match data.get(&"vaults".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<9>()
            .0
            .iter()
            .map(|c| {
                let pos = [c[0], c[1], c[2]];
                let controller = [c[3], c[4], c[5]];
                let part = create_engine::vault::Part {
                    // The block's own state gives its axis and `large` when it is found.
                    axis: Axis::X,
                    controller: (controller != pos).then_some(controller),
                    width: c[6].clamp(1, create_engine::vault::MAX_WIDTH),
                    length: c[7].clamp(1, create_engine::vault::max_length(3)),
                    dirty: c[8] != 0,
                    large: false,
                };
                (pos, part)
            })
            .collect(),
        _ => Vec::new(),
    };
    let filters = match data.get(&"chute_filters".into()) {
        Some(Tag::Compound(c)) => match (c.get(&"pos".into()), c.get(&"items".into())) {
            (Some(Tag::IntArray(pos)), Some(Tag::List(items))) => pos
                .as_chunks::<3>()
                .0
                .iter()
                .zip(items)
                .filter_map(|(p, i)| Some(([p[0], p[1], p[2]], filters::filter_from_nbt(host, i)?)))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    (vaults, filters)
}

/// Belts' items as saved (`BeltInventory.write`): for each belt its controller (`pos`), each
/// item's numbers (position and side offset as float bits, angle, count, locked) and its id.
pub(super) fn belts_to_nbt(belts: &[([i32; 3], create_engine::belt::Inventory)]) -> Tag {
    Tag::List(
        belts
            .iter()
            .map(|(c, inv)| {
                let mut b = Compound::new();
                b.insert("pos".into(), Tag::IntArray(c.to_vec()));
                let numbers = inv
                    .items
                    .iter()
                    .flat_map(|i| {
                        [
                            i.position.to_bits() as i32,
                            i.side_offset.to_bits() as i32,
                            i.angle,
                            i32::from(i.count),
                            i32::from(i.locked),
                        ]
                    })
                    .collect();
                b.insert("items".into(), Tag::IntArray(numbers));
                b.insert(
                    "ids".into(),
                    Tag::List(
                        inv.items
                            .iter()
                            .map(|i| Tag::String(i.item.as_str().into()))
                            .collect(),
                    ),
                );
                b.insert(
                    "positive".into(),
                    Tag::IntArray(vec![i32::from(inv.positive)]),
                );
                // TransportedItemStack.serializeNBT: insertion context survives a reload.
                b.insert(
                    "insertion".into(),
                    Tag::IntArray(
                        inv.items
                            .iter()
                            .flat_map(|i| [i.inserted_at, i.inserted_from.map_or(6, |d| d as i32)])
                            .collect(),
                    ),
                );
                // Items in a sequenced assembly: their index and step, and the recipe.
                let assembly: Vec<Tag> = inv
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(n, i)| {
                        let p = i.progress.as_ref()?;
                        let mut a = Compound::new();
                        a.insert("at".into(), Tag::IntArray(vec![n as i32, p.step as i32]));
                        a.insert("id".into(), Tag::String(p.id.as_str().into()));
                        Some(Tag::Compound(a))
                    })
                    .collect();
                if !assembly.is_empty() {
                    b.insert("assembly".into(), Tag::List(assembly));
                }
                Tag::Compound(b)
            })
            .collect(),
    )
}

/// What `belts_to_nbt` wrote.
pub(super) fn belts_from_nbt(data: &Compound) -> Vec<([i32; 3], create_engine::belt::Inventory)> {
    let Some(Tag::List(list)) = data.get(&"belts".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(numbers)) = b.get(&"items".into()) else {
                return None;
            };
            let Some(Tag::List(ids)) = b.get(&"ids".into()) else {
                return None;
            };
            let positive =
                matches!(b.get(&"positive".into()), Some(Tag::IntArray(v)) if v.first() == Some(&1));
            let mut items: Vec<create_engine::belt::Item> = numbers
                .as_chunks::<5>()
                .0
                .iter()
                .zip(ids)
                .filter_map(|(n, id)| {
                    let Tag::String(id) = id else { return None };
                    let mut item = create_engine::belt::Item::new(
                        &id.to_string_lossy(),
                        n[3].clamp(1, 255) as u8,
                        n[2],
                        false,
                    );
                    item.position = f32::from_bits(n[0] as u32);
                    item.side_offset = f32::from_bits(n[1] as u32);
                    item.locked = n[4] != 0;
                    Some(item)
                })
                .collect();
            if let Some(Tag::IntArray(context)) = b.get(&"insertion".into()) {
                for (item, pair) in items.iter_mut().zip(context.as_chunks::<2>().0) {
                    item.inserted_at = pair[0];
                    item.inserted_from = usize::try_from(pair[1]).ok().and_then(|n| Dir::ALL.get(n)).copied();
                }
            }
            if let Some(Tag::List(assembly)) = b.get(&"assembly".into()) {
                for a in assembly {
                    let Tag::Compound(a) = a else { continue };
                    let (Some(Tag::IntArray(at)), Some(Tag::String(id))) =
                        (a.get(&"at".into()), a.get(&"id".into()))
                    else {
                        continue;
                    };
                    let (Some(n), Some(step)) = (at.first(), at.get(1)) else {
                        continue;
                    };
                    if let Some(item) = items.get_mut(*n as usize) {
                        item.progress = Some(create_engine::sequenced::Progress {
                            id: id.to_string_lossy().to_string(),
                            step: (*step).max(0) as u32,
                        });
                    }
                }
            }
            let c = [*pos.first()?, *pos.get(1)?, *pos.get(2)?];
            Some((c, create_engine::belt::Inventory::from_saved(items, positive)))
        })
        .collect()
}

/// Brass tunnels' stacks: x, y, z, mode, side it came in by (6: none), count, then the item ids.
pub(super) fn brass_to_nbt(brass: &[([i32; 3], create_engine::tunnel::Brass)]) -> Tag {
    let mut nums = Vec::new();
    let mut items = Vec::new();
    for (p, b) in brass {
        let (item, n) = b.stack.clone().unwrap_or_default();
        nums.extend([
            p[0],
            p[1],
            p[2],
            b.mode.index() as i32,
            b.entered_from.map_or(6, |d| d as i32),
            i32::from(n),
        ]);
        items.push(Tag::String(item.as_str().into()));
    }
    let mut c = Compound::new();
    c.insert("nums".into(), Tag::IntArray(nums));
    c.insert("items".into(), Tag::List(items));
    Tag::Compound(c)
}

/// What `brass_to_nbt` wrote.
pub(super) fn brass_from_nbt(data: &Compound) -> Vec<([i32; 3], create_engine::tunnel::Brass)> {
    let Some(Tag::Compound(c)) = data.get(&"brass".into()) else {
        return Vec::new();
    };
    let (Some(Tag::IntArray(nums)), Some(Tag::List(items))) =
        (c.get(&"nums".into()), c.get(&"items".into()))
    else {
        return Vec::new();
    };
    nums.as_chunks::<6>()
        .0
        .iter()
        .zip(items)
        .map(|(v, item)| {
            let item = match item {
                Tag::String(s) => s.to_string_lossy(),
                _ => String::new(),
            };
            let n = v[5].clamp(0, 64) as u8;
            (
                [v[0], v[1], v[2]],
                create_engine::tunnel::Brass {
                    stack: (!item.is_empty() && n > 0).then_some((item, n)),
                    entered_from: Dir::ALL.get(v[4].clamp(0, 6) as usize).copied(),
                    mode: create_engine::tunnel::Mode::ALL[v[3].clamp(0, 6) as usize],
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod belt_save_tests {
    use super::*;

    #[test]
    fn belt_inventory_keeps_insertion_context() {
        let mut item = create_engine::belt::Item::new("minecraft:iron_ingot", 16, 180, false);
        item.position = 1.4375;
        item.inserted_at = 1;
        item.inserted_from = Some(Dir::Up);
        let inv = create_engine::belt::Inventory::from_saved(vec![item.clone()], true);
        let data = Compound::from([("belts".into(), belts_to_nbt(&[([0, 8, 0], inv)]))]);
        let saved = belts_from_nbt(&data);
        assert_eq!(saved[0].1.items[0], item);
    }
}
