//! Create's display links, display boards and nixie tubes (and the clipboard): links read the
//! block they are attached to and write its text to nixie tubes, signs, lecterns' books and
//! display boards; the boards' flaps turn toward their text and the tubes glow on clients
//! (`client`). A nixie tube on a train signal shows the signal's panel and lights instead of
//! text (`NixieTubeRenderer.renderAsSignal`, from the signal's state in its block entity data),
//! the lights and their glows added to the frame through the glue's world pipelines. A board's
//! lines also take dyes, glow ink, name tags and clipboards by hand.
//!
//! The sources: speedometers and stressometers, target blocks' redstone power, cuckoo clocks'
//! time and stopwatch, item names along depots, belts and ejectors, nixie tubes' text, a smart
//! observer's item and fluid counts and lists and its package's address, a threshold switch's
//! fill level, who sits on a seat, a tunnel's accumulated items and throughput (told by the
//! belt, `items_through_tunnel`), an enchanting table's max cost, a fluid tank's boiler, an
//! elevator contact's floor (its names and screen are `contact_screen`), a station's summary of
//! the trains coming (`summary`: Create's `GlobalTrainDisplayData`, every train's arrival
//! predictions, by station name filter, with its platform and column settings) and its train's
//! schedule status (refreshed when that train asks, `displayLinkUpdateRequested`), a track
//! observer's train's name, and the status of the factory gauges joined to the link
//! (`FactoryGaugeDisplaySource`; a link joined to gauges reads them wherever it is attached,
//! `getSourcePosition`). A display board in hand clicked on a board goes beside it (its placement
//! helper). Every source Create registers is here but the computer's (no computers) and the two
//! below.
//!
//! The sources' colours (the boiler's bars, the gauges' marks) ride their text as coloured runs
//! (`display::coloured`): signs and lecterns' books show them, boards and nixie tubes take the
//! plain text, as Create's `getString()`.
//!
//! Left out, the API having no way to do it:
//! - The scoreboard and death count sources: the API reads scoreboard objectives only as
//!   console text (`run_command_with_output`), with no typed read or write.
//! - Text read on the server in the game's language: the server has no language of its own
//!   to ask, so a display source's words and an item's name are Create's en_us text (an item's
//!   name its id's path in capitalised words); clients draw the boards' flap characters in their own.
//! - The display boards' text alpha (`.85`, `.975` when glowing): a block entity's text is
//!   drawn opaque.
//! - The glow over the link's bulb as it sends (`LinkBulbRenderer`, `bulb` draws its tube): a
//!   block model drawn additively in a fading colour, and a block entity's model is drawn in its
//!   own tints.

mod bulb;
pub(crate) mod carried;
pub mod client;
pub(crate) mod clipboard;
pub(crate) mod contact_screen;
mod link_item;
mod link_screen;
mod summary;

use super::items::filters;
use super::net;
use super::persist;
use super::redstone::{block_at, info};
use super::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::display::{
    self, Board, BoardState, Config, Face, Lang, Link, NixieState, Reading, Shown, Source, Stats,
    Target,
};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockClick, BlockPart, BlockState, ClientHost, Command, CommandArgumentValue, Compound, Flow,
    InteractionResult, ItemStack, LevelHost, PlayerRef, RegisterCommandsEvent, Registrar,
    RegistrationContext, RunModCommandEvent, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, StateForPlacementEvent, Tag,
    Text, UseItemOnEvent, UseWithoutItemEvent,
};
use std::collections::{HashMap, HashSet};

pub struct Display;

impl Subsystem for Display {
    fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext) {
        for &block in Block::ALL.iter() {
            if matches!(block, Block::DisplayLink | Block::DisplayBoard) {
                r.block_part(states::key(block), DisplayBlock);
            }
        }
        for block in create_engine::dye::NIXIE_TUBES {
            r.block_part(states::key(block), NixieBlock);
        }
        client::register(r);
        clipboard::register(r, cx);
        link_item::register(r);
        if let Some(kind) = super::kinetics::kind()
            && let Err(e) = r.block_entity_part(&kind, bulb::BulbDraw)
        {
            crate::create_log!("[create] the display link's bulb part was refused: {e:?}");
        }
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        &[bulb::PULSE]
    }

    fn world_closed(&self) {
        link_item::forget();
        summary::forget();
    }

    /// Drawn every frame: a display board's flaps turning (`client`), a display link's tube while
    /// it flashes (`bulb`), and nixie tubes as Create's renderer does: one on a train signal shows
    /// its lights (and an invalid one's blink) as the signal changes (`NixieTubeBlockEntity.tick`
    /// looks at it each tick).
    fn animated(&self, block: Block, pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
        display::is_nixie(block)
            || (block == Block::DisplayBoard && client::animated(pos, entity))
            || (block == Block::DisplayLink && bulb::animated(pos, entity))
    }

    fn right_click_block(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        link_item::right_click_block(&host, event);
        Flow::Continue
    }

    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], b: Block, out: &mut Compound) {
        carried::carry_out(lv, pos, b, out);
    }

    fn draw_carried(
        &self,
        host: &dyn ClientHost,
        block: Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        client::draw_carried(host, block, event);
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

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        if let Some(Tag::Compound(c)) = data.get(&"create_display".into()) {
            displays_from_nbt(lv.dim, c);
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        displays_chunk_save(lv.dim, chunk, data);
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_displays(lv);
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            sync_displays(&mut lv, player);
        }
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel == link_screen::CHANNEL {
            configure_from_client(host, player, data);
            return true;
        }
        if channel == contact_screen::CHANNEL {
            contact_screen::client_message(host, player, data);
            return true;
        }
        clipboard::client_message(host, player, channel, data)
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        client::server_message(host, channel, data)
            || contact_screen::server_message(host, channel, data)
            || clipboard::server_message(host, channel, data)
            || link_screen::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
        contact_screen::tick(host);
        clipboard::tick(host);
        link_screen::tick(host);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        clipboard::screen_render_post(host, event);
        contact_screen::screen_render_post(host, event);
        link_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if link_screen::screen_mouse_pressed(host, event) == Flow::Cancel
            || contact_screen::screen_mouse_pressed(host, event) == Flow::Cancel
        {
            return Flow::Cancel;
        }
        clipboard::screen_mouse_pressed(host, event)
    }

    /// The display link bulbs' and the nixie signals' glows (additive world quads).
    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        bulb::render_level_stage(host, event);
        client::render_signal_glows(host, event);
    }

    fn input_handled(&self, host: &mut dyn ClientHost) {
        contact_screen::tick(host);
        clipboard::tick(host);
        link_screen::tick(host);
    }

    fn screen_mouse_released(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        clipboard::screen_mouse_released(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if link_screen::screen_mouse_scrolled(host, event) == Flow::Cancel
            || contact_screen::screen_mouse_scrolled(host, event) == Flow::Cancel
        {
            return Flow::Cancel;
        }
        clipboard::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if link_screen::screen_key_pressed(host, event) == Flow::Cancel
            || contact_screen::screen_key_pressed(host, event) == Flow::Cancel
        {
            return Flow::Cancel;
        }
        clipboard::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if link_screen::screen_char_typed(host, event) == Flow::Cancel
            || contact_screen::screen_char_typed(host, event) == Flow::Cancel
        {
            return Flow::Cancel;
        }
        clipboard::screen_char_typed(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        clipboard::screen_closing(host, event);
        contact_screen::screen_closing(host, event);
        link_screen::screen_closing(host, event);
    }

    fn client_reset(&self) {
        client::reset();
        bulb::forget();
        clipboard::reset();
        contact_screen::reset();
        link_screen::reset();
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // A display link's target (the display link item's selection) and its screen's source
        // and settings, and what a link, nixie tube or display board holds, for tests.
        event.commands.push(Command::op(
            "create_debug display_link <pos:block_pos> <target:text>",
            2,
        ));
        event
            .commands
            .push(Command::op("create_debug display_state <pos:block_pos>", 2));
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        display_command(host, event)
    }
}

/// The language the server writes in: Create's en_us (it has no game language to ask).
const LANG: Lang<'static> = Lang::EN_US;

/// `ItemStack.getHoverName` of an item id, as the server can name it: Create's en_us text of
/// its `item.` then `block.` key, else its path in capitalised words (as most English names
/// read).
pub(crate) fn item_name(id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    ["item", "block"]
        .iter()
        .find_map(|p| display::en_us(&format!("{p}.{ns}.{path}")))
        .map_or_else(|| title_words(path), str::to_owned)
}

/// `oak_planks` as `Oak Planks`.
fn title_words(path: &str) -> String {
    path.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The value of property `key` of the block at `pos`.
fn property(lv: &ServerLevel<'_>, pos: [i32; 3], key: &str) -> Option<String> {
    lv.level.block_property(&*lv.host, pos, key)
}

/// The block's name at `pos` (`minecraft:target`).
fn block_name(lv: &ServerLevel<'_>, pos: [i32; 3]) -> String {
    lv.block_state(pos)
        .and_then(|s| lv.state_name(s))
        .unwrap_or_else(|| "minecraft:air".to_owned())
}

/// A display link's facing and `powered`; None for any other block.
fn link_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Dir, bool)> {
    match block_at(lv, pos)? {
        (Block::DisplayLink, i) => Some((i.facing?, i.powered)),
        _ => None,
    }
}

fn nixie_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<NixieState> {
    let (b, info) = block_at(lv, pos)?;
    if !display::is_nixie(b) {
        return None;
    }
    Some(NixieState {
        facing: info.facing?,
        face: Face::read(&property(lv, pos, "double_face")?)?,
        waterlogged: info.waterlogged,
    })
}

fn board_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<BoardState> {
    let (b, info) = block_at(lv, pos)?;
    if b != Block::DisplayBoard {
        return None;
    }
    Some(BoardState {
        facing: info.facing?,
        up: property(lv, pos, "up").as_deref() == Some("true"),
        down: property(lv, pos, "down").as_deref() == Some("true"),
        waterlogged: info.waterlogged,
    })
}

/// A vanilla sign's (`SignBlockEntity`) kind and lines: whether it hangs, its front and back.
fn sign_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(bool, [String; 4], [String; 4])> {
    let name = block_name(lv, pos);
    if !name.ends_with("_sign") {
        return None;
    }
    let data = lv.host.block_entity(lv.level, pos)?.data(&*lv.host)?;
    let side = |key: &str| -> [String; 4] {
        let mut lines: [String; 4] = Default::default();
        if let Some(Tag::Compound(t)) = data.get(&key.into())
            && let Some(Tag::List(messages)) = t.get(&"messages".into())
        {
            for (line, m) in lines.iter_mut().zip(messages) {
                *line = message_text(m);
            }
        }
        lines
    };
    Some((
        name.ends_with("hanging_sign"),
        side("front_text"),
        side("back_text"),
    ))
}

/// A sign line's plain text, as its component is saved: a string, or a compound with `text`
/// (and `extra`).
fn message_text(tag: &Tag) -> String {
    match tag {
        Tag::String(s) => s.to_string_lossy(),
        Tag::Compound(c) => {
            let mut out = match c.get(&"text".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            if let Some(Tag::List(extra)) = c.get(&"extra".into()) {
                for e in extra {
                    out.push_str(&message_text(e));
                }
            }
            out
        }
        Tag::List(parts) => parts.iter().map(message_text).collect(),
        _ => String::new(),
    }
}

/// `DisplaySource.getAll`: the sources of the block a link is attached to.
fn sources_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> &'static [Source] {
    match block_at(lv, pos).map(|(b, _)| b) {
        Some(Block::Speedometer) => &[Source::KineticSpeed],
        Some(Block::Stressometer) => &[Source::KineticStress],
        Some(Block::Depot | Block::Belt | Block::WeightedEjector) => &[Source::ItemNames],
        Some(Block::CuckooClock) => &[Source::TimeOfDay, Source::StopWatch],
        Some(b) if display::is_nixie(b) => &[Source::NixieTube],
        Some(Block::ContentObserver) => &[
            Source::CountItems,
            Source::ListItems,
            Source::CountFluids,
            Source::ListFluids,
            Source::PackageAddress,
        ],
        Some(Block::FluidTank) => &[Source::Boiler],
        Some(Block::ElevatorContact) => &[Source::CurrentFloor],
        Some(Block::TrackStation) => &[Source::StationSummary, Source::TrainStatus],
        Some(Block::FactoryGauge) => &[Source::GaugeStatus],
        Some(Block::TrackObserver) => &[Source::ObservedTrainName],
        Some(Block::StockpileSwitch) => &[Source::FillLevel],
        Some(Block::AndesiteTunnel | Block::BrassTunnel) => {
            &[Source::AccumulateItems, Source::ItemThroughput]
        }
        Some(b) if create_engine::dye::is_seat(b) => &[Source::EntityName],
        // AllDisplaySources.ENCHANT_POWER `associate(BlockEntityType.ENCHANTING_TABLE)`.
        None if block_name(lv, pos) == "minecraft:enchanting_table" => &[Source::EnchantPower],
        // AllDisplaySources.REDSTONE_POWER `associate(Blocks.TARGET)`.
        None if block_name(lv, pos) == "minecraft:target" => &[Source::RedstonePower],
        _ => &[],
    }
}

/// `FactoryPanelSupportBehaviour.getLinkedPanels` of the link at `pos`.
pub(crate) fn link_panels(dim: usize, pos: [i32; 3]) -> Vec<([i32; 3], u8)> {
    with_world(|w| {
        w.dims[dim]
            .displays
            .links
            .get(&pos)
            .map(|l| l.panels.clone())
    })
    .flatten()
    .unwrap_or_default()
}

/// `FactoryPanelSupportBehaviour.connect`: the panel joins the link, which refreshes
/// (`changed` → `notifyLink`).
pub(crate) fn panel_connected(dim: usize, link: [i32; 3], panel: ([i32; 3], u8)) {
    let changed = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let l = d.links.entry(link).or_default();
        if l.panels.contains(&panel) {
            return false;
        }
        l.panels.push(panel);
        l.last = None;
        d.pulses.push(link);
        true
    })
    .unwrap_or(false);
    if changed {
        persist::mark_pos_unsaved(dim, link);
    }
}

/// `FactoryPanelSupportBehaviour.disconnect`: the panel lets go of the link, which refreshes.
pub(crate) fn panel_disconnected(dim: usize, link: [i32; 3], panel: ([i32; 3], u8)) {
    let changed = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let Some(l) = d.links.get_mut(&link) else {
            return false;
        };
        let before = l.panels.len();
        l.panels.retain(|p| *p != panel);
        if l.panels.len() == before {
            return false;
        }
        l.last = None;
        d.pulses.push(link);
        true
    })
    .unwrap_or(false);
    if changed {
        persist::mark_pos_unsaved(dim, link);
    }
}

/// `FactoryPanelSupportBehaviour.notifyLink` (`updateGatheredData`): the link refreshes in the
/// level's tick.
pub(crate) fn notify_link(dim: usize, link: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        if let Some(l) = d.links.get_mut(&link) {
            l.last = None;
            d.pulses.push(link);
        }
    });
}

/// The facing of the display link at `pos` (`WrenchableDirectionalBlock.FACING`), if one is.
pub(crate) fn display_link_facing(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    link_at(lv, pos).map(|(f, _)| f)
}

/// `DisplayLinkBlockEntity.getSourcePosition`: the first factory gauge joined to it, else the
/// block it is attached to.
fn source_of(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> [i32; 3] {
    let dim = lv.dim;
    with_world(|w| {
        w.dims[dim]
            .displays
            .links
            .get(&pos)
            .and_then(|l| l.panels.first().map(|p| p.0))
    })
    .flatten()
    .unwrap_or_else(|| display::source_pos(pos, facing))
}

/// `DisplayTarget.get`: by block first (display boards), then by block entity (nixie tubes,
/// signs, lecterns).
fn target_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Target> {
    match block_at(lv, pos).map(|(b, _)| b) {
        Some(Block::DisplayBoard) => return Some(Target::Board),
        Some(b) if display::is_nixie(b) => return Some(Target::Nixie),
        Some(_) => return None,
        None => {}
    }
    if block_name(lv, pos) == "minecraft:lectern" {
        return Some(Target::Lectern);
    }
    sign_at(lv, pos).map(|(hanging, _, _)| Target::Sign { hanging })
}

/// The board controlling the board at `pos` (`getController`).
fn controller_of(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let dim = lv.dim;
    let facing = board_at(lv, pos)?.facing;
    let at = display::controller_pos(pos, facing, |p| board_at(lv, p))?;
    let is = with_world(|w| w.dims[dim].displays.boards.get(&at).map(|b| b.controller)).flatten();
    (is == Some(true)).then_some(at)
}

/// `AllBlocks.DISPLAY_LINK.has(level.getBlockState(p))`, from the state table alone (callable
/// while the world is borrowed).
fn is_link(table: &create_engine::world::StateTable, lv: &ServerLevel<'_>, p: [i32; 3]) -> bool {
    table
        .get(lv.block(p))
        .is_some_and(|(b, _)| b == Block::DisplayLink)
}

/// `DisplayTargetStats` of a target.
fn stats_of(lv: &ServerLevel<'_>, pos: [i32; 3], target: Target) -> Stats {
    let dim = lv.dim;
    match target {
        // SignDisplayTarget.provideStats.
        Target::Sign { .. } => Stats {
            rows: 4,
            columns: 15,
        },
        // LecternDisplayTarget.provideStats.
        Target::Lectern => Stats {
            rows: 50,
            columns: 256,
        },
        // NixieTubeDisplayTarget.getWidth: two characters a tube.
        Target::Nixie => Stats {
            rows: 1,
            columns: 2 * display::walk_nixies(pos, |p| nixie_at(lv, p)).len() as i32,
        },
        Target::Board => controller_of(lv, pos)
            .and_then(|c| with_world(|w| w.dims[dim].displays.boards.get(&c).map(Board::stats)))
            .flatten()
            .unwrap_or(Stats {
                rows: 1,
                columns: 1,
            }),
    }
}

/// What a source reads from the world now.
fn read_source(lv: &mut ServerLevel<'_>, pos: [i32; 3], source: Source) -> Reading {
    let dim = lv.dim;
    let (link_facing, _) = link_at(lv, pos).unwrap_or((Dir::Up, false));
    let at = source_of(lv, pos, link_facing);
    match source {
        Source::KineticSpeed | Source::KineticStress => {
            let shown = with_world(|w| {
                let g = w.dims[dim].gauges.get(&at)?;
                Some((g.kind, g.shown))
            })
            .flatten();
            use create_engine::gauge::Kind;
            match (source, shown) {
                // KineticBlockEntity.getSpeed: 0 while overstressed.
                (Source::KineticSpeed, Some((Kind::Speed, s))) => {
                    Reading::Speed(Some(if s.overstressed { 0.0 } else { s.speed }))
                }
                (Source::KineticSpeed, _) => Reading::Speed(None),
                (_, Some((Kind::Stress, s))) => Reading::Stress(Some((s.stress, s.capacity))),
                _ => Reading::Stress(None),
            }
        }
        Source::RedstonePower => {
            // `RedstonePowerDisplaySource`: the target block's POWER, or the direct signal into
            // it (`getDirectSignalTo`) when stronger.
            let own = property(lv, at, "power")
                .and_then(|p| p.parse::<i32>().ok())
                .unwrap_or(0);
            let direct = lv.level.direct_signal_to(&*lv.host, at).unwrap_or(0);
            Reading::Power(own.max(direct))
        }
        Source::TimeOfDay | Source::StopWatch => {
            let running = block_at(lv, at).is_some_and(|(b, _)| b == Block::CuckooClock)
                && with_world(|w| w.speed(dim, at))
                    .flatten()
                    .is_some_and(|s| s != 0.0);
            let now = lv.game_time();
            if source == Source::StopWatch {
                return Reading::Watch { now, running };
            }
            let mut rng = create_engine::processing::Rng::new(now as u64);
            let mut next = |n: i32| (rng.next_float() * n as f32) as i32;
            Reading::Time {
                day_time: lv.level.day_time(&*lv.host).unwrap_or(0),
                // Only the overworld's dimension type is `natural`.
                natural: dim == 0,
                running,
                random: [next(70), next(40)],
            }
        }
        Source::ItemNames => {
            // ItemNameDisplaySource: along the link's direction from its source, 32 blocks of
            // depots, ejectors and belts, the stack at each one's centre.
            let mut names = Vec::new();
            let mut p = at;
            let step = link_facing.opposite().offset();
            for _ in 0..32 {
                let Some((b, _)) = block_at(lv, p) else {
                    break;
                };
                let item: Option<String> = match b {
                    Block::Depot | Block::WeightedEjector => super::redstone::slot_stack(lv, p, 0)
                        .map(|s| super::redstone::item_id(lv, &s)),
                    // BeltBlockEntity.applyToAllItems(.25): the last item within a quarter of
                    // the segment's centre.
                    Block::Belt => with_world(|w| {
                        let d = &w.dims[dim];
                        let c = d.belt_of.get(&p)?;
                        let belt = d.belts.get(c)?;
                        let index = belt.chain.iter().position(|q| *q == p)? as f32;
                        belt.items
                            .items
                            .iter()
                            .rfind(|i| (index + 0.5 - i.position).abs() < 0.25)
                            .map(|i| i.item.clone())
                    })
                    .flatten(),
                    _ => break,
                };
                if let Some(id) = item {
                    names.push(item_name(&id));
                }
                p = [p[0] + step[0], p[1] + step[1], p[2] + step[2]];
            }
            Reading::Names(names)
        }
        Source::NixieTube => Reading::Nixie(if nixie_at(lv, at).is_some() {
            with_world(|w| w.dims[dim].displays.nixies.get(&at).map(|n| n.full_text())).flatten()
        } else {
            None
        }),
        // ItemCountDisplaySource and ItemListDisplaySource (`CountedItemStackList`: one entry
        // an item).
        Source::CountItems | Source::ListItems => {
            let items = super::redstone::observed(lv, at).items;
            if source == Source::CountItems {
                return Reading::Count(items.map(|i| i.iter().map(|(_, n)| i64::from(*n)).sum()));
            }
            let mut entries: Vec<(i64, String)> = Vec::new();
            for (id, n) in items.unwrap_or_default() {
                let name = item_name(&id);
                match entries.iter_mut().find(|e| e.1 == name) {
                    Some(e) => e.0 += i64::from(n),
                    None => entries.push((i64::from(n), name)),
                }
            }
            Reading::Entries(entries)
        }
        // FluidAmountDisplaySource and FluidListDisplaySource.
        Source::CountFluids | Source::ListFluids => {
            let fluids = super::redstone::observed(lv, at).fluids;
            if source == Source::CountFluids {
                return Reading::Fluid(fluids.map(|f| f.iter().map(|(_, n)| i64::from(*n)).sum()));
            }
            Reading::Entries(
                fluids
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(f, n)| (i64::from(n), f.name().to_owned()))
                    .collect(),
            )
        }
        // FillLevelDisplaySource.getProgress: `max(0, (level - min) / (max - min))`.
        Source::FillLevel => Reading::Fill(
            with_world(|w| {
                let s = w.dims[dim].detectors.switches.get(&at)?;
                Some(((s.level - s.min) as f32 / (s.max - s.min) as f32).max(0.0))
            })
            .flatten(),
        ),
        Source::EntityName => Reading::Text(super::contraptions::seats::rider_name(lv, at)),
        // EnchantPowerDisplaySource: each valid bookshelf (`isValidBookShelf`: a power provider
        // with a transmitter between it and the table) gives one (`getEnchantPowerBonus`).
        Source::EnchantPower => {
            if block_name(lv, at) != "minecraft:enchanting_table" {
                return Reading::Count(None);
            }
            let tagged = |p: [i32; 3], tag: &str| {
                lv.block_state(p)
                    .is_some_and(|s| lv.host.block_in_tag(s.block(), tag) == Some(true))
            };
            let power = display::bookshelf_offsets()
                .into_iter()
                .filter(|o| {
                    tagged(
                        [at[0] + o[0], at[1] + o[1], at[2] + o[2]],
                        "minecraft:enchantment_power_provider",
                    ) && tagged(
                        [at[0] + o[0] / 2, at[1] + o[1], at[2] + o[2] / 2],
                        "minecraft:enchantment_power_transmitter",
                    )
                })
                .count() as i32;
            let mut rng = create_engine::processing::Rng::new(lv.game_time() as u64);
            let a = (rng.next_float() * 8.0) as i32;
            let b = (rng.next_float() * (power.min(15) + 1) as f32) as i32;
            Reading::Count(Some(i64::from(display::enchant_cost(power, a, b))))
        }
        // The tunnels' counts are the link's own (`itemReceived`).
        Source::AccumulateItems | Source::ItemThroughput => Reading::Now(lv.game_time()),
        Source::Boiler => Reading::Boiler(boiler_view(lv, at)),
        Source::CurrentFloor => Reading::Text(super::contraptions::contact_floor(lv, at)),
        Source::TrainStatus => Reading::Text(super::trains::waiting_status(lv, at)),
        Source::ObservedTrainName => Reading::Text(super::trains::observed_train_name(lv, at)),
        Source::PackageAddress => Reading::Text(package_address(lv, at)),
        Source::StationSummary => {
            let filter = with_world(|w| {
                w.dims[dim]
                    .displays
                    .links
                    .get(&pos)
                    .and_then(|l| l.config.filter.clone())
            })
            .flatten()
            .unwrap_or_default();
            Reading::Summary(summary::arrivals(&filter))
        }
        Source::GaugeStatus => Reading::Entries(super::stock::gauge_status(lv, pos)),
    }
}

/// `DisplayLinkBlock.sendToGatherers(.., itemReceived, AccumulatedItemCountDisplaySource)`:
/// `amount` items went into the tunnel at `tunnel`; the links attached to it counting them (not
/// powered) count them and refresh.
/// `BoilerDisplaySource.getComponents`: the boiler of the tank at `pos` (its controller's), as
/// `calcMinMaxForSize` leaves it; a tank with no engine or whistle has the empty boiler data.
fn boiler_view(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<display::BoilerView> {
    use create_engine::steam;
    let dim = lv.dim;
    with_world(|w| {
        let d = &w.dims[dim];
        let (controller, multi) = d.tanks.get(pos)?;
        let size = multi.blocks();
        let empty = steam::Boiler::default();
        let b = d.boilers.get(&controller).map_or(&empty, |b| &b.data);
        Some(display::BoilerView {
            passive: b.passive_heat,
            active_heat: b.active_heat,
            for_size: steam::max_heat_for_size(size),
            for_water: b.max_heat_for_water(),
        })
    })
    .flatten()
}

/// `PackageAddressDisplaySource.provideLine`: the address of the first package passing the
/// smart observer's filter in the inventory it looks at, else on the chain conveyor there.
fn package_address(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let Some((Block::ContentObserver, info)) = block_at(lv, pos) else {
        return None;
    };
    let target = create_engine::observe::target_pos(pos, info.target?, info.facing?);
    let dim = lv.dim;
    let filter = with_world(|w| w.dims[dim].detectors.filter(pos).cloned()).flatten();
    let passes = |lv: &ServerLevel<'_>, stack: &ItemStack| {
        use create_engine::filter::Passes;
        let id = stack.item().id(&*lv.host);
        create_engine::package::is_package(&id)
            && filters::Detector {
                host: &*lv.host,
                filter: filter.as_ref(),
            }
            .item(&id)
    };
    match lv
        .level
        .capability(&mut *lv.host, &mod_api::ITEMS, target, None)
    {
        Some(handler) => {
            let size = handler.size(&mut *lv.host);
            for i in 0..size {
                if handler.amount(&mut *lv.host, i) == 0 {
                    continue;
                }
                let Some(stack) = handler
                    .resource(&mut *lv.host, i)
                    .and_then(|r| r.to_stack(1))
                else {
                    continue;
                };
                if passes(lv, &stack) {
                    return Some(filters::address(&stack));
                }
            }
            None
        }
        None => super::packages::looping_packages(dim, target)
            .into_iter()
            .find(|s| passes(lv, s))
            .map(|s| filters::address(&s)),
    }
}

pub(crate) fn items_through_tunnel(lv: &mut ServerLevel<'_>, tunnel: [i32; 3], amount: i32) {
    let dim = lv.dim;
    let now = lv.game_time();
    for d in Dir::ALL {
        let p = super::redstone::offset(tunnel, d);
        let Some((facing, powered)) = link_at(lv, p) else {
            continue;
        };
        if powered || display::source_pos(p, facing) != tunnel {
            continue;
        }
        let counted = with_world(|w| {
            let l = w.dims[dim].displays.links.get_mut(&p)?;
            match l.source? {
                Source::AccumulateItems => l.config.collected += amount,
                Source::ItemThroughput => l.config.throughput.received(amount, now),
                _ => return None,
            }
            l.last = None;
            Some(())
        })
        .flatten();
        if counted.is_some() {
            mark(lv, p);
            update(lv, p);
        }
    }
}

/// `DisplayLinkBlock.shouldBePowered`: a signal from any side but the one it is attached to.
fn link_powered(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> bool {
    Dir::ALL
        .into_iter()
        .filter(|&d| d != facing.opposite())
        .any(|d| {
            lv.signal(
                super::redstone::offset(pos, d),
                super::redstone::direction(d),
            ) != 0
        })
}

fn mark(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    persist::mark_pos_unsaved(lv.dim, pos);
}

/// SNBT string literal.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A line as a text component in SNBT: plain text as a string, a styled one (`display::spans`)
/// as an empty text with each run in its colour.
fn text_snbt(s: &str) -> String {
    let spans = display::spans(s);
    if spans.iter().all(|(_, c)| c.is_none()) {
        return quoted(&display::plain(s));
    }
    let extra: Vec<String> = spans
        .iter()
        .map(|(t, c)| match c {
            Some(rgb) => format!("{{text:{},color:\"#{rgb:06X}\"}}", quoted(t)),
            None => format!("{{text:{}}}", quoted(t)),
        })
        .collect();
    format!("{{text:\"\",extra:[{}]}}", extra.join(","))
}

/// `SignDisplayTarget.acceptText`: from `line` down both sides, each cut to the sign's line
/// width (`getString(getMaxTextLineWidth)`: 90, a hanging sign's 60); only lines that differ
/// are written, through `/data`, which redraws the sign.
fn write_sign(
    lv: &mut ServerLevel<'_>,
    link: [i32; 3],
    target: [i32; 3],
    line: i32,
    text: &[String],
) {
    let dim = lv.dim;
    let Some((hanging, front, back)) = sign_at(lv, target) else {
        return;
    };
    let width = if hanging { 60 } else { 90 };
    let current: Vec<(String, String)> = front.into_iter().zip(back).collect();
    let mut commands = Vec::new();
    for (i, t) in text.iter().enumerate() {
        let at = i as i32 + line;
        if !(0..4).contains(&at) {
            break;
        }
        let reserved = {
            let lv = &*lv;
            with_world(|w| {
                let table = &w.table;
                let d = &mut w.dims[dim].displays;
                if i == 0 {
                    d.reserve(target, at, link);
                    false
                } else {
                    d.is_reserved(target, at, link, |p| is_link(table, lv, p))
                }
            })
        };
        if reserved == Some(true) {
            break;
        }
        let content = display::truncate(t, width);
        let styled = display::spans(&content).iter().any(|(_, c)| c.is_some());
        let (front, back) = &current[at as usize];
        for (side, now) in [("front_text", front), ("back_text", back)] {
            // A styled line is written again: its colours may change under the same text.
            if *now != display::plain(&content) || styled {
                commands.push(format!(
                    "data modify block {} {} {} {side}.messages[{at}] set value {}",
                    target[0],
                    target[1],
                    target[2],
                    text_snbt(&content)
                ));
            }
        }
    }
    for c in commands {
        lv.run_command(&c);
    }
}

/// `LecternDisplayTarget.acceptText`: the lectern's book takes the text a page a line from
/// page `line` (blank pages before it, at most 50); a book and quill is signed first
/// ("Printed Book" by "Data Gatherer"). Through `/data`, which updates the lectern.
fn write_lectern(
    lv: &mut ServerLevel<'_>,
    link: [i32; 3],
    target: [i32; 3],
    line: i32,
    text: &[String],
) {
    let dim = lv.dim;
    let Some(data) = lv
        .host
        .block_entity(lv.level, target)
        .and_then(|e| e.data(&*lv.host))
    else {
        return;
    };
    let Some(Tag::Compound(book)) = data.get(&"Book".into()) else {
        return;
    };
    let id = match book.get(&"id".into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => return,
    };
    let components = match book.get(&"components".into()) {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    // A page's text, whether it is filtered or not.
    let page_text = |t: &Tag| -> String {
        match t {
            Tag::Compound(c) if c.get(&"raw".into()).is_some() => {
                c.get(&"raw".into()).map(message_text).unwrap_or_default()
            }
            other => message_text(other),
        }
    };
    let pages_of = |key: &str| -> Vec<String> {
        match components.get(&key.into()) {
            Some(Tag::Compound(c)) => match c.get(&"pages".into()) {
                Some(Tag::List(l)) => l.iter().map(page_text).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    };
    let (written, mut pages) = match id.as_str() {
        "minecraft:written_book" => (true, pages_of("minecraft:written_book_content")),
        "minecraft:writable_book" => (false, pages_of("minecraft:writable_book_content")),
        _ => return,
    };
    let before = pages.clone();
    let mut i = 0i32;
    while i - line < text.len() as i32 && i < 50 {
        let at = i as usize;
        if pages.len() <= at {
            pages.push(if i < line {
                String::new()
            } else {
                text[(i - line) as usize].clone()
            });
        } else if i >= line {
            let reserved = {
                let lv = &*lv;
                with_world(|w| {
                    let table = &w.table;
                    let d = &mut w.dims[dim].displays;
                    if i == line {
                        d.reserve(target, i, link);
                        false
                    } else {
                        // `isReserved(i - line, ..)`, as Create asks it.
                        d.is_reserved(target, i - line, link, |p| is_link(table, lv, p))
                    }
                })
            };
            if reserved == Some(true) {
                break;
            }
            pages[at] = text[(i - line) as usize].clone();
        }
        i += 1;
    }
    if written && pages == before {
        return;
    }
    let list = pages
        .iter()
        .map(|p| format!("{{raw:{}}}", text_snbt(p)))
        .collect::<Vec<_>>()
        .join(",");
    let command = if written {
        format!(
            "data modify block {} {} {} Book.components.\"minecraft:written_book_content\".pages \
             set value [{list}]",
            target[0], target[1], target[2]
        )
    } else {
        format!(
            "data modify block {} {} {} Book set value {{id:\"minecraft:written_book\",count:1,\
             components:{{\"minecraft:written_book_content\":{{title:{{raw:\"Printed Book\"}},\
             author:\"Data Gatherer\",generation:0,resolved:1b,pages:[{list}]}}}}}}",
            target[0], target[1], target[2]
        )
    };
    lv.run_command(&command);
}

/// `DisplayLinkBlockEntity.updateGatheredData`: the source's text written to the target.
fn update(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((facing, _)) = link_at(lv, pos) else {
        return;
    };
    let Some(link) = with_world(|w| w.dims[dim].displays.links.get(&pos).cloned()).flatten() else {
        return;
    };
    let source_pos = source_of(lv, pos, facing);
    let target_pos = link.target(pos);
    if !lv.is_loaded(target_pos) || !lv.is_loaded(source_pos) {
        return;
    }
    let target = target_at(lv, target_pos);
    // The source is gone from the block it reads: forgotten with its settings.
    if let Some(s) = link.source
        && !sources_at(lv, source_pos).contains(&s)
    {
        with_world(|w| {
            if let Some(l) = w.dims[dim].displays.links.get_mut(&pos) {
                l.source = None;
                l.config = Config::default();
                l.due = None;
                l.last = None;
            }
        });
        mark(lv, pos);
        return;
    }
    let (Some(source), Some(target)) = (link.source, target) else {
        return;
    };
    let stats = stats_of(lv, target_pos, target);
    let reading = read_source(lv, pos, source);
    // `StationSummaryDisplaySource.provideFlapDisplayText`: departures shown on a board.
    if let Reading::Summary(a) = &reading
        && !a.is_empty()
        && target == Target::Board
        && stats.rows > 0
    {
        crate::glue::advancements::award_placer(lv, pos, "display_board_0");
    }
    let Some((line, config, started)) = with_world(|w| {
        let l = w.dims[dim].displays.links.get_mut(&pos)?;
        let had = l.config.start_time;
        let line = l.line(reading, target, stats, &LANG);
        Some((line, l.config.clone(), had != l.config.start_time))
    })
    .flatten() else {
        return;
    };
    if started {
        mark(lv, pos);
    }
    // `sendPulseNextSync` after the transfer.
    bulb::pulse(lv, pos);
    crate::glue::advancements::award_placer(lv, pos, "display_link");
    match target {
        Target::Nixie => {
            let text = display::provide_text(source, &config, &line, target);
            // A tube shows plain text (`getString()`).
            let first = display::plain(&text.into_iter().next().unwrap_or_default());
            let row = display::walk_nixies(target_pos, |p| nixie_at(lv, p));
            let facings: HashMap<[i32; 3], Dir> = row
                .iter()
                .flat_map(|p| Dir::ALL.map(|d| super::redstone::offset(*p, d)))
                .filter_map(|p| link_at(lv, p).map(|(f, _)| (p, f)))
                .collect();
            let changed: Vec<[i32; 3]> = with_world(|w| {
                let d = &mut w.dims[dim].displays;
                let mut changed = Vec::new();
                for (i, p) in row.iter().enumerate() {
                    if d.nixies
                        .entry(*p)
                        .or_default()
                        .display_custom_text(&first, i as i32)
                    {
                        changed.push(*p);
                        d.shown.push(*p);
                        d.notify_gatherers(*p, |q| facings.get(&q).copied());
                    }
                }
                changed
            })
            .unwrap_or_default();
            for p in changed {
                mark(lv, p);
            }
        }
        Target::Sign { .. } => {
            let text = display::provide_text(source, &config, &line, target);
            write_sign(lv, pos, target_pos, link.target_line, &text);
        }
        Target::Lectern => {
            let text = display::provide_text(source, &config, &line, target);
            write_lectern(lv, pos, target_pos, link.target_line, &text);
        }
        Target::Board => {
            let Some(controller) = controller_of(lv, target_pos) else {
                return;
            };
            // `isSpeedRequirementFulfilled`: SpeedLevel.MEDIUM.
            let speed = with_world(|w| w.speed(dim, controller))
                .flatten()
                .unwrap_or(0.0);
            if speed.abs() < create_engine::gauge::MEDIUM_SPEED {
                return;
            }
            // A board shows plain text (`FlapDisplaySection.refresh`: `getString()`).
            let text: Vec<Vec<String>> = display::provide_flap_text(source, &config, &line)
                .into_iter()
                .map(|l| l.iter().map(|t| display::plain(t)).collect())
                .collect();
            // `flapDisplayContext`: a list's largest amount.
            let highest = line
                .entries
                .as_ref()
                .map_or(0, |e| e.iter().map(|x| x.0).max().unwrap_or(0));
            let line_at = link.target_line;
            let changed = {
                let lv = &*lv;
                with_world(|w| {
                    let table = &w.table;
                    let d = &mut w.dims[dim].displays;
                    d.reserve(controller, line_at, pos);
                    let rows = d.boards.get(&controller).map_or(0, |b| b.y_size * 2);
                    let reserved: Vec<bool> = (0..rows.max(0))
                        .map(|l| {
                            l > line_at
                                && d.is_reserved(controller, l, pos, |p| is_link(table, lv, p))
                        })
                        .collect();
                    let b = d.boards.get_mut(&controller)?;
                    let changed = display::accept_flap_text(
                        b,
                        line_at.max(0) as usize,
                        &text,
                        source,
                        &config,
                        line.numeric,
                        highest,
                        |l| reserved.get(l).copied().unwrap_or(false),
                    );
                    if changed {
                        d.shown.push(controller);
                    }
                    Some(changed)
                })
                .flatten()
            };
            if changed == Some(true) {
                mark(lv, controller);
            }
        }
    }
}

/// `tickSource`: due again in the source's interval; nothing while powered.
fn tick_source(lv: &mut ServerLevel<'_>, pos: [i32; 3], now: i64) {
    let dim = lv.dim;
    with_world(|w| w.dims[dim].displays.schedule(pos, now));
    let Some((_, powered)) = link_at(lv, pos) else {
        return;
    };
    if !powered {
        update(lv, pos);
    }
}

/// Boards placed or removed: the rows around them join again (`updateColumn`, `updateShape`),
/// then each board looks for its place (`updateControllerStatus`).
fn tick_boards(lv: &mut ServerLevel<'_>, checks: Vec<[i32; 3]>) {
    let dim = lv.dim;
    let mut todo: Vec<[i32; 3]> = Vec::new();
    for pos in checks {
        todo.push(pos);
        for d in Dir::ALL {
            todo.push(super::redstone::offset(pos, d));
        }
    }
    // Every board joined to those, in the plane of each one's face (at most a 64 by 64 wall).
    let mut seen: Vec<[i32; 3]> = Vec::new();
    let mut known: HashSet<[i32; 3]> = HashSet::new();
    while let Some(p) = todo.pop() {
        if seen.len() >= 4096 || !known.insert(p) {
            continue;
        }
        let Some(s) = board_at(lv, p) else {
            continue;
        };
        seen.push(p);
        let across = display::clockwise(s.facing);
        for d in [Dir::Up, Dir::Down, across, across.opposite()] {
            let q = super::redstone::offset(p, d);
            if board_at(lv, q).is_some_and(|t| t.facing == s.facing) {
                todo.push(q);
            }
        }
    }
    for &p in &seen {
        let Some(s) = board_at(lv, p) else {
            continue;
        };
        let (down, up) = display::column_connections(p, s.facing, |q| {
            if !lv.is_loaded(q) {
                return None;
            }
            board_at(lv, q).map(|b| b.facing)
        });
        for (key, now, want) in [("down", s.down, down), ("up", s.up, up)] {
            if now != want {
                crate::glue::items::wrench::switch_property(
                    lv,
                    p,
                    key,
                    if want { "true" } else { "false" },
                );
            }
        }
    }
    for &p in &seen {
        let Some(s) = board_at(lv, p) else {
            continue;
        };
        let (c, x, y) = display::controller_status(p, s, |q| board_at(lv, q));
        let changed = with_world(|w| {
            let d = &mut w.dims[dim].displays;
            let b = d.boards.entry(p).or_default();
            let changed = b.set_status(c, x, y);
            if changed {
                d.shown.push(p);
            }
            changed
        })
        .unwrap_or(false);
        if changed {
            mark(lv, p);
        }
    }
}

/// After a level's tick: links whose neighbours changed follow their signal, boards join, links
/// due or told by their source refresh, and clients hear of tubes and boards that changed.
/// Nothing runs when nothing is due.
fn tick_displays(lv: &mut ServerLevel<'_>) {
    link_item::take_placed(lv);
    summary::tick(lv);
    let dim = lv.dim;
    for station in summary::stations_to_tell(dim) {
        notify_gatherers(lv, station);
    }
    let now = lv.game_time();
    if with_world(|w| w.dims[dim].displays.idle(now)).unwrap_or(true) {
        return;
    }
    let Some((mut checks, boards, mut nixies)) = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        (
            std::mem::take(&mut d.checks),
            std::mem::take(&mut d.board_checks),
            std::mem::take(&mut d.nixie_checks),
        )
    }) else {
        return;
    };
    nixies.sort_unstable();
    nixies.dedup();
    for pos in nixies {
        update_nixie_power(lv, pos);
    }
    checks.sort_unstable();
    checks.dedup();
    for pos in checks {
        // DisplayLinkBlock.neighborChanged: `powered` follows; a link no longer powered
        // refreshes now (`onNoLongerPowered`), the stopwatch starting again.
        let Some((facing, was)) = link_at(lv, pos) else {
            continue;
        };
        // A neighbour of a link reading redstone power may be its source (`sendToGatherers`).
        let reads_power = with_world(|w| {
            w.dims[dim]
                .displays
                .links
                .get(&pos)
                .is_some_and(|l| l.source == Some(Source::RedstonePower))
        })
        .unwrap_or(false);
        let powered = link_powered(lv, pos, facing);
        if powered != was {
            lv.set_property(
                pos,
                "powered",
                if powered { "true" } else { "false" },
                UPDATE_CLIENTS,
            );
            if !powered {
                let had = with_world(|w| {
                    let d = &mut w.dims[dim].displays;
                    let l = d.links.get_mut(&pos)?;
                    l.source?;
                    // `onSignalReset`: the stopwatch starts again; a tunnel's count goes.
                    if l.source == Some(Source::StopWatch) {
                        l.config.start_time = Some(now);
                        l.last = None;
                    }
                    if matches!(
                        l.source,
                        Some(Source::AccumulateItems | Source::ItemThroughput)
                    ) {
                        l.config.collected = 0;
                        l.last = None;
                    }
                    d.schedule(pos, now);
                    Some(())
                })
                .flatten();
                if had.is_some() {
                    mark(lv, pos);
                    update(lv, pos);
                }
                continue;
            }
        }
        if reads_power {
            tick_source(lv, pos, now);
        }
    }
    if !boards.is_empty() {
        tick_boards(lv, boards);
    }
    let mut due = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let mut due = std::mem::take(&mut d.pulses);
        due.extend(d.take_due(now));
        due
    })
    .unwrap_or_default();
    due.sort_unstable();
    due.dedup();
    for pos in due {
        tick_source(lv, pos, now);
    }
    // Tubes changed by this tick's links may have told links reading them: next tick.
    send_changed(lv);
}

/// The tubes and boards changed since they were last sent, to the level's players.
fn send_changed(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let (shown, forgotten) = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        (
            std::mem::take(&mut d.shown),
            std::mem::take(&mut d.forgotten),
        )
    })
    .unwrap_or_default();
    if shown.is_empty() && forgotten.is_empty() {
        return;
    }
    let players = lv.players();
    send_displays(lv, &players, &shown, &forgotten);
}

fn shown_of(dim: usize, pos: [i32; 3]) -> Option<Shown> {
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        if let Some(n) = d.nixies.get(&pos) {
            return Some(Shown::Nixie {
                pos,
                text: n.custom.clone(),
                index: n.index,
                redstone: n.redstone,
            });
        }
        let b = d.boards.get_mut(&pos)?;
        if !b.controller {
            return Some(Shown::Board {
                pos,
                x_size: 1,
                y_size: 1,
                lines: Vec::new(),
                colour: Vec::new(),
                glowing: Vec::new(),
            });
        }
        let lines = b.lines().clone();
        // `write` clears `sendTransition` once sent.
        for l in b.lines().iter_mut() {
            for s in &mut l.sections {
                s.transition = false;
            }
        }
        Some(Shown::Board {
            pos,
            x_size: b.x_size,
            y_size: b.y_size,
            lines,
            colour: b.colour.clone(),
            glowing: b.glowing.clone(),
        })
    })
    .flatten()
}

fn send_displays(
    lv: &mut ServerLevel<'_>,
    players: &[PlayerRef],
    shown: &[[i32; 3]],
    forgotten: &[[i32; 3]],
) {
    if players.is_empty() {
        return;
    }
    let dim = lv.dim;
    let mut list: Vec<[i32; 3]> = shown.to_vec();
    list.sort_unstable();
    list.dedup();
    let mut all: Vec<Shown> = list.into_iter().filter_map(|p| shown_of(dim, p)).collect();
    all.extend(forgotten.iter().map(|&pos| Shown::Forget { pos }));
    // Well under the message size limit: 16 at a time.
    for chunk in all.chunks(16) {
        let m = display::encode(chunk);
        net::send_to_players(&mut *lv.host, players, display::CHANNEL, &m);
    }
    for contents in &all {
        let pos = match contents {
            Shown::Nixie { pos, .. } | Shown::Board { pos, .. } | Shown::Forget { pos } => *pos,
        };
        super::drawn::publish(
            lv,
            pos,
            super::drawn::snapshot([display::encode(std::slice::from_ref(contents))]),
        );
    }
}

/// A joining client gets every tube and every controller.
fn sync_displays(lv: &mut ServerLevel<'_>, player: PlayerRef) {
    let dim = lv.dim;
    let list: Vec<[i32; 3]> = with_world(|w| {
        let d = &w.dims[dim].displays;
        d.nixies
            .keys()
            .copied()
            .chain(
                d.boards
                    .iter()
                    .filter(|(_, b)| b.controller)
                    .map(|(p, _)| *p),
            )
            .collect()
    })
    .unwrap_or_default();
    if !list.is_empty() {
        send_displays(lv, &[player], &list, &[]);
    }
}

/// `NixieTubeBlock.getPower`: the strongest signal into the tube from any side, and the
/// strongest signal upward from the blocks beside it but the one behind it.
fn nixie_power(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> i32 {
    let mut power = 0;
    for d in Dir::ALL {
        let at = super::redstone::offset(pos, d);
        power = power.max(lv.signal(at, super::redstone::direction(d)));
        if facing.opposite() != d {
            power = power.max(lv.signal(at, mod_api::Direction::Up));
        }
    }
    power
}

/// The links beside `pos`, by their facing (for `notifyGatherers`).
/// `DisplayLinkBlock.notifyGatherers`: the links attached to `pos` refresh now.
pub(crate) fn notify_gatherers(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    let links = links_beside(lv, pos);
    if links.is_empty() {
        return;
    }
    let dim = lv.dim;
    with_world(|w| {
        w.dims[dim]
            .displays
            .notify_gatherers(pos, |q| links.get(&q).copied())
    });
}

fn links_beside(lv: &ServerLevel<'_>, pos: [i32; 3]) -> HashMap<[i32; 3], Dir> {
    Dir::ALL
        .into_iter()
        .map(|d| super::redstone::offset(pos, d))
        .filter_map(|p| link_at(lv, p).map(|(f, _)| (p, f)))
        .collect()
}

/// `NixieTubeBlock.updateDisplayedRedstoneValue`: a tube without custom text shows the power
/// it gets (`updateRedstoneStrength`), telling the links reading it (`notifyGatherers`).
fn update_nixie_power(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(state) = nixie_at(lv, pos) else {
        return;
    };
    let dim = lv.dim;
    let reacts = with_world(|w| {
        w.dims[dim]
            .displays
            .nixies
            .get(&pos)
            .is_some_and(|n| n.custom.is_none())
    })
    .unwrap_or(false);
    if !reacts {
        return;
    }
    let power = nixie_power(lv, pos, state.facing);
    let links = links_beside(lv, pos);
    let changed = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let changed = d
            .nixies
            .entry(pos)
            .or_default()
            .update_redstone_strength(power);
        if changed {
            d.shown.push(pos);
            d.notify_gatherers(pos, |q| links.get(&q).copied());
        }
        changed
    })
    .unwrap_or(false);
    if changed {
        mark(lv, pos);
    }
}

/// The nixie tubes' clicks (`NixieTubeBlock.useItemOn`).
struct NixieBlock;

#[mod_api::horizon_part]
impl BlockPart for NixieBlock {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let Some(stack) = event.click.stack.cloned() else {
            return Some(InteractionResult::TryWithEmptyHand);
        };
        use_nixie(host, event.click, Some(&stack))
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        use_nixie(host, event.click, None)
    }
}

/// `NixieTubeBlock.useItemOn`: not while sneaking. An empty hand clears a tube's custom text
/// (back to its redstone strength); a named name tag, or a clipboard (the first entry of the
/// page last viewed), writes its text across the tube's row, each tube its two characters; a
/// dye recolours the row.
fn use_nixie(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
    stack: Option<&ItemStack>,
) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    if player.entity().is_shift_key_down(&*host) == Some(true) {
        return None;
    }
    let item = stack.map(|s| s.item().id(&*host));
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = click.pos;
    let dim = lv.dim;
    let Some(stack) = stack else {
        // `reactsToRedstone`: nothing to clear.
        let cleared = with_world(|w| {
            let d = &mut w.dims[dim].displays;
            let cleared = d
                .nixies
                .get_mut(&pos)
                .is_some_and(|n| n.clear_custom_text());
            if cleared {
                d.shown.push(pos);
            }
            cleared
        })
        .unwrap_or(false);
        if !cleared {
            return None;
        }
        mark(&lv, pos);
        update_nixie_power(&mut lv, pos);
        return Some(InteractionResult::Success);
    };
    let item = item.unwrap_or_default();
    let clipboard = item == "create:clipboard";
    let name = if item == "minecraft:name_tag" {
        filters::plain_name(filters::component(
            &*lv.host,
            stack,
            "minecraft:custom_name",
        ))
    } else {
        None
    };
    let display = clipboard || name.is_some();
    let dye = create_engine::dye::DyeColor::of_item(&item);
    if !display && dye.is_none() {
        return None;
    }
    // `ClipboardEntry.getLastViewedEntries`: the page last opened.
    let text = if clipboard {
        let content = clipboard::content(stack);
        let page = usize::try_from(content.previously_opened_page)
            .unwrap_or(0)
            .min(content.pages.len().saturating_sub(1));
        content
            .pages
            .get(page)
            .and_then(|p| p.first())
            .map(|e| e.text.clone())
            .unwrap_or_default()
    } else {
        name.unwrap_or_default()
    };
    let row = display::walk_nixies(pos, |p| nixie_at(&lv, p));
    for (i, p) in row.into_iter().enumerate() {
        if display {
            let links = links_beside(&lv, p);
            let changed = with_world(|w| {
                let d = &mut w.dims[dim].displays;
                let changed = d
                    .nixies
                    .entry(p)
                    .or_default()
                    .display_custom_text(&text, i as i32);
                if changed {
                    d.shown.push(p);
                    d.notify_gatherers(p, |q| links.get(&q).copied());
                }
                changed
            })
            .unwrap_or(false);
            if changed {
                mark(&lv, p);
            }
        }
        if let Some(colour) = dye {
            let to = create_engine::dye::NIXIE_TUBES[colour as usize];
            if let Some(now) = lv.block_state(p)
                && let Some(next) = super::blocks::recoloured_state(&*lv.host, now, to)
                && next != now
            {
                lv.set_state(p, next, super::server::update_all());
            }
        }
    }
    Some(InteractionResult::Success)
}

// ---------------------------------------------------------------------------------------------
// Placement

/// `DisplayLinkBlock.getStateForPlacement` (on the clicked face) and `FlapDisplayBlock`'s
/// (facing the player, or as the board it was placed against unless sneaking).
struct DisplayBlock;

#[mod_api::horizon_part]
impl BlockPart for DisplayBlock {
    /// `DisplayLinkBlock.useWithoutItem` takes any hand (the item's own use passes to it); a
    /// display board takes a click on one of its lines (`FlapDisplayBlock.useItemOn`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let (block, _) = states::id(event.state).and_then(info)?;
        match block {
            Block::DisplayLink => Some(InteractionResult::TryWithEmptyHand),
            Block::DisplayBoard => use_board(host, event.click),
            _ => None,
        }
    }

    /// `DisplayLinkBlock.useWithoutItem`: not sneaking, its screen opens (`displayScreen`), or
    /// the player is told it has no target.
    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        let (block, _) = states::id(event.state).and_then(info)?;
        if block != Block::DisplayLink {
            return None;
        }
        let player = host.player(event.click.player)?;
        if super::items::sneaking(&*host, player) {
            return None;
        }
        let dimension = event.click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        open_screen(&mut lv, player, event.click.pos);
        Some(InteractionResult::SuccessServer)
    }

    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let (block, _) = states::id(event.state).and_then(info)?;
        let c = event.context;
        let clicked = Dir::ALL[c.face as usize];
        let facing = match block {
            Block::DisplayLink => clicked,
            Block::DisplayBoard => {
                let on = super::redstone::offset([c.pos.x, c.pos.y, c.pos.z], clicked.opposite());
                let against = level
                    .block_state(on)
                    .and_then(states::id)
                    .and_then(info)
                    .filter(|(b, _)| *b == Block::DisplayBoard)
                    .and_then(|(_, i)| i.facing);
                match against {
                    Some(f) if !c.sneaking => f,
                    _ => create_engine::place::horizontal_direction(c.yaw).opposite(),
                }
            }
            _ => return None,
        };
        let mut props = vec![("facing", super::kinetics::dir_name(facing))];
        // FlapDisplayBlock: `WATERLOGGED` from the fluid where it goes.
        if block == Block::DisplayBoard {
            let water = super::logistics::placement::in_water(level, event);
            props.push(("waterlogged", if water { "true" } else { "false" }));
        }
        let state = super::redstone::with_properties(level, event.state, &props)?;
        Some(Some(state))
    }
}

// ---------------------------------------------------------------------------------------------
// The link's screen

/// What the screen opens with: the link's sources, settings, target and line.
/// `FlapDisplayBlock.useItemOn` on the controller's line under the click (an eighth into the
/// board): an empty hand takes back a line's own text, glow ink lights it, a named name tag or
/// a clipboard's last viewed entries write it by hand, a dye colours it; the text ones only
/// while the board turns fast enough; a display board goes beside it (`place_board`). Not while
/// sneaking.
fn use_board(
    host: &mut dyn ServerHost,
    click: &mod_api::BlockClick<'_>,
) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    if super::items::sneaking(&*host, player) {
        return None;
    }
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let dim = lv.dim;
    let pos = click.pos;
    if let Some(stack) = click.stack
        && stack.item().id(&*lv.host) == "create:display_board"
    {
        return Some(place_board(&mut lv, player, click, stack));
    }
    let controller = controller_of(&lv, pos)?;
    let y = click.hit.point[1] - f64::from(click.hit.face[1]) * 0.125;
    let line = with_world(|w| {
        w.dims[dim]
            .displays
            .boards
            .get(&controller)
            .map(|b| b.line_index_at(controller[1], y))
    })
    .flatten()?;
    let running = with_world(|w| w.speed(dim, controller))
        .flatten()
        .is_some_and(|s| s.abs() >= create_engine::gauge::MEDIUM_SPEED);
    let stack = click.stack;
    let item = stack.map(|s| s.item().id(&*lv.host));
    // Create answers on the client too, so the off hand is never tried after a main hand the
    // board took; this client can't know, and tries it.
    if click.hand == mod_api::Hand::Off {
        let main = super::items::held(&*lv.host, player, mod_api::Hand::Main);
        let main_id = main.as_ref().map(|s| s.item().id(&*lv.host));
        let took = match main_id.as_deref() {
            None => running,
            Some("minecraft:glow_ink_sac") => true,
            Some("minecraft:name_tag" | "create:clipboard") => running,
            Some(id) => create_engine::dye::DyeColor::of_item(id).is_some(),
        };
        if took {
            return None;
        }
    }
    let sound = |lv: &mut ServerLevel<'_>, s: &str| lv.play_block_sound(s, pos, 1.0, 1.0);
    let changed = match item.as_deref() {
        None => {
            if !running {
                return None;
            }
            board_edit(dim, controller, |b| {
                b.apply_text_manually(line, None);
            })
        }
        Some("minecraft:glow_ink_sac") => {
            sound(&mut lv, "minecraft:item.glow_ink_sac.use");
            board_edit(dim, controller, |b| b.set_glowing(line))
        }
        Some(id) => {
            let stack = stack?;
            let name = if id == "minecraft:name_tag" {
                filters::plain_name(filters::component(
                    &*lv.host,
                    stack,
                    "minecraft:custom_name",
                ))
            } else {
                None
            };
            let clipboard = id == "create:clipboard";
            let dye = create_engine::dye::DyeColor::of_item(id);
            if !clipboard && name.is_none() && dye.is_none() {
                return None;
            }
            if dye.is_none() && !running {
                return None;
            }
            if clipboard {
                // `ClipboardEntry.getLastViewedEntries`: each line of each entry of the page
                // last opened, from the clicked line down.
                let content = clipboard::content(stack);
                let page = usize::try_from(content.previously_opened_page)
                    .unwrap_or(0)
                    .min(content.pages.len().saturating_sub(1));
                let lines: Vec<String> = content
                    .pages
                    .get(page)
                    .map(|p| {
                        p.iter()
                            .flat_map(|e| e.text.split('\n').map(str::to_owned).collect::<Vec<_>>())
                            .collect()
                    })
                    .unwrap_or_default();
                board_edit(dim, controller, |b| {
                    for (i, text) in lines.iter().enumerate() {
                        b.apply_text_manually(line + i, Some(text));
                    }
                })
            } else {
                if let Some(name) = &name {
                    board_edit(dim, controller, |b| {
                        b.apply_text_manually(line, Some(name));
                    });
                }
                if let Some(dye) = dye {
                    sound(&mut lv, "minecraft:item.dye.use");
                    board_edit(dim, controller, |b| b.set_colour(line, dye));
                }
                true
            }
        }
    };
    if changed {
        mark(&lv, controller);
        send_changed(&mut lv);
    }
    Some(InteractionResult::Success)
}

/// `FlapDisplayBlock.PlacementHelper`: a display board clicked on one goes on its nearest free
/// side across its facing (`orderedByDistanceExceptAxis`), facing the same way; none free, the
/// click is taken and nothing is placed (`PlacementOffset.fail`). Its column's connections
/// follow from the placement, as any board's.
fn place_board(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &mod_api::BlockClick<'_>,
    stack: &ItemStack,
) -> InteractionResult {
    let pos = click.pos;
    let Some(facing) = board_at(lv, pos).map(|b| b.facing) else {
        return InteractionResult::Pass;
    };
    if player.may_build(&*lv.host) != Some(true) {
        return InteractionResult::Pass;
    }
    let Some(at) = board_target(lv, pos, click.hit.point, facing) else {
        return InteractionResult::Pass;
    };
    let water = lv
        .block_state(at)
        .and_then(|s| lv.host.block_state_string(s))
        .is_some_and(|t| t.starts_with("minecraft:water"));
    let facts = mod_api::HostBlockStates::block_states(&*lv.host);
    let props = [
        ("up", "false"),
        ("down", "false"),
        ("waterlogged", if water { "true" } else { "false" }),
    ];
    let Some(state) = lv.block_state(pos).and_then(|s| {
        props
            .iter()
            .try_fold(s, |s, (k, v)| s.with(&facts, k, v).ok())
    }) else {
        return InteractionResult::Pass;
    };
    lv.set_state(at, state, update_all());
    lv.place_sound(at);
    if player.is_creative(&*lv.host) != Some(true)
        && let Some(living) = lv.host.living(player.entity())
    {
        let _ = living.set_hand_item(&mut *lv.host, click.hand, stack.clone().shrink(1));
    }
    InteractionResult::Success
}

/// `FlapDisplayBlock.PlacementHelper.getOffset`: the free side nearest the hit `point` of the
/// board at `pos` facing `facing`, across its facing.
fn board_target(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
    facing: Dir,
) -> Option<[i32; 3]> {
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    create_engine::place::ordered_by_distance(hit, |d| d.axis() != facing.axis())
        .into_iter()
        .map(|d| super::redstone::offset(pos, d))
        .find(|p| lv.free(*p))
}

/// The display board helper's target for the indicator: a display board item on a board.
pub(crate) fn helper_target(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
    item: &str,
) -> Option<crate::glue::interaction::HelperTarget> {
    if item != "create:display_board" {
        return None;
    }
    let facing = board_at(lv, pos)?.facing;
    let name = lv.block_state(pos).and_then(|s| {
        let facts = mod_api::HostBlockStates::block_states(&*lv.host);
        s.property(&facts, "facing")
            .ok()
            .flatten()
            .map(str::to_owned)
    })?;
    board_target(lv, pos, point, facing).map(|at| (at, vec![("facing", name)]))
}

/// A change to the board controller at `pos`, shown to its clients.
fn board_edit(dim: usize, pos: [i32; 3], f: impl FnOnce(&mut Board)) -> bool {
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let b = d.boards.get_mut(&pos)?;
        f(b);
        d.shown.push(pos);
        Some(())
    })
    .flatten()
    .is_some()
}

fn open_screen(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((facing, _)) = link_at(lv, pos) else {
        return;
    };
    let link = with_world(|w| w.dims[dim].displays.links.get(&pos).cloned())
        .flatten()
        .unwrap_or_default();
    if link.target_offset == [0, 0, 0] {
        super::items::action_bar(&mut *lv.host, player, "create.display_link.invalid");
        return;
    }
    let target_pos = link.target(pos);
    let target = target_at(lv, target_pos);
    let sources = sources_at(lv, source_of(lv, pos, facing)).to_vec();
    let source = link
        .source
        .and_then(|s| sources.iter().position(|a| *a == s))
        .unwrap_or(0);
    let rows = target.map_or(0, |t| stats_of(lv, target_pos, t).rows);
    let labels = sources
        .iter()
        .map(|s| target.is_some_and(|t| s.allows_labeling(t)))
        .collect();
    let mut config = if link.source.is_some_and(|s| sources.get(source) == Some(&s)) {
        link.config.clone()
    } else {
        Config::default()
    };
    let source_pos = source_of(lv, pos, facing);
    let station = summary::station_name(lv, source_pos);
    if sources.get(source) == Some(&Source::StationSummary) {
        summary::populate(&mut config, station.as_deref());
    }
    let settings = link_screen::Settings {
        pos,
        sources,
        source,
        config,
        target: match target {
            None => 0,
            Some(Target::Nixie) => 1,
            Some(Target::Lectern) => 3,
            Some(_) => 2,
        },
        rows,
        line: link.target_line.min(rows.max(1) - 1).max(0),
        labels,
        source_pos,
        target_pos,
        station: station.unwrap_or_default(),
    };
    net::send_to_player(
        &mut *lv.host,
        player,
        link_screen::CHANNEL,
        &settings.encode(),
    );
}

/// `DisplayLinkConfigurationPacket.applySettings`: a link given another source starts from the
/// settings sent; the same source merges them; then the line, and the link refreshes
/// (`updateGatheredData`). Only for a player near the link (`BlockEntityConfigurationPacket`).
fn configure_from_client(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(s) = link_screen::Settings::decode(data) else {
        return;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(at) = player.entity().position(&*host) else {
        return;
    };
    let near = (0..3)
        .map(|i| (at[i] - (f64::from(s.pos[i]) + 0.5)).powi(2))
        .sum::<f64>()
        < 20.0 * 20.0;
    if !near {
        return;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let pos = s.pos;
    let Some((facing, _)) = link_at(&lv, pos) else {
        return;
    };
    let available = sources_at(&lv, source_of(&lv, pos, facing));
    let source = s
        .sources
        .get(s.source)
        .copied()
        .filter(|x| available.contains(x));
    let dim = lv.dim;
    let now = lv.game_time();
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let Some(l) = d.links.get_mut(&pos) else {
            return;
        };
        l.target_line = s.line.max(0);
        match source {
            Some(src) if l.source != Some(src) => {
                l.source = Some(src);
                l.config = s.config.clone();
            }
            Some(_) => {
                l.config.label = s.config.label.clone();
                l.config.directional = s.config.directional;
                l.config.mode = s.config.mode;
                l.config.cycle = s.config.cycle;
                l.config.format = s.config.format;
                l.config.interval = s.config.interval;
                l.config.filter.clone_from(&s.config.filter);
                l.config.name_column = s.config.name_column;
                l.config.platform_column = s.config.platform_column;
            }
            None => {}
        }
        l.last = None;
        d.schedule(pos, now);
    });
    mark(&lv, pos);
    update(&mut lv, pos);
    send_changed(&mut lv);
}

// ---------------------------------------------------------------------------------------------
// Commands

const LINK_USAGE: &str = "usage: create_debug display_link <x y z> <x y z> [source] [Key=value...]";

/// `/create_debug display_link <link> <target> [source] [Key=value...]`: the display link item's
/// selection (`ClickToLinkBlockItem`, within `displayLinkRange` 64) and its screen's settings
/// (`DisplayLinkConfigurationPacket`); `/create_debug display_state <pos>`: what a link, tube or
/// board holds.
fn display_command(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let c = event.command;
    let literals: Vec<&str> = c.literals.iter().map(String::as_str).collect();
    let bind_link = match literals.as_slice() {
        ["create_debug", "display_link"] => true,
        ["create_debug", "display_state"] => false,
        _ => return Flow::Continue,
    };
    let arg = |name: &str| {
        c.arguments
            .iter()
            .find(|a| a.name == name)
            .map(|a| &a.value)
    };
    let Some(CommandArgumentValue::BlockPos(pos)) = arg("pos").cloned() else {
        return Flow::Continue;
    };
    let rest = match arg("target") {
        Some(CommandArgumentValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    let here = c.position;
    let Some(dimension) = c.dimension.id(&*host).map(|d| d.to_string()) else {
        return Flow::Cancel;
    };
    let line = match ServerLevel::new(host, &dimension) {
        None => "no Create world here".to_owned(),
        Some(mut lv) if bind_link => {
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let coord = |n: usize| -> Option<i32> {
                let t = tokens.get(n)?;
                match t.strip_prefix('~') {
                    Some(off) => Some(here[n % 3].floor() as i32 + off.parse::<i32>().unwrap_or(0)),
                    None => t.parse().ok(),
                }
            };
            match (coord(0), coord(1), coord(2)) {
                (Some(x), Some(y), Some(z)) => {
                    bind(&mut lv, pos, [x, y, z], tokens.get(3..).unwrap_or(&[]))
                }
                _ => LINK_USAGE.into(),
            }
        }
        Some(lv) => display_state(&lv, pos),
    };
    let _ = host.commands().feedback(Text::literal(line));
    Flow::Cancel
}

fn bind(lv: &mut ServerLevel<'_>, pos: [i32; 3], target: [i32; 3], words: &[&str]) -> String {
    let dim = lv.dim;
    let lang = |k: &str| LANG.get(k);
    let Some((facing, _)) = link_at(lv, pos) else {
        return format!("not a display link at {} {} {}", pos[0], pos[1], pos[2]);
    };
    if target_at(lv, target).is_none() {
        return lang("create.display_link.invalid");
    }
    // `closerThan(placedPos, 64)`.
    let d: i64 = (0..3).map(|i| i64::from(target[i] - pos[i]).pow(2)).sum();
    if d >= 64 * 64 {
        return lang("create.display_link.too_far");
    }
    let available = sources_at(lv, source_of(lv, pos, facing));
    let mut source = None;
    let mut config = Config::default();
    let mut line = 0;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w.split_once('=') {
            // A label is the rest of the line.
            Some(("Label", v)) => {
                let rest: Vec<&str> = std::iter::once(v)
                    .chain(words[i + 1..].iter().copied())
                    .collect();
                config.set("Label", &rest.join(" "));
                break;
            }
            Some(("Line", v)) => line = v.parse().unwrap_or(0),
            Some((k, v)) => {
                if !config.set(k, v) {
                    return format!("unknown setting {k}");
                }
            }
            None => match Source::from_id(w) {
                Some(s) => source = Some(s),
                None => return lang("create.display_link.no_source"),
            },
        }
        i += 1;
    }
    // The screen offers the attached block's sources, the first by default.
    let source = match source {
        Some(s) if available.contains(&s) => s,
        Some(_) => return lang("create.display_link.no_source"),
        None => match available.first() {
            Some(&s) => s,
            None => return lang("create.display_link.no_source"),
        },
    };
    if source == Source::StationSummary {
        let station = summary::station_name(lv, source_of(lv, pos, facing));
        summary::populate(&mut config, station.as_deref());
    }
    let now = lv.game_time();
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let l = d.links.entry(pos).or_insert_with(Link::default);
        l.target_offset = [target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]];
        l.target_line = line;
        if l.source != Some(source) {
            l.source = Some(source);
            l.config = config;
        } else {
            // The same source: the settings merge.
            if !config.label.is_empty() {
                l.config.label = config.label;
            }
            l.config.directional = config.directional;
            l.config.mode = config.mode;
            l.config.cycle = config.cycle;
            l.config.format = config.format;
            l.config.interval = config.interval;
            if config.filter.is_some() {
                l.config.filter = config.filter;
            }
            l.config.name_column = config.name_column.or(l.config.name_column);
            l.config.platform_column = config.platform_column.or(l.config.platform_column);
        }
        l.last = None;
        d.schedule(pos, now);
    });
    mark(lv, pos);
    update(lv, pos);
    send_changed(lv);
    format!(
        "{}: {}",
        lang("create.display_link.success"),
        lang(source.name_key())
    )
}

fn display_state(lv: &ServerLevel<'_>, pos: [i32; 3]) -> String {
    let dim = lv.dim;
    let [x, y, z] = pos;
    if let Some((facing, powered)) = link_at(lv, pos) {
        let link = with_world(|w| w.dims[dim].displays.links.get(&pos).cloned()).flatten();
        let Some(l) = link else {
            return format!("display link at {x} {y} {z}: no state");
        };
        let t = l.target(pos);
        let last = l
            .last
            .as_ref()
            .and_then(|(_, _, _, line)| line.text.clone())
            .unwrap_or_default();
        return format!(
            "display link at {x} {y} {z}: facing {}, powered {powered}, target {} {} {} line {}, \
             source {}, label {:?}, due {:?}, last {:?}",
            super::kinetics::dir_name(facing),
            t[0],
            t[1],
            t[2],
            l.target_line,
            l.source.map_or("none", Source::id),
            l.config.label,
            l.due,
            last
        );
    }
    if nixie_at(lv, pos).is_some() {
        let n = with_world(|w| w.dims[dim].displays.nixies.get(&pos).cloned())
            .flatten()
            .unwrap_or_default();
        let (a, b) = n.displayed();
        return format!(
            "nixie tube at {x} {y} {z}: text {:?}, index {}, shows {a:?} {b:?}",
            n.full_text(),
            n.index
        );
    }
    if board_at(lv, pos).is_some() {
        let Some(c) = controller_of(lv, pos) else {
            return format!("display board at {x} {y} {z}: no controller");
        };
        let b = with_world(|w| w.dims[dim].displays.boards.get(&c).cloned()).flatten();
        let Some(mut b) = b else {
            return format!("display board at {x} {y} {z}: no controller");
        };
        let lines: Vec<String> = b
            .lines()
            .iter()
            .map(|l| {
                let texts: Vec<String> = l
                    .sections
                    .iter()
                    .map(|s| s.text.clone().unwrap_or_default())
                    .collect();
                format!("{}{:?}", l.key, texts)
            })
            .collect();
        return format!(
            "display board at {x} {y} {z}: controller {} {} {}, {}x{}, {} chars, lines {}",
            c[0],
            c[1],
            c[2],
            b.x_size,
            b.y_size,
            b.max_chars(),
            lines.join(" | ")
        );
    }
    match sign_at(lv, pos) {
        Some((_, front, _)) => format!("sign at {x} {y} {z}: {front:?}"),
        None => format!("no display at {x} {y} {z}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Saving

/// The column's links, tubes, boards and reservations, under `create_display`.
fn displays_chunk_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let s = |v: &str| Tag::String(v.into());
    let Some(own) = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let mut own = Compound::new();
        // Links: x, y, z, the target's offset, line, Directional, Mode, Cycle, whether it has
        // a StartTime and its two halves; then the source id and the label.
        let mut links: Vec<(&[i32; 3], &Link)> =
            d.links.iter().filter(|(p, _)| inside(p)).collect();
        links.sort_by_key(|(p, _)| **p);
        if !links.is_empty() {
            let mut nums = Vec::new();
            let mut texts = Vec::new();
            for (p, l) in links {
                let start = l.config.start_time.unwrap_or(0);
                nums.extend(p);
                nums.extend(l.target_offset);
                nums.extend([
                    l.target_line,
                    l.config.directional,
                    l.config.mode,
                    l.config.cycle,
                    i32::from(l.config.start_time.is_some()),
                    (start >> 32) as i32,
                    start as i32,
                ]);
                texts.push(s(l.source.map_or("", Source::id)));
                texts.push(s(&l.config.label));
            }
            let mut c = Compound::new();
            c.insert("nums".into(), Tag::IntArray(nums));
            c.insert("texts".into(), Tag::List(texts));
            own.insert("links".into(), Tag::Compound(c));
        }
        // A list's number format, a tunnel's count and a throughput's interval, where any is
        // set: x, y, z, Format, Collected, Interval.
        let extra: Vec<i32> = {
            let mut l: Vec<(&[i32; 3], &Link)> = d
                .links
                .iter()
                .filter(|(p, l)| {
                    inside(p)
                        && (l.config.format != 0
                            || l.config.collected != 0
                            || l.config.interval != 0)
                })
                .collect();
            l.sort_by_key(|(p, _)| **p);
            l.into_iter()
                .flat_map(|(p, l)| {
                    [
                        p[0],
                        p[1],
                        p[2],
                        l.config.format,
                        l.config.collected,
                        l.config.interval,
                    ]
                })
                .collect()
        };
        if !extra.is_empty() {
            own.insert("link_extra".into(), Tag::IntArray(extra));
        }
        // The station summary's settings and the gauges joined (`LinkedGauges`: x, y, z, slot),
        // where a link has any.
        let mut more: Vec<(&[i32; 3], &Link)> = d
            .links
            .iter()
            .filter(|(p, l)| {
                inside(p)
                    && (l.config.filter.is_some()
                        || l.config.name_column.is_some()
                        || l.config.platform_column.is_some()
                        || !l.panels.is_empty())
            })
            .collect();
        more.sort_by_key(|(p, _)| **p);
        if !more.is_empty() {
            let list = more
                .into_iter()
                .map(|(p, l)| {
                    let mut c = Compound::new();
                    c.insert("Pos".into(), Tag::IntArray(p.to_vec()));
                    if let Some(f) = &l.config.filter {
                        c.insert("Filter".into(), s(f));
                    }
                    if let Some(n) = l.config.name_column {
                        c.insert("NameColumn".into(), Tag::Int(n));
                    }
                    if let Some(n) = l.config.platform_column {
                        c.insert("PlatformColumn".into(), Tag::Int(n));
                    }
                    if !l.panels.is_empty() {
                        c.insert(
                            "LinkedGauges".into(),
                            Tag::IntArray(
                                l.panels
                                    .iter()
                                    .flat_map(|(q, slot)| [q[0], q[1], q[2], i32::from(*slot)])
                                    .collect(),
                            ),
                        );
                    }
                    Tag::Compound(c)
                })
                .collect();
            own.insert("link_more".into(), Tag::List(list));
        }
        // Tubes with custom text: x, y, z, index; the text.
        let mut nixies: Vec<_> = d
            .nixies
            .iter()
            .filter(|(p, n)| inside(p) && n.custom.is_some())
            .collect();
        nixies.sort_by_key(|(p, _)| **p);
        if !nixies.is_empty() {
            let mut nums = Vec::new();
            let mut texts = Vec::new();
            for (p, n) in nixies {
                nums.extend(p);
                nums.push(n.index);
                texts.push(s(n.custom.as_deref().unwrap_or("")));
            }
            let mut c = Compound::new();
            c.insert("nums".into(), Tag::IntArray(nums));
            c.insert("texts".into(), Tag::List(texts));
            own.insert("nixies".into(), Tag::Compound(c));
        }
        // Controllers: x, y, z, size, line count, then per line its dye (-1 none), glow,
        // manual and section count, per section its width's bits, cycle and flags; the
        // texts: each line's key and each section's text.
        let mut boards: Vec<[i32; 3]> = d
            .boards
            .iter()
            .filter(|(p, b)| inside(p) && b.controller)
            .map(|(p, _)| *p)
            .collect();
        boards.sort_unstable();
        if !boards.is_empty() {
            let mut nums = Vec::new();
            let mut texts = Vec::new();
            for p in boards {
                let Some(b) = d.boards.get_mut(&p) else {
                    continue;
                };
                nums.extend(p);
                board_to_nums(b, &mut nums, &mut texts);
            }
            let mut c = Compound::new();
            c.insert("nums".into(), Tag::IntArray(nums));
            c.insert("texts".into(), Tag::List(texts));
            own.insert("boards".into(), Tag::Compound(c));
        }
        // Reservations of this column's targets: target, line, link.
        let mut reserved: Vec<_> = d.reserved.iter().filter(|((p, _), _)| inside(p)).collect();
        reserved.sort();
        if !reserved.is_empty() {
            let v: Vec<i32> = reserved
                .into_iter()
                .flat_map(|((t, line), l)| [t[0], t[1], t[2], *line, l[0], l[1], l[2]])
                .collect();
            own.insert("reserved".into(), Tag::IntArray(v));
        }
        Some(own)
    })
    .flatten() else {
        return;
    };
    if !own.is_empty() {
        data.insert("create_display".into(), Tag::Compound(own));
    }
}

/// A controlling board as a column saves it: size, line count, then per line its dye (-1 none),
/// glow, manual and section count, per section its width's bits, cycle and flags; the texts:
/// each line's key and each section's text.
pub(super) fn board_to_nums(b: &mut Board, nums: &mut Vec<i32>, texts: &mut Vec<Tag>) {
    let s = |v: &str| Tag::String(v.into());
    nums.extend([b.x_size, b.y_size]);
    let colour = b.colour.clone();
    let glowing = b.glowing.clone();
    let manual = b.manual.clone();
    let lines = b.lines();
    nums.push(lines.len() as i32);
    for (i, l) in lines.iter().enumerate() {
        nums.extend([
            colour.get(i).copied().flatten().map_or(-1, i32::from),
            i32::from(glowing.get(i).copied().unwrap_or(false)),
            i32::from(manual.get(i).copied().unwrap_or(false)),
            l.sections.len() as i32,
        ]);
        texts.push(s(&l.key));
        for sec in &l.sections {
            nums.extend([
                sec.size.to_bits() as i32,
                sec.cycle as i32,
                i32::from(sec.single_flap)
                    | i32::from(sec.has_gap) << 1
                    | i32::from(sec.right_aligned) << 2
                    | i32::from(sec.wide) << 3
                    | i32::from(sec.text.is_some()) << 5,
            ]);
            texts.push(s(sec.text.as_deref().unwrap_or("")));
        }
    }
}

/// What [`board_to_nums`] wrote: a controlling board, its lines kept when they fit its size.
pub(super) fn board_from_nums(
    next: &mut dyn FnMut() -> Option<i32>,
    texts: &mut dyn FnMut() -> Option<String>,
) -> Option<Board> {
    let (xs, ys, n) = (next()?, next()?, next()?);
    let mut b = Board::default();
    b.set_status(true, xs.clamp(1, 32), ys.clamp(1, 32));
    let mut lines = Vec::new();
    for i in 0..n.max(0) as usize {
        let (Some(colour), Some(glow), Some(manual), Some(m)) = (next(), next(), next(), next())
        else {
            break;
        };
        if let Some(c) = b.colour.get_mut(i) {
            *c = u8::try_from(colour).ok();
        }
        if let Some(g) = b.glowing.get_mut(i) {
            *g = glow != 0;
        }
        if let Some(g) = b.manual.get_mut(i) {
            *g = manual != 0;
        }
        let key = texts().unwrap_or_default();
        let mut sections = Vec::new();
        for _ in 0..m.max(0) {
            let (Some(size), Some(cycle), Some(f)) = (next(), next(), next()) else {
                break;
            };
            let text = texts().unwrap_or_default();
            let cycle = display::Cycle::ALL
                .get(cycle.clamp(0, display::Cycle::ALL.len() as i32 - 1) as usize)
                .copied()
                .unwrap_or(display::Cycle::Alphabet);
            let mut sec =
                display::Section::new(f32::from_bits(size as u32), cycle, f & 1 != 0, f & 2 != 0);
            sec.right_aligned = f & 4 != 0;
            sec.wide = f & 8 != 0;
            sec.text = (f & 32 != 0).then_some(text);
            sections.push(sec);
        }
        lines.push(display::Layout { key, sections });
    }
    if lines.len() == (b.y_size * 2) as usize {
        b.lines = Some(lines);
    }
    Some(b)
}

/// Reads what `displays_chunk_save` wrote, before the column's blocks are found.
fn displays_from_nbt(dim: usize, data: &Compound) {
    let part = |key: &str| -> Option<(Vec<i32>, Vec<String>)> {
        let Some(Tag::Compound(c)) = data.get(&key.into()) else {
            return None;
        };
        let nums = match c.get(&"nums".into()) {
            Some(Tag::IntArray(v)) => v.clone(),
            _ => return None,
        };
        let texts = match c.get(&"texts".into()) {
            Some(Tag::List(l)) => l
                .iter()
                .map(|t| match t {
                    Tag::String(s) => s.to_string_lossy(),
                    _ => String::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        Some((nums, texts))
    };
    with_world(|w| {
        let d = &mut w.dims[dim].displays;
        if let Some((nums, texts)) = part("links") {
            for (i, v) in nums.as_chunks::<13>().0.iter().enumerate() {
                let start = (i64::from(v[11]) << 32) | i64::from(v[12] as u32);
                let link = Link {
                    target_offset: [v[3], v[4], v[5]],
                    target_line: v[6],
                    source: texts.get(2 * i).and_then(|s| Source::from_id(s)),
                    config: Config {
                        label: texts.get(2 * i + 1).cloned().unwrap_or_default(),
                        directional: v[7],
                        mode: v[8],
                        cycle: v[9],
                        start_time: (v[10] != 0).then_some(start),
                        ..Config::default()
                    },
                    due: None,
                    last: None,
                    panels: Vec::new(),
                };
                d.links.insert([v[0], v[1], v[2]], link);
            }
        }
        if let Some(Tag::IntArray(v)) = data.get(&"link_extra".into()) {
            for f in v.as_chunks::<6>().0 {
                if let Some(l) = d.links.get_mut(&[f[0], f[1], f[2]]) {
                    l.config.format = f[3];
                    l.config.collected = f[4];
                    l.config.interval = f[5];
                }
            }
        }
        if let Some(Tag::List(list)) = data.get(&"link_more".into()) {
            for c in list {
                let Tag::Compound(c) = c else { continue };
                let Some(Tag::IntArray(p)) = c.get(&"Pos".into()) else {
                    continue;
                };
                let Some(l) = p
                    .get(..3)
                    .and_then(|p| d.links.get_mut(&[p[0], p[1], p[2]]))
                else {
                    continue;
                };
                if let Some(Tag::String(f)) = c.get(&"Filter".into()) {
                    l.config.filter = Some(f.to_string_lossy());
                }
                if let Some(Tag::Int(n)) = c.get(&"NameColumn".into()) {
                    l.config.name_column = Some(*n);
                }
                if let Some(Tag::Int(n)) = c.get(&"PlatformColumn".into()) {
                    l.config.platform_column = Some(*n);
                }
                if let Some(Tag::IntArray(g)) = c.get(&"LinkedGauges".into()) {
                    l.panels = g
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|v| ([v[0], v[1], v[2]], v[3].clamp(0, 3) as u8))
                        .collect();
                }
            }
        }
        if let Some((nums, texts)) = part("nixies") {
            for (i, v) in nums.as_chunks::<4>().0.iter().enumerate() {
                let n = d.nixies.entry([v[0], v[1], v[2]]).or_default();
                n.custom = texts.get(i).cloned();
                n.index = v[3];
            }
        }
        if let Some((nums, texts)) = part("boards") {
            let mut nums = nums.into_iter();
            let mut texts = texts.into_iter();
            let mut next = || nums.next();
            while let (Some(x), Some(y), Some(z)) = (next(), next(), next()) {
                let Some(b) = board_from_nums(&mut next, &mut || texts.next()) else {
                    break;
                };
                d.boards.insert([x, y, z], b);
            }
        }
        if let Some(Tag::IntArray(v)) = data.get(&"reserved".into()) {
            for r in v.as_chunks::<7>().0 {
                d.reserved
                    .insert(([r[0], r[1], r[2]], r[3]), [r[4], r[5], r[6]]);
            }
        }
    });
}
