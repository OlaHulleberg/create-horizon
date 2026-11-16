//! The factory gauge (`FactoryPanelBlock`, `FactoryPanelBlockEntity`, `FactoryPanelBehaviour`):
//! up to four panels on one face, each watching an item in its network. A tuned gauge is placed
//! on the panel the click is nearest (`getTargetedSlot`), another one clicked into a free panel
//! of a gauge; an untuned one is refused (`tune_before_placing`). A panel's value box takes the
//! use press: with no item set, its item (the held stack, or the set-item screen with an empty
//! hand); with one, a short click opens its screen (`FactoryPanelScreen`, `gauge_screen`) and a
//! held press the target amount's board. Panels are joined from the screen's `+` (the next
//! panel clicked feeds this one, `FactoryPanelConnectionHandler`), and a wrench bends their
//! arrows. Sneaking with a wrench takes one panel off; breaking a gauge with more than one
//! takes the one looked at.
//!
//! What clients draw (the panels, bulbs, arrows and items: `gauge_draw`) and what the screen
//! reads are each gauge's panels in its block entity's data (`FactoryPanels`), written when they
//! change.
//!
//! Display links join a panel from its screen's `+` too (the link clicked next,
//! `FactoryPanelConnectionHandler.onRightClick`): the panel's `targetedByLinks` here, the link's
//! `LinkedGauges` in the display port, which reads them (`FactoryGaugeDisplaySource`); a panel
//! whose satisfaction or amount changes tells its links (`notifyRedstoneOutputs`).
//!
//! Redstone links join the same way (`RedstoneLinkBlockEntity`'s `FactoryPanelSupportBehaviour`):
//! a receiving link that receives a signal pauses the panels it is joined to
//! (`checkForRedstoneInput`, `redstonePowered`), a transmitting one sends 15 while one of its
//! panels is satisfied (`shouldBePoweredTristate`). Which panels a link feeds is read from the
//! panels' `targetedByLinks` (each link's `LinkedGauges` in Create).
//!
//! A panel is relocated from its screen's move button (`relocate`, `moveTo`). Left out: the
//! block's per-panel shape (the declared shape is Create's `FACTORY_PANEL_FALLBACK`, the whole
//! face; API_GAPS.md).

use super::super::items;
use super::super::server::ServerLevel;
use super::super::world::{level_index, with_world};
use super::{
    Key, Net, body, broadcast, item_of, mark_dirty, network_stock, next_int, packager_stock,
    perform, restocked, sources, with,
};
use crate::glue::interaction::values::may_use_value_box;
use create_engine::Block;
use create_engine::factory::{self as fx, SLOTS};
use create_engine::kinetics::Dir;
use create_engine::steam::Face;
use create_engine::stock::{
    self as st, GaugeStep, Input, Network, Order, Panel, PanelPos, RequestType,
};
use mod_api::{
    Compound, Hand, InteractionResult, ItemStack, PlayerRef, ServerHost, SoundSource, Tag, Text,
};

/// The gauge's panels in its block entity's data, for its clients.
pub const PANELS: &str = "FactoryPanels";

/// The face and facing of the gauge at `pos`.
pub(super) fn orientation(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Face, Dir)> {
    let state = lv.block(pos);
    with_world(|w| {
        let (b, i) = w.table.get(state)?;
        (b == Block::FactoryGauge).then_some(())?;
        Some((i.face?, i.facing?))
    })
    .flatten()
}

fn chunk_of(p: [i32; 3]) -> (i32, i32) {
    (p[0] >> 4, p[2] >> 4)
}

/// `FactoryPanelBlockEntity.addPanel`: a panel there, on `network`.
pub(super) fn add_panel(n: &mut Net, pp: PanelPos, network: Network) -> bool {
    if n.gauges.contains_key(&pp) {
        return false;
    }
    n.gauges.insert(pp, Panel::new(network));
    n.redraw.push(pp.0);
    n.unsaved.push(chunk_of(pp.0));
    true
}

/// `FactoryPanelBehaviour.disable` (`disconnectAll`): the panel gone, the panels it fed and
/// those feeding it told.
pub(super) fn remove_panel(n: &mut Net, pp: PanelPos) -> bool {
    let Some(p) = n.gauges.remove(&pp) else {
        return false;
    };
    n.due.remove(&pp);
    n.timers.remove(&pp);
    for (other, g) in n.gauges.iter_mut() {
        let before = (g.inputs.len(), g.outputs.len());
        g.inputs.retain(|i| i.from != pp);
        g.outputs.retain(|o| *o != pp);
        if before != (g.inputs.len(), g.outputs.len()) {
            n.redraw.push(other.0);
            n.unsaved.push(chunk_of(other.0));
        }
    }
    // `disconnectAllLinks`.
    for l in &p.links {
        crate::glue::display::panel_disconnected(n.dim, l.from.0, pp);
        crate::glue::redstone::notify_link(n.dim, l.from.0);
    }
    n.redraw.push(pp.0);
    n.unsaved.push(chunk_of(pp.0));
    true
}

/// `FactoryPanelSupportBehaviour.destroy`: the display or redstone link at `link` is gone from
/// the panels it was joined to.
pub(super) fn link_gone(n: &mut Net, link: [i32; 3]) {
    for (pp, g) in n.gauges.iter_mut() {
        let before = g.links.len();
        g.links.retain(|l| l.from.0 != link);
        if g.links.len() != before {
            n.redraw.push(pp.0);
            n.unsaved.push(chunk_of(pp.0));
            n.redstone_checks.push(*pp);
        }
    }
}

/// `notifyRedstoneOutputs`: the links joined to the panel that are no outputs refresh
/// (`notifyLink`): a display link gathers again, a transmitting redstone link sends again
/// (`updateTransmittedSignal`).
fn notify_links(dim: usize, links: &[Input]) {
    for l in links {
        crate::glue::display::notify_link(dim, l.from.0);
        crate::glue::redstone::notify_link(dim, l.from.0);
    }
}

/// `FactoryPanelBehaviour.checkForRedstoneInput` for the panel `pp`: powered (paused) while a
/// receiving redstone link joined to it receives a signal (`shouldPanelBePowered`); a link not
/// loaded leaves it as it is. Changed, the panel redraws and looks again at once (`timer = 1`).
pub(super) fn check_redstone_input(lv: &ServerLevel<'_>, pp: PanelPos) {
    let dim = lv.dim;
    let Some(links) = with(dim, |n| n.gauges.get(&pp).map(|g| g.links.clone())) else {
        return;
    };
    let mut power = false;
    for l in &links {
        let at = l.from.0;
        if !lv.is_loaded(at) {
            return;
        }
        power |= crate::glue::redstone::link_receiving(lv, at) == Some(true);
    }
    let now = super::now(lv);
    with(dim, |n| {
        let Some(g) = n.gauges.get_mut(&pp) else {
            return;
        };
        if g.redstone_powered == power {
            return;
        }
        g.redstone_powered = power;
        n.timers.insert(pp, now);
        schedule(n, pp, now);
        n.redraw.push(pp.0);
        n.unsaved.push(chunk_of(pp.0));
    });
}

/// `FactoryPanelSupportBehaviour.notifyPanels`: the panels the link at `link` is joined to
/// look at their redstone input again.
pub(super) fn link_panels_changed(lv: &ServerLevel<'_>, link: [i32; 3]) {
    let panels: Vec<PanelPos> = with(lv.dim, |n| {
        n.gauges
            .iter()
            .filter(|(_, g)| g.links.iter().any(|l| l.from.0 == link))
            .map(|(pp, _)| *pp)
            .collect()
    });
    for pp in panels {
        check_redstone_input(lv, pp);
    }
}

/// `FactoryPanelSupportBehaviour.shouldBePoweredTristate` for the link at `link`: whether a
/// panel joined to it is satisfied and asks for an amount (15 into what it sends); None while
/// one of them is not loaded.
pub(super) fn link_panels_power(lv: &ServerLevel<'_>, link: [i32; 3]) -> Option<bool> {
    let panels: Vec<(PanelPos, bool)> = with(lv.dim, |n| {
        n.gauges
            .iter()
            .filter(|(_, g)| g.links.iter().any(|l| l.from.0 == link))
            .map(|(pp, g)| (*pp, g.satisfied && g.count != 0))
            .collect()
    });
    let mut any = false;
    for (pp, on) in panels {
        if !lv.is_loaded(pp.0) {
            return None;
        }
        any |= on;
    }
    Some(any)
}

/// The panels of the gauge at `pos`.
pub(super) fn panels_at(n: &Net, pos: [i32; 3]) -> Vec<u8> {
    SLOTS
        .into_iter()
        .filter(|s| n.gauges.contains_key(&(pos, *s)))
        .collect()
}

/// A panel's item changed: looked at from the next tick, or never.
fn schedule(n: &mut Net, pp: PanelPos, now: u64) {
    if n.gauges.get(&pp).is_some_and(|g| g.filter.is_some()) {
        n.due.insert(pp, now);
        n.next = n.next.min(now);
    } else {
        n.due.remove(&pp);
    }
}

// ---------------------------------------------------------------------------------------------
// Looking at the network

/// A panel looked at (`tickStorageMonitor` at Create's summary age, `tickRequests` on its timer).
pub(super) fn look(lv: &mut ServerLevel<'_>, dim: usize, pp: PanelPos, now: u64) {
    let Some(mut panel) = with(dim, |n| n.gauges.get(&pp).cloned()) else {
        return;
    };
    let Some(item) = panel.filter.clone() else {
        return;
    };
    let pos = pp.0;
    let max = body::stack_limit(&*lv.host, &item.0);
    let packager = if panel.restocker {
        restocked(lv, pos)
    } else {
        None
    };
    let unloaded = if panel.restocker {
        u32::from(packager.is_none())
    } else {
        with(dim, |n| n.unloaded.get(&panel.network).map_or(0, Vec::len)) as u32
    };
    let in_storage = if panel.restocker {
        packager.map_or(0, |p| packager_stock(lv, dim, p).0.count_of(&item))
    } else {
        network_stock(lv, dim, panel.network, false).count_of(&item)
    };
    let expiry = panel.promise_expiry();
    let promised = if panel.restocker {
        panel.restocker_promises.total(&item, expiry, now)
    } else {
        with(dim, |n| {
            n.promises.get_mut(&panel.network).map_or(0, |p| {
                let before = p.list.len();
                let total = p.total(&item, expiry, now);
                if p.list.len() != before {
                    mark_dirty();
                }
                total
            })
        })
    };
    let before = panel.clone();
    let mut step = panel.look(max, in_storage, promised, unloaded);
    // `tickStorageMonitor`: the confirm sounds as it becomes satisfied.
    if !before.satisfied && panel.satisfied && panel.demand(max) > 0 {
        let at = pos.map(|v| f64::from(v) + 0.5);
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "create:confirm",
            at,
            SoundSource::Blocks,
            0.075,
            1.0,
        );
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "create:confirm_2",
            at,
            SoundSource::Blocks,
            0.125,
            0.575,
        );
    }
    // `tickRequests`' timer (`getConfigRequestIntervalInTicks`).
    if step != GaugeStep::Idle {
        let due = with(dim, |n| n.timers.get(&pp).copied().unwrap_or(0));
        if now < due {
            step = GaugeStep::Idle;
        } else {
            with(dim, |n| {
                n.timers.insert(pp, now + u64::from(st::GAUGE_INTERVAL))
            });
        }
    }
    let changed = shown_changed(&before, &panel);
    if before.satisfied != panel.satisfied {
        notify_links(dim, &panel.links);
    }
    let (net, address) = (panel.network, panel.recipe_address.clone());
    with(dim, |n| {
        if let Some(g) = n.gauges.get_mut(&pp) {
            *g = panel.clone();
        }
        if changed {
            n.redraw.push(pos);
        }
    });
    match step {
        GaugeStep::Idle => {}
        // `tryRestock`.
        GaugeStep::Restock { item, demand } => {
            let Some(p) = packager else { return };
            let Some(inv) = super::packages::available(lv, dim, p).1 else {
                return;
            };
            let available: i32 = sources(lv, dim, net, Some(inv))
                .iter()
                .filter(|s| s.packager.is_some())
                .map(|s| s.stock.count_of(&item))
                .sum();
            if available == 0 {
                flash(dim, pp);
                return;
            }
            flash(dim, pp);
            let amount = st::restock_amount(demand, promised, in_storage, max, available);
            let order = Order::simple(vec![(item.clone(), amount)]);
            if broadcast(
                lv,
                dim,
                net,
                RequestType::Restock,
                &order,
                Some(inv),
                &address,
            ) {
                with(dim, |n| {
                    if let Some(g) = n.gauges.get_mut(&pp) {
                        g.restocker_promises.add(item, amount, now);
                    }
                });
            }
        }
        GaugeStep::Request => request_inputs(lv, dim, pp, &panel, now),
    }
}

/// Whether what clients see of a panel changed.
fn shown_changed(a: &Panel<Key>, b: &Panel<Key>) -> bool {
    a.satisfied != b.satisfied
        || a.promised_satisfied != b.promised_satisfied
        || a.waiting != b.waiting
        || a.last_level != b.last_level
        || a.last_promised != b.last_promised
        || a.restocker != b.restocker
}

/// `sendEffect`: the bulb of `pp` lights up on its clients.
fn flash(dim: usize, pp: PanelPos) {
    with(dim, |n| {
        *n.flashes.entry(pp).or_default() += 1;
        n.redraw.push(pp.0);
    });
}

/// `tickRequests` of a panel fed by others: what each asks for, from its network, all or none.
fn request_inputs(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pp: PanelPos,
    panel: &Panel<Key>,
    now: u64,
) {
    let groups = with(dim, |n| {
        st::consolidate(&panel.inputs, |p| {
            n.gauges.get(&p).map(|g| (g.network, g.filter.clone()))
        })
    });
    let Some(groups) = groups else { return };
    let mut failed = false;
    let mut wanted: Vec<(Network, Vec<(Key, u32)>)> = Vec::new();
    let mut results: Vec<(Option<Key>, Network, bool)> = Vec::new();
    for (net, items) in groups {
        let stock = network_stock(lv, dim, net, true);
        let mut list = Vec::new();
        for (item, total) in items {
            match item {
                Some(k) if total > 0 && stock.count_of(&k) >= total as i32 => {
                    results.push((Some(k.clone()), net, true));
                    list.push((k, total));
                }
                other => {
                    results.push((other, net, false));
                    failed = true;
                }
            }
        }
        wanted.push((net, list));
    }
    // `sendEffect` for each input: its arrow flashes with whether its item was there.
    with(dim, |n| {
        let sources: Vec<(PanelPos, Option<Key>, Network)> = panel
            .inputs
            .iter()
            .filter_map(|i| {
                let g = n.gauges.get(&i.from)?;
                Some((i.from, g.filter.clone(), g.network))
            })
            .collect();
        if let Some(g) = n.gauges.get_mut(&pp) {
            for input in g.inputs.iter_mut() {
                if let Some((_, item, net)) = sources.iter().find(|s| s.0 == input.from)
                    && let Some(r) = results.iter().find(|r| r.0 == *item && r.1 == *net)
                {
                    input.success = r.2;
                }
            }
        }
        *n.flashes.entry(pp).or_default() += 1;
        n.redraw.push(pp.0);
    });
    if failed {
        return;
    }
    let crafts = if panel.craft.is_empty() {
        Vec::new()
    } else {
        vec![(panel.craft.iter().map(|k| (k.clone(), 1)).collect(), 1)]
    };
    let mut all = Vec::new();
    for (net, stacks) in wanted {
        let order = Order {
            stacks,
            crafts: crafts.clone(),
        };
        let srcs = sources(lv, dim, net, None);
        let time = lv.game_time();
        let order_id = next_int(time);
        let r = st::find_packagers(&srcs, &order, &panel.recipe_address, order_id, |n| {
            (next_int(time) as u32 % n.max(1) as u32) as usize
        });
        all.push(st::by_packager(r));
    }
    if all
        .iter()
        .flatten()
        .any(|(p, _)| super::packages::too_busy(dim, *p, RequestType::Restock))
    {
        return;
    }
    for g in all {
        perform(lv, dim, g);
    }
    crate::glue::advancements::award_placer(lv, pp.0, "factory_gauge");
    // The output promised to the panel's network (`getQueuedPromises`: a network with links).
    let Some(item) = panel.filter.clone() else {
        return;
    };
    let known = with(dim, |n| {
        n.links.values().any(|l| l.network == Some(panel.network))
    });
    if known {
        with(dim, |n| {
            n.promises
                .entry(panel.network)
                .or_default()
                .add(item, panel.recipe_output, now)
        });
        mark_dirty();
    }
}

// ---------------------------------------------------------------------------------------------
// What clients see

fn panel_tag(host: &dyn mod_api::Host, n: &Net, pp: PanelPos, g: &Panel<Key>) -> Tag {
    let flag = |b: bool| Tag::Byte(i8::from(b));
    let inputs: Vec<Tag> = g
        .inputs
        .iter()
        .map(|i| {
            Tag::compound([
                ("Pos", Tag::IntArray(i.from.0.to_vec())),
                ("Slot", Tag::Byte(i.from.1 as i8)),
                ("Amount", Tag::Int(i.amount as i32)),
                ("Bend", Tag::Int(i.bend)),
                ("Success", flag(i.success)),
            ])
        })
        .collect();
    let mut c = vec![
        ("Slot", Tag::Byte(pp.1 as i8)),
        ("Count", Tag::Int(g.count as i32)),
        ("UpTo", flag(g.up_to)),
        ("Satisfied", flag(g.satisfied)),
        ("PromisedSatisfied", flag(g.promised_satisfied)),
        ("Waiting", flag(g.waiting)),
        ("Powered", flag(g.redstone_powered)),
        ("Restocker", flag(g.restocker)),
        ("Address", Tag::String(g.recipe_address.as_str().into())),
        ("Output", Tag::Int(g.recipe_output as i32)),
        ("Level", Tag::Int(g.last_level)),
        ("Promised", Tag::Int(g.last_promised as i32)),
        ("Clearing", Tag::Int(g.promise_clearing)),
        ("Feeds", flag(!g.outputs.is_empty())),
        (
            "Flash",
            Tag::Int(n.flashes.get(&pp).copied().unwrap_or(0) as i32),
        ),
        ("Inputs", Tag::List(inputs)),
        (
            "Links",
            Tag::List(
                g.links
                    .iter()
                    .map(|l| {
                        Tag::compound([
                            ("Pos", Tag::IntArray(l.from.0.to_vec())),
                            ("Slot", Tag::Byte(l.from.1 as i8)),
                            ("Bend", Tag::Int(l.bend)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "Craft",
            Tag::List(g.craft.iter().map(|k| body::to_nbt(host, &k.0)).collect()),
        ),
        (
            "Freq",
            Tag::IntArray(if g.network == 0 {
                Vec::new()
            } else {
                st::network_ints(g.network).to_vec()
            }),
        ),
    ];
    if let Some(f) = &g.filter {
        c.push(("Filter", body::to_nbt(host, &f.0)));
    }
    Tag::compound(c)
}

/// The gauges whose panels changed: their block entities' data, which goes to their clients.
pub(super) fn send_redraws(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut redraw = with(dim, |n| std::mem::take(&mut n.redraw));
    if redraw.is_empty() {
        return;
    }
    redraw.sort_unstable();
    redraw.dedup();
    for pos in redraw {
        let host: &dyn mod_api::Host = &*lv.host;
        let list = with(dim, |n| {
            panels_at(n, pos)
                .into_iter()
                .filter_map(|s| Some(panel_tag(host, n, (pos, s), n.gauges.get(&(pos, s))?)))
                .collect::<Vec<_>>()
        });
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(PANELS.into(), Tag::List(list))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

// ---------------------------------------------------------------------------------------------
// Placing and clicking

fn deny(lv: &mut ServerLevel<'_>, at: [i32; 3]) {
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "create:deny",
        at.map(|v| f64::from(v) + 0.5),
        SoundSource::Blocks,
        1.0,
        1.0,
    );
}

fn say(host: &mut dyn ServerHost, player: PlayerRef, key: &str) {
    let _ = player.action_bar(host, Text::translate(key, Vec::new()));
}

/// `FactoryPanelBlockItem.place`: an untuned gauge is refused.
pub(super) fn refuse_untuned(host: &mut dyn ServerHost, player: PlayerRef, at: [i32; 3]) {
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    deny(&mut lv, at);
    say(lv.host, player, "create.factory_panel.tune_before_placing");
}

/// `FactoryPanelBlock.setPlacedBy`: the first panel, where the click was, on the item's network.
pub(super) fn placed(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    click: [f64; 3],
    network: Option<Network>,
) {
    let Some((face, facing)) = orientation(lv, pos) else {
        return;
    };
    let local = [0, 1, 2].map(|i| click[i] - f64::from(pos[i]));
    let slot = fx::targeted_slot(face, facing, local);
    let dim = lv.dim;
    let net = network.unwrap_or_else(|| super::new_network(lv));
    with(dim, |n| add_panel(n, (pos, slot), net));
}

/// `FactoryPanelBlock.useItemOn`: a gauge clicked with a gauge outside its panels' value boxes.
/// A tuned one adds a panel where it was clicked; an untuned one is refused.
pub(super) fn use_item_on(
    host: &mut dyn ServerHost,
    click: &mod_api::BlockClick<'_>,
) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    let stack = click.stack?.clone();
    if stack.item().id(&*host) != "create:factory_gauge" {
        return Some(InteractionResult::Success);
    }
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let Some(net) = super::tuned(&stack) else {
        deny(&mut lv, click.pos);
        say(lv.host, player, "create.factory_panel.tune_before_placing");
        return Some(InteractionResult::Fail);
    };
    let (face, facing) = orientation(&lv, click.pos)?;
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(click.pos[i]));
    let slot = fx::targeted_slot(face, facing, local);
    let dim = lv.dim;
    if !with(dim, |n| add_panel(n, (click.pos, slot), net)) {
        return Some(InteractionResult::Success);
    }
    say(lv.host, player, "create.logistically_linked.connected");
    // `addPanel`: a second panel's sound, and the block's own place sound.
    let at = click.pos.map(|v| f64::from(v) + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.place",
        at,
        SoundSource::Blocks,
        1.0,
        0.8,
    );
    if !items::creative(&*lv.host, player) {
        let mut s = stack;
        let _ = s.set_count(s.count().saturating_sub(1));
        items::set_held(lv.host, player, click.hand, (s.count() > 0).then_some(s));
    }
    Some(InteractionResult::Success)
}

/// The active panel whose value box a hit at `point` on the gauge at `pos` lands on.
pub(super) fn hit_panel(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
) -> Option<(PanelPos, Face, Dir)> {
    let (face, facing) = orientation(lv, pos)?;
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let dim = lv.dim;
    let slot = SLOTS.into_iter().find(|s| {
        fx::hits_slot(face, facing, *s, local) && with(dim, |n| n.gauges.contains_key(&(pos, *s)))
    })?;
    Some(((pos, slot), face, facing))
}

/// `ValueSettingsInputHandler.onBlockActivated` on a panel's value box (the server's side): the
/// click is taken; a panel with no item takes one at once (`onShortInteract`: the set-item
/// screen with an empty hand, else the held stack), one with an item waits for the client's
/// hold (the board) or short click (`short_click`).
pub(super) fn right_click(
    host: &mut dyn ServerHost,
    event: &mut mod_api::RightClickBlockEvent<'_>,
) -> mod_api::Flow {
    let click = event.click;
    let Some(player) = host.player(click.player) else {
        return mod_api::Flow::Continue;
    };
    let dimension = click.dimension.key().id().to_string();
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return mod_api::Flow::Continue;
    };
    if super::block_at(&lv, click.pos) != Some(Block::FactoryGauge)
        || !may_use_value_box(&*lv.host, player)
    {
        return mod_api::Flow::Continue;
    }
    let Some((pp, _, _)) = hit_panel(&lv, click.pos, click.hit.point) else {
        return mod_api::Flow::Continue;
    };
    *event.cancellation_result = InteractionResult::Success;
    let empty = with(lv.dim, |n| {
        n.gauges.get(&pp).is_some_and(|g| g.filter.is_none())
    });
    if empty && click.hand == Hand::Main {
        short_interact(&mut lv, player, click.hand, pp, false);
    }
    mod_api::Flow::Cancel
}

/// `LogisticsManager.mayInteract`: an unlocked network, or its owner.
fn may_interact(net: Network, player: PlayerRef) -> bool {
    super::OWNERS.with_borrow(|o| {
        o.get(&net)
            .is_none_or(|o| !o.locked || o.owner.is_none_or(|p| p == player.uuid()))
    })
}

/// `FactoryPanelBehaviour.onShortInteract` on the server.
pub(super) fn short_interact(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    hand: Hand,
    pp: PanelPos,
    wrench: bool,
) {
    let dim = lv.dim;
    let Some(panel) = with(dim, |n| n.gauges.get(&pp).cloned()) else {
        return;
    };
    if !may_interact(panel.network, player) {
        say(lv.host, player, "create.logistically_linked.protected");
        return;
    }
    let held = items::held(&*lv.host, player, hand);
    // The wrench bends the arrows of the panels it feeds, and of the links joined to it.
    if wrench && panel.outputs.len() + panel.links.len() > 0 {
        let mut mode = -1;
        with(dim, |n| {
            for to in panel.outputs.clone() {
                let Some(g) = n.gauges.get_mut(&to) else {
                    continue;
                };
                let Some(i) = g.inputs.iter_mut().find(|i| i.from == pp) else {
                    continue;
                };
                if mode == -1 {
                    mode = (i.bend + 1).rem_euclid(4);
                }
                i.bend = mode;
                n.redraw.push(to.0);
                n.unsaved.push(chunk_of(to.0));
            }
            if let Some(g) = n.gauges.get_mut(&pp)
                && !g.links.is_empty()
            {
                for l in &mut g.links {
                    if mode == -1 {
                        mode = (l.bend + 1).rem_euclid(4);
                    }
                    l.bend = mode;
                }
                n.redraw.push(pp.0);
                n.unsaved.push(chunk_of(pp.0));
            }
        });
        if mode >= 0 {
            let mut boxes = ['\u{25a1}'; 4];
            boxes[mode as usize] = '\u{25a0}';
            let boxes: String = boxes.iter().collect();
            let _ = player.action_bar(
                lv.host,
                Text::translate(
                    "create.factory_panel.cycled_arrow_path",
                    vec![Text::literal(boxes)],
                ),
            );
        }
        return;
    }
    if panel.filter.is_none() {
        let Some(stack) = held else {
            // The set-item screen (`FactoryPanelSetItemMenu`).
            super::gauge_screen::open_set_item(lv, player, pp);
            return;
        };
        // `FilteringBehaviour.onShortInteract` (`canShortInteract`: anything) and `setFilter`.
        let id = stack.item().id(&*lv.host);
        if id == "create:filter" || id == "create:attribute_filter" || id == "create:package_filter"
        {
            say(lv.host, player, "create.logistics.filter.invalid_item");
            let at = player
                .entity()
                .position(&*lv.host)
                .unwrap_or([0.0; 3])
                .map(|v| v.floor() as i32);
            deny(lv, at);
            return;
        }
        set_filter(lv, pp, Some(Key::of(&stack)));
        return;
    }
    // A linking item takes the panel's network.
    if let Some(stack) = held
        && super::linked_item(&stack.item().id(&*lv.host))
    {
        tune_to(lv.host, player, hand, stack, panel.network);
    }
}

/// `FactoryPanelBehaviour.setFilter` and its sound.
pub(super) fn set_filter(lv: &mut ServerLevel<'_>, pp: PanelPos, filter: Option<Key>) {
    let dim = lv.dim;
    let now = super::now(lv);
    with(dim, |n| {
        if let Some(g) = n.gauges.get_mut(&pp) {
            g.filter = filter;
        }
        schedule(n, pp, now);
        n.redraw.push(pp.0);
        n.unsaved.push(chunk_of(pp.0));
    });
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:entity.item_frame.add_item",
        pp.0.map(|v| f64::from(v) + 0.5),
        SoundSource::Blocks,
        0.25,
        0.1,
    );
}

/// `LogisticallyLinkedBlockItem.assignFrequency` to `net`.
fn tune_to(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    hand: Hand,
    mut stack: ItemStack,
    net: Network,
) {
    items::set_custom(
        &mut stack,
        &[(
            super::FREQ,
            Some(Tag::String(st::network_text(net).as_str().into())),
        )],
    );
    let _ = stack.set(&super::Glint, true);
    items::set_held(host, player, hand, Some(stack));
    say(host, player, "create.logistically_linked.tuned");
}

/// `FactoryPanelBehaviour.setValueSettings`: the target amount from the board (`row` 0 items,
/// 1 stacks).
pub(super) fn set_amount(lv: &mut ServerLevel<'_>, pp: PanelPos, row: i32, value: i32) {
    let dim = lv.dim;
    let now = super::now(lv);
    let changed = with(dim, |n| {
        let g = n.gauges.get_mut(&pp)?;
        let (count, up_to) = (value.max(0) as u32, row == 0);
        if g.count == count && g.up_to == up_to {
            return Some(false);
        }
        g.count = count;
        g.up_to = up_to;
        // `resetTimerSlightly`.
        n.timers.insert(pp, now + u64::from(st::GAUGE_INTERVAL) / 2);
        schedule(n, pp, now);
        n.redraw.push(pp.0);
        n.unsaved.push(chunk_of(pp.0));
        Some(true)
    });
    if changed == Some(true) {
        let links = with(dim, |n| n.gauges.get(&pp).map(|g| g.links.clone()));
        notify_links(dim, &links.unwrap_or_default());
        // `playFeedbackSound`.
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "minecraft:block.note_block.hat",
            pp.0.map(|v| f64::from(v) + 0.5),
            SoundSource::Blocks,
            0.25,
            2.0,
        );
    }
}

/// `FactoryPanelPacket`'s connection (`FactoryPanelBehaviour.addConnection`): `from` feeds `to`.
pub(super) fn connect(lv: &mut ServerLevel<'_>, from: PanelPos, to: PanelPos) -> bool {
    let dim = lv.dim;
    let restocker = with(dim, |n| n.gauges.get(&to).map(|g| g.restocker));
    if restocker != Some(false) || from == to {
        return false;
    }
    with(dim, |n| {
        if !n.gauges.contains_key(&from) {
            return false;
        }
        let Some(g) = n.gauges.get_mut(&to) else {
            return false;
        };
        if g.inputs.len() >= st::MAX_INPUTS || g.inputs.iter().any(|i| i.from == from) {
            return false;
        }
        g.inputs.push(Input::new(from, 1));
        if let Some(f) = n.gauges.get_mut(&from)
            && !f.outputs.contains(&to)
        {
            f.outputs.push(to);
        }
        n.redraw.push(to.0);
        n.redraw.push(from.0);
        n.unsaved.push(chunk_of(to.0));
        n.unsaved.push(chunk_of(from.0));
        true
    })
}

/// `FactoryPanelConnectionPacket` for a display link (`addConnection` with a
/// `FactoryPanelSupportBehaviour` there): the link at `link` joins panel `pp`, its arrow from the
/// link's slot nearest the panel (`onRightClick`'s choice). Create's message key of what stops
/// it (`checkForIssues`), or None once joined.
pub(super) fn connect_link(
    lv: &mut ServerLevel<'_>,
    pp: PanelPos,
    link: [i32; 3],
    link_facing: Dir,
    redstone: bool,
) -> Result<(), &'static str> {
    let dim = lv.dim;
    if !with(dim, |n| n.gauges.contains_key(&pp)) {
        return Err("factory_panel.connection_aborted");
    }
    let Some((face, facing)) = orientation(lv, pp.0) else {
        return Err("factory_panel.connection_aborted");
    };
    let connected = fx::connected_direction(face, facing);
    if connected != link_facing {
        return Err("factory_panel.same_orientation");
    }
    let diff = [0, 1, 2].map(|i| link[i] - pp.0[i]);
    let along = match connected.axis() {
        create_engine::kinetics::Axis::X => diff[0],
        create_engine::kinetics::Axis::Y => diff[1],
        create_engine::kinetics::Axis::Z => diff[2],
    };
    if along != 0 {
        return Err("factory_panel.same_surface");
    }
    if diff.iter().map(|v| v * v).sum::<i32>() >= 16 * 16 {
        return Err("factory_panel.too_far_apart");
    }
    // The link's slot whose path to the panel is shortest (the last of equals).
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    for slot in SLOTS {
        let d = fx::path_diff(face, facing, (link, slot), pp);
        let l = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if best_distance < l {
            continue;
        }
        best_distance = l;
        best = slot;
    }
    with(dim, |n| {
        if let Some(g) = n.gauges.get_mut(&pp) {
            g.links.retain(|l| l.from.0 != link);
            g.links.push(Input::new((link, best), 1));
        }
        n.redraw.push(pp.0);
        n.unsaved.push(chunk_of(pp.0));
    });
    if redstone {
        // `FactoryPanelSupportBehaviour.connect`: `changed`, so a link that is no output sends
        // again (`notifyLink`); a receiving one powers the panel at its next look
        // (`checkForRedstoneInput`).
        crate::glue::redstone::notify_link(dim, link);
        check_redstone_input(lv, pp);
    } else {
        crate::glue::display::panel_connected(dim, link, pp);
    }
    Ok(())
}

/// What the configuration screen sends (`FactoryPanelConfigurationPacket.applySettings`).
pub(super) struct Config {
    pub address: String,
    pub inputs: Vec<(PanelPos, u32)>,
    pub craft: Vec<Key>,
    pub output: u32,
    pub clearing: i32,
    pub remove: Option<PanelPos>,
    pub clear_promises: bool,
    pub reset: bool,
    /// `redstoneReset`: the links joined to it let go (`disconnectAllLinks`).
    pub links_reset: bool,
}

pub(super) fn configure(lv: &mut ServerLevel<'_>, player: PlayerRef, pp: PanelPos, c: Config) {
    let dim = lv.dim;
    let now = super::now(lv);
    let Some(net) = with(dim, |n| n.gauges.get(&pp).map(|g| g.network)) else {
        return;
    };
    if !may_interact(net, player) {
        return;
    }
    if c.reset {
        // `disable` and `enable`: the panel as new, its network kept.
        with(dim, |n| {
            remove_panel(n, pp);
            add_panel(n, pp, net);
        });
        return;
    }
    if c.links_reset {
        with(dim, |n| {
            let Some(g) = n.gauges.get_mut(&pp) else {
                return;
            };
            for l in std::mem::take(&mut g.links) {
                crate::glue::display::panel_disconnected(dim, l.from.0, pp);
                // `disconnect`: a transmitting redstone link sends without it.
                crate::glue::redstone::notify_link(dim, l.from.0);
            }
            n.redraw.push(pp.0);
            n.unsaved.push(chunk_of(pp.0));
        });
        return;
    }
    with(dim, |n| {
        for (from, amount) in &c.inputs {
            if let Some(g) = n.gauges.get_mut(&pp)
                && let Some(i) = g.inputs.iter_mut().find(|i| i.from == *from)
            {
                i.amount = (*amount).clamp(1, 64);
            }
        }
        if let Some(r) = c.remove {
            if let Some(g) = n.gauges.get_mut(&pp) {
                g.inputs.retain(|i| i.from != r);
            }
            if let Some(f) = n.gauges.get_mut(&r) {
                f.outputs.retain(|o| *o != pp);
            }
            n.redraw.push(r.0);
            n.unsaved.push(chunk_of(r.0));
        }
        let Some(g) = n.gauges.get_mut(&pp) else {
            return;
        };
        if c.clear_promises {
            // `forceClearPromises`.
            if let Some(item) = g.filter.clone() {
                g.restocker_promises.clear(&item);
                if let Some(p) = n.promises.get_mut(&g.network) {
                    p.clear(&item);
                    mark_dirty();
                }
            }
        }
        g.recipe_address = c.address.chars().take(50).collect();
        g.recipe_output = c.output.clamp(1, 64);
        g.promise_clearing = c.clearing.clamp(-1, 31);
        g.craft = c.craft;
        schedule(n, pp, now);
        n.redraw.push(pp.0);
        n.unsaved.push(chunk_of(pp.0));
    });
}

/// `FactoryPanelBlock.onSneakWrenched`: the panel the wrench hit comes off, its gauge back to the
/// player; the block goes with its last panel.
pub(super) fn sneak_wrenched(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    point: [f64; 3],
) -> bool {
    let Some((face, facing)) = orientation(lv, pos) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let slot = fx::targeted_slot(face, facing, local);
    let dim = lv.dim;
    if !with(dim, |n| remove_panel(n, (pos, slot))) {
        return true;
    }
    if !items::creative(&*lv.host, player)
        && let Some(stack) = body::new_stack(&*lv.host, "create:factory_gauge", 1)
    {
        items::give_back(lv.host, player, stack);
    }
    // `IWrenchable.playRemoveSound`.
    let at = pos.map(|v| f64::from(v) + 0.5);
    for name in ["create:wrench_remove", "create:wrench_remove_compounded_1"] {
        let _ = lv
            .level
            .play_sound(&mut *lv.host, name, at, SoundSource::Blocks, 0.25, 0.75);
    }
    if with(dim, |n| panels_at(n, pos).is_empty()) {
        lv.destroy_block(pos, false);
    }
    true
}

/// `FactoryPanelBehaviour.moveTo`: the panel `from` moves to `to` (`FactoryPanelConnectionPacket`
/// with `relocate`): onto a free panel of a gauge as its own, or into the air (a gauge placed as
/// its own), while every panel and link joined to it stays within 24 blocks of the new place
/// (`closerThan`); the joined panels and links follow it. A gauge left with no panel goes
/// (`FactoryPanelBlockEntity.lazyTick`).
pub(super) fn relocate(lv: &mut ServerLevel<'_>, player: PlayerRef, from: PanelPos, to: PanelPos) {
    let dim = lv.dim;
    let Some(state) = lv.block_state(from.0) else {
        return;
    };
    if orientation(lv, from.0).is_none() {
        return;
    }
    let Some(panel) = with(dim, |n| n.gauges.get(&from).cloned()) else {
        return;
    };
    if with(dim, |n| n.gauges.contains_key(&to)) {
        return;
    }
    let Some(existing) = lv.block_state(to.0) else {
        return;
    };
    let gauge = orientation(lv, to.0).is_some();
    let air = existing.block().id(&*lv.host) == "minecraft:air";
    if !air && !gauge || gauge && existing != state {
        return;
    }
    // `canSurvive` (the client asks it before sending): a sturdy face behind it.
    if !gauge && let Some((face, facing)) = orientation(lv, from.0) {
        let o = fx::connected_direction(face, facing).offset();
        let behind = [to.0[0] - o[0], to.0[1] - o[1], to.0[2] - o[2]];
        let side = crate::glue::decor::face_of(fx::connected_direction(face, facing));
        let sturdy = lv
            .level
            .block_face_sturdy(&*lv.host, behind, side, mod_api::SupportType::Full)
            .unwrap_or(false);
        if !sturdy {
            say(lv.host, player, "create.factory_panel.relocation_aborted");
            return;
        }
    }
    let close = |p: [i32; 3]| (0..3).map(|i| (p[i] - to.0[i]).pow(2)).sum::<i32>() < 24 * 24;
    if !panel.links.iter().all(|l| close(l.from.0))
        || !panel.inputs.iter().all(|i| close(i.from.0))
        || !panel.outputs.iter().all(|o| close(o.0))
    {
        return;
    }
    if !gauge {
        lv.set_state(to.0, state, super::super::server::update_all());
    }
    // Links let go of it, and take it again where it is.
    for l in &panel.links {
        crate::glue::display::panel_disconnected(dim, l.from.0, from);
    }
    with(dim, |n| {
        let Some(p) = n.gauges.remove(&from) else {
            return;
        };
        for (k, v) in [(n.due.remove(&from), true), (n.timers.remove(&from), false)] {
            if let Some(t) = k {
                if v {
                    n.due.insert(to, t);
                } else {
                    n.timers.insert(to, t);
                }
            }
        }
        if let Some(f) = n.flashes.remove(&from) {
            n.flashes.insert(to, f);
        }
        for out in &p.outputs {
            if let Some(g) = n.gauges.get_mut(out) {
                for i in g.inputs.iter_mut().filter(|i| i.from == from) {
                    i.from = to;
                }
                n.redraw.push(out.0);
                n.unsaved.push(chunk_of(out.0));
            }
        }
        for input in &p.inputs {
            if let Some(g) = n.gauges.get_mut(&input.from) {
                for o in g.outputs.iter_mut().filter(|o| **o == from) {
                    *o = to;
                }
                n.redraw.push(input.from.0);
                n.unsaved.push(chunk_of(input.from.0));
            }
        }
        n.gauges.insert(to, p);
        n.redraw.push(from.0);
        n.redraw.push(to.0);
        n.unsaved.push(chunk_of(from.0));
        n.unsaved.push(chunk_of(to.0));
    });
    for l in &panel.links {
        if super::block_at(lv, l.from.0) == Some(Block::DisplayLink) {
            crate::glue::display::panel_connected(dim, l.from.0, to);
        }
        crate::glue::redstone::notify_link(dim, l.from.0);
    }
    with(dim, |n| n.redstone_checks.push(to));
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some("green".to_owned());
    let _ = player.action_bar(
        lv.host,
        Text::translate("create.factory_panel.relocated", Vec::new()).with_style(style),
    );
    let at = to.0.map(|v| f64::from(v) + 0.5);
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.break",
        at,
        SoundSource::Blocks,
        1.0,
        1.0,
    );
    if with(dim, |n| panels_at(n, from.0).is_empty()) {
        lv.destroy_block(from.0, false);
    }
}

/// `FactoryPanelBlock.tryDestroySubPanelFirst`: a player breaking a gauge with two panels or
/// more takes the one looked at (its gauge dropped outside creative); the block stays.
pub(super) fn break_sub_panel(host: &mut dyn ServerHost, pos: [i32; 3]) -> bool {
    let Some(player) = host.players().acting_player() else {
        return false;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return false;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let dim = lv.dim;
    if with(dim, |n| panels_at(n, pos).len()) < 2 {
        return false;
    }
    let Some((face, facing)) = orientation(&lv, pos) else {
        return false;
    };
    let reach = 6.0;
    let Some(hit) = player.entity().pick(&*lv.host, reach) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| hit.location[i] - f64::from(pos[i]));
    let slot = fx::targeted_slot(face, facing, local);
    if !with(dim, |n| remove_panel(n, (pos, slot))) {
        return false;
    }
    if !items::creative(&*lv.host, player)
        && let Some(stack) = body::new_stack(&*lv.host, "create:factory_gauge", 1)
    {
        let at = pos.map(|v| f64::from(v) + 0.5);
        let _ = lv
            .level
            .spawn_item(&mut *lv.host, at[0], at[1], at[2], stack, 10);
    }
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.break",
        pos.map(|v| f64::from(v) + 0.5),
        SoundSource::Blocks,
        1.0,
        0.8,
    );
    true
}

/// `FactoryPanelBlockEntity.destroy`: a gauge broken with more than one panel drops the others.
pub(super) fn extra_drops(dim: usize, pos: [i32; 3]) -> usize {
    with(dim, |n| panels_at(n, pos).len().saturating_sub(1))
}

// ---------------------------------------------------------------------------------------------
// Saving

pub(super) fn save(
    host: &dyn mod_api::Host,
    n: &Net,
    now: u64,
    inside: &dyn Fn(&[i32; 3]) -> bool,
) -> Vec<Tag> {
    let mut keys: Vec<PanelPos> = n.gauges.keys().filter(|p| inside(&p.0)).copied().collect();
    keys.sort_unstable();
    keys.into_iter()
        .map(|pp| {
            let g = &n.gauges[&pp];
            let mut c = vec![
                ("Pos", Tag::IntArray(pp.0.to_vec())),
                ("Slot", Tag::Byte(pp.1 as i8)),
                ("Freq", super::freq((g.network != 0).then_some(g.network))),
                (
                    "Settings",
                    Tag::IntArray(vec![
                        g.count as i32,
                        i32::from(g.up_to),
                        g.recipe_output as i32,
                        g.promise_clearing,
                        i32::from(g.restocker),
                        i32::from(g.satisfied),
                        i32::from(g.promised_satisfied),
                        i32::from(g.waiting),
                    ]),
                ),
                (
                    "RecipeAddress",
                    Tag::String(g.recipe_address.as_str().into()),
                ),
                (
                    "Inputs",
                    Tag::IntArray(
                        g.inputs
                            .iter()
                            .flat_map(|i| {
                                let p = i.from.0;
                                [
                                    p[0],
                                    p[1],
                                    p[2],
                                    i32::from(i.from.1),
                                    i.amount as i32,
                                    i.bend,
                                ]
                            })
                            .collect(),
                    ),
                ),
                (
                    "TargetedByLinks",
                    Tag::IntArray(
                        g.links
                            .iter()
                            .flat_map(|l| {
                                let p = l.from.0;
                                [p[0], p[1], p[2], i32::from(l.from.1), l.bend]
                            })
                            .collect(),
                    ),
                ),
                (
                    "Outputs",
                    Tag::IntArray(
                        g.outputs
                            .iter()
                            .flat_map(|o| [o.0[0], o.0[1], o.0[2], i32::from(o.1)])
                            .collect(),
                    ),
                ),
                (
                    "Craft",
                    Tag::List(g.craft.iter().map(|k| body::to_nbt(host, &k.0)).collect()),
                ),
            ];
            if let Some(f) = &g.filter {
                c.push(("Filter", body::to_nbt(host, &f.0)));
            }
            if !g.restocker_promises.is_empty() {
                // `RequestPromise`: the stack, its count and age.
                let list = g
                    .restocker_promises
                    .list
                    .iter()
                    .map(|(k, n, at)| {
                        Tag::compound([
                            ("Item", body::to_nbt(host, &k.0)),
                            (
                                "Count",
                                Tag::IntArray(vec![*n as i32, now.saturating_sub(*at) as i32]),
                            ),
                        ])
                    })
                    .collect();
                c.push(("Promises", Tag::List(list)));
            }
            Tag::compound(c)
        })
        .collect()
}

pub(super) fn load(host: &dyn mod_api::Host, n: &mut Net, now: u64, list: &[Compound]) {
    use super::{ints, pos_of, text};
    for c in list {
        let Some(pos) = pos_of(c) else { continue };
        let slot = match c.get(&"Slot".into()) {
            Some(Tag::Byte(b)) => (*b as u8).min(3),
            // A gauge saved before its panels: its one panel bottom left.
            _ => 2,
        };
        let net = st::network_from_ints(&ints(c.get(&"Freq".into()))).unwrap_or(0);
        let mut g = Panel::new(net);
        let s = ints(c.get(&"Settings".into()));
        let at = |i: usize| s.get(i).copied().unwrap_or(0);
        g.count = at(0).max(0) as u32;
        g.up_to = at(1) != 0;
        g.recipe_output = at(2).max(1) as u32;
        g.promise_clearing = if s.len() > 3 { at(3) } else { -1 };
        g.restocker = at(4) != 0;
        g.satisfied = at(5) != 0;
        g.promised_satisfied = at(6) != 0;
        g.waiting = at(7) != 0;
        g.recipe_address = text(c.get(&"RecipeAddress".into()));
        g.inputs = ints(c.get(&"Inputs".into()))
            .as_chunks::<6>()
            .0
            .iter()
            .map(|v| Input {
                from: ([v[0], v[1], v[2]], v[3].clamp(0, 3) as u8),
                amount: v[4].max(0) as u32,
                bend: v[5],
                success: true,
            })
            .collect();
        // Before panels: `TargetedBy` of whole gauges.
        g.inputs.extend(
            ints(c.get(&"TargetedBy".into()))
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| Input::new(([v[0], v[1], v[2]], 2), v[3].max(0) as u32)),
        );
        g.links = ints(c.get(&"TargetedByLinks".into()))
            .as_chunks::<5>()
            .0
            .iter()
            .map(|v| Input {
                from: ([v[0], v[1], v[2]], v[3].clamp(0, 3) as u8),
                amount: 1,
                bend: v[4],
                success: true,
            })
            .collect();
        g.outputs = ints(c.get(&"Outputs".into()))
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| ([v[0], v[1], v[2]], v[3].clamp(0, 3) as u8))
            .collect();
        g.outputs.extend(
            ints(c.get(&"Targeting".into()))
                .as_chunks::<3>()
                .0
                .iter()
                .map(|v| ([v[0], v[1], v[2]], 2)),
        );
        if let Some(Tag::List(l)) = c.get(&"Craft".into()) {
            g.craft = l
                .iter()
                .filter_map(|t| body::from_nbt(host, t))
                .map(|s| Key::of(&s))
                .collect();
        }
        g.filter = c
            .get(&"Filter".into())
            .and_then(|t| body::from_nbt(host, t))
            .map(|s| Key::of(&s));
        if let Some(Tag::List(l)) = c.get(&"Promises".into()) {
            for t in l {
                let Tag::Compound(e) = t else { continue };
                let Some(item) = e.get(&"Item".into()).and_then(|t| body::from_nbt(host, t)) else {
                    continue;
                };
                let v = ints(e.get(&"Count".into()));
                let count = v.first().copied().unwrap_or(0).max(0) as u32;
                let age = v.get(1).copied().unwrap_or(0).max(0) as u64;
                g.restocker_promises
                    .add(Key::of(&item), count, now.saturating_sub(age));
            }
        }
        let pp = (pos, slot);
        if g.filter.is_some() {
            n.due.insert(pp, now);
            n.next = n.next.min(now);
        }
        n.gauges.insert(pp, g);
        n.redraw.push(pos);
    }
}

// ---------------------------------------------------------------------------------------------
// Debug commands, for tests

/// The first panel of the gauge at `pos`, or its panel `#slot` as the last word.
fn panel_arg(dim: usize, pos: [i32; 3], args: &[&str]) -> (Option<PanelPos>, usize) {
    let explicit = args
        .last()
        .and_then(|w| w.strip_prefix('#'))
        .and_then(|s| s.parse::<u8>().ok());
    let used = usize::from(explicit.is_some());
    let slot = explicit.or_else(|| with(dim, |n| panels_at(n, pos).first().copied()));
    (slot.map(|s| (pos, s)), used)
}

/// The panel's item, amount and address: `<item|-> <count> <items|stacks> [address] [#slot]`.
pub(super) fn set_gauge(
    lv: &ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage =
        "usage: create_debug gauge <x y z> <item|-> <count> <items|stacks> [address] [#slot]";
    let (pp, used) = panel_arg(dim, pos, args);
    let args = &args[..args.len() - used];
    let [item, count, mode, address @ ..] = args else {
        return Err(usage.into());
    };
    let pp = pp.ok_or("no factory gauge there")?;
    let filter = if *item == "-" {
        None
    } else {
        Some(item_of(lv, item).ok_or_else(|| format!("no item {item}"))?)
    };
    let count: u32 = count.parse().map_err(|_| usage.to_owned())?;
    let up_to = match *mode {
        "items" => true,
        "stacks" => false,
        _ => return Err(usage.into()),
    };
    let address = address.join(" ");
    let now = super::now(lv);
    let restocker = restocked(lv, pos).is_some();
    with(dim, |n| {
        let g = n.gauges.get_mut(&pp)?;
        g.filter = filter;
        g.count = count;
        g.up_to = up_to;
        g.recipe_address = address.clone();
        g.restocker = restocker;
        schedule(n, pp, now);
        n.redraw.push(pos);
        n.unsaved.push(chunk_of(pos));
        Some(format!(
            "gauge {item} x{count} {mode} to \"{address}\" (panel {})",
            pp.1
        ))
    })
    .ok_or_else(|| "no factory gauge there".into())
}

/// `<to x y z> [#from slot]` on a display or redstone link: the panel at `from` joins it, as
/// `FactoryPanelConnectionHandler.onRightClick` on the link does. None when `to` is no link.
pub(super) fn connect_link_command(
    lv: &mut ServerLevel<'_>,
    from: [i32; 3],
    args: &[&str],
    here: [f64; 3],
) -> Option<Result<String, String>> {
    let to = super::coords(args, Some(here))?;
    let (facing, redstone) = match crate::glue::display::display_link_facing(lv, to) {
        Some(f) => (f, false),
        None => (crate::glue::redstone::link_facing(lv, to)?, true),
    };
    let slot = args
        .iter()
        .find_map(|w| w.strip_prefix('#')?.parse().ok())
        .or_else(|| with(lv.dim, |n| panels_at(n, from).first().copied()));
    let Some(slot) = slot else {
        return Some(Err("no factory gauge to connect from".into()));
    };
    Some(
        connect_link(lv, (from, slot), to, facing, redstone)
            .map(|()| format!("gauge {from:?} #{slot} joins the link at {to:?}"))
            .map_err(|key| format!("create.{key}")),
    )
}

/// `<to x y z> [amount] [#from slot] [#to slot]`: the panel at `from` feeds the one at `to`.
pub(super) fn connect_command(
    dim: usize,
    from: [i32; 3],
    args: &[&str],
    here: [f64; 3],
) -> Result<String, String> {
    let usage = "usage: create_debug gauge_connect <from x y z> <to x y z> [amount] [#from] [#to]";
    let to = super::coords(args, Some(here)).ok_or_else(|| usage.to_owned())?;
    let slots: Vec<u8> = args
        .iter()
        .filter_map(|w| w.strip_prefix('#')?.parse().ok())
        .collect();
    let amount = match args.get(3).filter(|w| !w.starts_with('#')) {
        Some(a) => a.parse::<u32>().map_err(|_| usage.to_owned())?,
        None => 1,
    };
    let first = |pos| with(dim, |n| panels_at(n, pos).first().copied());
    let fs = slots
        .first()
        .copied()
        .or_else(|| first(from))
        .ok_or("no factory gauge to connect from")?;
    let ts = slots
        .get(1)
        .copied()
        .or_else(|| first(to))
        .ok_or("no factory gauge to connect to")?;
    let (f, t) = ((from, fs), (to, ts));
    with(dim, |n| {
        if !n.gauges.contains_key(&f) || f == t {
            return Err("no factory gauge to connect from".into());
        }
        let g = n
            .gauges
            .get_mut(&t)
            .ok_or("no factory gauge to connect to")?;
        if g.restocker {
            return Err("a restocking gauge takes no inputs".into());
        }
        match g.inputs.iter().position(|i| i.from == f) {
            Some(i) => g.inputs[i].amount = amount,
            None if g.inputs.len() >= st::MAX_INPUTS => return Err("nine inputs already".into()),
            None => g.inputs.push(Input::new(f, amount)),
        }
        if let Some(fg) = n.gauges.get_mut(&f)
            && !fg.outputs.contains(&t)
        {
            fg.outputs.push(t);
        }
        n.redraw.push(t.0);
        n.redraw.push(f.0);
        n.unsaved.push(chunk_of(t.0));
        n.unsaved.push(chunk_of(f.0));
        Ok(format!("gauge {f:?} feeds {t:?} x{amount}"))
    })
}

/// The panel's recipe: `<output count> [item...] [#slot]`.
pub(super) fn set_craft(
    lv: &ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage = "usage: create_debug gauge_craft <x y z> <output count> [item...] [#slot]";
    let (pp, used) = panel_arg(dim, pos, args);
    let args = &args[..args.len() - used];
    let Some((count, items)) = args.split_first() else {
        return Err(usage.into());
    };
    let pp = pp.ok_or("no factory gauge there")?;
    let output: u32 = count.parse().map_err(|_| usage.to_owned())?;
    if items.len() > create_engine::package::SLOTS {
        return Err("nine items at most".into());
    }
    let craft = items
        .iter()
        .map(|w| item_of(lv, w).ok_or_else(|| format!("no item {w}")))
        .collect::<Result<Vec<_>, _>>()?;
    with(dim, |n| {
        let g = n.gauges.get_mut(&pp)?;
        g.recipe_output = output.max(1);
        g.craft = craft;
        n.redraw.push(pos);
        n.unsaved.push(chunk_of(pos));
        Some(format!("gauge makes {output} from {} items", items.len()))
    })
    .ok_or_else(|| "no factory gauge there".into())
}

/// A gauge's panels, for `create_debug stock`.
pub(super) fn describe(host: &dyn mod_api::Host, dim: usize, pos: [i32; 3]) -> Option<String> {
    let id = |k: &Key| body::item_id(host, &k.0);
    let panels: Vec<(u8, Panel<Key>)> = with(dim, |n| {
        panels_at(n, pos)
            .into_iter()
            .filter_map(|s| Some((s, n.gauges.get(&(pos, s))?.clone())))
            .collect()
    });
    if panels.is_empty() {
        return None;
    }
    let lines: Vec<String> = panels
        .iter()
        .map(|(s, g)| {
            let inputs: Vec<String> = g
                .inputs
                .iter()
                .map(|i| format!("{:?}#{}x{}", i.from.0, i.from.1, i.amount))
                .collect();
            format!(
                "panel {} ({}): {} x{}{} address=\"{}\" network={} restocker={} satisfied={} promised_satisfied={} waiting={} level={} promised={} inputs=[{}] output={} craft={}",
                s,
                fx::slot_name(*s),
                g.filter.as_ref().map_or("-".to_owned(), id),
                g.count,
                if g.up_to { "" } else { " stacks" },
                g.recipe_address,
                if g.network == 0 {
                    "-".to_owned()
                } else {
                    st::network_text(g.network)
                },
                g.restocker,
                g.satisfied,
                g.promised_satisfied,
                g.waiting,
                g.last_level,
                g.last_promised,
                inputs.join(", "),
                g.recipe_output,
                g.craft.len(),
            )
        })
        .collect();
    Some(format!("gauge: {}", lines.join(" | ")))
}

/// The dimension index of a player's level.
pub(super) fn dim_of(host: &dyn mod_api::Host, player: PlayerRef) -> Option<(usize, String)> {
    let d = player.dimension(host)?.to_string();
    Some((level_index(&d)?, d))
}
