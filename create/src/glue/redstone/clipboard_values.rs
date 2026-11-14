//! The clipboard's copying and pasting of block settings (`ClipboardValueSettingsHandler`) on
//! the blocks whose behaviours are `ClipboardCloneable`: a brass diode's `Block` (inverted) and
//! `Timings` (its delay's board row and value), a redstone link's `Frequencies`, the `Filtering`
//! (its filter) of a smart observer, threshold switch, brass funnel or smart chute, and the
//! value boxes served by `interaction::values` (each `ValueSettingsBehaviour`'s board row and
//! value under its key: a motor's `Speed`, an arm's or tunnel's `SelectionMode`, the movement
//! modes, an ejector's or valve handle's `Settings`...). A right click with a clipboard copies
//! them into its `copied_values`; a left click pastes them. Not while sneaking. Looking at such
//! a block with a clipboard in hand shows what a click does over the hotbar (`clientTick`'s
//! `showHoverTip`).
//!
//! The server sees the right click (`right_click_block`); a left click has no player on the
//! server's hook, so the client takes the attack on such a block (`interaction_key`) and asks
//! the server on `CHANNEL`, which checks reach as `BlockEntityConfigurationPacket`s do.
//!
//! Left out: the cloneable behaviours of boxes not served by `interaction::values` (a brass
//! funnel's or chute's filter count, a chassis' range, a tunnel's side filters, a deployer's
//! filter), and copying from a placed clipboard.

use super::block_at;
use crate::glue::display::clipboard;
use crate::glue::interaction::values;
use crate::glue::items::{self, filters};
use crate::glue::persist;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, Compound, CrosshairTarget, Flow, Hand, HostSide, InteractionAttempt,
    InteractionKey, InteractionKeyMappingTriggeredEvent, InteractionResult, ItemStack, PlayerRef,
    RightClickBlockEvent, ServerHost, Tag, Text,
};

/// Client to server: paste the clipboard in hand onto a block (position, face).
pub(super) const CHANNEL: &str = "create:clipboard_paste";

/// Whether `block` has settings the clipboard copies.
fn cloneable(block: Block) -> bool {
    create_engine::world::diode_kind(block).is_some()
        || detector(block)
        || chute_filtered(block)
        || values::clipboard_key(block).is_some()
        || block == Block::RedstoneLink
}

/// A smart observer's or threshold switch's filter (`detectors`).
fn detector(block: Block) -> bool {
    matches!(block, Block::ContentObserver | Block::StockpileSwitch)
}

/// A filter kept with the chutes' (`chute_filters`): a brass funnel's or smart chute's.
fn chute_filtered(block: Block) -> bool {
    matches!(
        block,
        Block::BrassFunnel | Block::BrassBeltFunnel | Block::SmartChute
    )
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

/// `ItemStack.saveOptional` of an item id ("" for none).
fn stack_tag(lv: &ServerLevel<'_>, item: &str) -> Tag {
    let stack = mod_api::Identifier::parse(item)
        .ok()
        .and_then(|i| lv.host.item_key(&i))
        .map(ItemStack::of);
    Tag::Compound(stack.and_then(|s| s.to_nbt(&*lv.host)).unwrap_or_default())
}

/// The item id `stack_tag` wrote ("" for none).
fn stack_item(tag: &Compound) -> String {
    match tag.get(&"id".into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

/// `writeToClipboard` of the block's cloneable behaviours, each under its key.
fn write(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> Option<Compound> {
    let (block, info) = block_at(lv, pos)?;
    let dim = lv.dim;
    let mut tag = Compound::new();
    if create_engine::world::diode_kind(block).is_none()
        && let Some(key) = values::clipboard_key(block)
    {
        // ValueSettingsBehaviour.writeToClipboard: its board's row and value.
        let v = values::settings_at(lv, player, pos, None, None)?;
        let s = values::board_pick(block, v);
        tag.insert(
            key.into(),
            Tag::Compound(Compound::from([
                ("Value".into(), Tag::Int(s.value)),
                ("Row".into(), Tag::Int(s.row)),
            ])),
        );
        return Some(tag);
    }
    if create_engine::world::diode_kind(block).is_some() {
        // BrassDiodeBlockEntity: `Inverted`; BrassDiodeScrollValueBehaviour (`Timings`): the
        // board's row and value of its delay.
        let max = with_world(|w| w.dims[dim].diodes.get(&pos).map(|d| d.max)).flatten()?;
        let s = create_engine::board::diode_settings(max);
        tag.insert(
            "Block".into(),
            Tag::Compound(Compound::from([(
                "Inverted".into(),
                Tag::Byte(i8::from(info.inverted)),
            )])),
        );
        tag.insert(
            "Timings".into(),
            Tag::Compound(Compound::from([
                ("Value".into(), Tag::Int(s.value)),
                ("Row".into(), Tag::Int(s.row)),
            ])),
        );
    } else if block == Block::RedstoneLink {
        let key = with_world(|w| w.dims[dim].links.links.get(&pos).map(|l| l.key.clone()))
            .flatten()
            .unwrap_or_default();
        tag.insert(
            "Frequencies".into(),
            Tag::Compound(Compound::from([
                ("First".into(), stack_tag(lv, &key.0)),
                ("Last".into(), stack_tag(lv, &key.1)),
            ])),
        );
    } else {
        // FilteringBehaviour: no value settings on a detector, its filter.
        let filter = if chute_filtered(block) {
            with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten()
        } else {
            with_world(|w| w.dims[dim].detectors.filter(pos).cloned()).flatten()
        };
        let saved = filter
            .and_then(|f| filters::filter_to_nbt(&*lv.host, &f))
            .unwrap_or(Tag::Compound(Compound::new()));
        tag.insert(
            "Filtering".into(),
            Tag::Compound(Compound::from([("Filter".into(), saved)])),
        );
    }
    Some(tag)
}

/// `readFromClipboard` of the block's cloneable behaviours: true when any took its settings.
fn read(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], tag: &Compound) -> bool {
    let Some((block, info)) = block_at(lv, pos) else {
        return false;
    };
    let dim = lv.dim;
    if create_engine::world::diode_kind(block).is_none()
        && let Some(key) = values::clipboard_key(block)
    {
        // ValueSettingsBehaviour.readFromClipboard: `setValueSettings` with the row and value.
        let Some(t) = sub(tag, key) else {
            return false;
        };
        let (Some(value), Some(row)) = (int(t, "Value"), int(t, "Row")) else {
            return false;
        };
        let pick = create_engine::board::Settings { row, value };
        return values::settings_at(lv, player, pos, Some(pick), None).is_some();
    }
    if create_engine::world::diode_kind(block).is_some() {
        let mut any = false;
        if let Some(Tag::Byte(b)) = sub(tag, "Block").and_then(|c| c.get(&"Inverted".into())) {
            any = true;
            if (*b != 0) != info.inverted {
                super::ticks::toggle_diode(lv, pos, info.inverted);
            }
        }
        if let Some(t) = sub(tag, "Timings")
            && let (Some(value), Some(row)) = (int(t, "Value"), int(t, "Row"))
        {
            any = true;
            let delay =
                create_engine::board::diode_value(create_engine::board::Settings { row, value });
            with_world(|w| w.set_diode_delay(dim, pos, delay));
            super::ticks::show_diode(lv, pos);
        }
        return any;
    }
    if block == Block::RedstoneLink {
        let Some(f) = sub(tag, "Frequencies") else {
            return false;
        };
        let (Some(first), Some(last)) = (sub(f, "First"), sub(f, "Last")) else {
            return false;
        };
        let (first, last) = (stack_item(first), stack_item(last));
        with_world(|w| {
            w.link_frequency(dim, pos, true, &first);
            w.link_frequency(dim, pos, false, &last);
        });
        return true;
    }
    // FilteringBehaviour.readFromClipboard: a filter item comes out of the player's inventory
    // outside creative (the one the slot held goes back).
    let Some(saved) = sub(tag, "Filtering").and_then(|c| c.get(&"Filter".into())) else {
        return false;
    };
    let filter = filters::filter_from_nbt(&*lv.host, saved);
    if let Some(f) = filter.as_ref().filter(|f| f.is_filter_item())
        && player.game_mode(&*lv.host) != Some(mod_api::GameMode::Creative)
    {
        let Some(stack) = filters::filter_stack(&*lv.host, f) else {
            return false;
        };
        let item = stack.item();
        let slot = player
            .inventory_slots(&*lv.host)
            .unwrap_or_default()
            .into_iter()
            .find(|(_, s)| s.item() == item);
        let Some((slot, s)) = slot else {
            let mut style = TextStyle::default();
            style.color = Some("white".into());
            let name = Text::translate(
                format!("item.{}", item.id(&*lv.host).replace(':', ".")),
                Vec::new(),
            )
            .with_style(style);
            let _ = player.action_bar(
                &mut *lv.host,
                Text::translate(
                    "create.logistics.filter.requires_item_in_inventory",
                    vec![name],
                ),
            );
            return false;
        };
        let _ = player.set_inventory_slot(&mut *lv.host, slot, s.shrink(1));
    }
    if chute_filtered(block) {
        let shown = filter.clone();
        let old = with_world(|w| {
            let d = &mut w.dims[dim];
            let old = match filter {
                Some(f) => d.chute_filters.insert(pos, f),
                None => d.chute_filters.remove(&pos),
            };
            // A funnel looks at its inventory again (`invVersionTracker.reset`).
            if let Some(due) = d.funnels.get_mut(&pos) {
                *due = 0;
                d.funnel_timers.at(0, pos);
            }
            old
        })
        .flatten();
        persist::mark_pos_unsaved(dim, pos);
        crate::glue::logistics::show_filter(lv, pos, shown.as_ref());
        filters::return_filter(lv, player, pos, old);
        return true;
    }
    let old = with_world(|w| w.dims[dim].detectors.set_filter(pos, filter));
    let old = match old {
        Some(Ok(old)) => {
            persist::mark_pos_unsaved(dim, pos);
            old
        }
        _ => None,
    };
    filters::return_filter(lv, player, pos, old);
    true
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

/// `rightClickToCopy`: a right click with a clipboard on such a block copies its settings into
/// the clipboard (on the server; the client only takes the click).
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
        let ours = super::client_block_of(&*h, click.pos).is_some_and(cloneable);
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
    if !block_at(&lv, click.pos).is_some_and(|(b, _)| cloneable(b)) {
        return Flow::Continue;
    }
    if let Some(tag) = write(&mut lv, player, click.pos) {
        let stack = clipboard::with_copied_values(&*lv.host, stack, tag);
        items::set_held(&mut *lv.host, player, click.hand, Some(stack));
        tell(&mut lv, player, click.pos, false);
    }
    *event.cancellation_result = InteractionResult::Success;
    Flow::Cancel
}

/// `leftClickToPaste` on the client: an attack with a clipboard holding copied settings on such
/// a block goes to the server when pressed, and breaks nothing.
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
    if stack.item().id(&*host) != "create:clipboard" || clipboard::copied_values(&stack).is_none() {
        return Flow::Continue;
    }
    let Some(CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return Flow::Continue;
    };
    let pos = [pos.x, pos.y, pos.z];
    if !super::client_block_of(&*host, pos).is_some_and(cloneable) {
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
    let Some(tag) = clipboard::copied_values(&stack) else {
        return true;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return true;
    };
    if !block_at(&lv, pos).is_some_and(|(b, _)| cloneable(b)) {
        return true;
    }
    if read(&mut lv, player, pos, &tag) {
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
    if create_engine::world::diode_kind(block).is_some() {
        return has("Block", &["Inverted"]) || has("Timings", &["Value", "Row"]);
    }
    if let Some(key) = values::clipboard_key(block) {
        return has(key, &["Value", "Row"]);
    }
    if block == Block::RedstoneLink {
        return has("Frequencies", &["First", "Last"]);
    }
    (detector(block) || chute_filtered(block)) && has("Filtering", &["Filter"])
}

/// `ClipboardValueSettingsHandler.clientTick`: with a clipboard in the main hand on a block it
/// can copy from or paste to, what a click does (`showHoverTip`), shown as the value boxes' tip
/// is.
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
    let block = super::client_block_of(host, pos).filter(|b| cloneable(*b))?;
    let line = |key: &str, mapping: Option<&str>| {
        let args = mapping
            .map(|m| vec![Text::new(mod_api::text::TextContent::Keybind(m.into()))])
            .unwrap_or_default();
        host.text_string(&Text::translate(key, args))
            .unwrap_or_else(|_| key.to_owned())
    };
    let mut tip = vec![
        line("create.clipboard.actions", None),
        line("create.clipboard.to_copy", Some("key.use")),
    ];
    if clipboard::copied_values(&stack).is_some_and(|t| pastes(block, &t)) {
        tip.push(line("create.clipboard.to_paste", Some("key.attack")));
    }
    Some((pos, tip))
}
