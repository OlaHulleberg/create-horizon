//! The factory gauge's screens and messages. `FactoryPanelScreen`: the panel's recipe (the
//! panels feeding it, with the amount each sends: the wheel changes one, a click disconnects
//! it; or the 3x3 crafting arrangement when a crafting recipe makes the output from them), its
//! output count, the address inputs go to, when promises expire, and its promises (a click
//! clears them); `+` starts joining another panel to it, the bin resets it. Closing sends the
//! settings (`FactoryPanelConfigurationPacket`). `FactoryPanelSetItemScreen`: a panel without an
//! item takes a ghost of one picked from the player's inventory.
//!
//! As the redstone requester's, the screens draw the player's inventory themselves (a click
//! picks a ghost, nothing moves). The move button starts relocating the panel
//! (`FactoryPanelConnectionHandler.startRelocating`, `gauge_client`): the next click puts it
//! where the crosshair is (`FactoryPanelBehaviour.moveTo`, the `R` message).

use super::super::net;
use super::super::server::ServerLevel;
use super::gauge::{self, Config};
use super::gauge_client::{self, Shown};
use super::keeper::{Reader, put_i32, put_stack, put_str};
use super::{Key, with};
use create_engine::factory as fx;
use create_engine::stock::PanelPos;
use mod_api::{
    ClientHost, DrawContext, Flow, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Text,
};
use std::cell::RefCell;

/// Both ways: see each message's letter.
pub(crate) const CHANNEL: &str = "create:factory_panel";
const MOD: &str = "create";

fn put_pp(m: &mut Vec<u8>, pp: PanelPos) {
    for v in pp.0 {
        put_i32(m, v);
    }
    m.push(pp.1);
}

fn read_pp(r: &mut Reader<'_>) -> Option<PanelPos> {
    Some((r.pos()?, r.u8()?.min(3)))
}

// ---------------------------------------------------------------------------------------------
// Server

/// `FactoryPanelSetItemMenu` opened (`openMenu`): the panel and the player's inventory by slot.
pub(super) fn open_set_item(lv: &mut ServerLevel<'_>, player: PlayerRef, pp: PanelPos) {
    let mut m = vec![b'O'];
    put_pp(&mut m, pp);
    let inventory: Vec<(u32, ItemStack)> = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .collect();
    m.push(inventory.len() as u8);
    for (i, s) in &inventory {
        m.push(*i as u8);
        put_stack(&*lv.host, &mut m, s, s.count());
    }
    net::send_to_player(lv.host, player, CHANNEL, &m);
}

fn say(host: &mut dyn ServerHost, player: PlayerRef, key: &str, args: Vec<Text>, color: &str) {
    let mut style = mod_api::text::TextStyle::default();
    if !color.is_empty() {
        style.color = Some(color.to_owned());
    }
    let _ = player.action_bar(host, Text::translate(key, args).with_style(style));
}

/// A message from a player's screen or clicks, checked as a client's packet is (in reach).
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some((&kind, rest)) = data.split_first() else {
        return;
    };
    let mut r = Reader(rest);
    let Some((dim, dimension)) = gauge::dim_of(&*host, player) else {
        return;
    };
    let Some(at) = player.entity().position(&*host) else {
        return;
    };
    let near = |p: [i32; 3]| {
        let d2: f64 = (0..3)
            .map(|i| (at[i] - f64::from(p[i]) - 0.5).powi(2))
            .sum();
        d2 <= 24.0 * 24.0
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    match kind {
        // The set-item screen's ghost.
        b'I' => {
            let Some(pp) = read_pp(&mut r) else { return };
            if !near(pp.0) {
                return;
            }
            let Some((stack, _)) = r.stack(&*lv.host) else {
                return;
            };
            let id = stack.item().id(&*lv.host);
            if matches!(
                id.as_str(),
                "create:filter" | "create:attribute_filter" | "create:package_filter"
            ) {
                say(
                    lv.host,
                    player,
                    "create.logistics.filter.invalid_item",
                    Vec::new(),
                    "",
                );
                return;
            }
            gauge::set_filter(&mut lv, pp, Some(Key::of(&stack)));
        }
        // The configuration screen closed.
        b'K' => {
            let Some(pp) = read_pp(&mut r) else { return };
            if !near(pp.0) {
                return;
            }
            let Some(address) = r.str() else { return };
            let Some(n) = r.u8() else { return };
            let mut inputs = Vec::new();
            for _ in 0..n {
                let (Some(from), Some(amount)) = (read_pp(&mut r), r.i32()) else {
                    return;
                };
                inputs.push((from, amount.max(1) as u32));
            }
            let Some(n) = r.u8() else { return };
            let mut craft = Vec::new();
            for _ in 0..n {
                let Some((s, _)) = r.stack(&*lv.host) else {
                    return;
                };
                craft.push(Key::of(&s));
            }
            let (Some(output), Some(clearing), Some(flags)) = (r.i32(), r.i32(), r.u8()) else {
                return;
            };
            let remove = if flags & 1 != 0 {
                read_pp(&mut r)
            } else {
                None
            };
            let c = Config {
                address,
                inputs,
                craft,
                output: output.max(1) as u32,
                clearing,
                remove,
                clear_promises: flags & 2 != 0,
                reset: flags & 4 != 0,
                links_reset: flags & 8 != 0,
            };
            gauge::configure(&mut lv, player, pp, c);
        }
        // `FactoryPanelConnectionHandler.clientTick`: the message while joining or relocating.
        b'S' => {
            let _ = read_pp(&mut r);
            let key = if r.u8() == Some(1) {
                "create.factory_panel.click_to_relocate"
            } else {
                "create.factory_panel.click_second_panel"
            };
            say(lv.host, player, key, Vec::new(), "");
        }
        // `FactoryPanelConnectionPacket` with `relocate`: the panel `from` moves to `to`.
        b'R' => {
            let (Some(to), Some(from)) = (read_pp(&mut r), read_pp(&mut r)) else {
                return;
            };
            if !near(to.0) || !near(from.0) {
                return;
            }
            gauge::relocate(&mut lv, player, from, to);
        }
        // Joining aborted: by clicking the same panel (an empty message), or elsewhere; or
        // relocating given up.
        b'A' => {
            let key = match r.u8() {
                Some(1) => "create.factory_panel.connection_aborted",
                Some(2) => "create.factory_panel.relocation_aborted",
                _ => "",
            };
            if key.is_empty() {
                let _ = player.action_bar(lv.host, Text::literal(""));
            } else {
                say(lv.host, player, key, Vec::new(), "");
            }
        }
        // `FactoryPanelConnectionPacket`: `from` feeds `to` (`checkForIssues` first).
        b'L' => {
            let (Some(from), Some(to)) = (read_pp(&mut r), read_pp(&mut r)) else {
                return;
            };
            if !near(to.0) {
                return;
            }
            connect(&mut lv, player, from, to);
        }
        // `FactoryPanelConnectionHandler.onRightClick` on a display link: `checkForIssues`, then
        // `FactoryPanelConnectionPacket` with the link's nearest slot.
        b'D' => {
            let Some(pp) = read_pp(&mut r) else { return };
            let (Some(x), Some(y), Some(z)) = (r.i32(), r.i32(), r.i32()) else {
                return;
            };
            let link = [x, y, z];
            if !near(link) {
                return;
            }
            // A display link or a redstone link (`FactoryPanelSupportBehaviour` there).
            let (link_facing, redstone) = match crate::glue::display::display_link_facing(&lv, link)
            {
                Some(f) => (f, false),
                None => match crate::glue::redstone::link_facing(&lv, link) {
                    Some(f) => (f, true),
                    None => return,
                },
            };
            match gauge::connect_link(&mut lv, pp, link, link_facing, redstone) {
                Err(key) => {
                    say(lv.host, player, &format!("create.{key}"), Vec::new(), "red");
                    let at = player.entity().position(&*lv.host).unwrap_or([0.0; 3]);
                    let _ = lv.level.play_sound(
                        &mut *lv.host,
                        "create:deny",
                        at,
                        mod_api::SoundSource::Blocks,
                        1.0,
                        1.0,
                    );
                }
                Ok(()) => {
                    let name = Text::translate(
                        if redstone {
                            "block.create.redstone_link"
                        } else {
                            "block.create.display_link"
                        },
                        Vec::new(),
                    );
                    say(
                        lv.host,
                        player,
                        "create.factory_panel.link_connected",
                        vec![name],
                        "green",
                    );
                    let _ = player.play_sound(
                        lv.host,
                        "minecraft:block.amethyst_block.place",
                        mod_api::SoundSource::Blocks,
                        0.5,
                        0.5,
                    );
                }
            }
        }
        // A short click on a panel with an item.
        b'H' => {
            let Some(pp) = read_pp(&mut r) else { return };
            let wrench = r.u8() == Some(1);
            if !near(pp.0)
                || !super::super::interaction::values::may_use_value_box(&*lv.host, player)
            {
                return;
            }
            gauge::short_interact(&mut lv, player, mod_api::Hand::Main, pp, wrench);
        }
        // The target amount's board.
        b'V' => {
            let Some(pp) = read_pp(&mut r) else { return };
            let (Some(row), Some(value)) = (r.i32(), r.i32()) else {
                return;
            };
            if !near(pp.0)
                || matches!(
                    player.game_mode(&*lv.host),
                    Some(mod_api::GameMode::Adventure | mod_api::GameMode::Spectator)
                )
            {
                return;
            }
            gauge::set_amount(&mut lv, pp, row, value.clamp(0, 100));
        }
        _ => {}
    }
    let _ = dim;
}

/// `panelClicked`'s checks and message, then the connection.
fn connect(lv: &mut ServerLevel<'_>, player: PlayerRef, from: PanelPos, to: PanelPos) {
    let dim = lv.dim;
    let info = with(dim, |n| {
        let f = n.gauges.get(&from)?;
        let t = n.gauges.get(&to)?;
        Some((
            f.inputs.iter().map(|i| i.from).collect::<Vec<_>>(),
            t.restocker,
            f.filter.clone(),
            t.filter.clone(),
        ))
    });
    // `checkForIssues(at, panel)`: `at` is the panel joining (`from`, which will be fed), and
    // the clicked panel feeds it.
    let Some((inputs, clicked_restocker, from_item, to_item)) = info else {
        say(
            lv.host,
            player,
            "create.factory_panel.connection_aborted",
            Vec::new(),
            "red",
        );
        return;
    };
    let (Some(fo), Some(to_o)) = (gauge::orientation(lv, from.0), gauge::orientation(lv, to.0))
    else {
        return;
    };
    let issue = fx::connection_issue(
        &inputs,
        to,
        from.0,
        fo == to_o,
        fo.0,
        fo.1,
        clicked_restocker,
        from_item.is_some() && to_item.is_some(),
    );
    if let Some(key) = issue {
        say(lv.host, player, &format!("create.{key}"), Vec::new(), "red");
        let at = player.entity().position(&*lv.host).unwrap_or([0.0; 3]);
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "create:deny",
            at,
            mod_api::SoundSource::Blocks,
            1.0,
            1.0,
        );
        return;
    }
    // The clicked panel (`to` here) feeds the one joining.
    if !gauge::connect(lv, to, from) {
        return;
    }
    let name = |s: &Option<Key>| {
        s.as_ref()
            .map(|k| Text::translate(name_key(&*lv.host, &k.0), Vec::new()))
            .unwrap_or_else(|| Text::literal(""))
    };
    let args = vec![name(&to_item), name(&from_item)];
    say(
        lv.host,
        player,
        "create.factory_panel.panels_connected",
        args,
        "green",
    );
    let at = player.entity().position(&*lv.host).unwrap_or([0.0; 3]);
    let _ = player.play_sound(
        lv.host,
        "minecraft:block.amethyst_block.place",
        mod_api::SoundSource::Blocks,
        0.5,
        0.5,
    );
    let _ = at;
}

/// An item's name key (`getDescriptionId`): `block.<ns>.<path>` for a block's item.
fn name_key(host: &dyn mod_api::Host, stack: &ItemStack) -> String {
    let id = stack.item().id(host);
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", &id));
    let kind = if host.item_block(stack.item()).is_some() {
        "block"
    } else {
        "item"
    };
    format!("{kind}.{ns}.{path}")
}

// ---------------------------------------------------------------------------------------------
// Client

/// What a screen shows.
enum Screen {
    /// `FactoryPanelScreen`.
    Config(Box<ConfigScreen>),
    /// `FactoryPanelSetItemScreen`.
    SetItem {
        pp: PanelPos,
        ghost: Option<ItemStack>,
        carried: Option<ItemStack>,
        inventory: Vec<(u32, ItemStack)>,
    },
}

struct ConfigScreen {
    pp: PanelPos,
    panel: Shown,
    restocker: bool,
    address: String,
    editing: bool,
    /// The inputs: their panels, items and amounts (`inputConfig`).
    inputs: Vec<(PanelPos, Option<ItemStack>, u32)>,
    output: u32,
    clearing: i32,
    /// `availableCraftingRecipe`'s arrangement and output count, and whether it is on.
    recipe: Option<(Vec<Option<ItemStack>>, u32)>,
    crafting: bool,
    remove: Option<PanelPos>,
    clear_promises: bool,
    reset: bool,
    /// `sendRedstoneReset`: the links joined to it let go.
    links_reset: bool,
    sent: bool,
}

struct Open {
    screen: Screen,
    size: [f32; 2],
}

/// What the screen does at the next client tick: messages to send, whether it closes, and a
/// panel to start joining or relocating from (`true`: relocating).
type Later = (Vec<Vec<u8>>, bool, Option<(PanelPos, bool)>);

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Messages to send and whether to close, at the next client tick.
    static LATER: RefCell<Later> =
        const { RefCell::new((Vec::new(), false, None)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false, None));
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let Some((&b'O', rest)) = data.split_first() else {
        return true;
    };
    let mut r = Reader(rest);
    let Some(pp) = read_pp(&mut r) else {
        return true;
    };
    let mut inventory = Vec::new();
    for _ in 0..r.u8().unwrap_or(0) {
        let (Some(i), Some((s, n))) = (r.u8(), r.stack(&*host)) else {
            break;
        };
        if let Some(s) = super::body::with_count(&s, n) {
            inventory.push((u32::from(i), s));
        }
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            screen: Screen::SetItem {
                pp,
                ghost: None,
                carried: None,
                inventory,
            },
            size: [0.0; 2],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

/// `searchForCraftingRecipe` and `convertRecipeToPackageOrderContext`: a crafting recipe making
/// the output from the inputs' items (each used), laid out 3x3.
fn find_recipe(
    host: &dyn ClientHost,
    output: &ItemStack,
    inputs: &[(PanelPos, Option<ItemStack>, u32)],
) -> Option<(Vec<Option<ItemStack>>, u32)> {
    let out_id = output.item().id(host);
    let items: Vec<&ItemStack> = inputs.iter().filter_map(|i| i.1.as_ref()).collect();
    if items.is_empty() {
        return None;
    }
    let mut distinct: Vec<String> = items.iter().map(|s| s.item().id(host)).collect();
    distinct.sort();
    distinct.dedup();
    for kind in ["minecraft:crafting_shaped", "minecraft:crafting_shapeless"] {
        for r in crate::glue::processing::recipes::crafting_of(host, kind).iter() {
            let Some(result) = r.results.first() else {
                continue;
            };
            if result.item().id(host) != out_id {
                continue;
            }
            // Shaped recipes keep their grid: rows of the pattern, `ingredients` in order with
            // empty cells.
            let (width, height, cells) = shaped_cells(r);
            let mut used: Vec<String> = Vec::new();
            let mut grid: Vec<Option<ItemStack>> = Vec::new();
            let mut ok = true;
            for cell in &cells {
                match cell {
                    None => grid.push(None),
                    Some(ing) => match items.iter().find(|s| ing.test(host, s)) {
                        Some(s) => {
                            used.push(s.item().id(host));
                            grid.push(super::body::with_count(s, 1));
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    },
                }
            }
            if !ok {
                continue;
            }
            used.sort();
            used.dedup();
            if used.len() < distinct.len() {
                continue;
            }
            // `convertRecipeToPackageOrderContext`: the grid padded to 3x3.
            let mut out = Vec::new();
            if height == 1 {
                out.extend([None, None, None]);
            }
            if width == 1 {
                out.push(None);
            }
            for (i, c) in grid.into_iter().enumerate() {
                out.push(c);
                if width < 3 && (i + 1) % width == 0 {
                    for _ in 0..3 - width {
                        if out.len() < 9 {
                            out.push(None);
                        }
                    }
                }
            }
            out.resize(9, None);
            out.truncate(9);
            return Some((out, result.count()));
        }
    }
    None
}

/// A crafting recipe's width, height and cells (`ShapedRecipe.getWidth`, `getHeight`; a
/// shapeless one's ingredients in a row of up to three).
fn shaped_cells(r: &mod_api::Recipe) -> (usize, usize, Vec<Option<&mod_api::Ingredient>>) {
    if let Some(pattern) = r.json["pattern"].as_array() {
        let rows: Vec<&str> = pattern.iter().filter_map(|p| p.as_str()).collect();
        let width = rows
            .iter()
            .map(|r| r.chars().count())
            .max()
            .unwrap_or(1)
            .max(1);
        let height = rows.len().max(1);
        let mut it = r.ingredients.iter();
        let mut cells = Vec::new();
        for row in &rows {
            for c in row
                .chars()
                .chain(std::iter::repeat_n(' ', width - row.chars().count()))
            {
                cells.push(if c == ' ' { None } else { it.next() });
            }
        }
        return (width, height, cells);
    }
    let n = r.ingredients.len();
    let width = n.clamp(1, 3);
    let height = (n / 3 + 1).min(3);
    (width, height, r.ingredients.iter().map(Some).collect())
}

/// `FactoryPanelBehaviour.displayScreen`: the configuration screen of panel `pp`.
pub(super) fn open_config(host: &mut dyn ClientHost, pp: PanelPos) {
    let Some(panel) = gauge_client::panel(host, pp) else {
        return;
    };
    let inputs: Vec<(PanelPos, Option<ItemStack>, u32)> = panel
        .inputs
        .iter()
        .map(|i| {
            let item = gauge_client::panel(host, i.from).and_then(|p| p.filter);
            (i.from, item, i.amount)
        })
        .collect();
    let recipe = panel
        .filter
        .as_ref()
        .filter(|_| !inputs.is_empty())
        .and_then(|f| find_recipe(host, f, &inputs));
    let crafting = !panel.craft.is_empty() && recipe.is_some();
    let screen = ConfigScreen {
        pp,
        restocker: panel.restocker,
        address: panel.address.clone(),
        editing: false,
        output: panel.output,
        clearing: panel.clearing,
        inputs,
        recipe,
        crafting,
        remove: None,
        clear_promises: false,
        reset: false,
        links_reset: false,
        sent: false,
        panel,
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            screen: Screen::Config(Box::new(screen)),
            size: [0.0; 2],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

/// `sendIt`: the settings as `FactoryPanelConfigurationPacket` carries them.
fn settings(host: &dyn ClientHost, c: &ConfigScreen) -> Vec<u8> {
    let mut m = vec![b'K'];
    put_pp(&mut m, c.pp);
    put_str(&mut m, &c.address);
    m.push(c.inputs.len() as u8);
    for (from, item, amount) in &c.inputs {
        put_pp(&mut m, *from);
        // With crafting on, each input sends as many as the arrangement uses.
        let n = match (&c.recipe, c.crafting, item) {
            (Some((grid, _)), true, Some(item)) => grid
                .iter()
                .flatten()
                .filter(|g| g.is_stackable_with(item))
                .count() as i32,
            _ => *amount as i32,
        };
        put_i32(&mut m, n);
    }
    let craft: Vec<&ItemStack> = match (&c.recipe, c.crafting) {
        (Some((grid, _)), true) => grid.iter().flatten().collect(),
        _ => Vec::new(),
    };
    // An empty cell is sent as nothing; the order keeps its place with the grid's own.
    m.push(craft.len() as u8);
    for s in craft {
        put_stack(host, &mut m, s, 1);
    }
    put_i32(&mut m, c.output as i32);
    put_i32(&mut m, c.clearing);
    let flags = u8::from(c.remove.is_some())
        | (u8::from(c.clear_promises) << 1)
        | (u8::from(c.reset) << 2)
        | (u8::from(c.links_reset) << 3);
    m.push(flags);
    if let Some(r) = c.remove {
        put_pp(&mut m, r);
    }
    m
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close, connect) =
        LATER.with_borrow_mut(|l| (std::mem::take(&mut l.0), l.1, l.2.take()));
    LATER.with_borrow_mut(|l| l.1 = false);
    if close {
        host.mod_screen_close(MOD);
    }
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if let Some((pp, relocate)) = connect {
        gauge_client::start_connection(host, pp, relocate);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(o) = OPEN.with_borrow_mut(Option::take)
    {
        let m = closing_message(&*host, &o);
        if let Some(m) = m {
            net::send_to_server(host, CHANNEL, &m);
        }
    }
}

fn closing_message(host: &dyn ClientHost, o: &Open) -> Option<Vec<u8>> {
    match &o.screen {
        Screen::Config(c) if !c.sent => Some(settings(host, c)),
        Screen::SetItem {
            pp, ghost: Some(g), ..
        } => {
            let mut m = vec![b'I'];
            put_pp(&mut m, *pp);
            put_stack(host, &mut m, g, 1);
            Some(m)
        }
        _ => None,
    }
}

// Layout ---------------------------------------------------------------------------------------

const BOTTOM: [f32; 4] = [32.0, 176.0, 200.0, 64.0];
const RECIPE: [f32; 4] = [32.0, 0.0, 192.0, 96.0];
const RESTOCK: [f32; 4] = [32.0, 112.0, 192.0, 40.0];
const TEX: &str = "create:gui/factory_gauge";

fn window(c: &ConfigScreen) -> [f32; 2] {
    let top = if c.restocker { RESTOCK[3] } else { RECIPE[3] };
    [BOTTOM[2], top + BOTTOM[3]]
}

fn corner(size: [f32; 2], w: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - w[0]) / 2.0).floor(),
        ((size[1] - w[1]) / 2.0).floor(),
    )
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn input_at(x: f32, y: f32, restocker: bool, i: usize) -> [f32; 2] {
    if restocker {
        [x + 88.0, y + 12.0]
    } else {
        [
            x + 68.0 + (i % 3) as f32 * 20.0,
            y + 28.0 + (i / 3) as f32 * 20.0,
        ]
    }
}

struct Rects {
    address: [f32; 4],
    confirm: [f32; 4],
    delete: [f32; 4],
    promise: [f32; 4],
    connect: [f32; 4],
    relocate: [f32; 4],
    craft: [f32; 4],
    output: [f32; 4],
    promises: [f32; 4],
    links: [f32; 4],
}

fn rects(x: f32, y: f32, w: [f32; 2]) -> Rects {
    Rects {
        address: [x + 36.0, y + w[1] - 51.0, 108.0, 10.0],
        confirm: [x + w[0] - 33.0, y + w[1] - 25.0, 18.0, 18.0],
        delete: [x + w[0] - 55.0, y + w[1] - 25.0, 18.0, 18.0],
        promise: [x + 97.0, y + w[1] - 24.0, 28.0, 16.0],
        connect: [x + 31.0, y + 47.0, 18.0, 18.0],
        relocate: [x + 31.0, y + 67.0, 18.0, 18.0],
        craft: [x + 31.0, y + 27.0, 18.0, 18.0],
        output: [x + 160.0, y + 48.0, 16.0, 16.0],
        promises: [x + 68.0, y + w[1] - 24.0, 16.0, 16.0],
        links: [x + 9.0, y + w[1] - 24.0, 16.0, 16.0],
    }
}

fn icon_button(
    draw: &mut dyn DrawContext,
    r: [f32; 4],
    icon: [f32; 2],
    green: bool,
    hovered: bool,
) {
    let u = if hovered {
        18.0
    } else if green {
        72.0
    } else {
        0.0
    };
    draw.sprite("create:gui/widgets", [u, 0.0, 18.0, 18.0], r, None);
    draw.sprite(
        "create:gui/icons",
        [icon[0], icon[1], 16.0, 16.0],
        [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
        None,
    );
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

fn blink() -> bool {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
        / 300)
        .is_multiple_of(2)
}

fn styled(text: String, color: &str, italic: bool) -> Text {
    let mut style = mod_api::text::TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text).with_style(style)
}

/// `ScrollInput.HEADER_RGB`.
const HEADER: &str = "#5391E1";

fn render_config(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    c: &ConfigScreen,
    size: [f32; 2],
    mouse: [f32; 2],
) {
    let w = window(c);
    let (x, y) = corner(size, w);
    let r = rects(x, y, w);
    let line = |k: &str, f: &str| Text::literal(tr(host, k, f));
    let head = |k: &str, f: &str| styled(tr(host, k, f), HEADER, false);
    let gray = |k: &str, f: &str| styled(tr(host, k, f), "gray", false);
    let hint = |k: &str, f: &str| styled(tr(host, k, f), "dark_gray", true);
    // Background.
    if c.restocker {
        draw.sprite(TEX, RECIPE, [x, y - 16.0, RECIPE[2], RECIPE[3]], None);
        draw.sprite(TEX, RESTOCK, [x, y, RESTOCK[2], RESTOCK[3]], None);
    } else {
        draw.sprite(TEX, RECIPE, [x, y, RECIPE[2], RECIPE[3]], None);
    }
    let top = if c.restocker { RESTOCK[3] } else { RECIPE[3] };
    draw.sprite(TEX, BOTTOM, [x, y + top, BOTTOM[2], BOTTOM[3]], None);
    let mut tip: Option<Vec<Text>> = None;
    // Recipe.
    let crafting = c.crafting && c.recipe.is_some();
    if crafting {
        if let Some((grid, _)) = &c.recipe {
            for (i, s) in grid.iter().enumerate() {
                let at = input_at(x, y, false, i);
                if let Some(s) = s {
                    draw.gui_draw_item_with(s, at, Some(""));
                }
                if inside([at[0] - 2.0, at[1] - 2.0, 20.0, 20.0], mouse) {
                    tip = Some(vec![
                        head(
                            "create.gui.factory_panel.crafting_input",
                            "Packaged Ingredients",
                        ),
                        gray(
                            "create.gui.factory_panel.crafting_input_tip",
                            "Unpackage into connected",
                        ),
                        gray(
                            "create.gui.factory_panel.crafting_input_tip_1",
                            "mechanical crafters (3x3)",
                        ),
                    ]);
                }
            }
        }
    } else if !c.restocker {
        for (i, (_, item, amount)) in c.inputs.iter().enumerate() {
            let at = input_at(x, y, false, i);
            if let Some(s) = item {
                draw.gui_draw_item_with(s, at, Some(&amount.to_string()));
            }
            if inside([at[0] - 2.0, at[1] - 2.0, 20.0, 20.0], mouse) {
                tip = Some(match item {
                    None => vec![
                        head("create.gui.factory_panel.empty_panel", "Empty panel"),
                        hint(
                            "create.gui.factory_panel.left_click_disconnect",
                            "Left-Click to disconnect",
                        ),
                    ],
                    Some(s) => {
                        let name = format!("{} x{amount}", host.hover_name(s));
                        let sending = tr(
                            host,
                            "create.gui.factory_panel.sending_item",
                            "Sending %1$s",
                        )
                        .replace("%1$s", &name);
                        vec![
                            styled(sending, HEADER, false),
                            hint(
                                "create.gui.factory_panel.scroll_to_change_amount",
                                "Scroll to change amount",
                            ),
                            hint(
                                "create.gui.factory_panel.left_click_disconnect",
                                "Left-Click to disconnect",
                            ),
                        ]
                    }
                });
            }
        }
        if c.inputs.is_empty() {
            let at = input_at(x, y, false, 0);
            if mouse[1] > at[1]
                && mouse[1] < at[1] + 60.0
                && mouse[0] > at[0]
                && mouse[0] < at[0] + 60.0
            {
                tip = Some(vec![
                    head(
                        "create.gui.factory_panel.unconfigured_input",
                        "Recipe ingredients",
                    ),
                    gray(
                        "create.gui.factory_panel.unconfigured_input_tip",
                        "Ingredients can be added by",
                    ),
                    gray(
                        "create.gui.factory_panel.unconfigured_input_tip_1",
                        "connecting other factory gauges",
                    ),
                ]);
            }
        }
    }
    if c.restocker
        && let Some(f) = &c.panel.filter
    {
        let at = input_at(x, y, true, 0);
        draw.gui_draw_item_with(f, at, Some(""));
        if inside([at[0] - 2.0, at[1] - 2.0, 20.0, 20.0], mouse) {
            let sending = tr(
                host,
                "create.gui.factory_panel.sending_item",
                "Sending %1$s",
            )
            .replace("%1$s", &host.hover_name(f));
            tip = Some(vec![
                styled(sending, HEADER, false),
                gray(
                    "create.gui.factory_panel.sending_item_tip",
                    "whenever local stock level",
                ),
                gray(
                    "create.gui.factory_panel.sending_item_tip_1",
                    "is below the target amount",
                ),
            ]);
        }
    }
    // Output.
    if !c.restocker {
        if let Some(f) = &c.panel.filter {
            draw.gui_draw_item_with(f, [r.output[0], r.output[1]], Some(&c.output.to_string()));
            if inside([r.output[0] - 1.0, r.output[1] - 1.0, 18.0, 18.0], mouse) {
                let name = format!("{} x{}", host.hover_name(f), c.output);
                let expecting = tr(
                    host,
                    "create.gui.factory_panel.expected_output",
                    "Expecting %1$s",
                )
                .replace("%1$s", &name);
                let mut lines = vec![
                    styled(expecting, HEADER, false),
                    gray(
                        "create.gui.factory_panel.expected_output_tip",
                        "The amount of output returning",
                    ),
                    gray(
                        "create.gui.factory_panel.expected_output_tip_1",
                        "after each successful request",
                    ),
                ];
                if !crafting {
                    lines.push(hint(
                        "create.gui.factory_panel.expected_output_tip_2",
                        "Scroll to change",
                    ));
                }
                tip = Some(lines);
            }
        }
        // `+` and the crafting toggle.
        icon_button(draw, r.connect, [0.0, 0.0], false, inside(r.connect, mouse));
        if inside(r.connect, mouse) {
            tip = Some(vec![line(
                "create.gui.factory_panel.connect_input",
                "Add new connection",
            )]);
        }
        // `I_MOVE_GAUGE`.
        icon_button(
            draw,
            r.relocate,
            [64.0, 192.0],
            false,
            inside(r.relocate, mouse),
        );
        if inside(r.relocate, mouse) {
            tip = Some(vec![line("create.gui.factory_panel.relocate", "Relocate")]);
        }
        if c.recipe.is_some() {
            icon_button(
                draw,
                r.craft,
                [32.0, 0.0],
                c.crafting,
                inside(r.craft, mouse),
            );
            if inside(r.craft, mouse) {
                tip = Some(vec![line(
                    "create.gui.factory_panel.activate_crafting",
                    "Use Mechanical Crafting",
                )]);
            }
        }
    }
    // Address.
    let mut text = c.address.clone();
    if c.editing && blink() {
        text.push('_');
    }
    draw.text(
        &text,
        [r.address[0], r.address[1] + 1.0],
        0xFF55_5555,
        false,
    );
    if inside(r.address, mouse) && !c.editing {
        tip = Some(if c.address.trim().is_empty() {
            if c.restocker {
                vec![
                    head(
                        "create.gui.factory_panel.restocker_address",
                        "Send items to...",
                    ),
                    gray(
                        "create.gui.factory_panel.restocker_address_tip",
                        "Enter the address that will",
                    ),
                    gray(
                        "create.gui.factory_panel.restocker_address_tip_1",
                        "cause packages to arrive here.",
                    ),
                    hint("create.gui.schedule.lmb_edit", "Left-Click to Edit"),
                ]
            } else {
                vec![
                    head(
                        "create.gui.factory_panel.recipe_address",
                        "Send inputs to...",
                    ),
                    gray(
                        "create.gui.factory_panel.recipe_address_tip",
                        "Enter the address where",
                    ),
                    gray(
                        "create.gui.factory_panel.recipe_address_tip_1",
                        "this recipe is crafted.",
                    ),
                    hint("create.gui.schedule.lmb_edit", "Left-Click to Edit"),
                ]
            }
        } else {
            let k = if c.restocker {
                "create.gui.factory_panel.restocker_address_given"
            } else {
                "create.gui.factory_panel.recipe_address_given"
            };
            vec![
                head(k, "Sending to"),
                styled(format!("'{}'", c.address), "gray", false),
            ]
        });
    }
    // Title.
    let title = if c.restocker {
        tr(
            host,
            "create.gui.factory_panel.title_as_restocker",
            "Restocker Settings",
        )
    } else {
        tr(
            host,
            "create.gui.factory_panel.title_as_recipe",
            "Recipe Settings",
        )
    };
    let tw = draw.text_width(&title);
    let ty = y + if c.restocker { -12.0 } else { 4.0 };
    draw.text(
        &title,
        [x + 97.0 - (tw / 2.0).floor(), ty],
        0xFF3D_3C48,
        false,
    );
    // The gauge, large, with its item.
    let preview = if c.restocker { 0.0 } else { 60.0 };
    if let Some(k) = item_key(host, "create:factory_gauge") {
        draw.item(k, [x + 195.0, y + 55.0 + preview - 64.0 + 16.0], 64.0);
    }
    if let Some(f) = &c.panel.filter {
        draw.item_stack(f, [x + 214.0, y + 68.0 + preview - 26.0 + 13.0], 26.0);
    }
    // Links joined to it (`REDSTONE LINKS`: the redstone link's item in a frogport slot).
    if !c.panel.links.is_empty() {
        let l = r.links;
        draw.sprite(
            "create:gui/frogport_and_mailbox",
            [26.0, 55.0, 18.0, 18.0],
            [l[0] - 1.0, l[1] - 1.0, 18.0, 18.0],
            None,
        );
        if let Some(k) = item_key(host, "create:redstone_link") {
            draw.item(k, [l[0], l[1]], 16.0);
        }
        if inside(l, mouse) {
            tip = Some(vec![
                styled(
                    tr(
                        host,
                        "create.gui.factory_panel.has_link_connections",
                        "Gauge has connected links",
                    ),
                    HEADER,
                    false,
                ),
                styled(
                    tr(
                        host,
                        "create.gui.factory_panel.left_click_disconnect",
                        "Left-Click to disconnect",
                    ),
                    "dark_gray",
                    true,
                ),
            ]);
        }
    }
    // Promises expire after.
    let state = c.clearing;
    let label = match state {
        -1 => " /".to_owned(),
        0 => "30s".to_owned(),
        m => format!("{m}m"),
    };
    draw.text(
        &label,
        [r.promise[0] + 3.0, r.promise[1] + 4.0],
        0xFFEE_EEEE,
        true,
    );
    if inside(r.promise, mouse) {
        let title = if state == -1 {
            tr(
                host,
                "create.gui.factory_panel.promises_do_not_expire",
                "Promises do not expire",
            )
        } else {
            tr(
                host,
                "create.gui.factory_panel.promises_expire_title",
                "Promises expire after",
            )
        };
        tip = Some(vec![
            styled(title, HEADER, false),
            hint("create.gui.scrollInput.scrollToModify", "Scroll to Modify"),
        ]);
    }
    // Promises.
    if let Some(k) = item_key(host, "create:cardboard_package_10x12") {
        draw.item(k, [r.promises[0], r.promises[1]], 16.0);
        let n = c.panel.promised.to_string();
        let nw = draw.text_width(&n);
        draw.text(
            &n,
            [r.promises[0] + 17.0 - nw, r.promises[1] + 9.0],
            0xFFFF_FFFF,
            true,
        );
    }
    if inside(r.promises, mouse) {
        tip = Some(if c.panel.promised == 0 {
            let k = if c.restocker { "restocker" } else { "recipe" };
            vec![
                head(
                    "create.gui.factory_panel.no_open_promises",
                    "No open promises",
                ),
                gray(
                    &format!("create.gui.factory_panel.{k}_promises_tip"),
                    "When inputs are sent, a promise",
                ),
                gray(
                    &format!("create.gui.factory_panel.{k}_promises_tip_1"),
                    "is held until outputs arrive.",
                ),
                gray(
                    "create.gui.factory_panel.promise_prevents_oversending",
                    "This prevents over-sending.",
                ),
            ]
        } else {
            let name = c
                .panel
                .filter
                .as_ref()
                .map(|f| host.hover_name(f))
                .unwrap_or_default();
            vec![
                head("create.gui.factory_panel.promised_items", "Promised items"),
                Text::literal(format!("{name} x{}", c.panel.promised)),
                hint(
                    "create.gui.factory_panel.left_click_reset",
                    "Left-Click to reset",
                ),
            ]
        });
    }
    // Delete and confirm.
    icon_button(draw, r.delete, [16.0, 0.0], false, inside(r.delete, mouse));
    icon_button(
        draw,
        r.confirm,
        [0.0, 16.0],
        false,
        inside(r.confirm, mouse),
    );
    if inside(r.delete, mouse) {
        tip = Some(vec![line(
            "create.gui.factory_panel.reset",
            "Reset all settings",
        )]);
    }
    if inside(r.confirm, mouse) {
        tip = Some(vec![line(
            "create.gui.factory_panel.save_and_close",
            "Save and close",
        )]);
    }
    if let Some(lines) = tip {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// `FACTORY_GAUGE_SET_ITEM` over the player's inventory.
const SET_W: f32 = 184.0;
const SET_H: f32 = 88.0 + 108.0;

fn set_item_slots(x: f32, y: f32) -> Vec<(u32, [f32; 4])> {
    let mut out = Vec::new();
    for row in 0..3 {
        for col in 0..9 {
            let at = [x + 13.0 + col as f32 * 18.0, y + 112.0 + row as f32 * 18.0];
            out.push((9 + row * 9 + col, [at[0], at[1], 16.0, 16.0]));
        }
    }
    for col in 0..9 {
        out.push((col, [x + 13.0 + col as f32 * 18.0, y + 170.0, 16.0, 16.0]));
    }
    out
}

fn render_set_item(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    ghost: &Option<ItemStack>,
    carried: &Option<ItemStack>,
    inventory: &[(u32, ItemStack)],
    size: [f32; 2],
    mouse: [f32; 2],
) {
    let (x, y) = corner(size, [SET_W, SET_H]);
    draw.sprite(
        "create:gui/requester",
        [16.0, 160.0, 184.0, 88.0],
        [x - 5.0, y, 184.0, 88.0],
        None,
    );
    draw.sprite(
        "create:gui/player_inventory",
        [0.0, 0.0, 176.0, 108.0],
        [x + 5.0, y + 94.0, 176.0, 108.0],
        None,
    );
    let inv = tr(host, "container.inventory", "Inventory");
    draw.text(&inv, [x + 13.0, y + 100.0], 0xFF40_4040, false);
    let title = tr(
        host,
        "create.gui.factory_panel.place_item_to_monitor",
        "Place Item to Monitor",
    );
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [x + SET_W / 2.0 - tw / 2.0 - 5.0, y + 4.0],
        0xFF3D_3C48,
        false,
    );
    if let Some(k) = item_key(host, "create:factory_gauge") {
        draw.item(k, [x + 180.0, y + 48.0 - 24.0], 48.0);
    }
    let slot = [x + 74.0, y + 28.0, 16.0, 16.0];
    if let Some(g) = ghost {
        draw.gui_draw_item_with(g, [slot[0], slot[1]], None);
    }
    if inside(slot, mouse) {
        draw.fill(slot, 0x80FF_FFFF);
    }
    for (i, r) in set_item_slots(x, y) {
        if let Some((_, s)) = inventory.iter().find(|(k, _)| *k == i) {
            draw.gui_draw_item_with(s, [r[0], r[1]], None);
        }
        if inside(r, mouse) {
            draw.fill(r, 0x80FF_FFFF);
        }
    }
    let confirm = [x + SET_W - 40.0, y + 88.0 - 25.0, 18.0, 18.0];
    icon_button(draw, confirm, [0.0, 16.0], false, inside(confirm, mouse));
    if let Some(c) = carried {
        draw.item_stack(c, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
    }
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        o.size = size;
        match &o.screen {
            Screen::Config(c) => render_config(host, draw, c, size, mouse),
            Screen::SetItem {
                ghost,
                carried,
                inventory,
                ..
            } => render_set_item(host, draw, ghost, carried, inventory, size, mouse),
        }
    });
}

fn click(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

fn close() {
    LATER.with_borrow_mut(|l| l.1 = true);
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        let size = o.size;
        match &mut o.screen {
            Screen::SetItem {
                ghost,
                carried,
                inventory,
                ..
            } => {
                let (x, y) = corner(size, [SET_W, SET_H]);
                let confirm = [x + SET_W - 40.0, y + 88.0 - 25.0, 18.0, 18.0];
                if inside(confirm, mouse) {
                    click(host);
                    close();
                    return;
                }
                if inside([x + 74.0, y + 28.0, 16.0, 16.0], mouse) {
                    *ghost = carried.as_ref().and_then(|c| super::body::with_count(c, 1));
                    return;
                }
                if let Some((i, _)) = set_item_slots(x, y)
                    .into_iter()
                    .find(|(_, r)| inside(*r, mouse))
                {
                    let stack = inventory
                        .iter()
                        .find(|(k, _)| *k == i)
                        .map(|(_, s)| s.clone());
                    *carried = if carried.is_some() { None } else { stack };
                    return;
                }
                *carried = None;
            }
            Screen::Config(c) => {
                let w = window(c);
                let (x, y) = corner(size, w);
                let r = rects(x, y, w);
                c.editing = inside(r.address, mouse);
                if c.editing {
                    return;
                }
                // Remove connections.
                if !c.crafting {
                    for i in 0..c.inputs.len() {
                        let at = input_at(x, y, false, i);
                        if inside([at[0], at[1], 16.0, 16.0], mouse) {
                            c.remove = Some(c.inputs[i].0);
                            click(host);
                            let m = settings(host, c);
                            c.remove = None;
                            c.inputs.remove(i);
                            LATER.with_borrow_mut(|l| l.0.push(m));
                            return;
                        }
                    }
                }
                // Remove the links' connections (`sendRedstoneReset`).
                if inside(r.links, mouse) {
                    c.links_reset = true;
                    click(host);
                    let m = settings(host, c);
                    c.links_reset = false;
                    c.panel.links.clear();
                    LATER.with_borrow_mut(|l| l.0.push(m));
                    return;
                }
                // Clear promises.
                if inside(r.promises, mouse) {
                    c.clear_promises = true;
                    click(host);
                    let m = settings(host, c);
                    c.clear_promises = false;
                    c.panel.promised = 0;
                    LATER.with_borrow_mut(|l| l.0.push(m));
                    return;
                }
                if inside(r.confirm, mouse) {
                    click(host);
                    close();
                    return;
                }
                if inside(r.delete, mouse) {
                    click(host);
                    c.reset = true;
                    close();
                    return;
                }
                if !c.restocker && inside(r.connect, mouse) {
                    click(host);
                    LATER.with_borrow_mut(|l| l.2 = Some((c.pp, false)));
                    close();
                    return;
                }
                if !c.restocker && inside(r.relocate, mouse) {
                    click(host);
                    LATER.with_borrow_mut(|l| l.2 = Some((c.pp, true)));
                    close();
                    return;
                }
                if !c.restocker && c.recipe.is_some() && inside(r.craft, mouse) {
                    click(host);
                    c.crafting = !c.crafting;
                    if c.crafting
                        && let Some((_, n)) = &c.recipe
                    {
                        c.output = *n;
                    }
                }
            }
        }
    });
    Flow::Cancel
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let step = if event.modifiers & 1 != 0 { 10 } else { 1 };
    let up = event.scroll[1] > 0.0;
    let delta = if up { step } else { -step };
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        let size = o.size;
        let Screen::Config(c) = &mut o.screen else {
            return;
        };
        let w = window(c);
        let (x, y) = corner(size, w);
        let r = rects(x, y, w);
        if inside(r.promise, mouse) {
            c.clearing = (c.clearing + if up { 1 } else { -1 }).clamp(-1, 31);
            return;
        }
        if c.crafting {
            return;
        }
        for i in 0..c.inputs.len() {
            let at = input_at(x, y, false, i);
            if inside([at[0], at[1] - 2.0, 16.0, 16.0], mouse) {
                if c.inputs[i].1.is_some() {
                    c.inputs[i].2 = (c.inputs[i].2 as i32 + delta).clamp(1, 64) as u32;
                }
                return;
            }
        }
        if !c.restocker && inside(r.output, mouse) {
            c.output = (c.output as i32 + delta).clamp(1, 64) as u32;
        }
    });
    Flow::Cancel
}

pub(super) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let editing = OPEN.with_borrow(|o| {
        o.as_ref()
            .is_some_and(|o| matches!(&o.screen, Screen::Config(c) if c.editing))
    });
    if !editing {
        return if event.key == "key.keyboard.e" {
            close();
            Flow::Cancel
        } else {
            Flow::Continue
        };
    }
    match event.key {
        "key.keyboard.backspace" => OPEN.with_borrow_mut(|o| {
            if let Some(Open {
                screen: Screen::Config(c),
                ..
            }) = o.as_mut()
            {
                c.address.pop();
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" | "key.keyboard.tab" => OPEN
            .with_borrow_mut(|o| {
                if let Some(Open {
                    screen: Screen::Config(c),
                    ..
                }) = o.as_mut()
                {
                    c.editing = false;
                }
            }),
        "key.keyboard.escape" => return Flow::Continue,
        _ => {}
    }
    Flow::Cancel
}

pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(Open {
            screen: Screen::Config(s),
            ..
        }) = o.as_mut()
            && s.editing
            && s.address.chars().count() < 50
        {
            s.address.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: the settings (or the ghost) go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(o) = OPEN.with_borrow_mut(Option::take)
        && let Some(m) = closing_message(host, &o)
    {
        LATER.with_borrow_mut(|l| l.0.push(m));
    }
}
