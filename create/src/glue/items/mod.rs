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
//! The wand of symmetry's and the worldshaper's screens (`symmetry_screen`,
//! `worldshaper_screen`, drawn with `gadget_gui`'s widgets) are the client's own screens, opened
//! by a sneaking use; their settings come back on their packets' channels.
//!
//! Left out (no API for it):
//! - The wand of symmetry's mirrored placements take the placed state turned, not
//!   `Block.updateFromNeighbourShapes` of it: no host call asks a state's shape
//!   against its neighbours.
//! - Super glue's selection (`SuperGlueSelectionHandler`) is the contraptions' to port: what it
//!   makes (`SuperGlueEntity` areas) is found and saved with contraptions (`contraption_found`,
//!   `sticky_sides`, `create_glue`). The train schedule and its screen are the trains'
//!   (`trains::schedule_screen`, `trains::conductor`).

mod belt_connector;
pub(crate) use belt_connector::create_belts;
pub(crate) mod controller_screen;
mod fertilizer;
pub(crate) mod filter_screen;
pub(crate) mod gadget_gui;
// The filter slots' helpers, for the subsystems whose blocks hold one.
pub(crate) mod filters;
mod nugget;
// The potato cannon: a subsystem of its own (its projectiles' flight and hits).
pub(crate) mod potato;
mod sandpaper;
mod sequenced;
mod sword;
mod symmetry;
pub(crate) use symmetry::obstructed;
mod symmetry_screen;
mod tooltip;
mod worldshaper;
mod worldshaper_screen;
pub(crate) mod wrench;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use mod_api::{
    AttackEntityEvent, BreakBlockEvent, ClientHost, Compound, EntityPlaceEvent, Flow, Hand, Host,
    ItemKey, ItemStack, LeftClickBlockEvent, PlayerRef, RegisterCommandsEvent, Registrar,
    RegistrationContext, RenderLevelStageEvent, RunModCommandEvent, ScreenClosingEvent,
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
/// The data reload listeners of the items (the item tags `in_tag` offers).
pub(crate) fn add_reload_listeners(listeners: &mut dyn mod_api::ServerReloadListeners) {
    filters::add_reload_listeners(listeners);
}

/// The numeric item model properties that turn the partial models of Create's item renderers
/// (`create-data`'s `SPINNERS`): the frame of a whole turn cut in `SPIN_FRAMES`, at the speed
/// each renderer turns it (`WrenchItemRenderer`: `ScrollValueHandler`'s passive scroll, half a
/// degree a tick; `PotatoCannonItemRenderer`: -2.5 degrees a tick; `WorldshaperItemRenderer`:
/// -25 degrees a second). Left out: the extra turns of a scroll on a value board
/// (`wrenchCog.bump`), a shot and the worldshaper's use (their hand animations).
const SPINS: [(&str, f64); 3] = [
    ("create:wrench_gear", 10.0),
    ("create:potato_cannon_cog", -50.0),
    ("create:worldshaper_accelerator", -25.0),
];
const SPIN_FRAMES: f64 = 64.0;

pub(crate) fn spin_properties(properties: &mut Vec<String>) {
    properties.extend(SPINS.iter().map(|(id, _)| (*id).to_owned()));
}

/// The frame a turning part shows now.
pub(crate) fn spin(host: &dyn ClientHost, property: &str) -> Option<f32> {
    let (_, speed) = SPINS.iter().find(|(id, _)| *id == property)?;
    let seconds = host.millis() as f64 / 1000.0;
    let angle = (seconds * speed).rem_euclid(360.0);
    Some((angle / 360.0 * SPIN_FRAMES).floor() as f32)
}

/// Create's tooltips on its items (`ClientEvents.addToItemTooltip`).
pub(crate) fn item_tooltip(host: &dyn ClientHost, event: &mut mod_api::ItemTooltipEvent<'_>) {
    tooltip::item_tooltip(host, event);
}

/// `ItemStack.getTooltipLines(.., TooltipFlag.NORMAL)` on a client: the game's lines with
/// Create's added, as `item_tooltip` adds them (other mods' callbacks are not run).
pub(crate) fn tooltip_lines(host: &dyn ClientHost, stack: &ItemStack) -> Vec<mod_api::Text> {
    let Some(game) = mod_api::ItemResource::of(stack).tooltip(host, false, false) else {
        return Vec::new();
    };
    let mut lines: Vec<(mod_api::Text, u32)> = game
        .into_iter()
        .filter_map(|e| match e {
            mod_api::TooltipElement::Text(t, c) => Some((t, c)),
            _ => None,
        })
        .collect();
    let display = mod_api::TooltipDisplay::for_stack(stack);
    let mut event = mod_api::ItemTooltipEvent {
        player: 0,
        player_entity_id: -1,
        stack,
        display: &display,
        advanced: false,
        creative: false,
        lines: &mut lines,
    };
    item_tooltip(host, &mut event);
    lines.into_iter().map(|(t, _)| t).collect()
}

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
        symmetry::register(r);
        let _ = sandpaper::ITEM_PARTICLE.set(r.particle_key("minecraft:item"));
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

    /// `CardboardSwordItem.cardboardSwordsCannotHurtYou`, and
    /// `WrenchItem.wrenchInstaKillsMinecarts`.
    fn attack_entity(&self, host: &mut dyn ServerHost, event: &mut AttackEntityEvent<'_>) -> Flow {
        wrench_hits_minecart(host, event);
        sword::attack(host, event)
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        filters::level_tick(lv);
        symmetry::level_tick(lv);
    }

    fn world_closed(&self) {
        symmetry::world_closed();
    }

    /// `ConfigureSymmetryWandPacket`, `ConfigureWorldshaperPacket`.
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            symmetry_screen::CHANNEL => symmetry::configure(host, player, data),
            worldshaper_screen::CHANNEL => worldshaper::configure(host, player, data),
            filter_screen::CHANNEL => filters::client_message(host, player, data),
            controller_screen::CHANNEL => {
                super::contraptions::controller::chosen(host, player, data)
            }
            _ => return false,
        }
        true
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        symmetry::server_message(channel, data)
            || filter_screen::server_message(host, channel, data)
            || controller_screen::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        symmetry::tick(host);
        symmetry_screen::tick(host);
        worldshaper::tick(&*host);
        worldshaper_screen::tick(host);
        filter_screen::tick(host);
        controller_screen::tick(host);
    }

    fn client_reset(&self) {
        symmetry::reset();
        symmetry_screen::reset();
        worldshaper::reset();
        worldshaper_screen::reset();
        filter_screen::reset();
        controller_screen::reset();
    }

    fn standalone_models(&self, models: &mut Vec<String>) {
        models.extend(symmetry::MODELS.iter().map(|m| (*m).to_owned()));
    }

    fn models_baked(&self, host: &mut dyn ClientHost) {
        symmetry::models_baked(&*host);
    }

    /// The mirrors (`SymmetryHandler.onRenderWorld`) and the worldshaper's outline.
    fn render_level_stage(&self, host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
        symmetry::render_level_stage(host, event);
        worldshaper::render_level_stage(host, event);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        symmetry_screen::screen_render_post(host, event);
        worldshaper_screen::screen_render_post(host, event);
        filter_screen::screen_render_post(host, event);
        controller_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        let screens: [for<'e> fn(
            &dyn ClientHost,
            &mut ScreenMouseButtonPressedPreEvent<'e>,
        ) -> Flow; 4] = [
            symmetry_screen::screen_mouse_pressed,
            worldshaper_screen::screen_mouse_pressed,
            filter_screen::screen_mouse_pressed,
            controller_screen::screen_mouse_pressed,
        ];
        screens
            .iter()
            .map(|screen| screen(host, event))
            .find(|flow| *flow != Flow::Continue)
            .unwrap_or(Flow::Continue)
    }

    fn screen_mouse_released(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        let screens: [for<'e> fn(
            &dyn ClientHost,
            &mut ScreenMouseButtonReleasedPreEvent<'e>,
        ) -> Flow; 4] = [
            symmetry_screen::screen_mouse_released,
            worldshaper_screen::screen_mouse_released,
            filter_screen::screen_mouse_released,
            controller_screen::screen_mouse_released,
        ];
        screens
            .iter()
            .map(|screen| screen(host, event))
            .find(|flow| *flow != Flow::Continue)
            .unwrap_or(Flow::Continue)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        let screens: [for<'e> fn(&dyn ClientHost, &mut ScreenMouseScrolledPreEvent<'e>) -> Flow;
            3] = [
            symmetry_screen::screen_mouse_scrolled,
            worldshaper_screen::screen_mouse_scrolled,
            filter_screen::screen_mouse_scrolled,
        ];
        screens
            .iter()
            .map(|screen| screen(host, event))
            .find(|flow| *flow != Flow::Continue)
            .unwrap_or(Flow::Continue)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        match filter_screen::screen_key_pressed(host, event) {
            Flow::Continue => controller_screen::screen_key_pressed(host, event),
            f => f,
        }
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        symmetry_screen::screen_closing(host, event);
        worldshaper_screen::screen_closing(host, event);
        filter_screen::screen_closing(host, event);
        controller_screen::screen_closing(host, event);
    }

    /// `SandPaperItem.finishUsingItem`.
    fn use_item_finished(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::LivingEntityUseItemFinishEvent<'_>,
    ) {
        sandpaper::finished(host, event);
    }

    fn use_item_tick(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::LivingEntityUseItemTickEvent<'_>,
    ) {
        sandpaper::use_tick(host, event.stack, event.entity, event.duration);
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        filters::register_commands(event.commands);
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
            _ => return Flow::Continue,
        };
        let _ = host.commands().feedback(Text::literal(line));
        Flow::Cancel
    }
}

/// The slot a debug command names: `hand`, `offhand` or an inventory slot (0 to 35), and
/// the stack there.
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

/// `WrenchItem.wrenchInstaKillsMinecarts`: a minecart hit with a wrench in the main hand, outside
/// creative, takes 100 of the player's attack damage.
fn wrench_hits_minecart(host: &mut dyn ServerHost, event: &AttackEntityEvent<'_>) {
    if !event.kind.ends_with("minecart") {
        return;
    }
    let Some(player) = host.players().acting_player() else {
        return;
    };
    if creative(&*host, player) {
        return;
    }
    let wrench = host
        .living(player.entity())
        .and_then(|l| l.hand_item(&*host, Hand::Main))
        .is_some_and(|s| s.item().id(&*host) == "create:wrench");
    if !wrench {
        return;
    }
    let Some(cart) = u32::try_from(event.entity)
        .ok()
        .and_then(|id| host.entity(id))
    else {
        return;
    };
    let me = Some(player.entity());
    let _ = cart.hurt(host, "minecraft:player_attack", 100.0, me, me);
}
