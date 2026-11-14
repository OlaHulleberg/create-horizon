//! Analog levers, smart observers (content observers) and threshold switches (stockpile
//! switches): their signals, clicks, looks at their targets and output ticks, the levers'
//! handles to clients, and the debug commands standing in for the switch's screen.

use super::block_at;
use crate::glue::net;
use crate::glue::persist;
use crate::glue::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use crate::glue::world::{try_with_world, with_world};
use create_engine::Block;
use create_engine::filter::Filter;
use create_engine::kinetics::Dir;
use create_engine::observe::{self, CENTRE_REACH, Seen, Slot, Watch};
use create_engine::parts::StateInfo;
use mod_api::{
    BlockClick, Command, CommandArgumentValue, Flow, ItemStack, PlayerRef, RegisterCommandsEvent,
    RunModCommandEvent, ServerHost, Text,
};

/// `getSignal` / `getDirectSignal` of an analog lever (its block entity's state), a smart
/// observer (15 while `powered`) or a threshold switch (15 after its delay), none into an
/// observer's or switch's target. `side` is vanilla's direction. Read without borrowing a world
/// a write of the glue holds.
pub(super) fn detector_signal(
    dim: usize,
    pos: [i32; 3],
    block: Block,
    info: &StateInfo,
    side: Dir,
    direct: bool,
) -> i32 {
    use create_engine::lever;
    let facing = info.facing.unwrap_or(Dir::North);
    let read = |f: &dyn Fn(&create_engine::world::Dimension) -> i32| {
        try_with_world(|w| f(&w.dims[dim])).unwrap_or(0)
    };
    let target = || {
        observe::target_direction(
            info.target.unwrap_or(create_engine::steam::Face::Wall),
            facing,
        )
    };
    match block {
        Block::AnalogLever => {
            let face = info.face.unwrap_or(create_engine::steam::Face::Floor);
            let connected = lever::connected(face, facing);
            read(&|d| lever::signal(d.levers.state(pos), connected, side, direct))
        }
        _ if direct => 0,
        Block::ContentObserver => observe::observer_signal(info.powered, target(), side),
        _ => {
            let target = target();
            read(&|d| {
                d.detectors
                    .switches
                    .get(&pos)
                    .map_or(0, |s| s.signal(target, side))
            })
        }
    }
}

/// `AnalogLeverBlock.useWithoutItem` (any item but the wrench, `ItemUseOverrides`): one up, or
/// down while sneaking; the click's sound at the new state's pitch. True when it was taken.
pub(super) fn use_lever(lv: &mut ServerLevel<'_>, pos: [i32; 3], back: bool, wrench: bool) -> bool {
    if wrench {
        return false;
    }
    let dim = lv.dim;
    let now = lv.game_time();
    let Some(state) = with_world(|w| w.dims[dim].levers.click(pos, back, now)).flatten() else {
        return true;
    };
    persist::mark_pos_unsaved(dim, pos);
    lv.play_block_sound(
        "minecraft:block.lever.click",
        pos,
        0.2,
        create_engine::lever::click_pitch(state),
    );
    true
}

/// A click on a smart observer or threshold switch: on its filter slot
/// (`FilteredDetectorFilterSlot`, `FilteringBehaviour.onShortInteract`) the held item becomes
/// its filter, an empty hand clears it; elsewhere a switch opens its screen (none here: the
/// thresholds are set with `/create_debug threshold`), which takes the click all the same.
/// True when it was taken.
pub(super) fn use_detector(
    lv: &mut ServerLevel<'_>,
    click: &BlockClick<'_>,
    switch: bool,
    info: &StateInfo,
    held: Option<&str>,
    back: bool,
    may_build: bool,
) -> bool {
    if held == Some("create:wrench") {
        return false;
    }
    let pos = click.pos;
    let side = Dir::from_offset(click.hit.face);
    // `ValueSettingsInputHandler.canInteract`: a builder, not sneaking, no clipboard in hand.
    let may_use_value_box = may_build && !back && held != Some("create:clipboard");
    let on_slot = may_use_value_box
        && held != Some("create:mechanical_arm")
        && match (side, info.target, info.facing) {
            (Some(side), Some(t), Some(f)) => {
                let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
                observe::filter_side(t, f, side, switch)
                    && create_engine::board::hits_box(observe::filter_slot(side), local)
            }
            _ => false,
        };
    if on_slot {
        // The held item is the filter; a configured filter item is the items port's reading.
        let filter = held.map(Filter::item);
        let dim = lv.dim;
        let set = with_world(|w| w.dims[dim].detectors.set_filter(pos, filter));
        if matches!(set, Some(Ok(_))) {
            persist::mark_pos_unsaved(dim, pos);
        }
        lv.play_block_sound("minecraft:entity.item_frame.add_item", pos, 0.25, 0.1);
        return true;
    }
    // ThresholdSwitchBlock.useItemOn: SUCCESS (the screen, on the client).
    switch
}

/// Whether a smart observer (`observer`) or threshold switch placed beside `at` would look at it
/// (`getStateForPlacement`): an item or fluid handler, and for the observer a belt, depot,
/// ejector, fluid pipe or funnel too.
pub(super) fn detects(lv: &mut ServerLevel<'_>, at: [i32; 3], observer: bool) -> bool {
    let block = block_at(lv, at).map(|(b, _)| b);
    if matches!(block, Some(Block::FluidTank | Block::ItemVault)) {
        return true;
    }
    if observer
        && matches!(
            block,
            Some(
                Block::Belt
                    | Block::Depot
                    | Block::WeightedEjector
                    | Block::FluidPipe
                    | Block::AndesiteFunnel
                    | Block::BrassFunnel
                    | Block::AndesiteBeltFunnel
                    | Block::BrassBeltFunnel
            )
        )
    {
        return true;
    }
    lv.level
        .capability(&mut *lv.host, &mod_api::ITEMS, at, None)
        .is_some()
}

/// The stack in slot `slot` of the block entity at `pos`.
pub(crate) fn slot_stack(lv: &ServerLevel<'_>, pos: [i32; 3], slot: u32) -> Option<ItemStack> {
    let entity = lv.host.block_entity(lv.level, pos)?;
    entity.item_slot(&*lv.host, slot).ok().flatten()
}

/// The item id of `stack`.
pub(crate) fn item_id(lv: &ServerLevel<'_>, stack: &ItemStack) -> String {
    stack.item().id(&*lv.host)
}

/// `getMaxStackSize` of `stack`'s item.
pub(crate) fn max_stack(lv: &ServerLevel<'_>, stack: &ItemStack) -> u32 {
    lv.host.max_stack_size(stack.item()).unwrap_or(64)
}

/// Every slot of the item handler at `pos`, whatever its sides (`bypassSidedness`): each one's
/// item, count and largest stack there. None when there is none.
fn container_slots(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<Vec<Slot>> {
    let handler = lv
        .level
        .capability(&mut *lv.host, &mod_api::ITEMS, pos, None)?;
    let size = handler.size(&mut *lv.host);
    let mut slots = Vec::with_capacity(size);
    for i in 0..size {
        let amount = handler.amount(&mut *lv.host, i);
        let slot = match handler.resource(&mut *lv.host, i) {
            Some(r) if amount > 0 => Slot {
                item: Some(r.item().id(&*lv.host)),
                count: amount,
                max_stack: lv
                    .host
                    .max_stack_size(r.item())
                    .unwrap_or(64)
                    .min(handler.capacity(&mut *lv.host, i, &r).max(1)),
            },
            _ => Slot {
                item: None,
                count: 0,
                max_stack: 64,
            },
        };
        slots.push(slot);
    }
    Some(slots)
}

/// What the detector at `pos` finds at `target`, and how it is watched from now on. The
/// target's own item id is read only for an observer with a filter.
fn seen_at(lv: &mut ServerLevel<'_>, target: [i32; 3], block_item: bool) -> (Seen, Watch) {
    let dim = lv.dim;
    let mut seen = Seen {
        block_item: if block_item {
            lv.block_state(target)
                .and_then(|s| lv.state_name(s))
                .filter(|n| n != "minecraft:air")
        } else {
            None
        },
        ..Seen::default()
    };
    let block = block_at(lv, target).map(|(b, _)| b);
    match block {
        // `TransportedItemStackHandlerBehaviour.handleCenteredProcessingOnAllItems(.45)`: the
        // items within .45 of the segment's centre.
        Some(Block::Belt) => {
            let items = with_world(|w| {
                let d = &w.dims[dim];
                let c = d.belt_of.get(&target)?;
                let belt = d.belts.get(c)?;
                let index = belt.chain.iter().position(|q| *q == target)? as f32;
                Some(
                    belt.items
                        .items
                        .iter()
                        .filter(|i| (index + 0.5 - i.position).abs() < CENTRE_REACH)
                        .map(|i| i.item.clone())
                        .collect::<Vec<String>>(),
                )
            })
            .flatten()
            .unwrap_or_default();
            seen.transported = Some(items);
            (seen, Watch::Belt)
        }
        // `DepotBehaviour.applyToAllItems`: the held stack once within .45 of the centre (a
        // stack still sliding in is looked at again each tick).
        Some(Block::Depot | Block::WeightedEjector) => {
            let held = with_world(|w| w.dims[dim].depots.get(&target).copied().flatten()).flatten();
            let item = slot_stack(lv, target, 0).map(|s| item_id(lv, &s));
            let near = held.is_none_or(|h| 0.5 - h.position <= CENTRE_REACH);
            seen.transported = Some(item.filter(|_| near).into_iter().collect());
            (seen, if near { Watch::Notified } else { Watch::Belt })
        }
        // An item vault's whole inventory (`getSlotLimit` 64).
        Some(Block::ItemVault) => {
            if let Some(parts) = with_world(|w| w.dims[dim].vaults.inventory(target)).flatten() {
                let mut slots = Vec::with_capacity(parts.len() * create_engine::vault::SLOTS);
                for p in parts {
                    for i in 0..create_engine::vault::SLOTS as u32 {
                        slots.push(match slot_stack(lv, p, i) {
                            Some(s) => Slot {
                                max_stack: max_stack(lv, &s).clamp(1, 64),
                                item: Some(item_id(lv, &s)),
                                count: s.count(),
                            },
                            None => Slot {
                                item: None,
                                count: 0,
                                max_stack: 64,
                            },
                        });
                    }
                }
                seen.inventory = Some((slots, 64));
            }
            (seen, Watch::Notified)
        }
        // A fluid tank's fluid handler: the whole tank.
        Some(Block::FluidTank) => {
            seen.tank = with_world(|w| {
                let (_, m) = w.dims[dim].tanks.get(target)?;
                Some((m.tank.fluid, m.tank.amount, m.capacity()))
            })
            .flatten();
            (seen, Watch::Notified)
        }
        // Another Create block's slots tell of their changes; a vanilla container does not.
        other => {
            seen.inventory = container_slots(lv, target).map(|slots| (slots, 64));
            let watch = if seen.inventory.is_some() && other.is_none() {
                Watch::Lazy
            } else {
                Watch::Notified
            };
            (seen, watch)
        }
    }
}

/// A smart observer's or threshold switch's look at its target (`SmartObserverBlockEntity
/// .tick`, `ThresholdSwitchBlockEntity.updateCurrentLevel`).
fn look(lv: &mut ServerLevel<'_>, pos: [i32; 3], now: i64) {
    let dim = lv.dim;
    let Some((block, info)) = block_at(lv, pos) else {
        return;
    };
    let (Some(t), Some(f)) = (info.target, info.facing) else {
        return;
    };
    let target = observe::target_pos(pos, t, f);
    let filter = with_world(|w| w.dims[dim].detectors.filter(pos).cloned()).flatten();
    match block {
        Block::ContentObserver => {
            let (seen, watch) = seen_at(lv, target, filter.is_some());
            let found = observe::observe(filter.as_ref(), &seen);
            let effect = with_world(|w| {
                let d = &mut w.dims[dim].detectors;
                d.watch(pos, watch, now);
                d.observer_look(pos, found, now, info.powered)
            })
            .unwrap_or_default();
            if effect.power_on {
                // `activate`: `setBlockAndUpdate`, then `updateNeighborsAt`.
                lv.set_flags(pos, &[("powered", true)], update_all());
                lv.update_signal_neighbors(pos);
            }
        }
        Block::StockpileSwitch => {
            let (seen, watch) = seen_at(lv, target, false);
            let reading = observe::fill_level(filter.as_ref(), &seen);
            // A belt is no inventory: the switch looks at it as at any other block.
            let watch = if watch == Watch::Belt {
                Watch::Notified
            } else {
                watch
            };
            let Some((u, changed)) = with_world(|w| {
                let d = &mut w.dims[dim];
                d.detectors.watch(pos, watch, now);
                let s = d.detectors.switches.get_mut(&pos)?;
                let before = (s.level, s.state);
                let u = s.update_level(reading);
                let changed = (s.level, s.state) != before;
                if u.schedule {
                    d.detectors.schedule_switch(pos, now);
                }
                Some((u, changed))
            })
            .flatten() else {
                return;
            };
            if changed {
                persist::mark_pos_unsaved(dim, pos);
            }
            if let Some(level) = u.display
                && level != info.level
            {
                let flags = if u.update {
                    update_all()
                } else {
                    UPDATE_CLIENTS
                };
                const LEVELS: [&str; 6] = ["0", "1", "2", "3", "4", "5"];
                lv.set_property(pos, "level", LEVELS[usize::from(level.min(5))], flags);
            }
        }
        _ => {}
    }
}

/// Analog levers whose neighbours are due to hear of them, smart observers and threshold
/// switches whose targets changed (or whose lazy interval came round, or whose belt or depot
/// carries items), their turn-offs and output ticks. Nothing runs without any of these.
pub(super) fn tick_detectors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((due, removed, shown)) = with_world(|w| {
        w.detectors_tanks_changed(dim);
        let l = &mut w.dims[dim].levers;
        (
            l.take_due(now),
            std::mem::take(&mut l.removed),
            std::mem::take(&mut l.shown),
        )
    }) else {
        return;
    };
    // `AnalogLeverBlockEntity.tick` (`lastChange` ran out) and `onRemove`: `updateNeighbors`.
    for pos in due.into_iter().chain(removed) {
        lv.update_signal_neighbors(pos);
    }
    if !shown.is_empty() {
        let players = lv.players();
        send_levers(lv, &players, &shown);
    }
    let Some((checks, ticks)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut checks = d.detectors.take_checks(now);
        let ticks = d.detectors.take_switch_ticks(now);
        if !d.detectors.belt_watch.is_empty() {
            // Observers on a belt (or a depot a stack slides onto) that holds items look
            // each tick.
            for p in &d.detectors.belt_watch {
                let busy = d.detectors.target_of.get(p).is_some_and(|t| {
                    d.belt_of
                        .get(t)
                        .and_then(|c| d.belts.get(c))
                        .is_some_and(|b| !b.items.items.is_empty())
                        || d.depots.get(t).is_some_and(|h| h.is_some())
                });
                if busy {
                    checks.push(*p);
                }
            }
            checks.sort_unstable();
            checks.dedup();
        }
        (checks, ticks)
    }) else {
        return;
    };
    // `ThresholdSwitchBlock.tick` -> `updatePowerAfterDelay`: the output follows, then
    // `blockUpdated`.
    for pos in ticks {
        let follows = with_world(|w| {
            w.dims[dim]
                .detectors
                .switches
                .get_mut(&pos)
                .map(|s| s.power_after_delay())
        })
        .flatten();
        if follows == Some(true) {
            persist::mark_pos_unsaved(dim, pos);
            lv.update_signal_neighbors(pos);
        }
    }
    for pos in checks {
        look(lv, pos, now);
    }
    // `SmartObserverBlock.tick`: `powered` false (`setBlock(.., UPDATE_CLIENTS)`), then
    // `updateNeighborsAt`.
    for pos in with_world(|w| w.dims[dim].detectors.take_offs(now)).unwrap_or_default() {
        if lv.set_flags(pos, &[("powered", false)], UPDATE_CLIENTS) {
            lv.update_signal_neighbors(pos);
        }
    }
}

/// Levers' states to `players` (gone ones: their handles hidden).
pub(super) fn send_levers(lv: &mut ServerLevel<'_>, players: &[PlayerRef], levers: &[[i32; 3]]) {
    if players.is_empty() || levers.is_empty() {
        return;
    }
    let dim = lv.dim;
    let mut shown: Vec<create_engine::lever::Shown> = Vec::new();
    for &pos in levers {
        let at = block_at(lv, pos)
            .filter(|(b, _)| *b == Block::AnalogLever)
            .and_then(|(_, i)| Some((i.face?, i.facing?)));
        let lever = with_world(|w| w.dims[dim].levers.levers.get(&pos).map(|l| l.state)).flatten();
        match (at, lever) {
            (Some((face, facing)), Some(s)) => shown.push((pos, Some(s), face, facing)),
            _ => shown.push((pos, None, create_engine::steam::Face::Floor, Dir::North)),
        }
    }
    for chunk in shown.chunks(256) {
        let m = create_engine::lever::encode(chunk);
        net::send_to_players(&mut *lv.host, players, create_engine::lever::CHANNEL, &m);
    }
}

// ---------------------------------------------------------------------------------------------
// Debug commands

const THRESHOLD_USAGE: &str =
    "usage: create_debug threshold <x y z> <offBelow> <onAbove> [inverted] [stacks]";

/// `/create_debug threshold <pos> <offBelow> <onAbove> [inverted] [stacks]`, standing in for
/// the switch's screen (`ConfigureThresholdSwitchPacket.applySettings`), and
/// `/create_debug redstone_state <pos>`: what a lever, observer or switch holds. For tests.
pub(super) fn register_commands(event: &mut RegisterCommandsEvent<'_>) {
    event.commands.push(Command::op(
        "create_debug threshold <pos:block_pos> <values:text>",
        2,
    ));
    event.commands.push(Command::op(
        "create_debug redstone_state <pos:block_pos>",
        2,
    ));
}

pub(super) fn run_command(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let c = event.command;
    let literals: Vec<&str> = c.literals.iter().map(String::as_str).collect();
    let set = match literals.as_slice() {
        ["create_debug", "threshold"] => true,
        ["create_debug", "redstone_state"] => false,
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
    let Some(dimension) = c.dimension.id(&*host).map(|d| d.to_string()) else {
        return Flow::Cancel;
    };
    let line = match ServerLevel::new(host, &dimension) {
        None => "no Create world here".to_owned(),
        Some(mut lv) => {
            if set {
                let values = match arg("values") {
                    Some(CommandArgumentValue::String(s)) => s.clone(),
                    _ => String::new(),
                };
                set_threshold(&mut lv, pos, &values)
            } else {
                let dim = lv.dim;
                let now = lv.game_time();
                with_world(|w| redstone_state(&w.dims[dim], pos, now)).unwrap_or_default()
            }
        }
    };
    let _ = host.commands().feedback(Text::literal(line));
    Flow::Cancel
}

fn set_threshold(lv: &mut ServerLevel<'_>, pos: [i32; 3], values: &str) -> String {
    let tokens: Vec<&str> = values.split_whitespace().collect();
    let num = |n: usize| tokens.get(n).and_then(|t| t.parse::<i32>().ok());
    let flag = |n: usize| tokens.get(n).is_some_and(|t| *t == "true" || *t == "1");
    let (Some(off), Some(on)) = (num(0), num(1)) else {
        return THRESHOLD_USAGE.into();
    };
    let dim = lv.dim;
    let changed = with_world(|w| {
        let d = &mut w.dims[dim];
        let s = d.detectors.switches.get_mut(&pos)?;
        s.off_below = off;
        s.on_above = on;
        s.in_stacks = flag(3);
        let inverted = flag(2);
        // `setInverted`: the output follows at once.
        let follows = inverted != s.inverted && {
            s.inverted = inverted;
            s.power_after_delay()
        };
        d.detectors.checks.push(pos);
        Some(follows)
    })
    .flatten();
    match changed {
        Some(follows) => {
            persist::mark_pos_unsaved(dim, pos);
            if follows {
                lv.update_signal_neighbors(pos);
            }
            format!("threshold switch: off below {off}, on above {on}")
        }
        None => "no threshold switch there".into(),
    }
}

/// `/create_debug redstone_state`'s line.
fn redstone_state(d: &create_engine::world::Dimension, pos: [i32; 3], now: i64) -> String {
    if let Some(l) = d.levers.levers.get(&pos) {
        return format!(
            "analog lever: state {}, neighbours told {}",
            l.state,
            l.due
                .map_or("already".into(), |t| format!("in {} ticks", t - now))
        );
    }
    if let Some(o) = d.detectors.observers.get(&pos) {
        return format!(
            "smart observer: filter {}, target {:?}, sees {}, off {}",
            o.filter.as_ref().map_or("none".into(), Filter::describe),
            d.detectors.target_of.get(&pos),
            o.sustained.is_some(),
            o.off_at
                .map_or("not pending".into(), |t| format!("in {} ticks", t - now))
        );
    }
    if let Some(s) = d.detectors.switches.get(&pos) {
        return format!(
            "threshold switch: level {} of {}..{}, off below {}, on above {}, state {}, inverted \
             {}, powered {}, filter {}",
            s.level,
            s.min,
            s.max,
            s.off_below,
            s.on_above,
            s.state,
            s.inverted,
            s.powered,
            s.filter.as_ref().map_or("none".into(), Filter::describe)
        );
    }
    "no analog lever, smart observer or threshold switch there".into()
}
