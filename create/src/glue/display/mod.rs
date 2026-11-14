//! Create's display links, display boards and nixie tubes (and the clipboard): links read the
//! block they are attached to and write its text to nixie tubes, signs and display boards; the
//! boards' flaps turn toward their text and the tubes glow on clients (`client`).
//!
//! Left out, the API having no way to do it:
//! - Text read on the server in the game's language: the server has no language of its own
//!   to ask, so a display source's words and an item's name are Create's en_us text (an item's
//!   name its id's path in words); clients draw the boards' flap characters in their own.
//! - The display boards' text alpha (`.85`, `.975` when glowing): a block entity's text is
//!   drawn opaque.

pub mod client;
mod clipboard;

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
    BlockPart, BlockState, ClientHost, Command, CommandArgumentValue, Compound, Flow, LevelHost,
    PlayerRef, RegisterCommandsEvent, Registrar, RegistrationContext, RunModCommandEvent,
    ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, StateForPlacementEvent, Tag,
    Text,
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
        client::register(r);
        clipboard::register(r, cx);
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
        clipboard::client_message(host, player, channel, data)
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        client::server_message(host, channel, data)
            || clipboard::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
        clipboard::tick(host);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        clipboard::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        clipboard::screen_mouse_pressed(host, event)
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
        clipboard::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        clipboard::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        clipboard::screen_char_typed(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        clipboard::screen_closing(host, event);
    }

    fn client_reset(&self) {
        client::reset();
        clipboard::reset();
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
/// its `item.` then `block.` key, else its path in words.
fn item_name(id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    ["item", "block"]
        .iter()
        .find_map(|p| display::en_us(&format!("{p}.{ns}.{path}")))
        .map_or_else(|| path.replace('_', " "), str::to_owned)
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
        // AllDisplaySources.REDSTONE_POWER `associate(Blocks.TARGET)`.
        None if block_name(lv, pos) == "minecraft:target" => &[Source::RedstonePower],
        _ => &[],
    }
}

/// `DisplayTarget.get`: by block first (display boards), then by block entity (nixie tubes,
/// signs). Lecterns are not targets yet.
fn target_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Target> {
    match block_at(lv, pos).map(|(b, _)| b) {
        Some(Block::DisplayBoard) => return Some(Target::Board),
        Some(b) if display::is_nixie(b) => return Some(Target::Nixie),
        _ => {}
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
fn read_source(lv: &ServerLevel<'_>, pos: [i32; 3], source: Source) -> Reading {
    let dim = lv.dim;
    let (link_facing, _) = link_at(lv, pos).unwrap_or((Dir::Up, false));
    let at = display::source_pos(pos, link_facing);
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
        let content: String = t.chars().take(width).collect();
        let (front, back) = &current[at as usize];
        for (side, now) in [("front_text", front), ("back_text", back)] {
            if *now != content {
                commands.push(format!(
                    "data modify block {} {} {} {side}.messages[{at}] set value {}",
                    target[0],
                    target[1],
                    target[2],
                    quoted(&content)
                ));
            }
        }
    }
    for c in commands {
        lv.run_command(&c);
    }
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
    let source_pos = display::source_pos(pos, facing);
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
    match target {
        Target::Nixie => {
            let text = display::provide_text(source, &config, &line, target);
            let first = text.into_iter().next().unwrap_or_default();
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
            let text = display::provide_flap_text(source, &config, &line);
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
                lv.set_property(p, key, if want { "true" } else { "false" }, update_all());
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
    let dim = lv.dim;
    let now = lv.game_time();
    if with_world(|w| w.dims[dim].displays.idle(now)).unwrap_or(true) {
        return;
    }
    let Some((mut checks, boards)) = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        (
            std::mem::take(&mut d.checks),
            std::mem::take(&mut d.board_checks),
        )
    }) else {
        return;
    };
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
                    // `onSignalReset`: the stopwatch starts again.
                    if l.source == Some(Source::StopWatch) {
                        l.config.start_time = Some(now);
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
}

/// A joining client gets every tube with custom text and every controller.
fn sync_displays(lv: &mut ServerLevel<'_>, player: PlayerRef) {
    let dim = lv.dim;
    let list: Vec<[i32; 3]> = with_world(|w| {
        let d = &w.dims[dim].displays;
        d.nixies
            .iter()
            .filter(|(_, n)| n.custom.is_some())
            .map(|(p, _)| *p)
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

// ---------------------------------------------------------------------------------------------
// Placement

/// `DisplayLinkBlock.getStateForPlacement` (on the clicked face) and `FlapDisplayBlock`'s
/// (facing the player, or as the board it was placed against unless sneaking).
struct DisplayBlock;

#[mod_api::horizon_part]
impl BlockPart for DisplayBlock {
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
        let state = super::redstone::with_properties(
            level,
            event.state,
            &[("facing", super::kinetics::dir_name(facing))],
        )?;
        Some(Some(state))
    }
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
    let available = sources_at(lv, display::source_pos(pos, facing));
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
                    },
                    due: None,
                    last: None,
                };
                d.links.insert([v[0], v[1], v[2]], link);
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
            while let (Some(x), Some(y), Some(z), Some(xs), Some(ys), Some(n)) =
                (next(), next(), next(), next(), next(), next())
            {
                let mut b = Board::default();
                b.set_status(true, xs.clamp(1, 32), ys.clamp(1, 32));
                let mut lines = Vec::new();
                for i in 0..n.max(0) as usize {
                    let (Some(colour), Some(glow), Some(manual), Some(m)) =
                        (next(), next(), next(), next())
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
                    let key = texts.next().unwrap_or_default();
                    let mut sections = Vec::new();
                    for _ in 0..m.max(0) {
                        let (Some(size), Some(cycle), Some(f)) = (next(), next(), next()) else {
                            break;
                        };
                        let text = texts.next().unwrap_or_default();
                        let cycle = display::Cycle::ALL
                            .get(cycle.clamp(0, 3) as usize)
                            .copied()
                            .unwrap_or(display::Cycle::Alphabet);
                        let mut sec = display::Section::new(
                            f32::from_bits(size as u32),
                            cycle,
                            f & 1 != 0,
                            f & 2 != 0,
                        );
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
