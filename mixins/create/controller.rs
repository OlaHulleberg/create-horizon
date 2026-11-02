// Create's linked controller and lectern controller in the game. A sibling of the `create`
// module (`runtime.rs`); the rules are `create_engine::controller`'s. The client half holds the
// controller's mode: the key hook (API 57) takes W, A, S, D, space and left shift while it is
// active and the client's tick sends the buttons held, as Create's packets, on
// `create:controller`. The server half resolves buttons to the frequencies on the held
// controller (in the player's tick, where their hands are) or on the lectern used, and holds
// them down as transmitters with no block in the level's redstone link networks until released
// or timed out. Lecterns holding a controller are typed per dimension and saved with their
// column as `create_controller`. Nothing runs without an active controller, a held frequency, a
// lectern in use or a message waiting: each hook's miss is one flag.
use super::Game;
use super::create::*;
use super::create_items_body as body;
use create_engine::Block;
use create_engine::controller::{self as ct, Client, Lectern, ToClient, ToServer};
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};
use std::cell::{Cell, RefCell};

pub(super) const ITEM: &str = "create:linked_controller";
const KEY: &str = "create_controller";

thread_local! {
    /// The client's controller (the window's thread) and whether it is active: the key hook's
    /// and the client tick's one test.
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// The server's: the hand packets waiting for their player's tick (player, packet, game
    /// time), and lecterns placed or removed.
    static INPUTS: RefCell<Vec<(u128, ToServer, i64)>> = const { RefCell::new(Vec::new()) };
    static CHANGES: RefCell<Vec<(usize, [i32; 3], bool, u32)>> = const { RefCell::new(Vec::new()) };
    /// Whether the level's tick has anything to do: frequencies held, a lectern in use, changes.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// The world closed (the server's state; the client's goes when it leaves a world).
pub(super) fn forget() {
    INPUTS.with_borrow_mut(Vec::clear);
    CHANGES.with_borrow_mut(Vec::clear);
    BUSY.set(false);
}

fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

fn is_controller(s: Option<&ItemStack>) -> bool {
    s.is_some_and(|s| &*s.item == ITEM)
}

fn sneaking(game: &Game<'_>) -> bool {
    game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
}

fn send_client(game: &mut Game<'_>, player: u128, m: ToClient) {
    let _ = game.native_net_send_to_player(player, ct::CHANNEL, &m.encode());
}

fn center(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

// ---------------------------------------------------------------------------------------------
// Client

/// `InputEvent.Key` (`LinkedControllerClientHandler`'s controls, which it sets up each tick):
/// while the controller is in use, its six keys are its buttons and their presses are kept from
/// the game (no walking, jumping or sneaking); the client tick reads them. The miss is one flag.
pub(super) fn input_key(
    key: &str,
    action: horizon_client::debug_keys::KeyAction,
    screen: bool,
    consumed: &mut bool,
) {
    if !ACTIVE.get() || screen {
        return;
    }
    if action != horizon_client::debug_keys::KeyAction::Release && ct::button(key).is_some() {
        *consumed = true;
    }
}

fn send_server(game: &mut Game<'_>, packets: Vec<ToServer>) {
    for p in packets {
        let _ = game.native_net_send_to_server(ct::CHANNEL, &p.encode());
    }
}

/// The server's word: the controller's use toggles it, a click on a link binds, a lectern
/// started or stopped being used by this player.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != ct::CHANNEL {
        return;
    }
    let Some(m) = ToClient::decode(data) else {
        return;
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
    send_server(game, packets);
    ACTIVE.set(CLIENT.with_borrow(Client::active));
}

/// `LinkedControllerClientHandler.tick`: idle again when a screen opens, escape is down, the
/// player is a spectator or no longer holds a controller (in neither hand, not at a lectern);
/// else the held buttons go to the server. Nothing runs while idle.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !ACTIVE.get() || !game.native_window() {
        return;
    }
    let lectern = CLIENT.with_borrow(|c| c.lectern.is_some());
    let holding = is_controller(game.inventory.get(game.selected).and_then(Option::as_ref))
        || is_controller(game.offhand.as_ref());
    let stop = !game.native_playing()
        || game.spectator()
        || game.native_is_key_down("key.keyboard.escape")
        || !lectern && !holding;
    // `isActuallyPressed`: the keys themselves, consumed or not.
    let held = if stop {
        0
    } else {
        (0..ct::KEYS.len())
            .filter(|&b| game.native_is_key_down(ct::KEYS[b]))
            .fold(0u8, |m, b| m | 1 << b)
    };
    let (packets, click) =
        CLIENT.with_borrow_mut(|c| if stop { (c.stop(), None) } else { c.tick(held) });
    send_server(game, packets);
    // `AllSoundEvents.CONTROLLER_CLICK` (the item frame's, at .35), for the player.
    if let Some(pitch) = click {
        game.audio
            .play("minecraft:entity.item_frame.add_item", None, 0.35, pitch);
    }
    ACTIVE.set(CLIENT.with_borrow(Client::active));
}

// ---------------------------------------------------------------------------------------------
// Server: the item

/// `LinkedControllerItem.use`: not sneaking, the controller toggles on its holder's client.
pub(super) fn use_item(game: &mut Game<'_>, stack: &ItemStack) {
    if &*stack.item != ITEM || sneaking(game) {
        return;
    }
    if let Some(p) = game.native_acting_player() {
        send_client(game, p.uuid, ToClient::Toggle);
    }
}

/// `LinkedControllerItem.onItemUseFirst`: on a redstone link the client binds a button to it;
/// on a lectern without a book the controller goes on it (a lectern controller). Elsewhere the
/// item's own use follows (`use_item`).
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    claimed: &mut bool,
) {
    if &*stack.item != ITEM || *claimed || !game.may_build() || sneaking(game) {
        return;
    }
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if block_of(state.0) == Some(Block::RedstoneLink) {
        *claimed = true;
        if let Some(p) = game.native_acting_player() {
            send_client(game, p.uuid, ToClient::BindMode(pos));
        }
        return;
    }
    let registry = game.pack.assets.registry;
    let Some(s) = registry.state(state) else {
        return;
    };
    if s.name != "minecraft:lectern"
        || s.properties.get("has_book").map(String::as_str) != Some("false")
    {
        return;
    }
    *claimed = true;
    let prop = |k: &str, or: &'static str| s.properties.get(k).cloned().unwrap_or(or.into());
    let (facing, powered) = (prop("facing", "north"), prop("powered", "false"));
    let Some(new) = state_with(
        registry,
        "create:lectern_controller",
        &[
            ("facing", facing.as_str()),
            ("powered", powered.as_str()),
            ("has_book", "true"),
        ],
    ) else {
        return;
    };
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    // `replaceLectern` and `setController` (a creative player keeps theirs).
    let items = body::controller_items(stack);
    with_world(|w| {
        w.dims[dim].lecterns.insert(
            pos,
            Lectern {
                items,
                ..Lectern::default()
            },
        )
    });
    if !game.creative {
        let i = game.selected;
        let slot = if game.hand == horizon_client::first_person::Hand::Off {
            &mut game.offhand
        } else {
            &mut game.inventory[i]
        };
        if let Some(s) = slot.as_mut() {
            s.count = s.count.saturating_sub(1);
            if s.count == 0 {
                *slot = None;
            }
        }
    }
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, new, super::native_hooks::flags::UPDATE_ALL);
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
    // `AllSoundEvents.CONTROLLER_PUT`: the book's.
    game.audio
        .play("minecraft:item.book.put", Some(center(pos)), 1.0, 1.0);
}

/// `create_debug controller <button> <first|-> [second|-]`: the controller in the main hand
/// takes a button's frequency (its screen, `LinkedControllerMenu`, stands in).
pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    commands.push(super::native_server::NativeCommand::op(
        "create_debug controller <button:integer> <items:text>",
        2,
    ));
}

/// An item id from a command word ("-" for none, `minecraft:` when it names none).
fn item_word(word: Option<&&str>) -> String {
    match word.copied() {
        None | Some("-") => String::new(),
        Some(w) if w.contains(':') => w.to_owned(),
        Some(w) => format!("minecraft:{w}"),
    }
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str) -> bool {
    let Some(rest) = command.strip_prefix("create_debug controller") else {
        return false;
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let line = match words.first().and_then(|w| w.parse::<u8>().ok()) {
        Some(b) if usize::from(b) < ct::KEYS.len() => {
            let key = (item_word(words.get(1)), item_word(words.get(2)));
            let i = game.selected;
            match game.inventory.get_mut(i).and_then(Option::as_mut) {
                Some(stack) if &*stack.item == ITEM => {
                    let mut items = body::controller_items(stack);
                    ct::bind(&mut items, b, &key);
                    body::set_controller_items(stack, &items);
                    format!("controller button {b}: {} + {}", key.0, key.1)
                }
                _ => "controller: hold a linked controller".to_owned(),
            }
        }
        _ => "usage: create_debug controller <button 0-5> <item|-> [item|-]".to_owned(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}

// ---------------------------------------------------------------------------------------------
// Server: packets

/// A player's packet (`LinkedControllerPacketBase.handle`): a lectern's at once, the hand's
/// in that player's next tick (where their hands are). The miss is the channel test.
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != ct::CHANNEL {
        return;
    }
    let Some(packet) = ToServer::decode(data) else {
        return;
    };
    let Some(p) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
    else {
        return;
    };
    let Some(dim) = dimension_index(&p.dimension) else {
        return;
    };
    let block_pos = p.position.map(|v| v.floor() as i32);
    match packet {
        ToServer::Input {
            buttons,
            press,
            lectern: Some(at),
        } => {
            // `handleLectern`: only its user's buttons, from the lectern's controller.
            let keys = with_world(|w| {
                let l = w.dims[dim].lecterns.get(&at)?;
                (l.user == Some(player)).then(|| {
                    ct::buttons(buttons)
                        .map(|b| ct::frequency(&l.items, b))
                        .collect::<Vec<_>>()
                })
            })
            .flatten();
            // `handleItem`: a spectator presses nothing.
            if let Some(keys) = keys
                && !(press && p.game_mode == 3)
            {
                with_world(|w| w.controller_input(dim, player, block_pos, &keys, press));
                BUSY.set(true);
            }
        }
        ToServer::StopLectern(at) => {
            let now = game.world.game_time;
            let stopped = with_world(|w| {
                let l = w.dims[dim].lecterns.get_mut(&at)?;
                (l.user == Some(player)).then(|| l.stop(now))
            })
            .flatten();
            if stopped.is_some() {
                send_client(game, player, ToClient::LecternOff(at));
            }
        }
        packet => {
            let now = game.world.game_time;
            INPUTS.with_borrow_mut(|i| i.push((player, packet, now)));
        }
    }
}

/// The end of a player's tick: their hand packets, with the controller in their main hand,
/// else the off hand (none: dropped, as Create's). Packets nobody's tick took for 40 ticks (a
/// player gone) are dropped. The miss is one test.
pub(super) fn player_tick(game: &mut Game<'_>) {
    if INPUTS.with_borrow(Vec::is_empty) {
        return;
    }
    let Some(me) = game.native_acting_player() else {
        return;
    };
    let now = game.world.game_time;
    let mine: Vec<ToServer> = INPUTS.with_borrow_mut(|i| {
        let mine = i.iter().filter(|x| x.0 == me.uuid).map(|x| x.1).collect();
        i.retain(|x| x.0 != me.uuid && now - x.2 < 40);
        mine
    });
    let Some(dim) = dimension_index(&me.dimension) else {
        return;
    };
    let spectator = me.game_mode == 3;
    for packet in mine {
        let i = game.selected;
        let slot = if is_controller(game.inventory.get(i).and_then(Option::as_ref)) {
            game.inventory.get_mut(i)
        } else if is_controller(game.offhand.as_ref()) {
            Some(&mut game.offhand)
        } else {
            None
        };
        let Some(stack) = slot.and_then(Option::as_mut) else {
            continue;
        };
        match packet {
            ToServer::Input { buttons, press, .. } => {
                if spectator && press {
                    continue;
                }
                let items = body::controller_items(stack);
                let keys: Vec<_> = ct::buttons(buttons)
                    .map(|b| ct::frequency(&items, b))
                    .collect();
                let at = me.position.map(|v| v.floor() as i32);
                with_world(|w| w.controller_input(dim, me.uuid, at, &keys, press));
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
                let mut items = body::controller_items(stack);
                ct::bind(&mut items, button, &key);
                body::set_controller_items(stack, &items);
                let label = horizon_client::text::Text::literal(ct::KEYS[usize::from(button)]);
                let line = horizon_client::text::Text::translate(
                    "create.linked_controller.key_bound",
                    &[label],
                );
                game.native_action_bar(me.uuid, line);
            }
            ToServer::StopLectern(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Server: lecterns

/// A click on a lectern controller (`LecternControllerBlock.useItemOn`, and the controller's
/// `onItemUseFirst` before it): sneaking with a controller swaps it for the lectern's; sneaking
/// otherwise takes the lectern's off (`replaceWithLectern`); else a player in range uses it.
pub(super) fn use_lectern(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    back: bool,
    canceled: &mut bool,
) {
    let Some(me) = game.native_acting_player() else {
        return;
    };
    let now = game.world.game_time;
    let i = game.selected;
    let off = game.hand == horizon_client::first_person::Hand::Off;
    let held = if off {
        game.offhand.as_ref()
    } else {
        game.inventory.get(i).and_then(Option::as_ref)
    };
    if back && is_controller(held) && game.may_build() {
        // `swapControllers`: the hand takes the lectern's, the lectern the hand's.
        *canceled = true;
        let new = held.map(body::controller_items).unwrap_or_default();
        let old = with_world(|w| {
            let l = w.dims[dim].lecterns.entry(pos).or_default();
            std::mem::replace(&mut l.items, new)
        })
        .unwrap_or_default();
        let mut stack = ItemStack::new(ITEM, 1);
        body::set_controller_items(&mut stack, &old);
        if off {
            game.offhand = Some(stack);
        } else {
            game.inventory[i] = Some(stack);
        }
        let dimension = game.dim.id().to_owned();
        game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
        game.audio
            .play("minecraft:item.book.put", Some(center(pos)), 1.0, 1.0);
        return;
    }
    if back {
        *canceled = true;
        replace_with_lectern(game, dim, pos);
        return;
    }
    let eyes = [me.position[0], me.position[1] + 1.62, me.position[2]];
    if !ct::in_lectern_range(eyes, pos) {
        return;
    }
    *canceled = true;
    let started = with_world(|w| {
        let busy = w.dims.iter().any(|d| {
            d.lecterns
                .iter()
                .any(|(p, l)| *p != pos && l.user == Some(me.uuid))
        });
        let l = w.dims[dim].lecterns.entry(pos).or_default();
        l.try_start(me.uuid, busy, true, now)
    })
    .unwrap_or(false);
    if started {
        BUSY.set(true);
        send_client(game, me.uuid, ToClient::LecternOn(pos));
    }
}

/// `replaceWithLectern`: back to a lectern (facing and power kept), the controller dropped
/// above it (`dropController`, its user stopped).
fn replace_with_lectern(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let registry = game.pack.assets.registry;
    let Some(s) = registry.state(state) else {
        return;
    };
    let prop = |k: &str, or: &'static str| s.properties.get(k).cloned().unwrap_or(or.into());
    let (facing, powered) = (prop("facing", "north"), prop("powered", "false"));
    let lectern = state_with(
        registry,
        "minecraft:lectern",
        &[
            ("facing", facing.as_str()),
            ("powered", powered.as_str()),
            ("has_book", "false"),
        ],
    );
    drop_controller(game, dim, pos, &facing);
    game.audio.play(
        "minecraft:entity.item_frame.remove_item",
        Some(center(pos)),
        1.0,
        1.0,
    );
    if let Some(lectern) = lectern {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(
            &dimension,
            pos,
            lectern,
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
}

/// `dropController`: the lectern's user stops, its controller drops a quarter block toward its
/// facing, one block up; the lectern is forgotten.
fn drop_controller(game: &mut Game<'_>, dim: usize, pos: [i32; 3], facing: &str) {
    let now = game.world.game_time;
    let Some(mut l) = with_world(|w| w.dims[dim].lecterns.remove(&pos)).flatten() else {
        return;
    };
    if let Some(user) = l.stop(now) {
        send_client(game, user, ToClient::LecternOff(pos));
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
    let mut stack = ItemStack::new(ITEM, 1);
    body::set_controller_items(&mut stack, &l.items);
    super::create_contraption_body::drop_stack(game, at, [0.0; 3], stack);
    let dimension = game.dim.id().to_owned();
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
}

/// A committed change on the server: a lectern controller placed or removed (one table
/// lookup per Create block changed).
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    let lectern = |s: u32| block_of(s) == Some(Block::LecternController);
    let (was, is) = (lectern(old), lectern(new));
    if was != is {
        CHANGES.with_borrow_mut(|c| c.push((dim, pos, is, old)));
        BUSY.set(true);
    }
}

/// The level's tick, before its links update: lecterns placed (by a command: an empty one) or
/// removed (the controller drops), lecterns' users out of range or gone stop
/// (`LecternControllerBlockEntity.tick`), held frequencies count down
/// (`LinkedControllerServerHandler.tick`). Nothing runs with none of these.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if !BUSY.get() {
        return;
    }
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
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let here = block_of(state) == Some(Block::LecternController);
        if placed && here {
            with_world(|w| {
                w.dims[dim].lecterns.entry(pos).or_default();
            });
        } else if !placed && !here {
            // Broken (`onRemove`): the lectern drops by its loot table, the controller here.
            let facing = game
                .pack
                .assets
                .registry
                .state(horizon_world::registry::BlockStateId(old))
                .and_then(|s| s.properties.get("facing").cloned())
                .unwrap_or_default();
            drop_controller(game, dim, pos, &facing);
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
        let players = game.native_server_players();
        let now = game.world.game_time;
        for (pos, user) in users {
            let stays = players.iter().any(|p| {
                p.uuid == user
                    && p.dimension == dimension
                    && ct::in_lectern_range(
                        [p.position[0], p.position[1] + 1.62, p.position[2]],
                        pos,
                    )
            });
            if !stays {
                with_world(|w| w.dims[dim].lecterns.get_mut(&pos).map(|l| l.stop(now)));
                send_client(game, user, ToClient::LecternOff(pos));
            }
        }
    }
    with_world(|w| w.tick_controllers(dim));
    let busy = with_world(|w| {
        w.dims
            .iter()
            .any(|d| !d.controllers.is_empty() || d.lecterns.values().any(|l| l.user.is_some()))
    })
    .unwrap_or(false);
    BUSY.set(busy || CHANGES.with_borrow(|c| !c.is_empty()));
}

// ---------------------------------------------------------------------------------------------
// Saved with the column

/// Lecterns of a column (`LecternControllerBlockEntity.write`: `ControllerData`; the user is
/// not kept, as Create's leaves it once its player is gone).
pub(super) fn chunk_data_save(dimension: &str, chunk: [i32; 2], data: &mut Compound) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
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
            Tag::compound([
                ("Pos", Tag::IntArray(p.to_vec())),
                (
                    "Items",
                    Tag::List(
                        items
                            .iter()
                            .map(|i| Tag::String(i.as_str().into()))
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    data.insert(KEY.into(), Tag::compound([("lecterns", Tag::List(tags))]));
}

/// A column joined a level: its lecterns back (`read`).
pub(super) fn chunk_load(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
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

/// A column left a level: its lecterns are forgotten (kept in its save).
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    with_world(|w| {
        w.dims[dim]
            .lecterns
            .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (chunk.x, chunk.z))
    });
}
