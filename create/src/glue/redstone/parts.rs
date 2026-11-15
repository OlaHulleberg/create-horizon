//! The redstone blocks' own moments: the signal they give (`getSignal`, `getDirectSignal`,
//! `isSignalSource`, from their state alone), the repeater ticks they take from the block they
//! are built on, a click on them, a minecart over a controller rail, and how they are placed.

use super::detectors;
use super::ticks::{
    diode_of, lamp_of, latch_of, link_of, rail_of, set_latch, toggle_diode, toggle_link,
};
use super::{block_at, info, offset};
use crate::glue::server::{ServerLevel, update_all};
use crate::glue::states;
use crate::glue::world::{level_index, try_with_world, with_world};
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockClick, BlockPart, BlockSignalEvent, BlockState, ContentsChangedEvent, Flow,
    InteractionResult, IsSignalSourceEvent, LevelHost, LevelWriteHost, MinecartPassEvent,
    Registrar, ServerHost, StateForPlacementEvent, TickBlockEvent, UseItemOnEvent,
    UseWithoutItemEvent,
};
use std::cell::RefCell;

/// The blocks this part is on.
const BLOCKS: [Block; 11] = [
    Block::PoweredLatch,
    Block::PoweredToggleLatch,
    Block::PulseRepeater,
    Block::PulseExtender,
    Block::PulseTimer,
    Block::RoseQuartzLamp,
    Block::RedstoneLink,
    Block::AnalogLever,
    Block::ContentObserver,
    Block::StockpileSwitch,
    Block::ControllerRail,
];

pub(super) fn register(r: &mut dyn Registrar) {
    for block in BLOCKS {
        r.block_part(states::key(block), RedstoneBlock);
    }
    for name in VANILLA_CONTAINERS {
        let key = r.block_key(name);
        r.block_part(key, WatchedContainer);
    }
    for tag in ["minecraft:shulker_boxes", "minecraft:copper_chests"] {
        r.block_part_tag(tag, WatchedContainer);
    }
}

/// The engine's index of a level a hook names, without touching a world borrowed by a write
/// the glue started.
pub(crate) fn dim_of(dimension: &str) -> Option<usize> {
    try_with_world(|_| ())?;
    level_index(dimension)
}

/// Vanilla's containers (with the shulker boxes and copper chests of their tags): smart observers
/// and threshold switches looking at one look again as soon as its contents change, as Create's
/// do each tick (`VersionedInventoryTrackerBehaviour`), not only every lazy interval.
const VANILLA_CONTAINERS: [&str; 12] = [
    "minecraft:chest",
    "minecraft:trapped_chest",
    "minecraft:barrel",
    "minecraft:hopper",
    "minecraft:dropper",
    "minecraft:dispenser",
    "minecraft:furnace",
    "minecraft:blast_furnace",
    "minecraft:smoker",
    "minecraft:brewing_stand",
    "minecraft:crafter",
    "minecraft:chiseled_bookshelf",
];

/// The part on vanilla's containers: their changes reach the detectors watching them.
struct WatchedContainer;

#[mod_api::horizon_part]
impl BlockPart for WatchedContainer {
    fn contents_changed(&self, host: &mut dyn ServerHost, event: &mut ContentsChangedEvent) {
        // Most containers have no detector on them: nothing more than this look.
        let watched = try_with_world(|w| w.dims.iter().any(|d| !d.detectors.watching.is_empty()));
        if watched != Some(true) {
            return;
        }
        let Some(dim) = event.level.dimension().id(&*host).and_then(|d| dim_of(&d)) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        with_world(|w| w.detectors_contents_changed(dim, pos));
    }
}

struct RedstoneBlock;

#[mod_api::horizon_part]
impl BlockPart for RedstoneBlock {
    /// `BlockState.getSignal` / `getDirectSignal`. The hook names the face touching the reader;
    /// vanilla's direction points from the reader to the block.
    fn block_signal(&self, _level: &dyn LevelHost, event: &mut BlockSignalEvent) {
        let Some((block, info)) = states::id(event.state).and_then(info) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let face = Dir::ALL[event.face as usize];
        let side = face.opposite();
        let dim = || dim_of(event.dimension.key().id());
        let signal = if let Some((_, l)) = latch_of(block, &info) {
            create_engine::latch::signal(&l, side)
        } else if let Some((_, s)) = diode_of(block, &info) {
            create_engine::diode::signal(&s, side)
        } else if let Some(l) = lamp_of(block, &info) {
            // The reader, on the face the hook names: another lamp reads 0.
            let reader = offset(pos, face);
            let reader_is_lamp = dim().is_some_and(|dim| {
                try_with_world(|w| w.block_at(dim, reader) == Some(Block::RoseQuartzLamp))
                    == Some(true)
            });
            create_engine::lamp::signal(&l, reader_is_lamp, event.direct)
        } else if let Some((facing, receiver, _)) = link_of(block, &info) {
            let received = if receiver {
                dim()
                    .and_then(|dim| try_with_world(|w| w.dims[dim].links.received(pos)))
                    .unwrap_or(0)
            } else {
                0
            };
            create_engine::link::signal(receiver, facing, received, side, event.direct)
        } else if matches!(
            block,
            Block::AnalogLever | Block::ContentObserver | Block::StockpileSwitch
        ) {
            let Some(dim) = dim() else {
                return;
            };
            detectors::detector_signal(dim, pos, block, &info, side, event.direct)
        } else {
            return;
        };
        event.signal = signal;
    }

    /// `isSignalSource`: a latch, a brass diode, a rose quartz lamp, a lever and a switch give a
    /// signal, and a powered receiver and a powered observer.
    fn is_signal_source(&self, _level: &dyn LevelHost, event: &mut IsSignalSourceEvent) {
        let Some((block, info)) = states::id(event.state).and_then(info) else {
            return;
        };
        let source = latch_of(block, &info).is_some()
            || diode_of(block, &info).is_some()
            || lamp_of(block, &info).is_some()
            || link_of(block, &info).is_some_and(|(_, receiver, powered)| receiver && powered)
            || matches!(block, Block::AnalogLever | Block::StockpileSwitch)
            || (block == Block::ContentObserver && info.powered);
        if source {
            event.source = true;
        }
    }

    /// A latch's or a brass diode's scheduled tick is the glue's (`ticks`): the repeater it is
    /// built on doesn't run its own.
    fn tick_block(&self, _level: &mut dyn LevelWriteHost, event: &mut TickBlockEvent) -> Flow {
        let Some((block, _)) = states::id(event.state).and_then(info) else {
            return Flow::Continue;
        };
        if create_engine::world::latch_kind(block).is_some()
            || create_engine::world::diode_kind(block).is_some()
        {
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let held = event.click.stack.map(|s| s.item().id(&*host));
        if held.is_none() {
            // `useWithoutItem` takes an empty hand.
            return Some(InteractionResult::TryWithEmptyHand);
        }
        use_block(host, event.state, event.click, held.as_deref())
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        use_block(host, event.state, event.click, None)
    }

    /// `ControllerRailBlock.onMinecartPass`: a cart on a controller rail going its way (or still)
    /// is set to `power / 15` of the top speed with the rail along it; one going against it, or
    /// on an unpowered rail, is pulled to the rail's centre (`decelerateCart`). The top speed is
    /// the rail's default (`getRailMaxSpeed`, 0.4); a furnace cart is pushed along the rail.
    fn minecart_pass(&self, host: &mut dyn ServerHost, event: &mut MinecartPassEvent) {
        let Some(rail) = states::id(event.state)
            .and_then(info)
            .and_then(|(b, i)| rail_of(b, &i))
        else {
            return;
        };
        let cart = event.entity;
        let (Some(at), Some(velocity)) = (cart.position(&*host), cart.velocity(&*host)) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let (motion, push) = create_engine::rail::cart_motion(
            rail,
            pos,
            at,
            velocity,
            create_engine::cart::RAIL_MAX_SPEED,
        );
        cart.set_velocity(host, motion);
        if let Some(m) = host.minecart(cart)
            && m.kind(&*host) == Some(mod_api::MinecartKind::Furnace)
        {
            m.set_push(host, [push[0], 0.0, push[1]]);
        }
    }

    /// `ControllerRailBlock.getStateForPlacement`: straight across the player's facing,
    /// backwards when facing a positive way. A smart observer or threshold switch looks for
    /// what it will watch once placed (`place_detectors`).
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let (block, _) = states::id(event.state).and_then(info)?;
        let c = event.context;
        match block {
            Block::ControllerRail => {
                let (shape, backwards) = create_engine::rail::placement(
                    create_engine::place::horizontal_direction(c.yaw),
                );
                let state = with_properties(
                    level,
                    event.state,
                    &[
                        ("shape", shape.name()),
                        ("backwards", if backwards { "true" } else { "false" }),
                        // `BaseRailBlock.getStateForPlacement`'s water.
                        (
                            "waterlogged",
                            if crate::glue::logistics::placement::in_water(level, event) {
                                "true"
                            } else {
                                "false"
                            },
                        ),
                    ],
                )?;
                Some(Some(state))
            }
            Block::ContentObserver | Block::StockpileSwitch => {
                let pos = [c.pos.x, c.pos.y, c.pos.z];
                PLACING.with_borrow_mut(|p| {
                    p.push(Placing {
                        dimension: event.dimension.key().id().to_owned(),
                        pos,
                        nearest: create_engine::place::ordered_by_nearest(c.yaw, c.pitch),
                        horizontal: create_engine::place::horizontal_direction(c.yaw),
                        sneaking: c.sneaking,
                        switch: block == Block::StockpileSwitch,
                    })
                });
                None
            }
            _ => None,
        }
    }
}

/// `state` with `props` set, through the level's own state parser.
pub(crate) fn with_properties(
    level: &dyn LevelHost,
    state: BlockState,
    props: &[(&str, &str)],
) -> Option<BlockState> {
    let text = level.state_string(state)?;
    let (name, mut own) = states::split(&text);
    for (k, v) in props {
        own.iter_mut().find(|(key, _)| key == k)?.1 = v;
    }
    let body: Vec<String> = own.iter().map(|(k, v)| format!("{k}={v}")).collect();
    level.block_state_id(&format!("{name}[{}]", body.join(",")))
}

/// A smart observer or threshold switch being placed, as its placer stood.
struct Placing {
    dimension: String,
    pos: [i32; 3],
    nearest: [Dir; 6],
    horizontal: Dir,
    sneaking: bool,
    switch: bool,
}

thread_local! {
    static PLACING: RefCell<Vec<Placing>> = const { RefCell::new(Vec::new()) };
}

/// Forgets the placements waiting (the world closed).
pub(super) fn forget() {
    PLACING.with_borrow_mut(Vec::clear);
}

/// `SmartObserverBlock.getStateForPlacement` / `ThresholdSwitchBlock`'s for the detectors placed
/// in `lv` since its last tick: the first side, nearest the placer's look first, with something
/// to watch, and the facing (`target`, `facing`) set at once. The game's placement reads no
/// item handlers, so it is set here, after placing.
pub(super) fn place_detectors(lv: &mut ServerLevel<'_>) {
    if PLACING.with_borrow(Vec::is_empty) {
        return;
    }
    let mine: Vec<Placing> = PLACING.with_borrow_mut(|p| {
        let (mine, rest) = std::mem::take(p)
            .into_iter()
            .partition(|x| x.dimension == lv.dimension);
        *p = rest;
        mine
    });
    for p in mine {
        let Some((block, _)) = block_at(lv, p.pos) else {
            continue;
        };
        let want = if p.switch {
            Block::StockpileSwitch
        } else {
            Block::ContentObserver
        };
        if block != want {
            continue;
        }
        let found: Vec<Dir> = Dir::ALL
            .into_iter()
            .filter(|&d| detectors::detects(lv, offset(p.pos, d), !p.switch))
            .collect();
        let (target, facing) =
            create_engine::observe::placement(p.nearest, p.horizontal, p.sneaking, |d| {
                found.contains(&d)
            });
        let Some(state) = lv.block_state(p.pos) else {
            continue;
        };
        let next = {
            use mod_api::HostBlockStates;
            let facts = lv.host.block_states();
            state
                .with(&facts, "target", target.name())
                .and_then(|s| s.with(&facts, "facing", crate::glue::kinetics::dir_name(facing)))
        };
        if let Ok(next) = next
            && next != state
        {
            lv.set_state(p.pos, next, update_all());
        }
    }
}

/// A click on one of the redstone blocks, with `held` the item id in the clicking hand.
fn use_block(
    host: &mut dyn ServerHost,
    state: BlockState,
    click: &BlockClick<'_>,
    held: Option<&str>,
) -> Option<InteractionResult> {
    let (block, info) = states::id(state).and_then(info)?;
    let player = host.player(click.player)?;
    let back = player.is_secondary_use_active(&*host).unwrap_or(false);
    let may_build = player.may_build(&*host).unwrap_or(false);
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = click.pos;
    let wrench = held == Some("create:wrench");
    match block {
        Block::AnalogLever => detectors::use_lever(&mut lv, pos, back, wrench)
            .then_some(InteractionResult::SuccessServer),
        Block::ContentObserver | Block::StockpileSwitch => detectors::use_detector(
            &mut lv,
            click,
            block == Block::StockpileSwitch,
            &info,
            held,
            back,
            may_build,
        )
        .then_some(InteractionResult::SuccessServer),
        Block::RedstoneLink => {
            let (facing, receiver, _) = link_of(block, &info)?;
            use_link(&mut lv, pos, click, facing, receiver, back, held)
                .then_some(InteractionResult::SuccessServer)
        }
        b if create_engine::world::diode_kind(b).is_some() => {
            // BrassDiodeBlock.useItemOn -> `toggle`: not while sneaking, nor with a wrench, nor
            // for a player who may not build. A click on its value box is the box's.
            if back || !may_build || wrench {
                return None;
            }
            let (_, s) = diode_of(block, &info)?;
            toggle_diode(&mut lv, pos, s.inverted);
            Some(InteractionResult::SuccessServer)
        }
        b if create_engine::world::latch_kind(b).is_some() => {
            // ToggleLatchBlock.useItemOn: not for a player who may not build, nor while
            // sneaking, nor with a wrench. A powered latch held by its inputs passes the click
            // on (`PoweredLatchBlock.activated`), so the held item is used.
            if !may_build || back || wrench {
                return None;
            }
            let (kind, l) = latch_of(block, &info)?;
            let n = create_engine::latch::activated(kind, &l)?;
            // `activated`: a lever click, 0.6 turning on, 0.5 turning off.
            let pitch = if l.powering { 0.5 } else { 0.6 };
            lv.play_block_sound("minecraft:block.lever.click", pos, 0.3, pitch);
            set_latch(&mut lv, pos, n);
            Some(InteractionResult::SuccessServer)
        }
        _ => None,
    }
}

/// `LinkHandler.onBlockActivated`: the item in hand (none: empty) tunes the slot under the
/// crosshair. Sneaking with an empty hand, or a wrench, toggles the link (`onWrenched`). True
/// when the click was taken.
fn use_link(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    click: &BlockClick<'_>,
    facing: Dir,
    receiver: bool,
    back: bool,
    held: Option<&str>,
) -> bool {
    if held == Some("create:wrench") {
        if back {
            return false;
        }
        toggle_link(lv, pos, receiver);
        return true;
    }
    if back {
        // `useWithoutItem` while sneaking: only with both hands empty does the block see it.
        if held.is_none() {
            toggle_link(lv, pos, receiver);
            return true;
        }
        return false;
    }
    if held == Some("create:linked_controller") {
        return false;
    }
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    let Some(first) = create_engine::link::hit_slot(facing, local) else {
        return false;
    };
    let dim = lv.dim;
    // `Frequency.of`: the item, and its colour when dyed.
    let key = super::frequency_key(&*lv.host, click.stack.filter(|_| held.is_some()));
    with_world(|w| w.link_frequency(dim, pos, first, &key));
    // `LinkHandler.onBlockActivated`: an item frame's add sound.
    lv.play_block_sound("minecraft:entity.item_frame.add_item", pos, 0.25, 0.1);
    true
}
