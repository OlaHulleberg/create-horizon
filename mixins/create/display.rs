// Create's display links in the game: links reading the block they are attached to and writing
// its text to nixie tubes, signs and display boards. A sibling of the `create` module.
use super::Game;
use super::create::*;
use super::create_display_body::{lang_get, sign_at};
use create_engine::Block;
use create_engine::display::{
    self, Board, BoardState, Config, Face, Lang, Link, NixieState, Reading, Shown, Source, Stats,
    Target,
};
use create_engine::kinetics::Dir;
use std::cell::RefCell;
use std::collections::HashMap;

/// `ItemStack.getHoverName` of an item id: its `item.` then `block.` name.
fn item_name(id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    ["item", "block"]
        .iter()
        .find_map(|p| lang_get(&format!("{p}.{ns}.{path}")))
        .unwrap_or_else(|| path.replace('_', " "))
}

fn block_of(game: &Game<'_>, pos: [i32; 3]) -> Option<(Block, create_engine::parts::StateInfo)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten()
}

fn property<'a>(game: &'a Game<'_>, pos: [i32; 3], key: &str) -> Option<&'a str> {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    game.pack
        .assets
        .registry
        .state(state)?
        .properties
        .get(key)
        .map(String::as_str)
}

fn block_name<'a>(game: &'a Game<'_>, pos: [i32; 3]) -> &'a str {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    game.pack
        .assets
        .registry
        .state(state)
        .map_or("minecraft:air", |s| s.name.as_str())
}

/// A display link's facing and `powered`; None for any other block.
fn link_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(Dir, bool)> {
    match block_of(game, pos)? {
        (Block::DisplayLink, i) => Some((i.facing?, i.powered)),
        _ => None,
    }
}

fn nixie_at(game: &Game<'_>, pos: [i32; 3]) -> Option<NixieState> {
    let (b, info) = block_of(game, pos)?;
    if !display::is_nixie(b) {
        return None;
    }
    Some(NixieState {
        facing: info.facing?,
        face: Face::read(property(game, pos, "double_face")?)?,
        waterlogged: info.waterlogged,
    })
}

fn board_at(game: &Game<'_>, pos: [i32; 3]) -> Option<BoardState> {
    let (b, info) = block_of(game, pos)?;
    if b != Block::DisplayBoard {
        return None;
    }
    Some(BoardState {
        facing: info.facing?,
        up: property(game, pos, "up") == Some("true"),
        down: property(game, pos, "down") == Some("true"),
        waterlogged: info.waterlogged,
    })
}

/// `DisplaySource.getAll`: the sources of the block a link is attached to.
fn sources_at(game: &Game<'_>, pos: [i32; 3]) -> &'static [Source] {
    match block_of(game, pos).map(|(b, _)| b) {
        Some(Block::Speedometer) => &[Source::KineticSpeed],
        Some(Block::Stressometer) => &[Source::KineticStress],
        Some(Block::Depot | Block::Belt | Block::WeightedEjector) => &[Source::ItemNames],
        Some(Block::CuckooClock) => &[Source::TimeOfDay, Source::StopWatch],
        Some(b) if display::is_nixie(b) => &[Source::NixieTube],
        // AllDisplaySources.REDSTONE_POWER `associate(Blocks.TARGET)`.
        None if block_name(game, pos) == "minecraft:target" => &[Source::RedstonePower],
        _ => &[],
    }
}

/// `DisplayTarget.get`: by block first (display boards), then by block entity (nixie tubes,
/// signs). Lecterns are not targets yet.
fn target_at(game: &Game<'_>, pos: [i32; 3]) -> Option<Target> {
    match block_of(game, pos).map(|(b, _)| b) {
        Some(Block::DisplayBoard) => return Some(Target::Board),
        Some(b) if display::is_nixie(b) => return Some(Target::Nixie),
        _ => {}
    }
    sign_at(game, pos).map(|(hanging, _, _)| Target::Sign { hanging })
}

/// The board controlling the board at `pos` (`getController`).
fn controller_of(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<[i32; 3]> {
    let facing = board_at(game, pos)?.facing;
    let at = display::controller_pos(pos, facing, |p| board_at(game, p))?;
    let is = with_world(|w| w.dims[dim].displays.boards.get(&at).map(|b| b.controller)).flatten();
    (is == Some(true)).then_some(at)
}

/// `AllBlocks.DISPLAY_LINK.has(level.getBlockState(p))`, from the state table alone (callable
/// while the world is borrowed).
fn is_link(table: &create_engine::world::StateTable, game: &Game<'_>, p: [i32; 3]) -> bool {
    table
        .get(game.world.block(p[0], p[1], p[2]).0)
        .is_some_and(|(b, _)| b == Block::DisplayLink)
}

/// `DisplayTargetStats` of a target.
fn stats_of(game: &Game<'_>, dim: usize, pos: [i32; 3], target: Target) -> Stats {
    match target {
        // SignDisplayTarget.provideStats.
        Target::Sign { .. } => Stats {
            rows: 4,
            columns: 15,
        },
        // NixieTubeDisplayTarget.getWidth: two characters a tube.
        Target::Nixie => Stats {
            rows: 1,
            columns: 2 * display::walk_nixies(pos, |p| nixie_at(game, p)).len() as i32,
        },
        Target::Board => controller_of(game, dim, pos)
            .and_then(|c| with_world(|w| w.dims[dim].displays.boards.get(&c).map(Board::stats)))
            .flatten()
            .unwrap_or(Stats {
                rows: 1,
                columns: 1,
            }),
    }
}

/// What a source reads from the world now.
fn read_source(game: &mut Game<'_>, dim: usize, pos: [i32; 3], source: Source) -> Reading {
    let (link_facing, _) = link_at(game, pos).unwrap_or((Dir::Up, false));
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
            // `getDirectSignalTo` is not a Horizon query: the strongest signal into it from
            // each side (`getSignal`), with the target block's POWER.
            let mut power = property(game, at, "power")
                .and_then(|p| p.parse::<i32>().ok())
                .unwrap_or(0);
            for d in Dir::ALL {
                let o = d.offset();
                let p =
                    horizon_core::position::BlockPos::new(at[0] + o[0], at[1] + o[1], at[2] + o[2]);
                let dir = horizon_core::geometry::Direction::ALL[d as usize];
                power =
                    power.max(game.with_level(|b, level, ticks| b.signal(level, ticks, p, dir)));
            }
            Reading::Power(power)
        }
        Source::TimeOfDay | Source::StopWatch => {
            let running = block_of(game, at).is_some_and(|(b, _)| b == Block::CuckooClock)
                && with_world(|w| w.speed(dim, at))
                    .flatten()
                    .is_some_and(|s| s != 0.0);
            if source == Source::StopWatch {
                return Reading::Watch {
                    now: game.world.game_time,
                    running,
                };
            }
            let mut rng = create_engine::processing::Rng::new(game.world.game_time as u64);
            let mut next = |n: i32| (rng.next_float() * n as f32) as i32;
            Reading::Time {
                day_time: game.world.day_time,
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
                let Some((b, _)) = block_of(game, p) else {
                    break;
                };
                let item: Option<String> = match b {
                    Block::Depot | Block::WeightedEjector => {
                        match game.world.block_entities.get(&(p[0], p[1], p[2])) {
                            Some(horizon_client::containers::BlockEntity::Mod(m)) => m
                                .items
                                .first()
                                .cloned()
                                .flatten()
                                .map(|s| s.item.to_string()),
                            _ => None,
                        }
                    }
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
                            .filter(|i| (index + 0.5 - i.position).abs() < 0.25)
                            .last()
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
        Source::NixieTube => Reading::Nixie(if nixie_at(game, at).is_some() {
            with_world(|w| w.dims[dim].displays.nixies.get(&at).map(|n| n.full_text())).flatten()
        } else {
            None
        }),
    }
}

/// `DisplayLinkBlock.shouldBePowered`: a signal from any side but the one it is attached to.
fn link_powered(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> bool {
    Dir::ALL
        .into_iter()
        .filter(|&d| d != facing.opposite())
        .any(|d| {
            let o = d.offset();
            let at =
                horizon_core::position::BlockPos::new(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
            let dir = horizon_core::geometry::Direction::ALL[d as usize];
            game.with_level(|b, level, ticks| b.signal(level, ticks, at, dir)) != 0
        })
}

fn mark(game: &mut Game<'_>, dimension: &str, pos: [i32; 3]) {
    game.native_mark_chunk_unsaved(dimension, [pos[0] >> 4, pos[2] >> 4]);
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
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    link: [i32; 3],
    target: [i32; 3],
    line: i32,
    text: &[String],
) {
    let Some((hanging, front, back)) = sign_at(game, target) else {
        return;
    };
    let width = if hanging { 60 } else { 90 };
    let current: Vec<(String, String)> = front.into_iter().zip(back).collect();
    let mut commands = Vec::new();
    for (i, t) in text.iter().enumerate() {
        let at = i as i32 + line;
        if at >= 4 {
            break;
        }
        let reserved = with_world(|w| {
            let table = &w.table;
            let d = &mut w.dims[dim].displays;
            if i == 0 {
                d.reserve(target, at, link);
                false
            } else {
                d.is_reserved(target, at, link, |p| is_link(table, game, p))
            }
        });
        if reserved == Some(true) {
            break;
        }
        let content: String = t.chars().take(width).collect();
        let (front, back) = &current[at as usize];
        for (side, now) in [("front_text", front), ("back_text", back)] {
            if *now != content {
                commands.push(format!(
                    "execute in {dimension} run data modify block {} {} {} {side}.messages[{at}] set value {}",
                    target[0],
                    target[1],
                    target[2],
                    quoted(&content)
                ));
            }
        }
    }
    for c in commands {
        game.native_run_command(&c);
    }
}

/// `DisplayLinkBlockEntity.updateGatheredData`: the source's text written to the target.
fn update(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some((facing, _)) = link_at(game, pos) else {
        return;
    };
    let Some(link) = with_world(|w| w.dims[dim].displays.links.get(&pos).cloned()).flatten() else {
        return;
    };
    let source_pos = display::source_pos(pos, facing);
    let target_pos = link.target(pos);
    if !game.world.loaded(target_pos[0], target_pos[2])
        || !game.world.loaded(source_pos[0], source_pos[2])
    {
        return;
    }
    let target = target_at(game, target_pos);
    // The source is gone from the block it reads: forgotten with its settings.
    if let Some(s) = link.source
        && !sources_at(game, source_pos).contains(&s)
    {
        with_world(|w| {
            if let Some(l) = w.dims[dim].displays.links.get_mut(&pos) {
                l.source = None;
                l.config = Config::default();
                l.due = None;
                l.last = None;
            }
        });
        mark(game, dimension, pos);
        return;
    }
    let (Some(source), Some(target)) = (link.source, target) else {
        return;
    };
    let stats = stats_of(game, dim, target_pos, target);
    let reading = read_source(game, dim, pos, source);
    let lang_fn = |k: &str| lang_get(k);
    let lang = Lang {
        resolve: Some(&lang_fn),
    };
    let Some((line, config, started)) = with_world(|w| {
        let l = w.dims[dim].displays.links.get_mut(&pos)?;
        let had = l.config.start_time;
        let line = l.line(reading, target, stats, &lang);
        Some((line, l.config.clone(), had != l.config.start_time))
    })
    .flatten() else {
        return;
    };
    if started {
        mark(game, dimension, pos);
    }
    match target {
        Target::Nixie => {
            let text = display::provide_text(source, &config, &line, target);
            let first = text.into_iter().next().unwrap_or_default();
            let row = display::walk_nixies(target_pos, |p| nixie_at(game, p));
            let facings: HashMap<[i32; 3], Dir> = row
                .iter()
                .flat_map(|p| {
                    Dir::ALL
                        .map(|d| d.offset())
                        .map(move |o| [p[0] + o[0], p[1] + o[1], p[2] + o[2]])
                })
                .filter_map(|p| link_at(game, p).map(|(f, _)| (p, f)))
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
                mark(game, dimension, p);
            }
        }
        Target::Sign { .. } => {
            let text = display::provide_text(source, &config, &line, target);
            write_sign(
                game,
                dimension,
                dim,
                pos,
                target_pos,
                link.target_line,
                &text,
            );
        }
        Target::Board => {
            let Some(controller) = controller_of(game, dim, target_pos) else {
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
            let changed = with_world(|w| {
                let table = &w.table;
                let d = &mut w.dims[dim].displays;
                d.reserve(controller, line_at, pos);
                let rows = d.boards.get(&controller).map_or(0, |b| b.y_size * 2);
                let reserved: Vec<bool> = (0..rows.max(0))
                    .map(|l| {
                        l > line_at
                            && d.is_reserved(controller, l, pos, |p| is_link(table, game, p))
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
            .flatten();
            if changed == Some(true) {
                mark(game, dimension, controller);
            }
        }
    }
}

/// `tickSource`: due again in the source's interval; nothing while powered.
fn tick_source(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], now: i64) {
    with_world(|w| w.dims[dim].displays.schedule(pos, now));
    let Some((_, powered)) = link_at(game, pos) else {
        return;
    };
    if !powered {
        update(game, dimension, dim, pos);
    }
}

/// Boards placed or removed: the rows around them join again (`updateColumn`, `updateShape`),
/// then each board looks for its place (`updateControllerStatus`).
fn tick_boards(game: &mut Game<'_>, dimension: &str, dim: usize, checks: Vec<[i32; 3]>) {
    let mut todo: Vec<[i32; 3]> = Vec::new();
    for pos in checks {
        todo.push(pos);
        for d in Dir::ALL {
            let o = d.offset();
            todo.push([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]);
        }
    }
    // Every board joined to those, in the plane of each one's face (at most a 64 by 64 wall).
    let mut seen: Vec<[i32; 3]> = Vec::new();
    let mut known: std::collections::HashSet<[i32; 3]> = Default::default();
    while let Some(p) = todo.pop() {
        if seen.len() >= 4096 || !known.insert(p) {
            continue;
        }
        let Some(s) = board_at(game, p) else {
            continue;
        };
        seen.push(p);
        let across = display::clockwise(s.facing);
        for d in [Dir::Up, Dir::Down, across, across.opposite()] {
            let o = d.offset();
            let q = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            if board_at(game, q).is_some_and(|t| t.facing == s.facing) {
                todo.push(q);
            }
        }
    }
    for &p in &seen {
        let Some(s) = board_at(game, p) else {
            continue;
        };
        let (down, up) = display::column_connections(p, s.facing, |q| {
            if !game.world.loaded(q[0], q[2]) {
                return None;
            }
            board_at(game, q).map(|b| b.facing)
        });
        for (key, now, want) in [("down", s.down, down), ("up", s.up, up)] {
            if now != want {
                game.native_set_block_property_with(
                    p,
                    key,
                    if want { "true" } else { "false" },
                    super::native_hooks::flags::UPDATE_ALL,
                );
            }
        }
    }
    for &p in &seen {
        let Some(s) = board_at(game, p) else {
            continue;
        };
        let (c, x, y) = display::controller_status(p, s, |q| board_at(game, q));
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
            mark(game, dimension, p);
        }
    }
}

/// After a level's tick: links whose neighbours changed follow their signal, boards join, links
/// due or told by their source refresh, and clients hear of tubes and boards that changed.
/// Nothing runs when nothing is due.
pub(super) fn tick_displays(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let now = game.world.game_time;
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
    checks.sort();
    checks.dedup();
    for pos in checks {
        // DisplayLinkBlock.neighborChanged: `powered` follows; a link no longer powered
        // refreshes now (`onNoLongerPowered`), the stopwatch starting again.
        let Some((facing, was)) = link_at(game, pos) else {
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
        let powered = link_powered(game, pos, facing);
        if powered != was {
            game.native_set_block_property_with(
                pos,
                "powered",
                if powered { "true" } else { "false" },
                super::native_hooks::flags::UPDATE_CLIENTS,
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
                    mark(game, dimension, pos);
                    update(game, dimension, dim, pos);
                }
                continue;
            }
        }
        if reads_power {
            tick_source(game, dimension, dim, pos, now);
        }
    }
    if !boards.is_empty() {
        tick_boards(game, dimension, dim, boards);
    }
    let mut due = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        let mut due = std::mem::take(&mut d.pulses);
        due.extend(d.take_due(now));
        due
    })
    .unwrap_or_default();
    due.sort();
    due.dedup();
    for pos in due {
        tick_source(game, dimension, dim, pos, now);
    }
    // Tubes changed by this tick's links may have told links reading them: next tick.
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
    let players: Vec<u128> = game
        .native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect();
    send_displays(&mut *game, &players, dim, &shown, &forgotten);
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
    game: &mut Game<'_>,
    players: &[u128],
    dim: usize,
    shown: &[[i32; 3]],
    forgotten: &[[i32; 3]],
) {
    let mut list: Vec<[i32; 3]> = shown.to_vec();
    list.sort();
    list.dedup();
    let mut all: Vec<Shown> = list.into_iter().filter_map(|p| shown_of(dim, p)).collect();
    all.extend(forgotten.iter().map(|&pos| Shown::Forget { pos }));
    // Well under the message size limit: 16 at a time.
    for chunk in all.chunks(16) {
        let m = display::encode(chunk);
        for &p in players {
            send_to_player(game, p, display::CHANNEL, &m);
        }
    }
}

/// A joining client gets every tube with custom text and every controller.
pub(super) fn sync_displays(game: &mut Game<'_>, player: u128, dim: usize) {
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
        send_displays(game, &[player], dim, &list, &[]);
    }
}

/// `DisplayLinkBlock.getStateForPlacement` (on the clicked face) and `FlapDisplayBlock`'s
/// (facing the player, or as the board it was placed against unless sneaking). True when the
/// block was one of them.
pub(super) fn place_display(
    game: &mut Game<'_>,
    pos: [i32; 3],
    block: Block,
    clicked: Dir,
) -> bool {
    match block {
        Block::DisplayLink => {
            if link_at(game, pos).map(|l| l.0) != Some(clicked) {
                game.native_set_block_property(pos, "facing", dir_name(clicked));
            }
            true
        }
        Block::DisplayBoard => {
            let o = clicked.offset();
            let on = [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]];
            let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
            let facing = match board_at(game, on) {
                Some(s) if !sneaking => s.facing,
                _ => create_engine::place::horizontal_direction(game.yaw).opposite(),
            };
            if board_at(game, pos).map(|s| s.facing) != Some(facing) {
                game.native_set_block_property(pos, "facing", dir_name(facing));
            }
            true
        }
        _ => false,
    }
}

/// `/create_debug display_link <link> <target> [source] [Key=value...]`: the display link item's
/// selection (`ClickToLinkBlockItem`, within `displayLinkRange` 64) and its screen's settings
/// (`DisplayLinkConfigurationPacket`); `/create_debug display_state <pos>`: what a link, tube or
/// board holds. True when the command was one of these.
pub(super) fn display_command(game: &mut Game<'_>, command: &str) -> bool {
    let link = command.strip_prefix("create_debug display_link ");
    let state = command.strip_prefix("create_debug display_state ");
    let Some(rest) = link.or(state) else {
        return false;
    };
    let player = game.native_acting_player();
    let dimension = player
        .as_ref()
        .map_or_else(|| "minecraft:overworld".to_owned(), |p| p.dimension.clone());
    let dim = create_engine::world::dimension_index(&dimension).unwrap_or(0);
    let here = player.as_ref().map(|p| p.position);
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let coord = |n: usize| -> Option<i32> {
        let t = tokens.get(n)?;
        match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        }
    };
    let pos = match (coord(0), coord(1), coord(2)) {
        (Some(x), Some(y), Some(z)) => [x, y, z],
        _ => {
            game.native_feedback(horizon_client::text::Text::literal(
                "usage: create_debug display_link <x y z> <x y z> [source] [Key=value...]",
            ));
            return true;
        }
    };
    let line = if state.is_some() {
        display_state(game, dim, pos)
    } else {
        match (coord(3), coord(4), coord(5)) {
            (Some(x), Some(y), Some(z)) => bind(
                game,
                &dimension,
                dim,
                pos,
                [x, y, z],
                tokens.get(6..).unwrap_or(&[]),
            ),
            _ => "usage: create_debug display_link <x y z> <x y z> [source] [Key=value...]".into(),
        }
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}

fn bind(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    target: [i32; 3],
    words: &[&str],
) -> String {
    let resolve = |k: &str| lang_get(k);
    let lang = |k: &str| {
        Lang {
            resolve: Some(&resolve),
        }
        .get(k)
    };
    let Some((facing, _)) = link_at(game, pos) else {
        return format!("not a display link at {} {} {}", pos[0], pos[1], pos[2]);
    };
    if target_at(game, target).is_none() {
        return lang("create.display_link.invalid");
    }
    // `closerThan(placedPos, 64)`.
    let d: i64 = (0..3).map(|i| i64::from(target[i] - pos[i]).pow(2)).sum();
    if d >= 64 * 64 {
        return lang("create.display_link.too_far");
    }
    let available = sources_at(game, display::source_pos(pos, facing));
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
    let now = game.world.game_time;
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
    mark(game, dimension, pos);
    update(game, dimension, dim, pos);
    let players: Vec<u128> = game
        .native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect();
    let (shown, forgotten) = with_world(|w| {
        let d = &mut w.dims[dim].displays;
        (
            std::mem::take(&mut d.shown),
            std::mem::take(&mut d.forgotten),
        )
    })
    .unwrap_or_default();
    send_displays(game, &players, dim, &shown, &forgotten);
    format!(
        "{}: {}",
        lang("create.display_link.success"),
        lang(source.name_key())
    )
}

fn display_state(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) -> String {
    let [x, y, z] = pos;
    if let Some((facing, powered)) = link_at(game, pos) {
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
            "display link at {x} {y} {z}: facing {}, powered {powered}, target {} {} {} line {}, source {}, label {:?}, due {:?}, last {:?}",
            dir_name(facing),
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
    if nixie_at(game, pos).is_some() {
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
    if board_at(game, pos).is_some() {
        let Some(c) = controller_of(game, dim, pos) else {
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
    match sign_at(game, pos) {
        Some((_, front, _)) => format!("sign at {x} {y} {z}: {front:?}"),
        None => format!("no display at {x} {y} {z}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Saving

/// The column's links, tubes, boards and reservations, under `create_display`.
pub(super) fn displays_chunk_save(
    dim: usize,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    use horizon_world::nbt::{Compound, Tag};
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
        boards.sort();
        if !boards.is_empty() {
            let mut nums = Vec::new();
            let mut texts = Vec::new();
            for p in boards {
                let b = d.boards.get_mut(&p).expect("listed");
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
    if own.is_empty() {
        data.remove(&"create_display".into());
    } else {
        data.insert("create_display".into(), Tag::Compound(own));
    }
}

/// Reads what `displays_chunk_save` wrote, before the column's blocks are found.
pub(super) fn displays_from_nbt(dim: usize, data: Option<&horizon_world::nbt::Compound>) {
    use horizon_world::nbt::Tag;
    let Some(data) = data else {
        return;
    };
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
            for (i, v) in nums.chunks_exact(13).enumerate() {
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
            for (i, v) in nums.chunks_exact(4).enumerate() {
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
            for r in v.chunks_exact(7) {
                d.reserved
                    .insert(([r[0], r[1], r[2]], r[3]), [r[4], r[5], r[6]]);
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Clients

/// A board on a client: its lines' sections spinning toward their text.
#[derive(Clone, Debug, Default)]
pub(super) struct ClientBoard {
    pub(super) x_size: i32,
    pub(super) y_size: i32,
    pub(super) lines: Vec<(String, Vec<display::ShownSection>)>,
    pub(super) colour: Vec<Option<u8>>,
    pub(super) glowing: Vec<bool>,
    /// Whether it turns fast enough for its flaps to move (`isSpeedRequirementFulfilled`), as
    /// last drawn.
    pub(super) running: bool,
}

/// What clients know of tubes and boards, and the text they last drew of each (API 55: as
/// `NixieTubeRenderer` and `FlapDisplayRenderer` draw it).
#[derive(Default)]
pub(super) struct ClientDisplays {
    pub(super) nixies: HashMap<[i32; 3], (String, String)>,
    /// Tubes whose characters changed since they were last drawn.
    pub(super) nixies_changed: Vec<[i32; 3]>,
    pub(super) boards: HashMap<[i32; 3], ClientBoard>,
    /// Boards with flaps still turning.
    pub(super) active: Vec<[i32; 3]>,
    /// Boards (controllers) whose text or a flap's character may have changed: drawn again.
    pub(super) boards_changed: Vec<[i32; 3]>,
    /// The board text each controller last set.
    pub(super) boards_drawn: HashMap<[i32; 3], Vec<display::FlapText>>,
    pub(super) rng: Option<create_engine::processing::Rng>,
}

thread_local! {
    pub(super) static DISPLAYS: RefCell<ClientDisplays> = RefCell::new(ClientDisplays::default());
}

pub(super) fn reset_client_displays() {
    DISPLAYS.with_borrow_mut(|d| *d = ClientDisplays::default());
}

/// `NixieTubeBlockEntity.read` / `FlapDisplayBlockEntity.read` on a client.
pub(super) fn displays_from_server(data: &[u8]) {
    let lang_fn = |k: &str| lang_get(k);
    let lang = Lang {
        resolve: Some(&lang_fn),
    };
    let counts: Vec<usize> = display::Cycle::ALL
        .iter()
        .map(|c| c.options(&lang).len())
        .collect();
    DISPLAYS.with_borrow_mut(|c| {
        for s in display::decode(data) {
            match s {
                Shown::Nixie {
                    pos,
                    text,
                    index,
                    redstone,
                } => {
                    let n = display::Nixie {
                        custom: text,
                        index,
                        redstone,
                    };
                    c.nixies.insert(pos, n.displayed());
                    c.nixies_changed.push(pos);
                }
                Shown::Board {
                    pos,
                    x_size,
                    y_size,
                    lines,
                    colour,
                    glowing,
                } => {
                    let b = c.boards.entry(pos).or_default();
                    // A new size forgets the lines (`lines = null`).
                    if b.x_size != x_size || b.y_size != y_size {
                        let max = Board {
                            x_size,
                            ..Board::default()
                        }
                        .max_chars();
                        let fresh = display::Layout::new(max);
                        b.lines = (0..lines.len())
                            .map(|_| {
                                let n = counts[fresh.sections[0].cycle as usize];
                                (
                                    fresh.key.clone(),
                                    fresh
                                        .sections
                                        .iter()
                                        .map(|s| display::ShownSection::new(s.clone(), n))
                                        .collect(),
                                )
                            })
                            .collect();
                    }
                    b.x_size = x_size;
                    b.y_size = y_size;
                    b.colour = colour;
                    b.glowing = glowing;
                    b.lines.resize_with(lines.len(), Default::default);
                    for (shown, layout) in b.lines.iter_mut().zip(lines) {
                        if shown.0 != layout.key || shown.1.len() != layout.sections.len() {
                            // FlapDisplayLayout.read: another layout's sections are new.
                            shown.0 = layout.key;
                            shown.1 = layout
                                .sections
                                .into_iter()
                                .map(|s| {
                                    let n = counts[s.cycle as usize];
                                    display::ShownSection::new(s, n)
                                })
                                .collect();
                        } else {
                            // FlapDisplaySection.update.
                            for (old, new) in shown.1.iter_mut().zip(layout.sections) {
                                if new.text.as_deref().is_some_and(|t| !t.is_empty()) {
                                    old.section.text = new.text;
                                }
                                old.refresh(new.transition);
                            }
                        }
                    }
                    if b.lines.iter().any(|l| l.1.iter().any(|s| !s.settled()))
                        && !c.active.contains(&pos)
                    {
                        c.active.push(pos);
                    }
                    c.boards_changed.push(pos);
                }
                Shown::Forget { pos } => {
                    if c.nixies.remove(&pos).is_some() {
                        c.nixies_changed.push(pos);
                    }
                    if c.boards.remove(&pos).is_some() {
                        c.boards_changed.push(pos);
                    }
                    c.active.retain(|p| *p != pos);
                }
            }
        }
    });
}

/// `FlapDisplayBlockEntity.tick` on a client, for boards with flaps still turning (while
/// running: `isSpeedRequirementFulfilled`): instant over 128 rpm; the flaps' sounds.
pub(super) fn tick_client_displays(game: &mut Game<'_>) {
    let active = DISPLAYS.with_borrow(|d| d.active.clone());
    if active.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for pos in active {
        let speed = CLIENT
            .with_borrow(|c| c.as_ref().and_then(|c| c.speeds.get(&pos).map(|s| s[0])))
            .unwrap_or(0.0);
        let running = speed.abs() >= create_engine::gauge::MEDIUM_SPEED;
        // Spinning flaps stop where they are and start again with the board (drawn once each).
        DISPLAYS.with_borrow_mut(|d| {
            if let Some(b) = d.boards.get_mut(&pos)
                && b.running != running
            {
                b.running = running;
                d.boards_changed.push(pos);
            }
        });
        if !running {
            keep.push(pos);
            continue;
        }
        let instant = speed.abs() > 128.0;
        let facing = {
            let state = game.world.block(pos[0], pos[1], pos[2]).0;
            CLIENT
                .with_borrow(|c| {
                    c.as_ref()
                        .and_then(|c| c.table.get(state).and_then(|(_, i)| i.facing))
                })
                .unwrap_or(Dir::North)
        };
        let (flaps, settled, size) = DISPLAYS.with_borrow_mut(|d| {
            let seed = game.world.game_time as u64 ^ 0x9E37_79B9;
            let mut rng = d
                .rng
                .take()
                .unwrap_or_else(|| create_engine::processing::Rng::new(seed));
            let mut next = |n: i32| (rng.next_float() * n.max(1) as f32) as i32;
            let Some(b) = d.boards.get_mut(&pos) else {
                d.rng = Some(rng);
                return (0, true, (1, 1));
            };
            let mut flaps = 0;
            for l in &mut b.lines {
                for s in &mut l.1 {
                    flaps += s.tick(instant, &mut next);
                }
            }
            let settled = b.lines.iter().all(|l| l.1.iter().all(|s| s.settled()));
            let size = (b.x_size, b.y_size);
            d.rng = Some(rng);
            // Its flaps turned (or came to rest): the characters shown may differ.
            d.boards_changed.push(pos);
            (flaps, settled, size)
        });
        if !settled {
            keep.push(pos);
        }
        if flaps == 0 {
            continue;
        }
        // At the middle of the board: `getDirection().getClockWise()` by xSize / 2, down by
        // ySize / 2.
        let side = display::clockwise(facing.opposite()).offset();
        let middle = [
            f64::from(pos[0] + side[0] * (size.0 / 2)),
            f64::from(pos[1] - size.1 / 2),
            f64::from(pos[2] + side[2] * (size.0 / 2)),
        ];
        let volume = (flaps as f32 / 20.0).clamp(0.25, 1.5);
        let bg = (flaps as f32 / 40.0).clamp(0.25, 1.0);
        game.audio
            .play("create:scroll_value", Some(middle), volume, 0.56);
        game.audio
            .play("minecraft:block.calcite.hit", Some(middle), 0.35 * bg, 1.95);
    }
    DISPLAYS.with_borrow_mut(|d| d.active = keep);
}

/// `NixieTubeRenderer` on a client: the tubes whose characters changed are drawn again through
/// Horizon's world text (API 55); a tube no longer there has its text cleared.
pub(super) fn draw_client_nixies(game: &mut Game<'_>) {
    let changed = DISPLAYS.with_borrow_mut(|c| std::mem::take(&mut c.nixies_changed));
    for pos in changed {
        let key = super::create_logistics::depot_key(pos, 223, 0);
        let shown = DISPLAYS.with_borrow(|c| c.nixies.get(&pos).cloned());
        // The client has no server world table: the tube's name and properties from the registry.
        let tube = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .and_then(|s| {
                let i = create_engine::dye::NIXIE_TUBES
                    .iter()
                    .position(|b| s.name.strip_prefix("create:") == Some(b.name()))?;
                let face = Face::read(s.properties.get("double_face")?)?;
                let facing = Dir::ALL.into_iter().find(|d| {
                    Some(dir_name(*d)) == s.properties.get("facing").map(String::as_str)
                })?;
                Some((create_engine::dye::DyeColor::ALL[i], face, facing))
            });
        let (Some((left, right)), Some((colour, face, facing))) = (shown, tube) else {
            game.native_text_clear(key);
            continue;
        };
        let lines: Vec<super::native_text::TextLine> =
            display::nixie_texts(face, facing, colour, &left, &right, |s| {
                game.native_text_width(s)
            })
            .into_iter()
            .map(|t| super::native_text::TextLine {
                pose: t.pose,
                text: t.text,
                color: 0xFF00_0000 | t.color,
                outline: 0,
                full_bright: true,
            })
            .collect();
        game.native_text_set(key, pos, &lines);
    }
}

/// `FlapDisplayRenderer` on a client: the boards whose text or flaps changed are laid out again
/// from their controller (`display::board_texts`) and set through Horizon's world text (API 55)
/// only when what they show differs from what was set; a board no longer there is cleared.
pub(super) fn draw_client_boards(game: &mut Game<'_>) {
    let mut changed = DISPLAYS.with_borrow_mut(|c| std::mem::take(&mut c.boards_changed));
    if changed.is_empty() {
        return;
    }
    changed.sort_unstable();
    changed.dedup();
    let lang_fn = |k: &str| lang_get(k);
    let lang = Lang {
        resolve: Some(&lang_fn),
    };
    let options: Vec<Vec<String>> = display::Cycle::ALL
        .iter()
        .map(|c| c.options(&lang))
        .collect();
    let ticks = game.world.game_time;
    for pos in changed {
        let key = super::create_logistics::depot_key(pos, 224, 0);
        let board = DISPLAYS.with_borrow(|c| c.boards.get(&pos).cloned());
        // The client has no server world table: the board's facing from the registry.
        let facing = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .filter(|s| s.name == "create:display_board")
            .and_then(|s| {
                let f = s.properties.get("facing")?;
                Dir::ALL.into_iter().find(|d| dir_name(*d) == f.as_str())
            });
        let (Some(b), Some(facing)) = (board, facing) else {
            if DISPLAYS.with_borrow_mut(|c| c.boards_drawn.remove(&pos).is_some()) {
                game.native_text_clear(key);
            }
            continue;
        };
        let texts = display::board_texts(
            facing,
            b.x_size,
            &b.lines,
            &b.colour,
            &b.glowing,
            &options,
            b.running,
            ticks,
            |s| game.native_text_width(s),
        );
        if DISPLAYS.with_borrow(|c| c.boards_drawn.get(&pos) == Some(&texts)) {
            continue;
        }
        let lines: Vec<super::native_text::TextLine> = texts
            .iter()
            .map(|t| super::native_text::TextLine {
                pose: t.pose,
                text: t.text.clone(),
                color: t.color,
                outline: 0,
                full_bright: t.full_bright,
            })
            .collect();
        game.native_text_set(key, pos, &lines);
        DISPLAYS.with_borrow_mut(|c| c.boards_drawn.insert(pos, texts));
    }
}
