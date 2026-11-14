//! The super glue item (`SuperGlueSelectionHandler`, `SuperGlueSelectionPacket`,
//! `SuperGlueRemovalPacket`): a right click on a block picks the first corner, a second click
//! glues the area between them when the blocks there hold together (one charge of the glue
//! outside creative), sneaking forgets the first corner, and a left click with the glue in hand
//! takes the glue off the block hit. The areas are kept and saved as `actors` keeps them.
//!
//! Create picks the corners on its client and draws the selection and glue outlines there;
//! here the clicks are taken on the server (the corner a player picked is the server's), and
//! nothing is drawn: the client is not told of the glue areas.

use super::super::items::{action_bar, creative, held, sneaking};
use super::super::persist::mark_unsaved;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::{center, found};
use create_engine::glue;
use mod_api::{
    EquipmentSlot, Hand, HostSide, InteractionResult, ItemPart, LeftClickBlockEvent, PlayerRef,
    Registrar, ServerHost, SoundSource, UseOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub const ITEM: &str = "create:super_glue";

thread_local! {
    /// Each player's first corner (`firstPos`), by player.
    static FIRST: RefCell<HashMap<u128, [i32; 3]>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    FIRST.with_borrow_mut(HashMap::clear);
}

pub(super) fn register(r: &mut dyn Registrar) {
    let item = r.item_key(ITEM);
    r.item_part(item, GlueItem);
}

struct GlueItem;

#[mod_api::horizon_part]
impl ItemPart for GlueItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        // `onMouseInput` reads the main hand only.
        if event.hand != Hand::Main {
            return None;
        }
        match host {
            HostSide::Server(host) => use_on(host, event),
            HostSide::Client(_) => Some(InteractionResult::Success),
        }
    }
}

fn is_glue(host: &dyn ServerHost, player: PlayerRef) -> bool {
    held(host, player, Hand::Main).is_some_and(|s| s.item().id(host) == ITEM)
}

fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    if player.may_build(&*host) != Some(true) {
        return None;
    }
    let id = event.player;
    let first = FIRST.with_borrow(|f| f.get(&id).copied());
    if sneaking(&*host, player) {
        // `discard`: sneaking forgets the first corner.
        first?;
        FIRST.with_borrow_mut(|f| f.remove(&id));
        action_bar(host, player, "create.super_glue.abort");
        return Some(InteractionResult::Success);
    }
    let pos = event.pos;
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let Some(from) = first else {
        FIRST.with_borrow_mut(|f| f.insert(id, pos));
        action_bar(lv.host, player, "create.super_glue.first_pos");
        slime_sound(&mut lv, pos, 0.85);
        return Some(InteractionResult::Success);
    };
    // `confirm`, then `SuperGlueSelectionPacket.handle`: the corners must hold together.
    let placed = glue::place(from, pos, |p| {
        found(&lv, p).map(|_| super::actors::sticky_sides(&lv, p))
    });
    let Ok(g) = placed else {
        let key = match placed {
            Err(glue::Refused::TooFar) => "create.super_glue.too_far",
            _ => "create.super_glue.cannot_reach",
        };
        action_bar(lv.host, player, key);
        return Some(InteractionResult::Success);
    };
    let dim = lv.dim;
    with_world(|w| w.dims[dim].glue.push(g));
    let (cx, cz) = g.column();
    mark_unsaved(dim, [cx, cz]);
    if !creative(&*lv.host, player)
        && let Some(living) = lv.host.living(player.entity())
    {
        living.hurt_and_break(lv.host, EquipmentSlot::MainHand, 1);
    }
    FIRST.with_borrow_mut(|f| f.remove(&id));
    action_bar(lv.host, player, "create.super_glue.success");
    slime_sound(&mut lv, pos, 0.95);
    Some(InteractionResult::Success)
}

/// `AllSoundEvents.SLIME_ADDED` and the item frame's click, as the selection plays them.
fn slime_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], pitch: f32) {
    for (sound, volume, pitch) in [
        ("create:slime_added", 0.5, pitch),
        ("minecraft:entity.item_frame.add_item", 0.75, 1.0),
    ] {
        lv.level.play_sound(
            &mut *lv.host,
            sound,
            center(pos),
            SoundSource::Blocks,
            volume,
            pitch,
        );
    }
}

/// `SuperGlueRemovalPacket`: a left click with the glue in hand takes off the glue holding the
/// block hit.
pub(super) fn left_click(host: &mut dyn ServerHost, event: &LeftClickBlockEvent) -> bool {
    let Some(player) = host.players().acting_player() else {
        return false;
    };
    if !is_glue(&*host, player) || player.may_build(&*host) != Some(true) {
        return false;
    }
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return false;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let dim = lv.dim;
    let pos = event.pos;
    let removed = with_world(|w| {
        let areas = &mut w.dims[dim].glue;
        let i = areas.iter().position(|g| g.contains(pos))?;
        Some(areas.remove(i))
    })
    .flatten();
    let Some(g) = removed else {
        return false;
    };
    let (cx, cz) = g.column();
    mark_unsaved(dim, [cx, cz]);
    lv.level.play_sound(
        &mut *lv.host,
        "create:slime_added",
        center(pos),
        SoundSource::Blocks,
        0.5,
        0.5,
    );
    true
}
