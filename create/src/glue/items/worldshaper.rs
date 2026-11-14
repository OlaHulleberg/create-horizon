//! The handheld worldshaper: its use shapes the block it looks at within 128 blocks
//! (`ZapperItem.use` → `WorldshaperItem.activate`), its left click picks the block it paints
//! with (`ZapperInteractionHandler.trySelect`). Its screen is stood in for by
//! `/create_debug worldshaper`, writing the held worldshaper's settings in its custom data under
//! Create's component names.

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::{
    Item, action_bar, custom, held, is, set_custom, set_held, set_slot, slot_stack, sneaking,
};
use create_engine::kinetics::Dir;
use create_engine::worldshaper as ws;
use mod_api::{
    BlockFace, BlockState, ClipBlock, ClipFluid, CommandArgumentValue, CommandSource, Hand, Host,
    HostBlockStates, HostSide, InteractionResult, ItemPart, ItemStack, PlayerRef,
    RunModCommandEvent, ServerHost, SoundSource, Tag, UseItemEvent,
};
use std::collections::BTreeMap;

pub(super) struct Shaper;

#[mod_api::horizon_part]
impl ItemPart for Shaper {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        if let HostSide::Server(host) = host {
            use_shaper(host, event);
        }
        Some(InteractionResult::Success)
    }
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The worldshaper's settings and the block it uses (`SHAPER_BLOCK_USED` as a state's text).
fn shaper(stack: &ItemStack) -> (ws::Settings, Option<String>) {
    let c = custom(stack);
    let name = |k: &str| text(c.get(&k.into())).unwrap_or_default();
    let settings = ws::Settings {
        brush: ws::Brush::parse(&name(ws::BRUSH_KEY)).unwrap_or_default(),
        params: match c.get(&ws::PARAMS_KEY.into()) {
            Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        },
        tool: ws::Tool::parse(&name(ws::TOOL_KEY)).unwrap_or_default(),
        placement: ws::Placement::parse(&name(ws::PLACEMENT_KEY)).unwrap_or_default(),
        pattern: ws::Pattern::parse(&name(ws::PATTERN_KEY)).unwrap_or_default(),
    };
    (settings, text(c.get(&ws::BLOCK_KEY.into())))
}

/// `WorldshaperItem.configureSettings` (and the block, when given: `trySelect`).
fn set_shaper(stack: &mut ItemStack, s: &ws::Settings, block: Option<&str>) {
    let mut entries = vec![
        (ws::BRUSH_KEY, Some(Tag::String(s.brush.name().into()))),
        (ws::PARAMS_KEY, s.params.map(|p| Tag::IntArray(p.to_vec()))),
        (ws::TOOL_KEY, Some(Tag::String(s.tool.name().into()))),
        (
            ws::PLACEMENT_KEY,
            Some(Tag::String(s.placement.name().into())),
        ),
        (ws::PATTERN_KEY, Some(Tag::String(s.pattern.name().into()))),
    ];
    if let Some(b) = block {
        entries.push((ws::BLOCK_KEY, Some(Tag::String(b.into()))));
    }
    set_custom(stack, &entries);
}

/// `state`'s properties.
fn props_of(host: &dyn Host, state: BlockState) -> BTreeMap<String, String> {
    let text = host.block_state_string(state).unwrap_or_default();
    let (_, own) = states::split(&text);
    own.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// `state` with `props` set.
fn with_props(host: &dyn Host, state: BlockState, props: &BTreeMap<String, String>) -> BlockState {
    let facts = host.block_states();
    props
        .iter()
        .fold(state, |s, (k, v)| s.with(&facts, k, v).unwrap_or(s))
}

/// The state a pick takes (`trySelect`): a state that can be picked, its age set to 0 when
/// used; None when it can't (`selected_state`).
fn picked(host: &dyn Host, state: BlockState) -> Option<BlockState> {
    let picked = ws::selected_state(&props_of(host, state))?;
    Some(with_props(host, state, &picked))
}

fn dir_of(face: BlockFace) -> Option<Dir> {
    Some(match face {
        BlockFace::Down => Dir::Down,
        BlockFace::Up => Dir::Up,
        BlockFace::North => Dir::North,
        BlockFace::South => Dir::South,
        BlockFace::West => Dir::West,
        BlockFace::East => Dir::East,
        _ => return None,
    })
}

/// The block the player looks at within 128 blocks (`Level.clip` with `Block.OUTLINE`, no
/// fluids): its position and the face entered.
fn aimed(lv: &ServerLevel<'_>, player: PlayerRef) -> Option<([i32; 3], Dir)> {
    let entity = player.entity();
    let eye = entity.eye_position(&*lv.host)?;
    let look = entity.view_vector(&*lv.host)?;
    let to = [0, 1, 2].map(|i| eye[i] + look[i] * ws::RANGE);
    let hit = lv
        .level
        .clip(&*lv.host, eye, to, ClipBlock::Outline, ClipFluid::None)?;
    if hit.miss {
        return None;
    }
    Some((hit.block, dir_of(hit.face)?))
}

/// Whether `state` is air (`BlockState.isAir`).
fn is_air(host: &dyn Host, state: BlockState) -> bool {
    matches!(
        state.block().id(host).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `ZapperInteractionHandler.leftClickingBlocksWithTheZapperSelectsTheBlock` → `trySelect`: the
/// worldshaper in the main hand (not sneaking) picks the block it looks at within 128 blocks,
/// unless it can't be picked (no item, a block entity: Create takes those of `create:safe_nbt`
/// with their data, which the port doesn't copy) or it already uses that state; the click
/// then breaks nothing. Whether it picked.
pub(super) fn left_click(host: &mut dyn ServerHost) -> bool {
    select(host).is_some()
}

fn select(host: &mut dyn ServerHost) -> Option<()> {
    let player = host.players().acting_player()?;
    let mut stack = held(&*host, player, Hand::Main)?;
    if !is(Some(&stack), Item::Worldshaper) || sneaking(&*host, player) {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let lv = ServerLevel::new(host, &dimension)?;
    let (hit, _) = aimed(&lv, player)?;
    let state = lv.block_state(hit)?;
    let facts = lv.host.block_states();
    if lv.host.block_item(state.block()).is_none() || facts.has_block_entity(state) == Some(true) {
        return None;
    }
    let text = lv.host.block_state_string(picked(&*lv.host, state)?)?;
    let (settings, used) = shaper(&stack);
    if used.as_deref() == Some(text.as_str()) {
        return None;
    }
    set_shaper(&mut stack, &settings, Some(&text));
    set_held(lv.host, player, Hand::Main, Some(stack));
    if let Some(at) = player.entity().position(&*lv.host) {
        lv.level.play_sound(
            &mut *lv.host,
            "create:confirm",
            at,
            SoundSource::Players,
            1.0,
            1.0,
        );
    }
    Some(())
}

/// `ZapperItem.use` (not sneaking) for the worldshaper: the block it looks at within 128
/// blocks is shaped by its brush and tool (`WorldshaperItem.activate`).
fn use_shaper(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> Option<()> {
    let player = host.player(event.player)?;
    if sneaking(&*host, player) {
        return None;
    }
    let (settings, block) = shaper(event.stack);
    // `validateUsage`.
    let key = if settings.params.is_none() {
        Some("create.terrainzapper.shiftRightClickToSet")
    } else if settings.tool.requires_block() && block.is_none() {
        Some("create.terrainzapper.leftClickToSet")
    } else {
        None
    };
    if let Some(key) = key {
        action_bar(host, player, key);
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let (hit, face) = aimed(&lv, player)?;
    if lv.block_state(hit).is_none_or(|s| is_air(&*lv.host, s)) {
        return None;
    }
    let paint = match &block {
        Some(b) => lv.host.block_state_id(b)?,
        None => lv.host.block_state_id("minecraft:air")?,
    };
    // `BlockHelper.setZeroAge`.
    let paint = {
        let mut props = props_of(&*lv.host, paint);
        ws::zero_age(&mut props);
        with_props(&*lv.host, paint, &props)
    };
    let mut seed = (lv.game_time() as u64) ^ 0x2545_f491_4f6c_dd1d;
    let positions = ws::positions(&settings, hit, face, || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed & 1 == 1
    });
    let air = lv.host.block_state_id("minecraft:air");
    let writes = ws::run(settings.tool, &positions, |p| {
        let here = lv.block_state(p);
        ws::Seen {
            replaceable: here
                .and_then(|s| lv.host.block_states().replaceable(s))
                .unwrap_or(false),
            painted: here == Some(paint),
        }
    });
    for (p, w) in writes {
        let to = match w {
            ws::Write::Paint => paint,
            ws::Write::Air => match air {
                Some(a) => a,
                None => continue,
            },
        };
        lv.set_state(p, to, update_all());
    }
    Some(())
}

pub(super) fn register_commands(commands: &mut Vec<mod_api::Command>) {
    // The worldshaper's screen and its left click: brush, size, tool, placement, pattern and
    // block.
    commands.push(mod_api::Command::op(
        "create_debug worldshaper <args:text>",
        2,
    ));
}

/// `create_debug worldshaper <slot> <cuboid|sphere|cylinder> <size>... <tool> [placement]
/// [pattern] [block=<state>]`: the screen's settings (`configureSettings`) and the block its
/// left click picks.
pub(super) fn command(host: &mut dyn ServerHost, event: &RunModCommandEvent<'_>) -> String {
    const USAGE: &str = "usage: create_debug worldshaper <hand|offhand|slot> <cuboid|sphere|cylinder> \
                         <size>... <fill|place|replace|clear|overlay> [merged|attached|inserted] \
                         [solid|checkered|inverse_checkered|chance25|chance50|chance75] [block=<state>]";
    let CommandSource::Player { player, .. } = event.source else {
        return "worldshaper: only a player has a worldshaper".into();
    };
    let player = *player;
    let rest = match event.command.argument("args") {
        Some(CommandArgumentValue::String(s)) => s.clone(),
        _ => return USAGE.into(),
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (Some(slot), Some(brush)) = (
        words.first(),
        words.get(1).and_then(|b| ws::Brush::parse(b)),
    ) else {
        return USAGE.into();
    };
    if !brush.ported() {
        return format!("worldshaper: the {} brush is not ported", brush.name());
    }
    let n = brush.params();
    let mut params = [0i32; 3];
    for (i, p) in params.iter_mut().enumerate().take(n) {
        let Some(v) = words.get(2 + i).and_then(|v| v.parse::<i32>().ok()) else {
            return USAGE.into();
        };
        let (lo, hi) = brush.bounds(i);
        *p = v.clamp(lo, hi);
    }
    let mut s = ws::Settings {
        brush,
        params: Some(params),
        ..ws::Settings::default()
    };
    let mut block = None;
    for w in &words[(2 + n).min(words.len())..] {
        if let Some(t) = ws::Tool::parse(w) {
            s.tool = t;
        } else if let Some(p) = ws::Placement::parse(w) {
            s.placement = p;
        } else if let Some(p) = ws::Pattern::parse(w) {
            s.pattern = p;
        } else if let Some(b) = w.strip_prefix("block=") {
            block = Some(b.to_owned());
        } else {
            return USAGE.into();
        }
    }
    if s.tool == ws::Tool::Flatten {
        return "worldshaper: the flatten tool is not ported".into();
    }
    // The left click's pick (`trySelect`): a state that can be picked, its age set to 0 when
    // used.
    let state = match &block {
        Some(b) => {
            let Some(st) = host.block_state_id(b) else {
                return format!("worldshaper: no block state {b}");
            };
            let Some(p) = picked(&*host, st) else {
                return format!("worldshaper: {b} can't be picked");
            };
            host.block_state_string(p)
        }
        None => None,
    };
    let Some((slot_at, stack)) = slot_stack(&*host, player, slot) else {
        return USAGE.into();
    };
    let Some(mut stack) = stack else {
        return format!("worldshaper: nothing in {slot}");
    };
    if !is(Some(&stack), Item::Worldshaper) {
        return format!(
            "worldshaper: {slot} holds {}, not a worldshaper",
            stack.item().id(&*host)
        );
    }
    set_shaper(&mut stack, &s, state.as_deref());
    set_slot(host, player, slot_at, stack);
    format!(
        "worldshaper: {} {:?} {} {} {}{}",
        s.brush.name(),
        &params[..n],
        s.tool.name(),
        s.placement.name(),
        s.pattern.name(),
        state.map(|b| format!(" with {b}")).unwrap_or_default()
    )
}
