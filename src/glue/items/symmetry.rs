//! The wand of symmetry: its click moves the mirror (`SymmetryWandItem.useOn`), its use in the
//! air turns it off (`use`), and every enabled wand in the hotbar copies a player's placements
//! and breaks to their mirror images (`SymmetryHandler`). Its screen is stood in for by
//! `/create_debug symmetry`, writing the held wand's mirror. The mirror is kept in the stack's
//! custom data under Create's component names (`SYMMETRY_WAND`, `SYMMETRY_WAND_ENABLE`).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::{
    Item, creative, custom, held, is, set_custom, set_held, set_slot, slot_stack, sneaking,
};
use create_engine::symmetry::{self, ENABLE_KEY, Kind, MIRROR_KEY, Wand};
use mod_api::{
    BlockState, BreakBlockEvent, CommandSource, Compound, EntityPlaceEvent, Hand, Host,
    HostBlockStates, HostSide, InteractionResult, ItemPart, ItemStack, PlayerRef,
    RunModCommandEvent, ServerHost, Tag, UseItemEvent, UseOnEvent,
};
use std::collections::BTreeMap;

pub(super) struct WandPart;

#[mod_api::horizon_part]
impl ItemPart for WandPart {
    /// `SymmetryWandItem.useOn` (not sneaking, main hand): the mirror moves before the clicked
    /// face. Sneaking opens its screen on the client (the command stands in for it).
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        if let HostSide::Server(host) = host {
            clicked(host, event);
        }
        Some(InteractionResult::Success)
    }

    /// `SymmetryWandItem.use` not sneaking: the mirror off.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        if let HostSide::Server(host) = host
            && let Some(player) = host.player(event.player)
            && !sneaking(&*host, player)
        {
            let mut stack = event.stack.clone();
            let mut w = wand(&stack);
            w.used();
            set_wand(&mut stack, &w);
            set_held(host, player, event.hand, Some(stack));
        }
        Some(InteractionResult::Success)
    }
}

fn clicked(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<()> {
    let player = host.player(event.player)?;
    if sneaking(&*host, player) || event.hand == Hand::Off {
        return None;
    }
    let entity = player.entity();
    let yaw = entity
        .head_yaw(&*host)
        .or_else(|| entity.rotation(&*host).map(|r| r[0]))?;
    let (p, f) = (event.pos, event.face);
    let at = [p[0] + f[0], p[1] + f[1], p[2] + f[2]];
    let mut stack = event.stack.clone();
    let mut w = wand(&stack);
    w.clicked(at, symmetry::faces_z(yaw), yaw);
    set_wand(&mut stack, &w);
    set_held(host, player, event.hand, Some(stack));
    Some(())
}

fn flag(t: Option<&Tag>) -> bool {
    matches!(t, Some(Tag::Byte(b)) if *b != 0)
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The wand's mirror and its `SYMMETRY_WAND_ENABLE` (`checkComponents`: an empty mirror at the
/// origin, disabled, when it has none).
fn wand(stack: &ItemStack) -> Wand {
    let c = custom(stack);
    let Some(Tag::Compound(m)) = c.get(&MIRROR_KEY.into()) else {
        return Wand::blank();
    };
    let position = match m.get(&"position".into()) {
        Some(Tag::List(v)) if v.len() == 3 => {
            let f = |t: &Tag| match t {
                Tag::Double(b) => f64::from_bits(*b),
                _ => 0.0,
            };
            [f(&v[0]), f(&v[1]), f(&v[2])]
        }
        _ => [0.0; 3],
    };
    let mut w = Wand {
        kind: Kind::parse(&text(m.get(&"type".into())).unwrap_or_default()),
        orientation: 0,
        position,
        mirror_enable: flag(m.get(&"enable".into())),
        enabled: flag(c.get(&ENABLE_KEY.into())),
    };
    let orientation = match m.get(&"orientation_index".into()) {
        Some(Tag::Int(i)) => (*i).max(0) as usize,
        _ => 0,
    };
    w.set_orientation(orientation);
    w
}

fn set_wand(stack: &mut ItemStack, w: &Wand) {
    let m = Compound::from([
        ("orientation_index".into(), Tag::Int(w.orientation as i32)),
        (
            "position".into(),
            Tag::List(
                w.position
                    .iter()
                    .map(|v| Tag::Double(v.to_bits()))
                    .collect(),
            ),
        ),
        ("type".into(), Tag::String(w.kind.name().into())),
        ("enable".into(), Tag::Byte(i8::from(w.mirror_enable))),
    ]);
    set_custom(
        stack,
        &[
            (MIRROR_KEY, Some(Tag::Compound(m))),
            (ENABLE_KEY, Some(Tag::Byte(i8::from(w.enabled)))),
        ],
    );
}

/// The wands in the hotbar (`SymmetryHandler`: every hotbar slot holding one).
fn hotbar_wands(host: &dyn Host, player: PlayerRef) -> Vec<Wand> {
    player
        .inventory_slots(host)
        .unwrap_or_default()
        .iter()
        .filter(|(slot, s)| *slot < 9 && is(Some(s), Item::WandOfSymmetry))
        .map(|(_, s)| wand(s))
        .collect()
}

/// `state` turned as a mirror turns it.
fn turned(host: &dyn Host, state: BlockState, turns: &[symmetry::Turn]) -> BlockState {
    let Some(text) = host.block_state_string(state) else {
        return state;
    };
    let (name, own) = states::split(&text);
    let before: BTreeMap<String, String> = own
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let mut props = before.clone();
    for t in turns {
        t.apply(name, &mut props);
    }
    let facts = host.block_states();
    let mut next = state;
    for (k, v) in &props {
        if before.get(k) != Some(v) {
            next = next.with(&facts, k, v).unwrap_or(next);
        }
    }
    next
}

/// `player.getInventory()` giving up one item of `item` (`ItemHelper.extract`'s first match).
fn take_one(host: &mut dyn ServerHost, player: PlayerRef, item: mod_api::ItemKey) -> bool {
    let slots = player.inventory_slots(&*host).unwrap_or_default();
    let Some((slot, stack)) = slots.into_iter().find(|(_, s)| s.item() == item) else {
        return false;
    };
    player
        .set_inventory_slot(host, slot, stack.shrink(1))
        .is_ok()
}

/// The player a placement or a break is by.
fn player_of(host: &mut dyn ServerHost, placer: Option<mod_api::EntityRef>) -> Option<PlayerRef> {
    let uuid = placer?.uuid(host)?;
    host.player(uuid)
}

/// `SymmetryHandler.onBlockPlaced` → `SymmetryWandItem.apply`: each enabled wand in the hotbar
/// copies the placed state, turned, to its mirror images (creative: over anything; survival:
/// only into replaceable, breakable blocks, paying one of the block's item each).
pub(super) fn placed(host: &mut dyn ServerHost, event: &EntityPlaceEvent) -> Option<()> {
    let player = player_of(host, event.placer)?;
    let wands = hotbar_wands(&*host, player);
    if wands.is_empty() {
        return None;
    }
    // `BlockItem.BY_BLOCK`: the block has an item of its own.
    let item = host.block_item(event.state.block())?;
    let dimension = player.dimension(&*host)?.to_string();
    let creative = creative(&*host, player);
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    for w in wands {
        for copy in w.copies(pos) {
            let to = turned(&*lv.host, event.state, &copy.turns);
            let p = copy.pos;
            if !creative {
                let Some(here) = lv.block_state(p) else {
                    continue;
                };
                let facts = lv.host.block_states();
                let replaceable = facts.replaceable(here) == Some(true);
                let unbreakable = facts.destroy_speed(here).is_some_and(|h| h < 0.0);
                if !replaceable || unbreakable || !take_one(lv.host, player, item) {
                    continue;
                }
            }
            lv.set_state(p, to, update_all());
        }
    }
    Some(())
}

/// Whether `state` is air (`BlockState.isAir`).
fn is_air(host: &dyn Host, state: BlockState) -> bool {
    matches!(
        state.block().id(host).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `SymmetryHandler.onBlockDestroyed` → `SymmetryWandItem.remove`: the mirror images of the
/// broken block break too (in survival only where the same block stands, its drops falling at
/// the broken block as Create drops them).
pub(super) fn broken(host: &mut dyn ServerHost, event: &BreakBlockEvent<'_>) -> Option<()> {
    let player = host.players().acting_player()?;
    let wands = hotbar_wands(&*host, player);
    if wands.is_empty() {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let creative = creative(&*host, player);
    let tool = held(&*host, player, Hand::Main);
    let lv = ServerLevel::new(host, &dimension)?;
    let og = event.state.block();
    let at = event.pos.map(|v| f64::from(v) + 0.5);
    for w in wands {
        for copy in w.copies(event.pos) {
            let p = copy.pos;
            let Some(here) = lv.block_state(p) else {
                continue;
            };
            if (!creative && here.block() != og) || is_air(&*lv.host, here) {
                continue;
            }
            let drops = lv
                .level
                .destroy_block(&mut *lv.host, p, !creative, tool.as_ref(), true)
                .ok()
                .flatten()
                .unwrap_or_default();
            for d in drops {
                let _ = lv.level.spawn_item_with_motion(
                    &mut *lv.host,
                    at[0],
                    at[1],
                    at[2],
                    d,
                    [0.0; 3],
                    10,
                );
            }
        }
    }
    Some(())
}

pub(super) fn register_commands(commands: &mut Vec<mod_api::Command>) {
    // The wand of symmetry's screen and click: its mirror's place, type and orientation.
    commands.push(mod_api::Command::op("create_debug symmetry <args:text>", 2));
}

/// `create_debug symmetry <slot> <x> <y> <z> <plane|cross_plane|triple_plane|empty>
/// [orientation]`: the screen's mirror and orientation, at a place the wand's click would put
/// it, enabled as that click enables it.
pub(super) fn command(host: &mut dyn ServerHost, event: &RunModCommandEvent<'_>) -> String {
    const USAGE: &str = "usage: create_debug symmetry <hand|offhand|slot> <x> <y> <z> <plane|cross_plane|triple_plane|empty> [orientation]";
    let CommandSource::Player { player, .. } = event.source else {
        return "symmetry: only a player has a wand".into();
    };
    let player = *player;
    let rest = match event.command.argument("args") {
        Some(mod_api::CommandArgumentValue::String(s)) => s.clone(),
        _ => return USAGE.into(),
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() < 5 {
        return USAGE.into();
    }
    let here = player.entity().position(&*host);
    let mut at = [0i32; 3];
    for i in 0..3 {
        let t = words[1 + i];
        at[i] = match t.strip_prefix('~') {
            Some(o) => here.map_or(0, |h| h[i].floor() as i32) + o.parse::<i32>().unwrap_or(0),
            None => match t.parse() {
                Ok(v) => v,
                Err(_) => return USAGE.into(),
            },
        };
    }
    let kind = Kind::parse(words[4]);
    let orientation = words
        .get(5)
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    let Some((slot, stack)) = slot_stack(&*host, player, words[0]) else {
        return USAGE.into();
    };
    let Some(mut stack) = stack else {
        return format!("symmetry: nothing in {}", words[0]);
    };
    if !is(Some(&stack), Item::WandOfSymmetry) {
        return format!(
            "symmetry: {} holds {}, not a wand",
            words[0],
            stack.item().id(&*host)
        );
    }
    let mut w = wand(&stack);
    w.kind = kind;
    w.position = at.map(f64::from);
    w.mirror_enable = true;
    w.set_orientation(orientation);
    w.enabled = kind != Kind::Empty;
    set_wand(&mut stack, &w);
    set_slot(host, player, slot, stack);
    format!(
        "symmetry: {} at {} {} {} orientation {}",
        kind.name(),
        at[0],
        at[1],
        at[2],
        w.orientation
    )
}
