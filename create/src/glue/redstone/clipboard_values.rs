//! The clipboard's copying and pasting of block settings (`ClipboardValueSettingsHandler`) on
//! the blocks whose behaviours are `ClipboardCloneable`, each under its key: a brass diode's
//! `Block` (inverted) and `Timings` (its delay's board row and value), a redstone link's
//! `Frequencies`, a chassis' range (`Chassis`), the value boxes served by `interaction::values`
//! (each `ValueSettingsBehaviour`'s board row and value: a motor's `Speed`, an arm's or tunnel's
//! `SelectionMode`, the movement modes, a cart assembler's `CartMovementMode`, an ejector's or
//! valve handle's `Settings`...), and every filter slot's `Filtering` (`FilteringBehaviour`: its
//! filter, and its amount's row and value while the amount shows): smart observers, threshold
//! switches, brass funnels, smart chutes, basins, saws, deployers, a brass tunnel's side filters
//! (the side clicked), smart pipes, contraption controls, rollers, item hatches, creative crates
//! track observers and table cloths' price tags. A right click with a clipboard copies them into its `copied_values`; a
//! left click pastes them (a filter item comes out of the inventory outside creative, the one
//! the slot held goes back). Not while sneaking. A clipboard clicked onto a placed clipboard
//! (`ClipboardBlockEntity`) takes the entries it lacks. Looking at such a block with a clipboard
//! in hand shows what a click does over the hotbar (`clientTick`'s `showHoverTip`).
//!
//! The server sees the right click (`right_click_block`); a left click has no player on the
//! server's hook, so the client takes the attack on such a block (`interaction_key`) and asks
//! the server on `CHANNEL`, which checks reach as `BlockEntityConfigurationPacket`s do.
//!
//! A steam engine's `RotationDirection` is a value box's; a table cloth's price is a filter
//! slot's (`TableClothFilteringBehaviour`: the payment item, `stock::shop`).

use super::block_at;
use crate::glue::display::clipboard;
use crate::glue::interaction::values;
use crate::glue::items::{self, filters};
use crate::glue::persist;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::filter::{Filter, FilterItem};
use create_engine::kinetics::Dir;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, Compound, CrosshairTarget, Flow, Hand, HostSide, InteractionAttempt,
    InteractionKey, InteractionKeyMappingTriggeredEvent, InteractionResult, ItemStack, PlayerRef,
    RightClickBlockEvent, ServerHost, Tag, Text,
};

/// Client to server: paste the clipboard in hand onto a block (position, face).
pub(super) const CHANNEL: &str = "create:clipboard_paste";

/// A filter slot the clipboard reads and writes (`FilteringBehaviour`), by where the port keeps
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// The logistics and processing blocks' (`chute_filters`): brass funnels, smart chutes,
    /// basins, saws, deployers.
    Shared,
    /// Smart observers' and threshold switches' (`detectors`).
    Detector,
    /// A brass tunnel's side filters (`SidedFilteringBehaviour`).
    Tunnel,
    /// Item hatches' (`decor`).
    Hatch,
    /// The rest, kept as an item: smart pipes, contraption controls, rollers, crates.
    Pipe,
    Controls,
    Roller,
    Crate,
    /// Track observers' (`observer_filters`).
    Observer,
    /// A shop's price tag (`TableClothFilteringBehaviour`, `stock::shop`).
    Price,
}

/// A block's `ClipboardCloneable` behaviours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Behaviour {
    /// A `ValueSettingsBehaviour` `interaction::values` serves, under its key.
    Value(&'static str),
    /// `ChassisScrollValueBehaviour` (`Chassis`).
    Chassis,
    /// `BrassDiodeBlockEntity` (`Block`) and its `BrassDiodeScrollValueBehaviour` (`Timings`).
    Diode,
    /// `LinkBehaviour` (`Frequencies`).
    Frequencies,
    Filtering(Slot),
}

fn slot_of(block: Block) -> Option<Slot> {
    Some(match block {
        Block::BrassFunnel
        | Block::BrassBeltFunnel
        | Block::SmartChute
        | Block::Basin
        | Block::MechanicalSaw
        | Block::Deployer => Slot::Shared,
        Block::ContentObserver | Block::StockpileSwitch => Slot::Detector,
        Block::BrassTunnel => Slot::Tunnel,
        Block::SmartFluidPipe => Slot::Pipe,
        Block::ContraptionControls => Slot::Controls,
        Block::MechanicalRoller => Slot::Roller,
        Block::ItemHatch => Slot::Hatch,
        Block::CreativeCrate => Slot::Crate,
        Block::TrackObserver => Slot::Observer,
        b if crate::glue::stock::shop::is_cloth(b) => Slot::Price,
        _ => return None,
    })
}

fn is_chassis(block: Block) -> bool {
    matches!(
        block,
        Block::LinearChassis | Block::SecondaryLinearChassis | Block::RadialChassis
    )
}

/// The block's cloneable behaviours (`getAllBehaviours` that are `ClipboardCloneable`).
fn behaviours(block: Block) -> Vec<Behaviour> {
    let mut out = Vec::new();
    if create_engine::world::diode_kind(block).is_some() {
        out.push(Behaviour::Diode);
    } else if let Some(key) = values::clipboard_key(block) {
        out.push(Behaviour::Value(key));
    }
    if is_chassis(block) {
        out.push(Behaviour::Chassis);
    }
    if block == Block::RedstoneLink {
        out.push(Behaviour::Frequencies);
    }
    if let Some(s) = slot_of(block) {
        out.push(Behaviour::Filtering(s));
    }
    out
}

/// Whether `block` has settings the clipboard copies, or is a placed clipboard.
fn cloneable(block: Block) -> bool {
    !behaviours(block).is_empty()
}

fn int(c: &Compound, key: &str) -> Option<i32> {
    match c.get(&key.into()) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    }
}

fn sub<'a>(c: &'a Compound, key: &str) -> Option<&'a Compound> {
    match c.get(&key.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// An item id's stack (`ItemStack.EMPTY` for none or an unknown id).
fn stack_of(host: &dyn mod_api::Host, item: &str) -> Option<ItemStack> {
    mod_api::Identifier::parse(item)
        .ok()
        .and_then(|i| host.item_key(&i))
        .map(ItemStack::of)
}

/// `ItemStack.saveOptional`: an empty compound for none.
fn save(host: &dyn mod_api::Host, stack: Option<&ItemStack>) -> Tag {
    Tag::Compound(stack.and_then(|s| s.to_nbt(host)).unwrap_or_default())
}

/// `ItemStack.parseOptional`: None for an empty compound.
fn parse(host: &dyn mod_api::Host, tag: &Compound) -> Option<ItemStack> {
    if tag.is_empty() {
        return None;
    }
    ItemStack::from_nbt(host, tag)
}

fn row_value(s: create_engine::board::Settings) -> Tag {
    Tag::Compound(Compound::from([
        ("Value".into(), Tag::Int(s.value)),
        ("Row".into(), Tag::Int(s.row)),
    ]))
}

/// The settings in `t` (`Value` and `Row` both there).
fn settings_in(t: &Compound) -> Option<create_engine::board::Settings> {
    Some(create_engine::board::Settings {
        value: int(t, "Value")?,
        row: int(t, "Row")?,
    })
}

/// The side of a brass tunnel at `pos` that has a filter slot (`sidedFilters.containsKey`).
fn tunnel_side(lv: &ServerLevel<'_>, pos: [i32; 3], side: Dir) -> bool {
    crate::glue::logistics::tunnel::tunnel_state(lv, pos).is_some_and(|(axis, shape)| {
        create_engine::tunnel::filter_slot(axis, shape, side).is_some()
    })
}

/// What the filter slot `slot` of the block at `pos` holds, seen from `side`
/// (`getFilter(side)`), as a stack.
fn filter_now(lv: &ServerLevel<'_>, pos: [i32; 3], slot: Slot, side: Dir) -> Option<ItemStack> {
    let dim = lv.dim;
    let host: &dyn mod_api::Host = &*lv.host;
    let filter = |f: Option<Filter>| f.and_then(|f| filters::filter_stack(host, &f));
    let item = |i: Option<String>| i.and_then(|i| stack_of(host, &i));
    match slot {
        Slot::Shared => {
            filter(with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten())
        }
        Slot::Detector => {
            filter(with_world(|w| w.dims[dim].detectors.filter(pos).cloned()).flatten())
        }
        Slot::Tunnel => {
            filter(with_world(|w| w.dims[dim].tunnel_filters.get(&(pos, side)).cloned()).flatten())
        }
        Slot::Observer => {
            filter(with_world(|w| w.dims[dim].observer_filters.get(&pos).cloned()).flatten())
        }
        Slot::Pipe => item(crate::glue::fluids::clicks::smart_pipe_filter(dim, pos)),
        Slot::Controls | Slot::Roller => item(crate::glue::contraptions::actors::mounted_filter(
            dim,
            pos,
            slot == Slot::Roller,
        )),
        Slot::Hatch => filter(crate::glue::decor::hatch_filter(dim, pos)),
        Slot::Crate => item(crate::glue::decor::crate_supply_of(dim, pos)),
        Slot::Price => crate::glue::stock::shop::price_of(dim, pos),
    }
}

/// `setFilter(side, stack)` on the filter slot `slot` of the block at `pos`: whether it took it.
fn set_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    slot: Slot,
    side: Dir,
    stack: Option<ItemStack>,
) -> bool {
    let dim = lv.dim;
    let f = stack.as_ref().map(|s| filters::filter_of(&*lv.host, s));
    let id = stack.as_ref().map(|s| s.item().id(&*lv.host));
    match slot {
        Slot::Shared => {
            let max = crate::glue::logistics::amount::filter_max(&*lv.host, f.as_ref());
            let shown = f.clone();
            with_world(|w| {
                if let Some(a) = w.dims[dim].filter_amounts.get_mut(&pos) {
                    *a = a.fitted(max);
                }
                if block == Block::SmartChute {
                    w.set_chute_filter(dim, pos, f);
                    return;
                }
                let d = &mut w.dims[dim];
                match f {
                    Some(f) => d.chute_filters.insert(pos, f),
                    None => d.chute_filters.remove(&pos),
                };
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                // `withCallback`: a basin's mixer or press looks again.
                if block == Block::Basin {
                    d.mixer_checks.push([pos[0], pos[1] + 2, pos[2]]);
                }
            });
            if matches!(block, Block::BrassFunnel | Block::BrassBeltFunnel) {
                crate::glue::logistics::funnel::wake_funnel(dim, pos);
            }
            crate::glue::logistics::show_filter(lv, pos, shown.as_ref());
            true
        }
        Slot::Detector => {
            if let Some(Ok(_)) = with_world(|w| w.dims[dim].detectors.set_filter(pos, f)) {
                persist::mark_pos_unsaved(dim, pos);
            }
            true
        }
        Slot::Tunnel => {
            // A side without a slot takes nothing, and that is no failure.
            if !tunnel_side(lv, pos, side) {
                return true;
            }
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                match f {
                    Some(f) => d.tunnel_filters.insert((pos, side), f),
                    None => d.tunnel_filters.remove(&(pos, side)),
                }
            });
            crate::glue::logistics::tunnel::show_tunnel_filters(lv, pos);
            true
        }
        Slot::Observer => {
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.railway.set_observer_filtered(pos, f.is_some());
                match f {
                    Some(f) => d.observer_filters.insert(pos, f),
                    None => d.observer_filters.remove(&pos),
                }
            });
            persist::mark_pos_unsaved(dim, pos);
            true
        }
        Slot::Pipe => {
            crate::glue::fluids::clicks::set_smart_pipe_filter(lv, pos, id);
            true
        }
        Slot::Controls | Slot::Roller => {
            crate::glue::contraptions::actors::set_mounted_filter(lv, pos, slot == Slot::Roller, id)
        }
        Slot::Hatch => {
            crate::glue::decor::set_hatch_filter(dim, pos, f);
            true
        }
        Slot::Crate => {
            crate::glue::decor::set_crate_supply(dim, pos, id);
            true
        }
        Slot::Price => crate::glue::stock::shop::paste_price(lv, pos, stack),
    }
}

/// Whether the stack is a filter item (`instanceof FilterItem`).
fn is_filter_item(host: &dyn mod_api::Host, stack: &ItemStack) -> bool {
    FilterItem::of(&stack.item().id(host)).is_some()
}

/// `FilteringBehaviour.readFromClipboard`'s filter: outside creative, a filter item copied
/// comes out of the inventory (stacks without data first; unless the slot's own is of its
/// kind), or the paste fails with Create's message; the slot's own filter item goes back.
fn paste_filter(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    block: Block,
    slot: Slot,
    side: Dir,
    copied: Option<ItemStack>,
) -> bool {
    let creative = items::creative(&*lv.host, player);
    let refund =
        filter_now(lv, pos, slot, side).filter(|s| !creative && is_filter_item(&*lv.host, s));
    if let Some(c) = copied
        .as_ref()
        .filter(|c| !creative && is_filter_item(&*lv.host, c))
    {
        let kind = c.item();
        let mut taken = refund.as_ref().is_some_and(|r| r.item() == kind);
        for without_data in [true, false] {
            if taken {
                break;
            }
            let found = player
                .inventory_slots(&*lv.host)
                .unwrap_or_default()
                .into_iter()
                .find(|(_, s)| {
                    s.item() == kind
                        && without_data == s.components().is_none_or(|c| c.as_nbt().is_empty())
                });
            if let Some((i, s)) = found {
                let _ = player.set_inventory_slot(&mut *lv.host, i, s.shrink(1));
                taken = true;
            }
        }
        if !taken {
            let mut white = TextStyle::default();
            white.color = Some("white".into());
            let mut red = TextStyle::default();
            red.color = Some("red".into());
            let name = Text::translate(
                format!("item.{}", kind.id(&*lv.host).replace(':', ".")),
                Vec::new(),
            )
            .with_style(white);
            let _ = player.action_bar(
                &mut *lv.host,
                Text::translate(
                    "create.logistics.filter.requires_item_in_inventory",
                    vec![name],
                )
                .with_style(red),
            );
            if let Some(at) = player.entity().position(&*lv.host) {
                let at = at.map(|v| v.floor() + 0.5);
                lv.level.play_sound(
                    &mut *lv.host,
                    "create:deny",
                    at,
                    mod_api::SoundSource::Blocks,
                    1.0,
                    1.0,
                );
            }
            return false;
        }
        if let Some(r) = refund.filter(|r| r.item() != kind) {
            items::give_back(lv.host, player, r);
        }
        return set_filter(lv, pos, block, slot, side, copied);
    }
    if let Some(r) = refund {
        items::give_back(lv.host, player, r);
    }
    set_filter(lv, pos, block, slot, side, copied)
}

/// `FilteringBehaviour.acceptsValueSettings` (`isCountVisible`) and its settings: a brass
/// funnel's or smart chute's amount while it shows.
fn filter_settings(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
) -> Option<create_engine::board::Settings> {
    // A shop's price amount, while it has a payment item.
    if crate::glue::stock::shop::is_cloth(block) {
        let dim = lv.dim;
        crate::glue::stock::shop::price_of(dim, pos)?;
        return Some(create_engine::board::Settings {
            row: 0,
            value: crate::glue::stock::shop::price_count(dim, pos)?,
        });
    }
    matches!(
        block,
        Block::BrassFunnel | Block::BrassBeltFunnel | Block::SmartChute
    )
    .then(|| crate::glue::logistics::amount::settings(lv, pos))
    .flatten()
}

/// `writeToClipboard` of each of the block's cloneable behaviours, under its key; None when
/// none wrote.
fn write(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    side: Dir,
) -> Option<Compound> {
    let (block, info) = block_at(lv, pos)?;
    let dim = lv.dim;
    let mut tag = Compound::new();
    for b in behaviours(block) {
        match b {
            // ValueSettingsBehaviour.writeToClipboard: its board's row and value.
            Behaviour::Value(key) => {
                if let Some(v) = values::settings_at(lv, player, pos, None, None) {
                    tag.insert(key.into(), row_value(values::board_pick(block, v)));
                }
            }
            Behaviour::Chassis => {
                let range = with_world(|w| w.dims[dim].chassis_range.get(&pos).copied())
                    .flatten()
                    .unwrap_or(create_engine::contraption::CHASSIS_RANGE);
                tag.insert(
                    "Chassis".into(),
                    row_value(create_engine::board::Settings {
                        row: 0,
                        value: range,
                    }),
                );
            }
            // BrassDiodeBlockEntity: `Inverted`; BrassDiodeScrollValueBehaviour (`Timings`).
            Behaviour::Diode => {
                tag.insert(
                    "Block".into(),
                    Tag::Compound(Compound::from([(
                        "Inverted".into(),
                        Tag::Byte(i8::from(info.inverted)),
                    )])),
                );
                if let Some(max) =
                    with_world(|w| w.dims[dim].diodes.get(&pos).map(|d| d.max)).flatten()
                {
                    tag.insert(
                        "Timings".into(),
                        row_value(create_engine::board::diode_settings(max)),
                    );
                }
            }
            Behaviour::Frequencies => {
                let key = with_world(|w| w.dims[dim].links.links.get(&pos).map(|l| l.key.clone()))
                    .flatten()
                    .unwrap_or_default();
                let host: &dyn mod_api::Host = &*lv.host;
                let first = super::frequency_stack(host, &key.0);
                let last = super::frequency_stack(host, &key.1);
                tag.insert(
                    "Frequencies".into(),
                    Tag::Compound(Compound::from([
                        ("First".into(), save(host, first.as_ref())),
                        ("Last".into(), save(host, last.as_ref())),
                    ])),
                );
            }
            // FilteringBehaviour: its value settings while they show, and its filter.
            Behaviour::Filtering(slot) => {
                let mut c = Compound::new();
                if let Some(s) = filter_settings(lv, pos, block)
                    && let Tag::Compound(v) = row_value(s)
                {
                    c = v;
                }
                let now = filter_now(lv, pos, slot, side);
                c.insert("Filter".into(), save(&*lv.host, now.as_ref()));
                tag.insert("Filtering".into(), Tag::Compound(c));
            }
        }
    }
    (!tag.is_empty()).then_some(tag)
}

/// `readFromClipboard` of each of the block's cloneable behaviours: true when any took its
/// settings.
fn read(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    side: Dir,
    tag: &Compound,
) -> bool {
    let Some((block, info)) = block_at(lv, pos) else {
        return false;
    };
    let dim = lv.dim;
    let mut any = false;
    for b in behaviours(block) {
        match b {
            // ValueSettingsBehaviour.readFromClipboard: `setValueSettings` with the row and value.
            Behaviour::Value(key) => {
                if let Some(pick) = sub(tag, key).and_then(settings_in) {
                    any |= values::settings_at(lv, player, pos, Some(pick), None).is_some();
                }
            }
            // ChassisScrollValueBehaviour: the whole group takes the range, 1 to 16.
            Behaviour::Chassis => {
                if let Some(pick) = sub(tag, "Chassis").and_then(settings_in) {
                    let group = crate::glue::contraptions::chassis_group(lv, pos);
                    let range = pick.value.clamp(1, 16);
                    with_world(|w| w.set_chassis_range(dim, &group, range));
                    any = true;
                }
            }
            Behaviour::Diode => {
                if let Some(Tag::Byte(b)) =
                    sub(tag, "Block").and_then(|c| c.get(&"Inverted".into()))
                {
                    any = true;
                    if (*b != 0) != info.inverted {
                        super::ticks::toggle_diode(lv, pos, info.inverted);
                    }
                }
                if let Some(pick) = sub(tag, "Timings").and_then(settings_in) {
                    any = true;
                    let delay = create_engine::board::diode_value(pick);
                    with_world(|w| w.set_diode_delay(dim, pos, delay));
                    super::ticks::show_diode(lv, pos);
                }
            }
            Behaviour::Frequencies => {
                let Some(f) = sub(tag, "Frequencies") else {
                    continue;
                };
                let (Some(first), Some(last)) = (sub(f, "First"), sub(f, "Last")) else {
                    continue;
                };
                let host: &dyn mod_api::Host = &*lv.host;
                let id = |c: &Compound| super::frequency_key(host, parse(host, c).as_ref());
                let (first, last) = (id(first), id(last));
                with_world(|w| {
                    w.link_frequency(dim, pos, true, &first);
                    w.link_frequency(dim, pos, false, &last);
                });
                any = true;
            }
            Behaviour::Filtering(slot) => {
                let Some(t) = sub(tag, "Filtering") else {
                    continue;
                };
                // `TableClothFilteringBehaviour.mayInteract`: a shop's owner only; its amount
                // set after the payment item.
                if slot == Slot::Price {
                    if !crate::glue::stock::shop::may_paste(lv.dim, pos, player.uuid()) {
                        continue;
                    }
                    if let Some(Tag::Compound(saved)) = t.get(&"Filter".into()) {
                        let copied = parse(&*lv.host, saved);
                        any |= paste_filter(lv, player, pos, block, slot, side, copied);
                    }
                    if let Some(pick) = settings_in(t) {
                        crate::glue::stock::shop::value_settings(
                            lv,
                            pos,
                            player,
                            Some(pick),
                            false,
                        );
                        any = true;
                    }
                    continue;
                }
                if filter_settings(lv, pos, block).is_some()
                    && let Some(pick) = settings_in(t)
                {
                    crate::glue::logistics::amount::value_settings(
                        lv,
                        pos,
                        player,
                        Some(pick),
                        false,
                    );
                    any = true;
                }
                let Some(Tag::Compound(saved)) = t.get(&"Filter".into()) else {
                    continue;
                };
                let copied = parse(&*lv.host, saved);
                any |= paste_filter(lv, player, pos, block, slot, side, copied);
            }
        }
    }
    any
}

/// `clipboard.copied_from` / `clipboard.pasted_to` with the block's name, in green.
fn tell(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], paste: bool) {
    let name = lv
        .block_state(pos)
        .and_then(|s| lv.state_name(s))
        .unwrap_or_default();
    let mut white = TextStyle::default();
    white.color = Some("white".into());
    let mut green = TextStyle::default();
    green.color = Some("green".into());
    let block =
        Text::translate(format!("block.{}", name.replace(':', ".")), Vec::new()).with_style(white);
    let key = if paste {
        "create.clipboard.pasted_to"
    } else {
        "create.clipboard.copied_from"
    };
    let _ = player.action_bar(
        &mut *lv.host,
        Text::translate(key, vec![block]).with_style(green),
    );
}

/// `interact` on a placed clipboard (`ClipboardBlockEntity`): the held clipboard takes the
/// entries it lacks, and Create's message.
fn copy_placed(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    hand: Hand,
    pos: [i32; 3],
    held: &ItemStack,
) {
    let from = crate::glue::decor::placed_clipboard(lv.dim, pos)
        .map(|s| clipboard::content(&s))
        .unwrap_or_default();
    let mut to = clipboard::content(held);
    create_engine::clipboard::copy_entries(&mut to, &from);
    let stack = clipboard::with_content(held, Some(&to));
    items::set_held(&mut *lv.host, player, hand, Some(stack));
    let mut white = TextStyle::default();
    white.color = Some("white".into());
    let mut green = TextStyle::default();
    green.color = Some("green".into());
    let name = Text::translate("block.create.clipboard", Vec::new()).with_style(white);
    let _ = player.action_bar(
        &mut *lv.host,
        Text::translate("create.clipboard.copied_from_clipboard", vec![name]).with_style(green),
    );
}

/// `rightClickToCopy`: a right click with a clipboard on such a block copies its settings into
/// the clipboard (on the server; the client only takes the click); on a placed clipboard, its
/// entries.
pub(super) fn right_click_block(mut host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(stack) = click.stack else {
        return Flow::Continue;
    };
    let (is_clipboard, sneaking) = match &host {
        HostSide::Server(h) => (
            stack.item().id(&**h) == "create:clipboard",
            h.player(click.player)
                .is_some_and(|p| items::sneaking(&**h, p)),
        ),
        HostSide::Client(h) => (
            stack.item().id(&**h) == "create:clipboard",
            h.local_player()
                .is_some_and(|p| p.entity().is_shift_key_down(&**h) == Some(true)),
        ),
    };
    if !is_clipboard || sneaking {
        return Flow::Continue;
    }
    let HostSide::Server(host) = host.reborrow() else {
        // The client: the same click on such a block is taken.
        let HostSide::Client(h) = host else {
            return Flow::Continue;
        };
        let ours = super::client_block_of(&*h, click.pos)
            .is_some_and(|b| cloneable(b) || b == Block::Clipboard);
        if ours {
            *event.cancellation_result = InteractionResult::Success;
            return Flow::Cancel;
        }
        return Flow::Continue;
    };
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    if player.is_spectator(&*host) == Some(true) {
        return Flow::Continue;
    }
    let dimension = click.dimension.key().id().to_owned();
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return Flow::Continue;
    };
    let Some((block, _)) = block_at(&lv, click.pos) else {
        return Flow::Continue;
    };
    if block == Block::Clipboard {
        copy_placed(&mut lv, player, click.hand, click.pos, stack);
        *event.cancellation_result = InteractionResult::Success;
        return Flow::Cancel;
    }
    if !cloneable(block) {
        return Flow::Continue;
    }
    let side = Dir::from_offset(click.hit.face).unwrap_or(Dir::Up);
    if let Some(tag) = write(&mut lv, player, click.pos, side) {
        // `ClipboardContent.setCopiedValues`: the copy replaces what it held.
        let stack = clipboard::with_copied_values(&*lv.host, stack, tag);
        items::set_held(&mut *lv.host, player, click.hand, Some(stack));
        tell(&mut lv, player, click.pos, false);
    }
    *event.cancellation_result = InteractionResult::Success;
    Flow::Cancel
}

/// `leftClickToPaste` on the client: an attack with a clipboard holding copied settings on such
/// a block (or with any clipboard on a placed one) goes to the server when pressed, and breaks
/// nothing.
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut InteractionKeyMappingTriggeredEvent,
) -> Flow {
    if event.key != InteractionKey::Attack {
        return Flow::Continue;
    }
    let Some(player) = host.local_player() else {
        return Flow::Continue;
    };
    if player.entity().is_shift_key_down(&*host) == Some(true) {
        return Flow::Continue;
    }
    let Some(stack) = items::held(&*host, player, Hand::Main) else {
        return Flow::Continue;
    };
    if stack.item().id(&*host) != "create:clipboard" {
        return Flow::Continue;
    }
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return Flow::Continue;
    };
    let pos = [pos.x, pos.y, pos.z];
    let Some(block) = super::client_block_of(&*host, pos) else {
        return Flow::Continue;
    };
    let placed = block == Block::Clipboard;
    if !placed && !(cloneable(block) && clipboard::copied_values(&stack).is_some()) {
        return Flow::Continue;
    }
    if event.attempt != InteractionAttempt::Press {
        return Flow::Cancel;
    }
    let mut data = Vec::new();
    for v in pos.into_iter().chain(hit.face) {
        data.extend_from_slice(&v.to_le_bytes());
    }
    crate::glue::net::send_to_server(host, CHANNEL, &data);
    Flow::Cancel
}

/// The paste a client asked for, from a player within reach of the block.
pub(super) fn client_message(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    channel: &str,
    data: &[u8],
) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let w: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    let Some(pos) = w.get(..3).map(|p| [p[0], p[1], p[2]]) else {
        return true;
    };
    let side = w
        .get(3..6)
        .and_then(|f| Dir::from_offset([f[0], f[1], f[2]]))
        .unwrap_or(Dir::Up);
    if items::sneaking(&*host, player) || player.is_spectator(&*host) == Some(true) {
        return true;
    }
    let near = player.entity().position(&*host).is_some_and(|at| {
        (0..3)
            .map(|i| (at[i] - (f64::from(pos[i]) + 0.5)).powi(2))
            .sum::<f64>()
            <= 20.0 * 20.0
    });
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return true;
    };
    let Some(stack) = items::held(&*host, player, Hand::Main) else {
        return true;
    };
    if !near || stack.item().id(&*host) != "create:clipboard" {
        return true;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return true;
    };
    let Some((block, _)) = block_at(&lv, pos) else {
        return true;
    };
    if block == Block::Clipboard {
        copy_placed(&mut lv, player, Hand::Main, pos, &stack);
        return true;
    }
    let Some(tag) = clipboard::copied_values(&stack) else {
        return true;
    };
    if !cloneable(block) {
        return true;
    }
    if read(&mut lv, player, pos, side, &tag) {
        tell(&mut lv, player, pos, true);
    }
    true
}

/// Whether `tag` holds what one of the cloneable behaviours of `block` reads
/// (`readFromClipboard`'s simulation).
fn pastes(block: Block, tag: &Compound) -> bool {
    let has = |key: &str, fields: &[&str]| {
        sub(tag, key).is_some_and(|c| fields.iter().all(|f| c.get(&(*f).into()).is_some()))
    };
    behaviours(block).into_iter().any(|b| match b {
        Behaviour::Value(key) => has(key, &["Value", "Row"]),
        Behaviour::Chassis => has("Chassis", &["Value", "Row"]),
        Behaviour::Diode => has("Block", &["Inverted"]) || has("Timings", &["Value", "Row"]),
        Behaviour::Frequencies => has("Frequencies", &["First", "Last"]),
        Behaviour::Filtering(_) => {
            has("Filtering", &["Filter"]) || has("Filtering", &["Value", "Row"])
        }
    })
}

/// `ClipboardValueSettingsHandler.clientTick`: with a clipboard in the main hand on a block it
/// can copy from or paste to, what a click does (`showHoverTip`), shown as the value boxes' tip
/// is; on a placed clipboard, that a click copies it.
pub(crate) fn hover_tip(host: &dyn ClientHost) -> Option<([i32; 3], Vec<String>)> {
    let player = host.local_player()?;
    let stack = items::held(host, player, Hand::Main)?;
    if stack.item().id(host) != "create:clipboard" {
        return None;
    }
    let Some(CrosshairTarget::Block { pos, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let block =
        super::client_block_of(host, pos).filter(|b| cloneable(*b) || *b == Block::Clipboard)?;
    let line = |key: &str, mapping: Option<&str>| {
        let args = mapping
            .map(|m| vec![Text::new(mod_api::text::TextContent::Keybind(m.into()))])
            .unwrap_or_default();
        host.text_string(&Text::translate(key, args))
            .unwrap_or_else(|_| key.to_owned())
    };
    if block == Block::Clipboard {
        return Some((
            pos,
            vec![
                line("create.clipboard.actions", None),
                line("create.clipboard.copy_other_clipboard", Some("key.use")),
            ],
        ));
    }
    let mut tip = vec![
        line("create.clipboard.actions", None),
        line("create.clipboard.to_copy", Some("key.use")),
    ];
    if clipboard::copied_values(&stack).is_some_and(|t| pastes(block, &t)) {
        tip.push(line("create.clipboard.to_paste", Some("key.attack")));
    }
    Some((pos, tip))
}
