//! The display link item (`DisplayLinkBlockItem`, `ClickToLinkBlockItem`): a click on any block
//! selects it as the target (`create.display_link.set`), a sneaking click clears the selection,
//! and the link placed with a selection writes there (`TargetOffset`) when within
//! `displayLinkRange` (64) of it. The selection is kept in the stack's custom data under
//! Create's component name (`create:click_to_link_data`).

use super::super::items;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use mod_api::{
    Compound, Hand, HostSide, InteractionResult, ItemPart, ItemStack, PlayerRef, Registrar,
    RightClickBlockEvent, Tag, TriState, UseOnEvent,
};
use std::cell::RefCell;

const DATA: &str = "create:click_to_link_data";
/// `AllConfigs.server().logistics.displayLinkRange`.
const RANGE: i64 = 64;

pub(super) fn register(r: &mut dyn Registrar) {
    let key = r.item_key("create:display_link");
    r.item_part(key, LinkItem);
}

/// The selection on a stack: position and dimension.
fn selection(stack: &ItemStack) -> Option<([i32; 3], String)> {
    let Some(Tag::Compound(c)) = items::custom(stack).get(&DATA.into()).cloned() else {
        return None;
    };
    let pos = match c.get(&"selected_pos".into()) {
        Some(Tag::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
        _ => return None,
    };
    let dim = match c.get(&"selected_dim".into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => return None,
    };
    Some((pos, dim))
}

fn with_selection(stack: &ItemStack, selected: Option<([i32; 3], &str)>) -> ItemStack {
    let mut stack = stack.clone();
    let value = selected.map(|(pos, dim)| {
        Tag::Compound(Compound::from([
            ("selected_pos".into(), Tag::IntArray(pos.to_vec())),
            ("selected_dim".into(), Tag::String(dim.into())),
        ]))
    });
    items::set_custom(&mut stack, &[(DATA, value)]);
    stack
}

/// A link placed with a selection, waiting for its block: the placing player, the level, where
/// it goes and its target.
struct Placing {
    player: u128,
    hand: Hand,
    dimension: String,
    /// The clicked block (taken when replaceable) and the one on its clicked face.
    at: [[i32; 3]; 2],
    target: [i32; 3],
}

thread_local! {
    static PLACING: RefCell<Vec<Placing>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn forget() {
    PLACING.with_borrow_mut(Vec::clear);
}

/// `ClickToLinkBlockItem.linkableItemAlwaysPlacesWhenUsed`: with the link item in hand, the
/// clicked block is not used (but a display link, whose screen opens).
pub(super) fn right_click_block(host: &HostSide, event: &mut RightClickBlockEvent<'_>) {
    let Some(stack) = event.click.stack else {
        return;
    };
    let id = match host {
        HostSide::Server(h) => stack.item().id(&**h),
        HostSide::Client(h) => stack.item().id(&**h),
    };
    if id != "create:display_link" {
        return;
    }
    let clicked = event.click.pos;
    let link = super::super::states::key(create_engine::Block::DisplayLink);
    let state = match host {
        HostSide::Server(h) => h
            .level(event.click.dimension.key().id())
            .and_then(|l| l.block_state(&**h, clicked)),
        HostSide::Client(h) => h
            .level(h.dimension())
            .and_then(|l| l.block_state(&**h, clicked)),
    };
    if state.is_some_and(|s| s.block() == link) {
        return;
    }
    *event.use_block = TriState::False;
}

struct LinkItem;

#[mod_api::horizon_part]
impl ItemPart for LinkItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => server_use_on(host, event),
            HostSide::Client(host) => {
                let player = host.local_player()?;
                let sneaking = player.entity().is_shift_key_down(&*host) == Some(true);
                match selection(event.stack) {
                    _ if sneaking && selection(event.stack).is_some() => {
                        Some(InteractionResult::Success)
                    }
                    None => Some(InteractionResult::Success),
                    Some((target, _)) => {
                        let placed = placed_pos(event);
                        (!within(target, placed)).then_some(InteractionResult::Fail)
                    }
                }
            }
        }
    }
}

/// `pos.relative(face, canBeReplaced ? 0 : 1)`: the game places on the clicked face's side
/// unless the clicked block is replaced; the placement hook finds out which.
fn placed_pos(event: &UseOnEvent<'_>) -> [i32; 3] {
    [
        event.pos[0] + event.face[0],
        event.pos[1] + event.face[1],
        event.pos[2] + event.face[2],
    ]
}

fn within(a: [i32; 3], b: [i32; 3]) -> bool {
    let d: i64 = (0..3).map(|i| i64::from(a[i] - b[i]).pow(2)).sum();
    d < RANGE * RANGE
}

fn server_use_on(
    host: &mut dyn mod_api::ServerHost,
    event: &mut UseOnEvent<'_>,
) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    let dimension = player.dimension(&*host)?.to_string();
    let sneaking = items::sneaking(&*host, player);
    let selected = selection(event.stack);
    if sneaking && selected.is_some() {
        items::action_bar(host, player, "create.display_link.clear");
        let cleared = with_selection(event.stack, None);
        items::set_held(host, player, event.hand, Some(cleared));
        return Some(InteractionResult::SuccessServer);
    }
    let Some((target, target_dim)) = selected else {
        // `isValidTarget`: any block.
        items::action_bar(host, player, "create.display_link.set");
        let set = with_selection(event.stack, Some((event.pos, &dimension)));
        items::set_held(host, player, event.hand, Some(set));
        return Some(InteractionResult::SuccessServer);
    };
    let placed = placed_pos(event);
    if !within(target, placed) || target_dim != dimension {
        too_far(host, player);
        return Some(InteractionResult::Fail);
    }
    PLACING.with_borrow_mut(|p| {
        p.push(Placing {
            player: event.player,
            hand: event.hand,
            dimension,
            at: [event.pos, placed],
            target,
        })
    });
    None
}

fn too_far(host: &mut dyn mod_api::ServerHost, player: PlayerRef) {
    let text = coloured("create.display_link.too_far", "red");
    let _ = player.action_bar(host, text);
}

/// The display link block was placed where its item said (`useOn` writing `TargetOffset` into
/// the placed block entity): the link at `at` (the clicked block when it was replaced) takes its
/// target; the stack's selection goes and the player is told (`.success`).
pub(super) fn take_placed(lv: &mut ServerLevel<'_>) {
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
    let dim = lv.dim;
    for p in mine {
        let at = p.at.into_iter().find(|q| super::link_at(lv, *q).is_some());
        let Some(at) = at else {
            continue;
        };
        let bound = with_world(|w| {
            let l = w.dims[dim].displays.links.get_mut(&at)?;
            l.target_offset = [0, 1, 2].map(|i| p.target[i] - at[i]);
            l.last = None;
            Some(())
        })
        .flatten();
        if bound.is_none() {
            continue;
        }
        super::mark(lv, at);
        let Some(player) = lv.host.player(p.player) else {
            continue;
        };
        if let Some(stack) = items::held(&*lv.host, player, p.hand)
            && selection(&stack).is_some()
        {
            items::set_held(lv.host, player, p.hand, Some(with_selection(&stack, None)));
        }
        let text = coloured("create.display_link.success", "green");
        let _ = player.action_bar(lv.host, text);
    }
}

/// `CreateLang.translateDirect(key).withStyle(colour)`.
fn coloured(key: &str, colour: &str) -> mod_api::Text {
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some(colour.to_owned());
    mod_api::Text::translate(key, Vec::new()).with_style(style)
}
