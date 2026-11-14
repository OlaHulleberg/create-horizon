//! Create's handheld items (`create-engine` holds the rules: `wrench`, `nugget`, `symmetry`,
//! `worldshaper`, `belt_connector`, `filter`): each is an `ItemPart` on its item, and the
//! moments that are not one item's (a placement or a break the wand of symmetry copies, a blow
//! with the cardboard sword) are the subsystem's global hooks.
//!
//! The generic plumbing every subsystem's items share lives here, `pub(crate)`: Create's item
//! keys (`key`, `is`), the stack in a player's hand (`held`, `set_held`, `shrink_held`), a stack
//! given back (`give_back`), sneaking, and a stack's `minecraft:custom_data` under Create's
//! component names (`custom`, `set_custom`: Horizon adds no component types).
//!
//! Left out (no API for it):
//! - The wand of symmetry's mirrored placements take the placed state turned, not
//!   `Block.updateFromNeighbourShapes` of it (the legacy's `update_from_neighbour_shapes`): no
//!   host call asks a state's shape against its neighbours.
//! - The items' 5-tick cooldowns (`getCooldowns().addCooldown`) and the belt connector's
//!   advancement: as in the legacy glue.
//! - Super glue's selection (`SuperGlueSelectionHandler`) is the contraptions' to port: what it
//!   makes (`SuperGlueEntity` areas) is found and saved with contraptions (`contraption_found`,
//!   `sticky_sides`, `create_glue`). The train schedule (`schedule`,
//!   `schedule_screen`) hands itself to a train through the trains' seats.

mod belt_connector;
mod fertilizer;
mod filter_screen;
// The filter slots' helpers, for the subsystems whose blocks hold one.
pub(crate) mod filters;
mod nugget;
mod sandpaper;
mod schedule;
mod schedule_screen;
mod sequenced;
mod sword;
mod symmetry;
mod worldshaper;
mod wrench;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use mod_api::{
    AttackEntityEvent, BreakBlockEvent, ClientHost, Compound, EntityPlaceEvent, Flow, Hand, Host,
    HostSide, ItemKey, ItemStack, LeftClickBlockEvent, PlayerRef, RegisterCommandsEvent, Registrar,
    RegistrationContext, RunModCommandEvent, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, Tag, Text,
};
use std::sync::OnceLock;

/// Create's items the glue asks about, by `Item as usize`.
const IDS: [&str; 9] = [
    "create:wrench",
    "create:goggles",
    "create:clipboard",
    "create:experience_nugget",
    create_engine::symmetry::WAND,
    create_engine::worldshaper::WORLDSHAPER,
    create_engine::belt_connector::ITEM,
    "create:cardboard_sword",
    "create:tree_fertilizer",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Wrench,
    Goggles,
    Clipboard,
    ExperienceNugget,
    WandOfSymmetry,
    Worldshaper,
    BeltConnector,
    CardboardSword,
    TreeFertilizer,
}

static KEYS: OnceLock<Vec<ItemKey>> = OnceLock::new();

/// The key of `item`, once registration asked for it.
pub(crate) fn key(item: Item) -> Option<ItemKey> {
    KEYS.get().map(|k| k[item as usize])
}

/// Whether `stack` is one of `item`. Hot: one compare.
pub(crate) fn is(stack: Option<&ItemStack>, item: Item) -> bool {
    stack.is_some_and(|s| Some(s.item()) == key(item))
}

/// The stack `player` holds in `hand`.
pub(crate) fn held(host: &dyn Host, player: PlayerRef, hand: Hand) -> Option<ItemStack> {
    host.living(player.entity())?.hand_item(host, hand)
}

/// Puts `stack` in `player`'s `hand` (None empties it).
pub(crate) fn set_held(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    hand: Hand,
    stack: Option<ItemStack>,
) {
    if let Some(living) = host.living(player.entity()) {
        let _ = living.set_hand_item(host, hand, stack);
    }
}

/// `shrink(n)` of the stack in `player`'s `hand`.
pub(crate) fn shrink_held(host: &mut dyn ServerHost, player: PlayerRef, hand: Hand, n: u32) {
    if let Some(stack) = held(&*host, player, hand) {
        set_held(host, player, hand, stack.shrink(n));
    }
}

/// `Inventory.placeItemBackInInventory`: into the inventory, the rest dropped at the player.
pub(crate) fn give_back(host: &mut dyn ServerHost, player: PlayerRef, stack: ItemStack) {
    let _ = player.give_stack(host, stack);
}

/// `Player.isShiftKeyDown`.
pub(crate) fn sneaking(host: &dyn Host, player: PlayerRef) -> bool {
    player.entity().is_shift_key_down(host) == Some(true)
}

/// `Player.isCreative`.
pub(crate) fn creative(host: &dyn Host, player: PlayerRef) -> bool {
    player.is_creative(host) == Some(true)
}

/// `player.displayClientMessage(Component.translatable(key), true)`.
pub(crate) fn action_bar(host: &mut dyn ServerHost, player: PlayerRef, key: &str) {
    let _ = player.action_bar(host, Text::translate(key, Vec::new()));
}

/// `minecraft:custom_data`, where Create's components are kept.
pub(crate) struct CustomData;

impl mod_api::Component for CustomData {
    type Value = Compound;
    fn id(&self) -> &str {
        "minecraft:custom_data"
    }
    fn decode(&self, value: &Tag) -> Option<Compound> {
        match value {
            Tag::Compound(c) => Some(c.clone()),
            _ => None,
        }
    }
    fn encode(&self, value: Compound) -> Tag {
        Tag::Compound(value)
    }
}

/// The stack's custom data (empty when it has none).
pub(crate) fn custom(stack: &ItemStack) -> Compound {
    stack.get(&CustomData).unwrap_or_default()
}

/// Sets (or, for None, removes) entries of the stack's custom data; the component goes when
/// it is left empty.
pub(crate) fn set_custom(stack: &mut ItemStack, entries: &[(&str, Option<Tag>)]) {
    let mut c = custom(stack);
    for (key, value) in entries {
        match value {
            Some(v) => {
                c.insert((*key).into(), v.clone());
            }
            None => {
                c.remove(&(*key).into());
            }
        }
    }
    if c.is_empty() {
        stack.remove(&CustomData);
    } else {
        let _ = stack.set(&CustomData, c);
    }
}

pub struct Items;

impl Subsystem for Items {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let keys: Vec<ItemKey> = IDS.iter().map(|id| r.item_key(id)).collect();
        let _ = KEYS.set(keys);
        let Some(k) = KEYS.get() else {
            return;
        };
        r.item_part(k[Item::Wrench as usize], wrench::Wrench);
        r.item_part(k[Item::ExperienceNugget as usize], nugget::Nugget);
        r.item_part(k[Item::WandOfSymmetry as usize], symmetry::WandPart);
        r.item_part(k[Item::Worldshaper as usize], worldshaper::Shaper);
        r.item_part(k[Item::BeltConnector as usize], belt_connector::Connector);
        r.item_part(k[Item::TreeFertilizer as usize], fertilizer::Fertilizer);
        for id in [
            create_engine::filter::FILTER,
            create_engine::filter::ATTRIBUTE_FILTER,
        ] {
            let key = r.item_key(id);
            r.item_part(key, filters::FilterPart);
        }
        let key = r.item_key(schedule::ITEM);
        r.item_part(key, schedule::SchedulePart);
        for id in sandpaper::IDS {
            let key = r.item_key(id);
            r.item_part(key, sandpaper::SandPaper);
        }
        for id in sequenced::IDS {
            let key = r.item_key(id);
            r.item_part(key, sequenced::Transitional);
        }
    }

    /// `SymmetryHandler.onBlockPlaced`.
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        let _ = symmetry::placed(host, event);
        Flow::Continue
    }

    /// `SymmetryHandler.onBlockDestroyed`.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        let _ = symmetry::broken(host, event);
        Flow::Continue
    }

    /// The worldshaper's pick (`leftClickingBlocksWithTheZapperSelectsTheBlock`) and a
    /// cardboard sword's bonk.
    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        if worldshaper::left_click(host) {
            return Flow::Cancel;
        }
        sword::left_click(host, event);
        Flow::Continue
    }

    /// `CardboardSwordItem.cardboardSwordsCannotHurtYou`.
    fn attack_entity(&self, host: &mut dyn ServerHost, event: &mut AttackEntityEvent<'_>) -> Flow {
        sword::attack(host, event)
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        filters::level_tick(lv);
    }

    /// `SandPaperItem.finishUsingItem`.
    fn use_item_finished(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::LivingEntityUseItemFinishEvent<'_>,
    ) {
        sandpaper::finished(host, event);
    }

    /// What a filter's screen chose.
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            filter_screen::CHANNEL => filters::client_message(host, player, data),
            schedule_screen::CHANNEL => schedule::client_message(host, player, data),
            _ => return false,
        }
        true
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        filter_screen::server_message(host, channel, data)
            || schedule_screen::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        filter_screen::tick(host);
        schedule_screen::tick(host);
    }

    fn client_reset(&self) {
        filter_screen::reset();
        schedule_screen::reset();
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        filter_screen::screen_render_post(host, event);
        schedule_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        match schedule_screen::screen_mouse_pressed(host, event) {
            Flow::Continue => filter_screen::screen_mouse_pressed(host, event),
            flow => flow,
        }
    }

    fn screen_mouse_released(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        match schedule_screen::screen_mouse_released(host, event) {
            Flow::Continue => filter_screen::screen_mouse_released(host, event),
            flow => flow,
        }
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        match schedule_screen::screen_mouse_scrolled(host, event) {
            Flow::Continue => filter_screen::screen_mouse_scrolled(host, event),
            flow => flow,
        }
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        match schedule_screen::screen_key_pressed(host, event) {
            Flow::Continue => filter_screen::screen_key_pressed(host, event),
            flow => flow,
        }
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        filter_screen::screen_closing(host, event);
        schedule_screen::screen_closing(host, event);
    }

    fn screen_char_typed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        schedule_screen::screen_char_typed(host, event)
    }

    /// A schedule handed to a conductor, or taken back from one.
    fn entity_interact(
        &self,
        host: HostSide,
        event: &mut mod_api::EntityInteractEvent<'_>,
    ) -> Option<mod_api::InteractionResult> {
        schedule::entity_interact(host, event)
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        filters::register_commands(event.commands);
        symmetry::register_commands(event.commands);
        worldshaper::register_commands(event.commands);
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let command = event.command;
        let line = match command.literals.get(1).map(String::as_str) {
            Some("filter") if command.literals[0] == "create_debug" => {
                filters::command(host, event)
            }
            Some("symmetry") if command.literals[0] == "create_debug" => {
                symmetry::command(host, event)
            }
            Some("worldshaper") if command.literals[0] == "create_debug" => {
                worldshaper::command(host, event)
            }
            _ => return Flow::Continue,
        };
        let _ = host.commands().feedback(Text::literal(line));
        Flow::Cancel
    }
}

/// The slot a debug command names: `hand`, `offhand` or an inventory slot (0 to 35), as the
/// legacy commands take it, and the stack there.
pub(crate) fn slot_stack(
    host: &dyn Host,
    player: PlayerRef,
    word: &str,
) -> Option<(Slot, Option<ItemStack>)> {
    match word {
        "hand" => Some((Slot::Hand(Hand::Main), held(host, player, Hand::Main))),
        "offhand" => Some((Slot::Hand(Hand::Off), held(host, player, Hand::Off))),
        n => {
            let i = n.parse::<u32>().ok().filter(|i| *i < 36)?;
            let stack = player
                .inventory_slots(host)
                .unwrap_or_default()
                .into_iter()
                .find(|(slot, _)| *slot == i)
                .map(|(_, s)| s);
            Some((Slot::Inventory(i), stack))
        }
    }
}

/// Where a debug command's stack is.
#[derive(Clone, Copy)]
pub(crate) enum Slot {
    Hand(Hand),
    Inventory(u32),
}

/// Puts `stack` back where `slot_stack` found it.
pub(crate) fn set_slot(host: &mut dyn ServerHost, player: PlayerRef, slot: Slot, stack: ItemStack) {
    match slot {
        Slot::Hand(hand) => set_held(host, player, hand, Some(stack)),
        Slot::Inventory(i) => {
            let _ = player.set_inventory_slot(host, i, Some(stack));
        }
    }
}
