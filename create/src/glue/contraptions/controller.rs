//! Create's linked controller and lectern controller; the rules are `create_engine::controller`'s.
//! The client half holds the controller's mode: while it is active the movement keys' mappings
//! (`ControlsUtil.getControls`: forward, back, left, right, jump, sneak) are kept from the game,
//! and the client's tick sends the buttons held, as Create's packets, on `create:controller`.
//! The server half resolves buttons to the frequencies on the held controller (in the player's
//! tick, where their hands are) or on the lectern used, and holds them down as transmitters with
//! no block in the level's redstone link networks until released or timed out. Lecterns holding
//! a controller are kept per level and saved with their column as `create_controller`. Nothing
//! runs without an active controller, a held frequency, a lectern in use or a message waiting.
//!
//! The controller's frequencies are kept on the stack's custom data
//! (`create:linked_controller_items`), and set in its frequency
//! screen (`items::controller_screen`), opened by a sneaking use.

use super::super::net;
use super::super::persist::mark_pos_unsaved;
use super::super::server::{ServerLevel, update_all};
use super::super::states::key;
use super::super::subsystem::Subsystem;
use super::super::world::{level_index, with_world};
use super::{center, create_info, in_click_level, name_props, prop};
use create_engine::Block;
use create_engine::controller::{self as ct, Client, Lectern, ToClient, ToServer};
use mod_api::{
    BlockPart, ClientHost, Command, CommandArgumentValue, CommandSource, Compound, Flow, Hand,
    Host, HostSide, InteractionResult, ItemPart, ItemStack, KeyMappingPreEvent, LivingRef,
    OnItemUseFirstEvent, PlayerRef, PlayerTickPostEvent, RegisterCommandsEvent, Registrar,
    RegistrationContext, RunModCommandEvent, ServerHost, SoundPlay, SoundSource, Tag, Text,
    UseItemEvent, UseItemOnEvent,
};
use std::cell::{Cell, RefCell};

pub const ITEM: &str = "create:linked_controller";

/// The linked controller's item, for drawing it on lecterns.
static CONTROLLER_KEY: std::sync::OnceLock<mod_api::ItemKey> = std::sync::OnceLock::new();

/// The key of whether a lectern controller has a user (`hasUser`), in its block entity's data.
const LECTERN_USER: &str = "LecternUser";

/// `LinkedControllerItemRenderer`'s partials: the powered controller and a button.
const POWERED_MODEL: &str = "create:item/linked_controller/powered";
const BUTTON_MODEL: &str = "create:item/linked_controller/button";

/// `LecternControllerRenderer`: the linked controller lying on the lectern's top, turned to its
/// facing and tilted 22° (`renderInLectern`). While someone uses it, it is the powered model
/// with its six buttons; for the local player using it, each button sinks as its key is held
/// (`renderDepression`, `buttons` chasing 0 or 1 at 0.4 a tick).
struct LecternDraw;

#[mod_api::horizon_part]
impl BlockPart for LecternDraw {}

#[mod_api::horizon_part]
impl mod_api::BlockEntityPart for LecternDraw {
    fn block_entity_submit(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        let Some(id) = super::super::states::id(event.state) else {
            return;
        };
        let Some((Block::LecternController, info)) =
            super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        let Some(key) = CONTROLLER_KEY.get() else {
            return;
        };
        let facing = info.facing.unwrap_or(create_engine::kinetics::Dir::North);
        let yaw = create_engine::decor::horizontal_angle(facing) - 90.0;
        let active = matches!(
            event.entity.nbt.get(&LECTERN_USER.into()),
            Some(Tag::Byte(1))
        );
        let mine = ACTIVE.get() && CLIENT.with_borrow(|c| c.lectern == Some(event.pos));
        let draw = &mut *event.draw;
        let t = draw.partial();
        draw.push();
        draw.translate([0.5, 1.45, 0.5]);
        draw.rotate_about([0.0, 1.0, 0.0], yaw, [0.0; 3]);
        draw.translate([0.28, 0.0, 0.0]);
        draw.rotate_about([0.0, 0.0, 1.0], -22.0, [0.0; 3]);
        if !(active && draw_powered(draw, host, mine.then_some(t))) {
            draw.item(
                &ItemStack::of(*key),
                mod_api::ItemDisplayContext::None,
                None,
                0,
            );
        }
        draw.pop();
        if !mine {
            draw.keep();
        }
    }
}

/// `LinkedControllerItemRenderer.render` for a lectern in use: the powered model and the six
/// buttons where Create puts them, each sunk by up to 3/4 px as it is pressed when `depression`
/// (the frame's partial tick) is given. False when the models are missing.
fn draw_powered(
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    host: &dyn ClientHost,
    depression: Option<f32>,
) -> bool {
    let (Some(powered), Some(button)) = (model(host, POWERED_MODEL), model(host, BUTTON_MODEL))
    else {
        return false;
    };
    let options = mod_api::TerrainModelOptions::default;
    // A model is drawn about its centre, as the item renderer does.
    draw.push();
    draw.translate([-0.5; 3]);
    let drawn = draw.terrain_model(powered, options()).is_ok();
    draw.pop();
    if !drawn {
        return false;
    }
    let s = 1.0 / 16.0;
    let b = s * -0.75;
    let presses = BUTTONS.with_borrow(|v| *v);
    // The buttons' places, each from the last (`msr.translate` between `renderButton`s).
    let steps: [[f32; 3]; 6] = [
        [2.0 * s, 0.0, 8.0 * s],
        [4.0 * s, 0.0, 0.0],
        [-2.0 * s, 0.0, 2.0 * s],
        [0.0, 0.0, -4.0 * s],
        [3.0 * s, 0.0, 3.0 * s],
        [2.0 * s, 0.0, 0.0],
    ];
    let mut at = [0.0f32; 3];
    for (i, step) in steps.iter().enumerate() {
        // The first four are drawn in a pushed pose, the last two from where it was.
        if i == 4 {
            at = [0.0; 3];
        }
        at = [at[0] + step[0], at[1] + step[1], at[2] + step[2]];
        let sink = depression.map_or(0.0, |t| {
            let (prev, now) = presses[i];
            b * (prev + (now - prev) * t)
        });
        draw.push();
        draw.translate([at[0] - 0.5, at[1] + sink - 0.5, at[2] - 0.5]);
        let _ = draw.terrain_model(button, options());
        draw.pop();
    }
    true
}

/// The standalone model `id`, resolved once.
fn model(host: &dyn ClientHost, id: &'static str) -> Option<mod_api::ModelRef> {
    if let Some(m) = MODELS.with_borrow(|m| m.get(id).copied()) {
        return Some(m);
    }
    let m = host
        .render_assets()
        .model(&mod_api::Identifier::parse(id).ok()?)
        .ok()?;
    MODELS.with_borrow_mut(|c| c.insert(id, m));
    Some(m)
}
const KEY: &str = "create_controller";
const CUSTOM_DATA: &str = "minecraft:custom_data";

/// The movement keys' mappings the controller takes while active.
const MAPPINGS: [&str; 6] = [
    "key.forward",
    "key.back",
    "key.left",
    "key.right",
    "key.jump",
    "key.sneak",
];

/// A lectern controller placed or removed: its level, position, whether placed, old state.
type Change = (usize, [i32; 3], bool, u32);

thread_local! {
    /// The client's controller (the window's thread) and whether it is active: the key hook's
    /// and the client tick's one test.
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// The buttons' presses (`LinkedControllerItemRenderer.buttons`): a tick ago and now.
    static BUTTONS: RefCell<[(f32, f32); 6]> = const { RefCell::new([(0.0, 0.0); 6]) };
    /// The lecterns' partial models, resolved.
    static MODELS: RefCell<std::collections::HashMap<&'static str, mod_api::ModelRef>> =
        RefCell::new(std::collections::HashMap::new());
    /// What each lectern controller's block entity was last told of its user (none yet after
    /// it loads: told once).
    static SHOWN_USERS: RefCell<std::collections::HashMap<(usize, [i32; 3]), bool>> =
        RefCell::new(std::collections::HashMap::new());
    /// The server's: the hand packets waiting for their player's tick (player, packet, game
    /// time), and lecterns placed or removed (dimension, position, placed, old state).
    static INPUTS: RefCell<Vec<(u128, ToServer, i64)>> = const { RefCell::new(Vec::new()) };
    static CHANGES: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    /// Whether the level's tick has anything to do: frequencies held, a lectern in use, changes.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

pub struct Controller;

impl Subsystem for Controller {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let item = r.item_key(ITEM);
        r.item_part(item, ControllerItem);
        r.block_part(key(Block::LecternController), LecternPart);
        let _ = CONTROLLER_KEY.set(item);
        match super::super::kinetics::kind() {
            Some(kind) => {
                if let Err(e) = r.block_entity_part(&kind, LecternDraw) {
                    crate::create_log!("[create] lectern controller drawing refused: {e:?}");
                }
            }
            None => crate::create_log!("[create] controller: no kinetic block entity type yet"),
        }
    }

    fn synced_keys(&self) -> &'static [&'static str] {
        &[LECTERN_USER]
    }

    fn standalone_models(&self, models: &mut Vec<String>) {
        models.extend([POWERED_MODEL, BUTTON_MODEL].map(str::to_owned));
    }

    fn models_baked(&self, _host: &mut dyn ClientHost) {
        MODELS.with_borrow_mut(std::collections::HashMap::clear);
    }

    /// A lectern the local player uses is drawn each frame (its buttons move).
    fn animated(&self, block: Block, pos: [i32; 3], _entity: &mod_api::ModBlockEntity) -> bool {
        block == Block::LecternController
            && ACTIVE.get()
            && CLIENT.with_borrow(|c| c.lectern == Some(pos))
    }

    fn world_closed(&self) {
        SHOWN_USERS.with_borrow_mut(std::collections::HashMap::clear);
        INPUTS.with_borrow_mut(Vec::clear);
        CHANGES.with_borrow_mut(Vec::clear);
        BUSY.set(false);
    }

    fn client_reset(&self) {
        CLIENT.with_borrow_mut(|c| *c = Client::default());
        BUTTONS.with_borrow_mut(|b| *b = [(0.0, 0.0); 6]);
        ACTIVE.set(false);
    }

    /// A committed change on the server: a lectern controller placed or removed.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let lectern = |s: u32| create_info(s).map(|(b, _)| b) == Some(Block::LecternController);
        let (was, is) = (lectern(old), lectern(new));
        if was != is {
            CHANGES.with_borrow_mut(|c| c.push((dim, pos, is, old)));
            BUSY.set(true);
        }
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        chunk_load(lv.dim, data);
        // Its lecterns' block entities are told again that nobody uses them (`show_users`).
        if data.get(&KEY.into()).is_some() {
            BUSY.set(true);
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        chunk_save(lv.dim, chunk, data);
    }

    /// A column left a level: its lecterns are forgotten (kept in its save).
    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        SHOWN_USERS.with_borrow_mut(|s| {
            s.retain(|(d, p), _| *d != dim || (p[0] >> 4, p[2] >> 4) != (chunk[0], chunk[1]))
        });
        with_world(|w| {
            w.dims[dim]
                .lecterns
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (chunk[0], chunk[1]))
        });
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        level_tick(lv);
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel != ct::CHANNEL {
            return false;
        }
        if let Some(packet) = ToServer::decode(data) {
            from_client(host, player, packet);
        }
        true
    }

    /// The server's word: the controller's use toggles it, a click on a link binds, a lectern
    /// started or stopped being used by this player.
    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        if channel != ct::CHANNEL {
            return false;
        }
        let Some(m) = ToClient::decode(data) else {
            return true;
        };
        let packets = CLIENT.with_borrow_mut(|c| match m {
            ToClient::Toggle => c.toggle(),
            ToClient::BindMode(at) => c.toggle_bind(at),
            ToClient::LecternOn(at) => {
                c.activate_in_lectern(at);
                Vec::new()
            }
            ToClient::LecternOff(_) => c.deactivate_in_lectern(),
        });
        send_server(host, packets);
        ACTIVE.set(CLIENT.with_borrow(Client::active));
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client_tick(host);
    }

    /// While the controller is in use, its six keys are its buttons and their presses are kept
    /// from the game (no walking, jumping or sneaking).
    fn key_mapping_pre(
        &self,
        _host: &mut dyn ClientHost,
        event: &mut KeyMappingPreEvent<'_>,
    ) -> Flow {
        if ACTIVE.get() && event.pressed && MAPPINGS.contains(&event.mapping) {
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn player_tick(&self, host: HostSide, event: &mut PlayerTickPostEvent) {
        if let HostSide::Server(host) = host {
            player_tick(host, event.player);
        }
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        event.commands.push(Command::op(
            "create_debug controller <button:integer> <items:text>",
            2,
        ));
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        run_command(host, event)
    }
}

// ---------------------------------------------------------------------------------------------
// The controller's frequencies on its stack

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.components()?.as_nbt().get(&CUSTOM_DATA.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

/// The controller's 12 frequency items (`LINKED_CONTROLLER_ITEMS`).
fn controller_items(stack: &ItemStack) -> Vec<String> {
    match custom(stack).and_then(|c| c.get(&ct::ITEMS_KEY.into())) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| match t {
                Tag::String(s) => s.to_string_lossy(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `set(LINKED_CONTROLLER_ITEMS)`: none when every slot is empty.
fn set_controller_items(stack: &mut ItemStack, items: &[String]) {
    let mut c = custom(stack).cloned().unwrap_or_default();
    if items.iter().any(|i| !i.is_empty()) {
        let list = items
            .iter()
            .map(|i| Tag::String(i.as_str().into()))
            .collect();
        c.insert(ct::ITEMS_KEY.into(), Tag::List(list));
    } else {
        c.remove(&ct::ITEMS_KEY.into());
    }
    let mut patch = stack
        .components()
        .map(|p| p.as_nbt().clone())
        .unwrap_or_default();
    if c.is_empty() {
        patch.remove(&CUSTOM_DATA.into());
    } else {
        patch.insert(CUSTOM_DATA.into(), Tag::Compound(c));
    }
    let _ = stack.set_components(patch);
}

fn is_controller(host: &dyn Host, s: Option<&ItemStack>) -> bool {
    s.is_some_and(|s| s.item().id(host) == ITEM)
}

/// A new controller holding `items`.
fn controller_stack(host: &dyn Host, items: &[String]) -> Option<ItemStack> {
    let key = host.item_key(&mod_api::Identifier::parse(ITEM).ok()?)?;
    let mut stack = ItemStack::of(key);
    set_controller_items(&mut stack, items);
    Some(stack)
}

fn send_client(host: &mut dyn ServerHost, player: PlayerRef, m: ToClient) {
    net::send_to_player(host, player, ct::CHANNEL, &m.encode());
}

fn send_server(host: &mut dyn ClientHost, packets: Vec<ToServer>) {
    for p in packets {
        let _ = net::send_to_server(host, ct::CHANNEL, &p.encode());
    }
}

// ---------------------------------------------------------------------------------------------
// Client

/// `LinkedControllerClientHandler.tick`: idle again when a screen opens, escape is down, the
/// player is a spectator or no longer holds a controller (in neither hand, not at a lectern);
/// else the held buttons go to the server. Nothing runs while idle.
fn client_tick(host: &mut dyn ClientHost) {
    if !ACTIVE.get() {
        return;
    }
    let Some(player) = host.local_player() else {
        return;
    };
    let lectern = CLIENT.with_borrow(|c| c.lectern.is_some());
    let living = host.living(player.entity());
    let hand = |h: Hand| living.and_then(|l| l.hand_item(&*host, h));
    let holding = is_controller(&*host, hand(Hand::Main).as_ref())
        || is_controller(&*host, hand(Hand::Off).as_ref());
    let stop = host.observed_screen_id().is_some()
        || player.is_spectator(&*host).unwrap_or(false)
        || host.is_key_down("key.keyboard.escape")
        || !lectern && !holding;
    // `isActuallyPressed`: the keys themselves, kept from the game or not.
    let held = if stop {
        0
    } else {
        (0..ct::KEYS.len())
            .filter(|&b| host.is_key_down(ct::KEYS[b]))
            .fold(0u8, |m, b| m | 1 << b)
    };
    let (packets, click) =
        CLIENT.with_borrow_mut(|c| if stop { (c.stop(), None) } else { c.tick(held) });
    send_server(host, packets);
    // `AllSoundEvents.CONTROLLER_CLICK` (the item frame's, at .35), for the player.
    if let Some(pitch) = click {
        let mut sound = SoundPlay::ui("minecraft:entity.item_frame.add_item");
        sound.volume = 0.35;
        sound.pitch = pitch;
        let _ = host.play_local_sound(&sound, 0);
    }
    ACTIVE.set(CLIENT.with_borrow(Client::active));
    // `LinkedControllerItemRenderer.tick`: each button chases its key, while active.
    let pressed = CLIENT.with_borrow(|c| c.pressed);
    let active = ACTIVE.get();
    BUTTONS.with_borrow_mut(|v| {
        for (i, b) in v.iter_mut().enumerate() {
            let target = if active && pressed & (1 << i) != 0 {
                1.0
            } else {
                0.0
            };
            b.0 = b.1;
            b.1 += (target - b.1) * 0.4;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Server: the item

struct ControllerItem;

fn sneaking(host: &dyn Host, player: Option<PlayerRef>) -> bool {
    player
        .and_then(|p| p.entity().is_shift_key_down(host))
        .unwrap_or(false)
}

#[mod_api::horizon_part]
impl ItemPart for ControllerItem {
    /// `LinkedControllerItem.use`: not sneaking, the controller toggles on its holder's client.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let player = host.player(event.player)?;
        if sneaking(&*host, Some(player)) {
            // Sneaking with it in the main hand opens its frequency screen
            // (`LinkedControllerMenu`), for a player who may build.
            if event.hand != Hand::Main {
                return None;
            }
            if player.may_build(&*host).unwrap_or(false) {
                let items = controller_items(event.stack);
                crate::glue::items::controller_screen::open(host, player, &items);
            }
            return Some(InteractionResult::Success);
        }
        send_client(host, player, ToClient::Toggle);
        // `getCooldowns().addCooldown(this, 2)`.
        player.set_item_cooldown(host, ITEM, 2);
        None
    }

    /// `LinkedControllerItem.onItemUseFirst`: on a redstone link the client binds a button to
    /// it; on a lectern without a book the controller goes on it (a lectern controller).
    /// Elsewhere the item's own use follows (`use_item`).
    fn on_item_use_first(
        &self,
        host: &mut dyn ServerHost,
        event: &mut OnItemUseFirstEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        if !player.may_build(&*host).unwrap_or(false) || sneaking(&*host, Some(player)) {
            return None;
        }
        let pos = click.pos;
        let stack = event.stack.clone();
        let hand = click.hand;
        in_click_level(host, click, |lv| {
            if lv.create_block(pos) == Some(Block::RedstoneLink) {
                send_client(lv.host, player, ToClient::BindMode(pos));
                return Some(InteractionResult::Success);
            }
            let (name, props) = lv.name_props(pos)?;
            if name != "minecraft:lectern" || prop(&props, "has_book") != Some("false") {
                return None;
            }
            let facing = prop(&props, "facing").unwrap_or("north").to_owned();
            let powered = prop(&props, "powered").unwrap_or("false").to_owned();
            let new = super::state_with(
                &*lv.host,
                "create:lectern_controller",
                &[
                    ("facing", facing.as_str()),
                    ("powered", powered.as_str()),
                    ("has_book", "true"),
                ],
            )?;
            // `replaceLectern` and `setController` (a creative player keeps theirs).
            let items = controller_items(&stack);
            let dim = lv.dim;
            with_world(|w| {
                w.dims[dim].lecterns.insert(
                    pos,
                    Lectern {
                        items,
                        ..Lectern::default()
                    },
                )
            });
            if !player.is_creative(&*lv.host).unwrap_or(false)
                && let Some(living) = lv.host.living(player.entity())
            {
                let left = stack.clone().shrink(1);
                let _ = living.set_hand_item(&mut *lv.host, hand, left);
            }
            lv.set_num(pos, new, update_all());
            mark_pos_unsaved(dim, pos);
            // `AllSoundEvents.CONTROLLER_PUT`: the book's.
            book_sound(lv, pos, "minecraft:item.book.put");
            Some(InteractionResult::Success)
        })?
    }
}

fn book_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], sound: &str) {
    lv.level.play_sound(
        &mut *lv.host,
        sound,
        center(pos),
        SoundSource::Blocks,
        1.0,
        1.0,
    );
}

// ---------------------------------------------------------------------------------------------
// Server: commands and packets

/// An item id from a command word ("-" for none, `minecraft:` when it names none).
fn item_word(word: Option<&&str>) -> String {
    match word.copied() {
        None | Some("-") => String::new(),
        Some(w) if w.contains(':') => w.to_owned(),
        Some(w) => format!("minecraft:{w}"),
    }
}

/// `create_debug controller <button> <first|-> [second|-]`: the controller in the main hand
/// takes a button's frequency (its screen, `LinkedControllerMenu`, stands in).
fn run_command(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let cmd = event.command;
    if cmd
        .literals
        .iter()
        .map(String::as_str)
        .ne(["create_debug", "controller"])
    {
        return Flow::Continue;
    }
    let button = match cmd.argument("button") {
        Some(CommandArgumentValue::Integer(b)) => Some(*b),
        _ => None,
    };
    let words = match cmd.argument("items") {
        Some(CommandArgumentValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    let words: Vec<&str> = words.split_whitespace().collect();
    let player = match event.source {
        CommandSource::Player { player, .. } => Some(*player),
        _ => None,
    };
    let line = match button.and_then(|b| u8::try_from(b).ok()) {
        Some(b) if usize::from(b) < ct::KEYS.len() => {
            let key = (item_word(words.first()), item_word(words.get(1)));
            let living = player.and_then(|p| host.living(p.entity()));
            match living.and_then(|l| Some((l, l.hand_item(&*host, Hand::Main)?))) {
                Some((l, mut stack)) if is_controller(&*host, Some(&stack)) => {
                    let mut items = controller_items(&stack);
                    ct::bind(&mut items, b, &key);
                    set_controller_items(&mut stack, &items);
                    let _ = l.set_hand_item(host, Hand::Main, Some(stack));
                    format!("controller button {b}: {} + {}", key.0, key.1)
                }
                _ => "controller: hold a linked controller".to_owned(),
            }
        }
        _ => "usage: create_debug controller <button 0-5> <item|-> [item|-]".to_owned(),
    };
    let _ = host.commands().feedback(Text::literal(line));
    Flow::Cancel
}

/// The frequency screen closed (`HeldItemGhostItemMenu.removed`, `saveData`): its twelve
/// frequencies go on the controller still in the player's main hand.
pub(crate) fn chosen(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(items) = crate::glue::items::controller_screen::chosen(data) else {
        return;
    };
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let Some(mut stack) = living.hand_item(&*host, Hand::Main) else {
        return;
    };
    if !is_controller(&*host, Some(&stack)) {
        return;
    }
    set_controller_items(&mut stack, &items);
    let _ = living.set_hand_item(host, Hand::Main, Some(stack));
}

/// A player's packet (`LinkedControllerPacketBase.handle`): a lectern's at once, the hand's in
/// that player's next tick (where their hands are).
fn from_client(host: &mut dyn ServerHost, player: PlayerRef, packet: ToServer) {
    let Some(dimension) = player.dimension(&*host) else {
        return;
    };
    let Some(dim) = level_index(&dimension.to_string()) else {
        return;
    };
    let uuid = player.uuid();
    let at = player
        .entity()
        .position(&*host)
        .unwrap_or_default()
        .map(|v| v.floor() as i32);
    let spectator = player.is_spectator(&*host).unwrap_or(false);
    match packet {
        ToServer::Input {
            buttons,
            press,
            lectern: Some(lectern),
        } => {
            // `handleLectern`: only its user's buttons, from the lectern's controller.
            let keys = with_world(|w| {
                let l = w.dims[dim].lecterns.get(&lectern)?;
                (l.user == Some(uuid)).then(|| {
                    ct::buttons(buttons)
                        .map(|b| ct::frequency(&l.items, b))
                        .collect::<Vec<_>>()
                })
            })
            .flatten();
            // `handleItem`: a spectator presses nothing.
            if let Some(keys) = keys
                && !(press && spectator)
            {
                if with_world(|w| w.controller_input(dim, uuid, at, &keys, press)) == Some(true) {
                    crate::glue::advancements::award(host, player, "linked_controller");
                }
                BUSY.set(true);
            }
        }
        ToServer::StopLectern(lectern) => {
            let now = host
                .level(&dimension.to_string())
                .and_then(|l| l.game_time(&*host))
                .unwrap_or(0);
            let stopped = with_world(|w| {
                let l = w.dims[dim].lecterns.get_mut(&lectern)?;
                (l.user == Some(uuid)).then(|| l.stop(now))
            })
            .flatten();
            if stopped.is_some() {
                send_client(host, player, ToClient::LecternOff(lectern));
            }
        }
        packet => {
            let now = host
                .level(&dimension.to_string())
                .and_then(|l| l.game_time(&*host))
                .unwrap_or(0);
            INPUTS.with_borrow_mut(|i| i.push((uuid, packet, now)));
        }
    }
}

/// The end of a player's tick: their hand packets, with the controller in their main hand,
/// else the off hand (none: dropped, as Create's). Packets nobody's tick took for 40 ticks (a
/// player gone) are dropped. The miss is one test.
fn player_tick(host: &mut dyn ServerHost, me: PlayerRef) {
    if INPUTS.with_borrow(Vec::is_empty) {
        return;
    }
    let Some(dimension) = me.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let now = host
        .level(&dimension)
        .and_then(|l| l.game_time(&*host))
        .unwrap_or(0);
    let uuid = me.uuid();
    let mine: Vec<ToServer> = INPUTS.with_borrow_mut(|i| {
        let mine = i.iter().filter(|x| x.0 == uuid).map(|x| x.1).collect();
        i.retain(|x| x.0 != uuid && now - x.2 < 40);
        mine
    });
    let Some(dim) = level_index(&dimension) else {
        return;
    };
    let Some(living) = host.living(me.entity()) else {
        return;
    };
    let spectator = me.is_spectator(&*host).unwrap_or(false);
    for packet in mine {
        let Some((hand, mut stack)) = held_controller(host, living) else {
            continue;
        };
        match packet {
            ToServer::Input { buttons, press, .. } => {
                if spectator && press {
                    continue;
                }
                let items = controller_items(&stack);
                let keys: Vec<_> = ct::buttons(buttons)
                    .map(|b| ct::frequency(&items, b))
                    .collect();
                let at = me
                    .entity()
                    .position(&*host)
                    .unwrap_or_default()
                    .map(|v| v.floor() as i32);
                if with_world(|w| w.controller_input(dim, uuid, at, &keys, press)) == Some(true) {
                    crate::glue::advancements::award(host, me, "linked_controller");
                }
                BUSY.set(true);
            }
            // `LinkedControllerBindPacket.handleItem`: the link's frequency on the button.
            ToServer::Bind { button, link } => {
                if spectator {
                    continue;
                }
                let Some(key) =
                    with_world(|w| w.dims[dim].links.links.get(&link).map(|l| l.key.clone()))
                        .flatten()
                else {
                    continue;
                };
                let mut items = controller_items(&stack);
                ct::bind(&mut items, button, &key);
                set_controller_items(&mut stack, &items);
                let _ = living.set_hand_item(host, hand, Some(stack));
                let label = Text::literal(ct::KEYS[usize::from(button)]);
                let line = Text::translate("create.linked_controller.key_bound", vec![label]);
                let _ = me.action_bar(host, line);
            }
            ToServer::StopLectern(_) => {}
        }
    }
}

/// The hand holding a controller, and its stack: the main hand first.
fn held_controller(host: &dyn ServerHost, living: LivingRef) -> Option<(Hand, ItemStack)> {
    [Hand::Main, Hand::Off].into_iter().find_map(|h| {
        let s = living.hand_item(host, h)?;
        is_controller(host, Some(&s)).then_some((h, s))
    })
}

// ---------------------------------------------------------------------------------------------
// Server: lecterns

struct LecternPart;

#[mod_api::horizon_part]
impl BlockPart for LecternPart {
    /// A click on a lectern controller (`LecternControllerBlock.useItemOn`): sneaking with a
    /// controller swaps it for the lectern's; sneaking otherwise takes the lectern's off
    /// (`replaceWithLectern`); else a player in range uses it.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let me = host.player(event.click.player)?;
        let back = sneaking(&*host, Some(me));
        let held = event.click.stack.cloned();
        let hand = event.click.hand;
        let pos = event.click.pos;
        in_click_level(host, event.click, |lv| {
            use_lectern(lv, me, pos, back, held, hand)
        })?
    }
}

fn use_lectern(
    lv: &mut ServerLevel<'_>,
    me: PlayerRef,
    pos: [i32; 3],
    back: bool,
    held: Option<ItemStack>,
    hand: Hand,
) -> Option<InteractionResult> {
    let dim = lv.dim;
    let now = lv.game_time();
    let holding = is_controller(&*lv.host, held.as_ref());
    if back && holding && me.may_build(&*lv.host).unwrap_or(false) {
        // `swapControllers`: the hand takes the lectern's, the lectern the hand's.
        let new = held.as_ref().map(controller_items).unwrap_or_default();
        let old = with_world(|w| {
            let l = w.dims[dim].lecterns.entry(pos).or_default();
            std::mem::replace(&mut l.items, new)
        })
        .unwrap_or_default();
        if let (Some(stack), Some(living)) = (
            controller_stack(&*lv.host, &old),
            lv.host.living(me.entity()),
        ) {
            let _ = living.set_hand_item(&mut *lv.host, hand, Some(stack));
        }
        mark_pos_unsaved(dim, pos);
        book_sound(lv, pos, "minecraft:item.book.put");
        return Some(InteractionResult::Success);
    }
    if back {
        replace_with_lectern(lv, pos);
        return Some(InteractionResult::Success);
    }
    let feet = me.entity().position(&*lv.host)?;
    let eyes = [feet[0], feet[1] + 1.62, feet[2]];
    if !ct::in_lectern_range(eyes, pos, interaction_range(&*lv.host, me)) {
        return None;
    }
    let uuid = me.uuid();
    let started = with_world(|w| {
        let busy = w.dims.iter().any(|d| {
            d.lecterns
                .iter()
                .any(|(p, l)| *p != pos && l.user == Some(uuid))
        });
        let l = w.dims[dim].lecterns.entry(pos).or_default();
        l.try_start(uuid, busy, true, now)
    })
    .unwrap_or(false);
    if started {
        BUSY.set(true);
        send_client(lv.host, me, ToClient::LecternOn(pos));
    }
    Some(InteractionResult::Success)
}

/// `Attributes.BLOCK_INTERACTION_RANGE` of `player` (4.5, 5 in creative).
fn interaction_range(host: &dyn Host, player: PlayerRef) -> f64 {
    host.living(player.entity())
        .and_then(|l| l.attribute_value(host, "minecraft:block_interaction_range"))
        .unwrap_or(4.5)
}

/// `replaceWithLectern`: back to a lectern (facing and power kept), the controller dropped
/// above it (`dropController`, its user stopped).
fn replace_with_lectern(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some((_, props)) = lv.name_props(pos) else {
        return;
    };
    let facing = prop(&props, "facing").unwrap_or("north").to_owned();
    let powered = prop(&props, "powered").unwrap_or("false").to_owned();
    let lectern = super::state_with(
        &*lv.host,
        "minecraft:lectern",
        &[
            ("facing", facing.as_str()),
            ("powered", powered.as_str()),
            ("has_book", "false"),
        ],
    );
    drop_controller(lv, pos, &facing);
    book_sound(lv, pos, "minecraft:entity.item_frame.remove_item");
    if let Some(lectern) = lectern {
        lv.set_num(pos, lectern, update_all());
    }
}

/// `dropController`: the lectern's user stops, its controller drops a quarter block toward its
/// facing, one block up; the lectern is forgotten.
fn drop_controller(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: &str) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some(mut l) = with_world(|w| w.dims[dim].lecterns.remove(&pos)).flatten() else {
        return;
    };
    if let Some(user) = l.stop(now)
        && let Some(p) = lv.host.player(user)
    {
        send_client(lv.host, p, ToClient::LecternOff(pos));
    }
    let step = match facing {
        "south" => [0.0, 0.25],
        "west" => [-0.25, 0.0],
        "east" => [0.25, 0.0],
        _ => [0.0, -0.25],
    };
    let at = [
        f64::from(pos[0]) + 0.5 + step[0],
        f64::from(pos[1]) + 1.0,
        f64::from(pos[2]) + 0.5 + step[1],
    ];
    if let Some(stack) = controller_stack(&*lv.host, &l.items) {
        lv.drop_stack(at, [0.0; 3], stack, 10);
    }
    mark_pos_unsaved(dim, pos);
}

/// The level's tick: lecterns placed (by a command: an empty one) or removed (the controller
/// drops), lecterns' users out of range or gone stop (`LecternControllerBlockEntity.tick`),
/// held frequencies count down (`LinkedControllerServerHandler.tick`). Nothing runs with none
/// of these.
fn level_tick(lv: &mut ServerLevel<'_>) {
    if !BUSY.get() {
        return;
    }
    let dim = lv.dim;
    let changes: Vec<([i32; 3], bool, u32)> = CHANGES.with_borrow_mut(|c| {
        let mine = c
            .iter()
            .filter(|x| x.0 == dim)
            .map(|x| (x.1, x.2, x.3))
            .collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (pos, placed, old) in changes {
        let here = lv.create_block(pos) == Some(Block::LecternController);
        if placed && here {
            with_world(|w| {
                w.dims[dim].lecterns.entry(pos).or_default();
            });
        } else if !placed && !here {
            // Broken (`onRemove`): the lectern drops by its loot table, the controller here.
            let facing = name_props(&*lv.host, old)
                .and_then(|(_, p)| prop(&p, "facing").map(str::to_owned))
                .unwrap_or_default();
            drop_controller(lv, pos, &facing);
        }
    }
    let users: Vec<([i32; 3], u128)> = with_world(|w| {
        w.dims[dim]
            .lecterns
            .iter()
            .filter_map(|(p, l)| Some((*p, l.user?)))
            .collect()
    })
    .unwrap_or_default();
    if !users.is_empty() {
        let now = lv.game_time();
        let here = lv.players();
        for (pos, user) in users {
            let player = here.iter().copied().find(|p| p.uuid() == user);
            let stays = player.is_some_and(|p| {
                p.entity().position(&*lv.host).is_some_and(|f| {
                    let range = interaction_range(&*lv.host, p);
                    ct::in_lectern_range([f[0], f[1] + 1.62, f[2]], pos, range)
                })
            });
            if !stays {
                with_world(|w| w.dims[dim].lecterns.get_mut(&pos).map(|l| l.stop(now)));
                if let Some(p) = lv.host.player(user) {
                    send_client(lv.host, p, ToClient::LecternOff(pos));
                }
            }
        }
    }
    show_users(lv);
    with_world(|w| w.tick_controllers(dim));
    let busy = with_world(|w| {
        w.dims
            .iter()
            .any(|d| !d.controllers.is_empty() || d.lecterns.values().any(|l| l.user.is_some()))
    })
    .unwrap_or(false);
    BUSY.set(busy || CHANGES.with_borrow(|c| !c.is_empty()));
}

/// Each lectern controller's block entity told whether it has a user, when that changed (its
/// client draws the powered controller then).
fn show_users(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let used: Vec<([i32; 3], bool)> = with_world(|w| {
        w.dims[dim]
            .lecterns
            .iter()
            .map(|(p, l)| (*p, l.user.is_some()))
            .collect()
    })
    .unwrap_or_default();
    for (pos, user) in used {
        let shown = SHOWN_USERS.with_borrow(|s| s.get(&(dim, pos)).copied());
        if shown == Some(user) {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(LECTERN_USER.into(), Tag::Byte(i8::from(user)))]);
        if entity.merge_data(&mut *lv.host, data).is_ok() {
            SHOWN_USERS.with_borrow_mut(|s| s.insert((dim, pos), user));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Saved with the column

/// Lecterns of a column (`LecternControllerBlockEntity.write`: `ControllerData`; the user is
/// not kept, as Create's leaves it once its player is gone).
fn chunk_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let Some(mut list) = with_world(|w| {
        w.dims[dim]
            .lecterns
            .iter()
            .filter(|(p, _)| inside(p))
            .map(|(p, l)| (*p, l.items.clone()))
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    if list.is_empty() {
        return;
    }
    list.sort_by_key(|x| x.0);
    let tags = list
        .into_iter()
        .map(|(p, items)| {
            Tag::Compound(Compound::from([
                ("Pos".into(), Tag::IntArray(p.to_vec())),
                (
                    "Items".into(),
                    Tag::List(
                        items
                            .iter()
                            .map(|i| Tag::String(i.as_str().into()))
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect();
    data.insert(
        KEY.into(),
        Tag::Compound(Compound::from([("lecterns".into(), Tag::List(tags))])),
    );
}

/// A column joined a level: its lecterns back (`read`).
fn chunk_load(dim: usize, data: &Compound) {
    let Some(Tag::Compound(data)) = data.get(&KEY.into()) else {
        return;
    };
    let Some(Tag::List(list)) = data.get(&"lecterns".into()) else {
        return;
    };
    for t in list {
        let Tag::Compound(c) = t else { continue };
        let Some(Tag::IntArray(p)) = c.get(&"Pos".into()) else {
            continue;
        };
        let Ok(pos) = <[i32; 3]>::try_from(p.as_slice()) else {
            continue;
        };
        let items = match c.get(&"Items".into()) {
            Some(Tag::List(l)) => l
                .iter()
                .map(|t| match t {
                    Tag::String(s) => s.to_string_lossy(),
                    _ => String::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        with_world(|w| {
            w.dims[dim].lecterns.insert(
                pos,
                Lectern {
                    items,
                    ..Lectern::default()
                },
            )
        });
    }
}
