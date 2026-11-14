// Create's handheld items through the hooks (`create-engine` holds the rules: `filter`,
// `symmetry`, `worldshaper`, `potato`): filters in filter slots, the wand of symmetry copying
// placements and breaks, the handheld worldshaper, super glue's two clicks, tree fertilizer and
// the schedule. Their screens are stood in for by `/create_debug filter|symmetry|worldshaper|
// schedule_item` writing the held stack's settings. Free when unused: nothing ticks; a
// placement or a break looks at the hotbar by item pointer (cached once per item).
use super::Game;
use super::create::with_world;
use super::create_items_body as body;
use create_engine::belt_connector as bc;
use create_engine::filter::{self, Attribute, Filter, Mode, Passes, Probe};
use create_engine::kinetics::Dir;
use create_engine::symmetry::{self, Wand};
use create_engine::world::dimension_index;
use create_engine::worldshaper as ws;
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    /// Item pointers seen in the hotbar and whether each is a wand of symmetry: a placement
    /// compares pointers, never strings, once an item was seen.
    static WANDS: RefCell<Vec<(Arc<str>, bool)>> = const { RefCell::new(Vec::new()) };
    /// Each player's first glue corner (`SuperGlueSelectionHandler.firstPos`) and its
    /// dimension.
    static GLUE_FIRST: RefCell<Vec<(u128, String, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

const WAND_CACHE: usize = 32;

/// World close: forget what was seen.
pub(super) fn world_closed() {
    WANDS.with_borrow_mut(Vec::clear);
    GLUE_FIRST.with_borrow_mut(Vec::clear);
}

fn is_wand(stack: &ItemStack) -> bool {
    WANDS.with_borrow_mut(|cache| {
        if let Some((_, w)) = cache.iter().find(|(a, _)| Arc::ptr_eq(a, &stack.item)) {
            return *w;
        }
        let w = &*stack.item == symmetry::WAND;
        if cache.len() >= WAND_CACHE {
            cache.remove(0);
        }
        cache.push((stack.item.clone(), w));
        w
    })
}

fn sneaking(game: &Game<'_>) -> bool {
    game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
}

fn feedback(game: &mut Game<'_>, line: impl Into<String>) {
    game.native_feedback(horizon_client::text::Text::literal(line.into()));
}

fn action_bar(game: &mut Game<'_>, key: &str) {
    if let Some(p) = game.native_acting_player() {
        let line = horizon_client::text::Text::translate(key, &[]);
        game.native_action_bar(p.uuid, line);
    }
}

/// The hand acting (`game.hand`).
fn hand_slot<'a>(game: &'a mut Game<'_>) -> &'a mut Option<ItemStack> {
    if game.hand == horizon_client::first_person::Hand::Off {
        &mut game.offhand
    } else {
        let i = game.selected;
        &mut game.inventory[i]
    }
}

/// `shrink(1)` of the stack in the acting hand.
fn shrink_hand(game: &mut Game<'_>) {
    let slot = hand_slot(game);
    if let Some(s) = slot.as_mut() {
        s.count = s.count.saturating_sub(1);
        if s.count == 0 {
            *slot = None;
        }
    }
}

// --- Filters ---

/// `FilterItemStack.of` on a copy of `stack` (a filter item one of it): the compiled filter,
/// keeping the stack for its save and its return.
pub(super) fn filter_of(stack: &ItemStack) -> Filter {
    let mut s = stack.clone();
    if filter::FilterItem::of(&s.item).is_some() {
        s.count = 1;
    }
    Filter::compile(&body::spec(&s)).with_source(Rc::new(s))
}

/// The stack a filter came from (its item alone for one saved before filter items).
pub(super) fn filter_stack(f: &Filter) -> ItemStack {
    if let Some(s) = f.source::<ItemStack>() {
        return s.clone();
    }
    match &f.kind {
        filter::Kind::Item { item, .. } => ItemStack::new(item, 1),
        _ => ItemStack::new(filter::FILTER, 1),
    }
}

/// A filter as saved with its column: the stack's NBT.
pub(super) fn filter_to_nbt(f: &Filter) -> horizon_world::nbt::Tag {
    super::create_package_body::to_nbt(&filter_stack(f))
}

/// What `filter_to_nbt` wrote, or an item id saved before filter items.
pub(super) fn filter_from_nbt(tag: &horizon_world::nbt::Tag) -> Option<Filter> {
    match tag {
        horizon_world::nbt::Tag::String(s) => {
            let item = s.to_string_lossy();
            (!item.is_empty()).then(|| Filter::item(&item))
        }
        t => super::create_package_body::from_nbt(t).map(|s| filter_of(&s)),
    }
}

/// A stack as a filter sees it.
pub(super) struct Looked<'a, 'g> {
    pub(super) game: &'a Game<'g>,
    pub(super) stack: &'a ItemStack,
}

impl Probe for Looked<'_, '_> {
    fn item(&self) -> &str {
        &self.stack.item
    }
    fn components(&self) -> String {
        body::components(self.stack)
    }
    fn address(&self) -> String {
        super::create_package_body::address(self.stack)
    }
    fn attribute(&self, a: &Attribute) -> bool {
        let item = &*self.stack.item;
        let recipe = |pick: fn(&super::create::Recipes) -> &create_engine::processing::Book| {
            super::create::ensure_recipes(self.game)
                && super::create::RECIPES.with_borrow(|r| {
                    r.as_ref()
                        .is_some_and(|r| pick(r).find_single(item).is_some())
                })
        };
        match a {
            Attribute::Washable => recipe(|r| &r.splashing),
            Attribute::Hauntable => recipe(|r| &r.haunting),
            Attribute::Crushable => recipe(|r| &r.crushing) || recipe(|r| &r.milling),
            Attribute::Smeltable => recipe(|r| &r.smelting),
            Attribute::Smokable => recipe(|r| &r.smoking),
            Attribute::Blastable => recipe(|r| &r.blasting),
            _ => body::attribute(self.game, self.stack, a),
        }
    }
}

/// `FilteringBehaviour.test(stack)`: anything with no filter.
pub(super) fn passes(game: &Game<'_>, filter: Option<&Filter>, stack: &ItemStack) -> bool {
    filter.is_none_or(|f| f.test(&Looked { game, stack }))
}

/// The same for an item without components (on a belt, a block's item).
pub(super) fn passes_item(game: &Game<'_>, filter: Option<&Filter>, item: &str) -> bool {
    let Some(f) = filter else {
        return true;
    };
    // A plain item filter needs no stack.
    if matches!(f.kind, filter::Kind::Item { .. }) {
        return f.test(item);
    }
    f.test(&Looked {
        game,
        stack: &ItemStack::new(item, 1),
    })
}

/// A detector's filter with the game to read attributes.
pub(super) struct Detector<'a, 'g> {
    pub(super) game: &'a Game<'g>,
    pub(super) filter: Option<&'a Filter>,
}

impl Passes for Detector<'_, '_> {
    fn filtered(&self) -> bool {
        self.filter.is_some()
    }
    fn item(&self, item: &str) -> bool {
        passes_item(self.game, self.filter, item)
    }
    fn fluid(&self, fluid: create_engine::tank::Fluid) -> bool {
        self.filter.is_none_or(|f| f.test_fluid(fluid))
    }
}

/// `FilteringBehaviour.onShortInteract`'s hand: what the slot is set to (None: emptied), a
/// filter item taken from the hand outside creative.
pub(super) fn take_filter(game: &mut Game<'_>) -> Option<Filter> {
    let held = hand_slot(game).clone()?;
    let f = filter_of(&held);
    if f.is_filter_item() && !game.creative {
        shrink_hand(game);
    }
    Some(f)
}

/// The filter the slot held before, given back when it was a filter item (in creative only
/// when the inventory has none like it), and the frame sound.
pub(super) fn return_filter(game: &mut Game<'_>, pos: [i32; 3], old: Option<Filter>) {
    if let Some(old) = old.filter(Filter::is_filter_item) {
        let stack = filter_stack(&old);
        let has = game
            .inventory
            .iter()
            .flatten()
            .any(|s| super::create_contraption_body::same_stack(s, &stack));
        if !game.creative || !has {
            body::give_back(game, stack);
        }
    }
    let center = pos.map(|v| f64::from(v) + 0.5);
    game.audio.play(
        "minecraft:entity.item_frame.add_item",
        Some(center),
        0.25,
        0.1,
    );
}

/// `FilteringBehaviour.destroy`: filter items of broken blocks drop at their centre.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let dropped = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.dropped_filters.is_empty() && d.detectors.dropped.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut d.dropped_filters);
        v.append(&mut d.detectors.dropped);
        v
    })
    .unwrap_or_default();
    for (pos, f) in dropped {
        let at = pos.map(|v| f64::from(v) + 0.5);
        super::create_contraption_body::drop_stack(game, at, [0.0; 3], filter_stack(&f));
    }
}

// --- Commands ---

pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    // The filters' screens: a list (allow or deny, respecting components), attributes (any,
    // all or none of them) or a package address on the filter in a slot or hand.
    commands.push(super::native_server::NativeCommand::op(
        "create_debug filter <args:text>",
        2,
    ));
    // The wand of symmetry's screen and click: its mirror's place, type and orientation.
    commands.push(super::native_server::NativeCommand::op(
        "create_debug symmetry <args:text>",
        2,
    ));
    // The worldshaper's screen and its left click: brush, size, tool, placement, pattern and
    // block.
    commands.push(super::native_server::NativeCommand::op(
        "create_debug worldshaper <args:text>",
        2,
    ));
    // The schedule's screen (its stations and waits), and handing it to the train at a station
    // (a conductor's click: no conductors without entities).
    commands.push(super::native_server::NativeCommand::op(
        "create_debug schedule_item <args:text>",
        2,
    ));
    commands.push(super::native_server::NativeCommand::op(
        "create_debug hand_schedule <pos:block_pos>",
        2,
    ));
}

/// `hand`, `offhand` or an inventory slot (0 to 35).
fn slot_of<'a>(game: &'a mut Game<'_>, word: &str) -> Option<&'a mut Option<ItemStack>> {
    match word {
        "hand" => {
            let i = game.selected;
            game.inventory.get_mut(i)
        }
        "offhand" => Some(&mut game.offhand),
        n => game.inventory.get_mut(n.parse::<usize>().ok()?),
    }
}

/// Create's item commands; false for any other command.
pub(super) fn perform_command(game: &mut Game<'_>, command: &str) -> bool {
    let line = if let Some(rest) = command.strip_prefix("create_debug filter ") {
        debug_filter(game, rest)
    } else if let Some(rest) = command.strip_prefix("create_debug symmetry ") {
        debug_symmetry(game, rest)
    } else if let Some(rest) = command.strip_prefix("create_debug worldshaper ") {
        debug_worldshaper(game, rest)
    } else if let Some(rest) = command.strip_prefix("create_debug schedule_item ") {
        debug_schedule_item(game, rest)
    } else if let Some(rest) = command.strip_prefix("create_debug hand_schedule ") {
        hand_schedule(game, rest)
    } else {
        return false;
    };
    feedback(game, line);
    true
}

/// `create_debug filter <slot> list <allow|deny> [nbt] <item>...`,
/// `... attribute <any|all|none> <[!]type[=value]>...`, `... address <text>`.
fn debug_filter(game: &mut Game<'_>, rest: &str) -> String {
    const USAGE: &str = "usage: create_debug filter <hand|offhand|slot> list <allow|deny> [nbt] \
                         <item>... | attribute <any|all|none> <[!]attribute[=value]>... | address <text>";
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (Some(slot), Some(kind)) = (words.first(), words.get(1)) else {
        return USAGE.into();
    };
    let args = &words[2.min(words.len())..];
    let Some(stack) = slot_of(game, slot).and_then(Option::as_mut) else {
        return format!("filter: nothing in {slot}");
    };
    let want = match *kind {
        "list" => filter::FILTER,
        "attribute" => filter::ATTRIBUTE_FILTER,
        "address" => filter::PACKAGE_FILTER,
        _ => return USAGE.into(),
    };
    if &*stack.item != want {
        return format!("filter: {slot} holds {}, not {want}", stack.item);
    }
    match *kind {
        "list" => {
            let deny = match args.first() {
                Some(&"allow") => false,
                Some(&"deny") => true,
                _ => return USAGE.into(),
            };
            let nbt = args.get(1) == Some(&"nbt");
            let items: Vec<ItemStack> = args[if nbt { 2 } else { 1 }..]
                .iter()
                .map(|i| {
                    let id = if i.contains(':') {
                        (*i).to_owned()
                    } else {
                        format!("minecraft:{i}")
                    };
                    ItemStack::new(&id, 1)
                })
                .collect();
            if items.len() > filter::LIST_SLOTS {
                return format!("filter: at most {} items", filter::LIST_SLOTS);
            }
            body::set_list(stack, &items, nbt, deny);
            format!(
                "filter: {} list of {} items{}",
                if deny { "deny" } else { "allow" },
                items.len(),
                if nbt { ", respecting components" } else { "" }
            )
        }
        "attribute" => {
            let mode = match args.first() {
                Some(&"any") => Mode::WhitelistDisj,
                Some(&"all") => Mode::WhitelistConj,
                Some(&"none") => Mode::Blacklist,
                _ => return USAGE.into(),
            };
            let mut tests = Vec::new();
            for a in &args[1.min(args.len())..] {
                let (inverted, a) = match a.strip_prefix('!') {
                    Some(a) => (true, a),
                    None => (false, *a),
                };
                let (kind, value) = match a.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (a, None),
                };
                match Attribute::parse(kind, value) {
                    Ok(at) => tests.push((at, inverted)),
                    Err(filter::Refused::NotPorted(n)) => {
                        return format!("filter: the {n} attribute is not ported");
                    }
                    Err(filter::Refused::Unknown(n)) => return format!("filter: {n}?"),
                }
            }
            body::set_attributes(stack, mode, &tests);
            format!("filter: {} of {} attributes", mode.name(), tests.len())
        }
        _ => {
            let address = args.join(" ");
            super::create_package_body::set_address(
                stack,
                (!address.is_empty()).then_some(address.as_str()),
            );
            format!("filter: address \"{address}\"")
        }
    }
}

// --- Wand of symmetry ---

/// `create_debug symmetry <slot> <x> <y> <z> <plane|cross_plane|triple_plane|empty>
/// [orientation]`: the screen's mirror and orientation, at a place the wand's click would put
/// it, enabled as that click enables it.
fn debug_symmetry(game: &mut Game<'_>, rest: &str) -> String {
    const USAGE: &str = "usage: create_debug symmetry <hand|offhand|slot> <x> <y> <z> <plane|cross_plane|triple_plane|empty> [orientation]";
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() < 5 {
        return USAGE.into();
    }
    let here = game.native_acting_player().map(|p| p.position);
    let mut at = [0i32; 3];
    for i in 0..3 {
        let t = words[1 + i];
        at[i] = match t.strip_prefix('~') {
            Some(o) => here.map_or(0, |h| h[i].floor() as i32) + o.parse::<i32>().unwrap_or(0),
            None => match t.parse() {
                Ok(v) => v,
                Err(_) => return USAGE.into(),
            },
        };
    }
    let kind = symmetry::Kind::parse(words[4]);
    let orientation = words
        .get(5)
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    let Some(stack) = slot_of(game, words[0]).and_then(Option::as_mut) else {
        return format!("symmetry: nothing in {}", words[0]);
    };
    if &*stack.item != symmetry::WAND {
        return format!("symmetry: {} holds {}, not a wand", words[0], stack.item);
    }
    let mut w = body::wand(stack);
    w.kind = kind;
    w.position = at.map(f64::from);
    w.mirror_enable = true;
    w.set_orientation(orientation);
    w.enabled = kind != symmetry::Kind::Empty;
    body::set_wand(stack, &w);
    format!(
        "symmetry: {} at {} {} {} orientation {}",
        kind.name(),
        at[0],
        at[1],
        at[2],
        w.orientation
    )
}

/// `SymmetryWandItem.useOn` (not sneaking, main hand): the mirror moves before the clicked
/// face. Sneaking opens its screen on the client (the command stands in for it).
fn use_wand_on(game: &mut Game<'_>, pos: [i32; 3], face: [i32; 3], claimed: &mut bool) {
    *claimed = true;
    if sneaking(game) || game.hand == horizon_client::first_person::Hand::Off {
        return;
    }
    let (yaw, at) = (
        game.yaw,
        [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]],
    );
    let Some(stack) = hand_slot(game).as_mut() else {
        return;
    };
    let mut w = body::wand(stack);
    w.clicked(at, symmetry::faces_z(yaw), yaw);
    body::set_wand(stack, &w);
}

/// `SymmetryWandItem.use` not sneaking: the mirror off.
fn use_wand(game: &mut Game<'_>, claimed: &mut bool) {
    *claimed = true;
    if sneaking(game) {
        return;
    }
    if let Some(stack) = hand_slot(game).as_mut() {
        let mut w = body::wand(stack);
        w.used();
        body::set_wand(stack, &w);
    }
}

/// The wands in the hotbar (`SymmetryHandler`: every hotbar slot holding one).
fn hotbar_wands(game: &Game<'_>) -> Vec<Wand> {
    (0..9)
        .filter_map(|i| game.inventory.get(i)?.as_ref())
        .filter(|s| is_wand(s))
        .map(body::wand)
        .collect()
}

/// A state turned as a mirror turns it.
fn turned(game: &Game<'_>, state: BlockStateId, turns: &[symmetry::Turn]) -> BlockStateId {
    let registry = game.pack.assets.registry;
    let Some(mut s) = registry.state(state).cloned() else {
        return state;
    };
    let mut props: std::collections::BTreeMap<String, String> = s
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for t in turns {
        t.apply(&s.name, &mut props);
    }
    for (k, v) in props {
        s.properties.insert(k, v);
    }
    registry.id(&s).unwrap_or(state)
}

/// `SymmetryHandler.onBlockPlaced` → `SymmetryWandItem.apply`: each enabled wand in the hotbar
/// copies the placed state, turned, to its mirror images (creative: over anything; survival:
/// only into replaceable, breakable blocks, paying one of the block's item each).
pub(super) fn place_block(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    if !(0..9).any(|i| {
        game.inventory
            .get(i)
            .and_then(Option::as_ref)
            .is_some_and(is_wand)
    }) {
        return;
    }
    let Some(name) = game
        .pack
        .assets
        .registry
        .state(state)
        .map(|s| s.name.clone())
    else {
        return;
    };
    // `BlockItem.BY_BLOCK`: the block has an item of its own id.
    if game.items().get(&name).is_none() {
        return;
    }
    let dimension = game.dim.id().to_owned();
    for wand in hotbar_wands(game) {
        for copy in wand.copies(pos) {
            let to = turned(game, state, &copy.turns);
            let p = copy.pos;
            let to = game.with_level(|b, level, ticks| {
                b.update_from_neighbour_shapes(
                    level,
                    ticks,
                    horizon_core::position::BlockPos::new(p[0], p[1], p[2]),
                    to,
                )
            });
            if !game.creative {
                let here = game.world.block(p[0], p[1], p[2]);
                let b = game.res.behaviours();
                if !b.is_replaceable(here) || b.facts(here).hardness < 0.0 {
                    continue;
                }
                if game.native_take_items(&name, 1) == 0 {
                    continue;
                }
            }
            game.native_set_block(&dimension, p, to, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}

/// `SymmetryHandler.onBlockDestroyed` → `SymmetryWandItem.remove`: the mirror images of the
/// broken block break too (in survival only where the same block stands, its drops falling at
/// the broken block as Create drops them).
pub(super) fn destroy_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: bool,
) {
    if canceled
        || !(0..9).any(|i| {
            game.inventory
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(is_wand)
        })
    {
        return;
    }
    let dimension = game.dim.id().to_owned();
    let registry = game.pack.assets.registry;
    let block_of = |s: BlockStateId| registry.state(s).map(|s| s.name.clone());
    let og = block_of(state);
    let tool = game.inventory.get(game.selected).cloned().flatten();
    for wand in hotbar_wands(game) {
        for copy in wand.copies(pos) {
            let p = copy.pos;
            let here = game.world.block(p[0], p[1], p[2]);
            if !game.creative && block_of(here) != og {
                continue;
            }
            if game.res.behaviours().is_air(here) {
                continue;
            }
            let drops =
                super::create_contraption_body::break_block(game, &dimension, p, tool.as_ref());
            if !game.creative {
                let at = pos.map(|v| f64::from(v) + 0.5);
                for d in drops {
                    super::create_contraption_body::drop_stack(game, at, [0.0; 3], d);
                }
            }
        }
    }
}

// --- Worldshaper ---

/// `create_debug worldshaper <slot> <cuboid|sphere|cylinder> <size>... <tool> [placement]
/// [pattern] [block=<state>]`: the screen's settings (`configureSettings`) and the block its
/// left click picks.
fn debug_worldshaper(game: &mut Game<'_>, rest: &str) -> String {
    const USAGE: &str = "usage: create_debug worldshaper <hand|offhand|slot> <cuboid|sphere|cylinder> \
                         <size>... <fill|place|replace|clear|overlay> [merged|attached|inserted] \
                         [solid|checkered|inverse_checkered|chance25|chance50|chance75] [block=<state>]";
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (Some(slot), Some(brush)) = (
        words.first(),
        words.get(1).and_then(|b| ws::Brush::parse(b)),
    ) else {
        return USAGE.into();
    };
    if !brush.ported() {
        return format!("worldshaper: the {} brush is not ported", brush.name());
    }
    let n = brush.params();
    let mut params = [0i32; 3];
    for i in 0..n {
        let Some(v) = words.get(2 + i).and_then(|v| v.parse::<i32>().ok()) else {
            return USAGE.into();
        };
        let (lo, hi) = brush.bounds(i);
        params[i] = v.clamp(lo, hi);
    }
    let mut s = ws::Settings {
        brush,
        params: Some(params),
        ..ws::Settings::default()
    };
    let mut block = None;
    for w in &words[(2 + n).min(words.len())..] {
        if let Some(t) = ws::Tool::parse(w) {
            s.tool = t;
        } else if let Some(p) = ws::Placement::parse(w) {
            s.placement = p;
        } else if let Some(p) = ws::Pattern::parse(w) {
            s.pattern = p;
        } else if let Some(b) = w.strip_prefix("block=") {
            block = Some(b.to_owned());
        } else {
            return USAGE.into();
        }
    }
    if s.tool == ws::Tool::Flatten {
        return "worldshaper: the flatten tool is not ported".into();
    }
    // The left click's pick (`trySelect`): a state that can be picked, its age set to 0 when
    // used.
    let state = match &block {
        Some(b) => {
            let registry = game.pack.assets.registry;
            let Some(id) = super::create::state_of_text(registry, b) else {
                return format!("worldshaper: no block state {b}");
            };
            let Some(st) = registry.state(BlockStateId(id)).cloned() else {
                return format!("worldshaper: no block state {b}");
            };
            let props = st
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let Some(picked) = ws::selected_state(&props) else {
                return format!("worldshaper: {b} can't be picked");
            };
            let mut next = st.clone();
            for (k, v) in picked {
                next.properties.insert(k, v);
            }
            let id = registry.id(&next).map_or(id, |i| i.0);
            Some(super::create::state_text(registry, id))
        }
        None => None,
    };
    let Some(stack) = slot_of(game, slot).and_then(Option::as_mut) else {
        return format!("worldshaper: nothing in {slot}");
    };
    if &*stack.item != ws::WORLDSHAPER {
        return format!(
            "worldshaper: {slot} holds {}, not a worldshaper",
            stack.item
        );
    }
    body::set_shaper(stack, &s, state.as_deref());
    format!(
        "worldshaper: {} {:?} {} {} {}{}",
        s.brush.name(),
        &params[..n],
        s.tool.name(),
        s.placement.name(),
        s.pattern.name(),
        state.map(|b| format!(" with {b}")).unwrap_or_default()
    )
}

/// `ZapperInteractionHandler.leftClickingBlocksWithTheZapperSelectsTheBlock` → `trySelect`: the
/// worldshaper in the main hand (not sneaking) picks the block it looks at within 128 blocks,
/// unless it can't be picked (no item, a block entity: Create takes those of `create:safe_nbt`
/// with their data, which the port doesn't copy) or it already uses that state; the click
/// then breaks nothing.
fn select_block(game: &mut Game<'_>, canceled: &mut bool) {
    if sneaking(game) {
        return;
    }
    let Some((eye, look)) = game.aim() else {
        return;
    };
    let to = [0, 1, 2].map(|i| eye[i] + look[i] * ws::RANGE);
    let Some((hit, _, _)) = ws::clip(eye, to, |p| {
        let state = game.world.block(p[0], p[1], p[2]);
        game.res.facts().get(state).outline.to_vec()
    }) else {
        return;
    };
    let state = game.world.block(hit[0], hit[1], hit[2]);
    let registry = game.pack.assets.registry;
    let Some(st) = registry.state(state).cloned() else {
        return;
    };
    let has_entity = game.res.behaviours().facts(state).flags
        & horizon_world::level::data::state_flags::BLOCK_ENTITY
        != 0;
    if game.items().get(&st.name).is_none() || has_entity {
        return;
    }
    let props = st
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let Some(picked) = ws::selected_state(&props) else {
        return;
    };
    let mut next = st.clone();
    for (k, v) in picked {
        next.properties.insert(k, v);
    }
    let id = registry.id(&next).map_or(state.0, |i| i.0);
    let text = super::create::state_text(registry, id);
    let i = game.selected;
    let Some(stack) = game.inventory[i].as_mut() else {
        return;
    };
    let (settings, used) = body::shaper(stack);
    if used.as_deref() == Some(text.as_str()) {
        return;
    }
    body::set_shaper(stack, &settings, Some(&text));
    *canceled = true;
    if let Some(p) = game.native_acting_player() {
        game.audio
            .play("create:confirm", Some(p.position), 1.0, 1.0);
    }
}

/// A left click on a block with Create's items: the worldshaper's pick, a cardboard sword's
/// bonk (`cardboardSwordsMakeNoiseOnClick`).
pub(super) fn left_click_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: &mut bool,
) {
    let _ = state;
    let held = game.inventory.get(game.selected).and_then(Option::as_ref);
    match held.map(|s| &*s.item) {
        Some(ws::WORLDSHAPER) if !*canceled => select_block(game, canceled),
        Some("create:cardboard_sword") => {
            let at = pos.map(|v| f64::from(v) + 0.5);
            game.audio
                .play("create:cardboard_bonk", Some(at), 0.5, 1.85);
        }
        _ => {}
    }
}

/// `ZapperItem.use` (not sneaking) for the worldshaper: the block it looks at within 128
/// blocks is shaped by its brush and tool (`WorldshaperItem.activate`).
fn use_worldshaper(game: &mut Game<'_>, claimed: &mut bool) {
    *claimed = true;
    if sneaking(game) {
        return;
    }
    let Some(stack) = hand_slot(game).clone() else {
        return;
    };
    let (settings, block) = body::shaper(&stack);
    // `validateUsage`.
    let key = if settings.params.is_none() {
        Some("create.terrainzapper.shiftRightClickToSet")
    } else if settings.tool.requires_block() && block.is_none() {
        Some("create.terrainzapper.leftClickToSet")
    } else {
        None
    };
    if let Some(key) = key {
        action_bar(game, key);
        return;
    }
    let Some((eye, look)) = game.aim() else {
        return;
    };
    let to = [0, 1, 2].map(|i| eye[i] + look[i] * ws::RANGE);
    let hit = ws::clip(eye, to, |p| {
        let state = game.world.block(p[0], p[1], p[2]);
        game.res.facts().get(state).outline.to_vec()
    });
    let Some((hit, face, _)) = hit else {
        return;
    };
    if game
        .res
        .behaviours()
        .is_air(game.world.block(hit[0], hit[1], hit[2]))
    {
        return;
    }
    let registry = game.pack.assets.registry;
    let paint = match &block {
        Some(b) => super::create::state_of_text(registry, b).map(BlockStateId),
        None => registry.default_state("minecraft:air"),
    };
    let Some(paint) = paint else {
        return;
    };
    // `BlockHelper.setZeroAge`.
    let paint = registry
        .state(paint)
        .cloned()
        .and_then(|mut s| {
            let mut props = s
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            ws::zero_age(&mut props);
            for (k, v) in props {
                s.properties.insert(k, v);
            }
            registry.id(&s)
        })
        .unwrap_or(paint);
    let mut seed = (game.world.game_time as u64) ^ 0x2545_f491_4f6c_dd1d;
    let positions = ws::positions(&settings, hit, face, || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed & 1 == 1
    });
    let air = registry.default_state("minecraft:air");
    let writes = ws::run(settings.tool, &positions, |p| {
        let here = game.world.block(p[0], p[1], p[2]);
        ws::Seen {
            replaceable: game.res.behaviours().is_replaceable(here),
            painted: here == paint,
        }
    });
    let dimension = game.dim.id().to_owned();
    for (p, w) in writes {
        let to = match w {
            ws::Write::Paint => paint,
            ws::Write::Air => match air {
                Some(a) => a,
                None => continue,
            },
        };
        game.native_set_block(&dimension, p, to, super::native_hooks::flags::UPDATE_ALL);
    }
}

// --- Super glue ---

/// `SuperGlueSelectionHandler.onMouseInput` with glue in the main hand, on the server: the
/// first click picks a corner, the second glues the area between them as
/// `SuperGlueSelectionPacket` does (one charge of the glue outside creative); sneaking drops a
/// picked corner. The click is the glue's (the block isn't used: `glueItemAlwaysPlacesWhenUsed`),
/// except on a linear chassis' end, which the glue makes sticky.
pub(super) fn right_click_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if *canceled || game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let glue = game
        .inventory
        .get(game.selected)
        .and_then(Option::as_ref)
        .is_some_and(|s| &*s.item == "create:super_glue");
    if !glue || !game.may_build() {
        return;
    }
    let Some(player) = game.native_acting_player() else {
        return;
    };
    let first = GLUE_FIRST.with_borrow(|g| {
        g.iter()
            .find(|(u, d, _)| *u == player.uuid && *d == player.dimension)
            .map(|x| x.2)
    });
    if sneaking(game) {
        if first.is_some() {
            GLUE_FIRST.with_borrow_mut(|g| g.retain(|x| x.0 != player.uuid));
            action_bar(game, "create.super_glue.abort");
            *canceled = true;
        }
        return;
    }
    if let Some(s) = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        && matches!(
            s.name.as_str(),
            "create:linear_chassis" | "create:secondary_linear_chassis"
        )
    {
        let axis = s.properties.get("axis").map_or("y", String::as_str);
        let i = ["x", "y", "z"].iter().position(|a| *a == axis).unwrap_or(1);
        if face[i] != 0 {
            return;
        }
    }
    *canceled = true;
    let Some(first) = first else {
        GLUE_FIRST.with_borrow_mut(|g| {
            g.retain(|x| x.0 != player.uuid);
            g.push((player.uuid, player.dimension.clone(), pos));
        });
        action_bar(game, "create.super_glue.first_pos");
        return;
    };
    let line = super::create_actors::debug_glue(game, &player.dimension, first, pos);
    if line.starts_with("glued") {
        GLUE_FIRST.with_borrow_mut(|g| g.retain(|x| x.0 != player.uuid));
        // `collectGlueFromInventory(player, 1)`: a charge of the glue in hand.
        if !game.creative {
            super::create_equipment_body::damage_hand(game, false);
        }
        let center = pos.map(|v| f64::from(v) + 0.5);
        game.audio
            .play("create:slime_added", Some(center), 0.5, 0.95);
    }
}

// --- Cardboard sword ---

/// `EntityTypeTags.ARTHROPOD` (vanilla 26.3's `entity_type/arthropod`).
const ARTHROPODS: [&str; 5] = [
    "minecraft:bee",
    "minecraft:endermite",
    "minecraft:silverfish",
    "minecraft:spider",
    "minecraft:cave_spider",
];

/// `CardboardSwordItem.cardboardSwordsCannotHurtYou`: a cardboard sword in the main hand bonks a
/// living target (a player or a mob) other than an arthropod instead of hurting it. Its
/// knockback and the slowness it gives animals are not ported: no helper knocks a mob back as
/// `LivingEntity.knockback` does, or gives it an effect.
pub(super) fn attack_entity(
    game: &mut Game<'_>,
    entity: i32,
    kind: &str,
    pos: [f64; 3],
    canceled: &mut bool,
) {
    let sword = game
        .inventory
        .get(game.selected)
        .and_then(Option::as_ref)
        .is_some_and(|s| &*s.item == "create:cardboard_sword");
    if *canceled || !sword || ARTHROPODS.contains(&kind) {
        return;
    }
    let dimension = game.dim.id().to_owned();
    let living = kind == "minecraft:player"
        || u32::try_from(entity).is_ok_and(|id| game.native_mob(&dimension, id).is_some());
    if !living {
        return;
    }
    *canceled = true;
    let at = game.native_acting_player().map_or(pos, |p| p.position);
    game.audio
        .play("create:cardboard_bonk", Some(at), 0.75, 1.85);
}

// --- Item uses ---

/// Items used on a block: the wand of symmetry, tree fertilizer.
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    match &*stack.item {
        symmetry::WAND => use_wand_on(game, pos, face, claimed),
        "create:tree_fertilizer" => fertilize(game, pos, claimed),
        bc::ITEM => use_belt_connector(game, pos, claimed),
        _ => {}
    }
}

/// Items used in the air (or after a click on a block they let through): the wand of
/// symmetry, the worldshaper.
pub(super) fn use_item(game: &mut Game<'_>, stack: &ItemStack, claimed: &mut bool) {
    match &*stack.item {
        symmetry::WAND => use_wand(game, claimed),
        ws::WORLDSHAPER => use_worldshaper(game, claimed),
        _ => {}
    }
}

/// `TreeFertilizerItem.useOn` on a sapling (not a hanging propagule): its tree grows now. Create
/// grows it in a simulated world at stage 1; here the sapling is set to stage 1 and bone meal is
/// applied (vanilla's 45% rolls) until it grows, so the tree needs vanilla's room to grow.
fn fertilize(game: &mut Game<'_>, pos: [i32; 3], claimed: &mut bool) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let b = game.res.behaviours();
    let sapling = b
        .tag("minecraft:saplings")
        .is_some_and(|t| t.contains(b.block(state)));
    let Some(s) = game.pack.assets.registry.state(state).cloned() else {
        return;
    };
    if !sapling || s.properties.get("hanging").is_some_and(|h| h == "true") {
        return;
    }
    *claimed = true;
    if s.properties.contains_key("stage") {
        game.native_set_block_property_with(
            pos,
            "stage",
            "1",
            super::native_hooks::flags::UPDATE_CLIENTS,
        );
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    for _ in 0..64 {
        let meal = ItemStack::new("minecraft:bone_meal", 1);
        let used = super::dispenser::create_use::use_item_at(game, below, Dir::Up, meal);
        // Not a valid target (no room to grow), or grown.
        if !matches!(used, Some(None)) || game.world.block(pos[0], pos[1], pos[2]) != state {
            break;
        }
    }
    if !game.creative {
        shrink_hand(game);
    }
}

// --- Belt connector ---

/// The world as the belt connector asks it, on the level in place.
struct Ground<'a, 'g> {
    game: &'a Game<'g>,
    dim: usize,
}

impl bc::Ground for Ground<'_, '_> {
    fn loaded(&self, pos: [i32; 3]) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }

    fn shaft(&self, pos: [i32; 3]) -> Option<create_engine::kinetics::Axis> {
        let state = self.game.world.block(pos[0], pos[1], pos[2]).0;
        with_world(|w| match w.table.get(state) {
            Some((create_engine::Block::Shaft, info)) => info.axis,
            _ => None,
        })
        .flatten()
    }

    fn speed(&self, pos: [i32; 3]) -> f32 {
        with_world(|w| w.speed(self.dim, pos))
            .flatten()
            .unwrap_or(0.0)
    }

    fn replaceable(&self, pos: [i32; 3]) -> bool {
        let state = self.game.world.block(pos[0], pos[1], pos[2]);
        self.game.res.behaviours().is_replaceable(state)
    }
}

/// `BeltConnectorItem.useOn` on the server: sneaking forgets the first shaft; a click on a shaft
/// remembers it, a click on a second shaft that connects lays the belt (one belt connector used
/// up outside creative) and forgets it. Refusals say nothing, as in Create (its client draws the
/// red particles of `BeltConnectorHandler`). Not here: the 5-tick item cooldown and the `belt`
/// advancement (no hook gives either).
fn use_belt_connector(game: &mut Game<'_>, pos: [i32; 3], claimed: &mut bool) {
    *claimed = true;
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let player = game.native_acting_player().is_some();
    let first = hand_slot(game).as_ref().and_then(body::belt_first_shaft);
    let click = bc::click(&Ground { game, dim }, sneaking(game), first, pos, player);
    let set_first = |game: &mut Game<'_>, p: Option<[i32; 3]>| {
        if let Some(s) = hand_slot(game).as_mut() {
            body::set_belt_first_shaft(s, p);
        }
    };
    match click {
        bc::Click::Forget => set_first(game, None),
        bc::Click::Fail { forget } => {
            if forget {
                set_first(game, None);
            }
        }
        bc::Click::Remember => set_first(game, Some(pos)),
        bc::Click::Refused => {}
        bc::Click::Connect(start) => {
            create_belts(game, dim, start, pos);
            if !game.creative {
                shrink_hand(game);
            }
            set_first(game, None);
        }
    }
}

/// `createBelts`: the wool sound, then each block of the chain becomes its belt segment
/// (`switchToBlockState`, waterlogged in a water source; what can't be replaced, the shafts, is
/// broken without drops first); an unbreakable block on the way stops it and takes every belt
/// block laid away again.
fn create_belts(game: &mut Game<'_>, dim: usize, start: [i32; 3], end: [i32; 3]) {
    use horizon_world::fluid::FluidKind;
    let at = bc::sound_at(start, end).map(|v| f64::from(v) + 0.5);
    game.audio
        .play("minecraft:block.wool.place", Some(at), 0.5, 1.0);
    let ground = Ground { game, dim };
    let Some(axis) = bc::Ground::shaft(&ground, start) else {
        return;
    };
    let layout = bc::layout(start, end, axis, |p| bc::Ground::shaft(&ground, p));
    let registry = game.pack.assets.registry;
    let Some(belt) = registry
        .default_state("create:belt")
        .and_then(|id| registry.state(id).cloned())
    else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    let all = super::native_hooks::flags::UPDATE_ALL;
    let air = registry.default_state("minecraft:air");
    let water = registry.default_state("minecraft:water");
    let mut failed = false;
    for (pos, seg) in &layout {
        let pos = *pos;
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let b = game.res.behaviours();
        if b.facts(state).hardness < 0.0 {
            failed = true;
            break;
        }
        let waterlogged = b.fluid(state).kind() == FluidKind::Water;
        if !b.is_replaceable(state) {
            // `destroyBlock(pos, false)`: what stays is its fluid.
            if let Some(id) = if waterlogged { water } else { air } {
                game.native_set_block(&dimension, pos, id, all);
            }
        }
        let mut s = belt.clone();
        for (k, v) in [
            ("facing", super::create::dir_name(seg.facing)),
            ("slope", bc::slope_name(seg.slope)),
            ("part", bc::part_name(seg.part)),
            ("waterlogged", if waterlogged { "true" } else { "false" }),
        ] {
            s.properties.insert(k.to_owned(), v.to_owned());
        }
        if let Some(id) = registry.id(&s)
            && game.world.block(pos[0], pos[1], pos[2]) != id
        {
            game.native_set_block(&dimension, pos, id, all);
        }
    }
    if !failed {
        return;
    }
    for (pos, _) in layout {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let is_belt = with_world(|w| {
            w.table
                .get(state.0)
                .is_some_and(|(b, _)| b == create_engine::Block::Belt)
        });
        if is_belt == Some(true) {
            let wet = game.res.behaviours().fluid(state).kind() == FluidKind::Water;
            if let Some(id) = if wet { water } else { air } {
                game.native_set_block(&dimension, pos, id, all);
            }
        }
    }
}

// --- Schedule ---

/// `create_debug schedule_item <slot> <station[@seconds]>...`: the schedule's screen.
fn debug_schedule_item(game: &mut Game<'_>, rest: &str) -> String {
    let mut words = rest.split_whitespace();
    let Some(slot) = words.next() else {
        return "usage: create_debug schedule_item <hand|offhand|slot> <station[@seconds]>..."
            .into();
    };
    let entries: Vec<(String, i32)> = words
        .map(|e| match e.rsplit_once('@') {
            Some((n, s)) => (n.to_owned(), s.parse::<i32>().unwrap_or(5).max(0)),
            None => (e.to_owned(), 5),
        })
        .collect();
    let Some(stack) = slot_of(game, slot).and_then(Option::as_mut) else {
        return format!("schedule: nothing in {slot}");
    };
    if &*stack.item != "create:schedule" {
        return format!("schedule: {slot} holds {}, not a schedule", stack.item);
    }
    body::set_schedule(stack, &entries);
    format!("schedule: {} stops", entries.len())
}

/// `create_debug hand_schedule <station>`: the held schedule handed to the train at that
/// station as a conductor takes it (`ScheduleItem.handScheduleTo`): refused with a schedule
/// already running or no stops; else the train runs it and the schedule is used up.
fn hand_schedule(game: &mut Game<'_>, rest: &str) -> String {
    let here = game.native_acting_player().map(|p| p.position);
    let parts: Vec<i32> = rest
        .split_whitespace()
        .enumerate()
        .filter_map(|(n, t)| match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        })
        .collect();
    let [x, y, z] = parts[..] else {
        return "usage: create_debug hand_schedule <x> <y> <z>".into();
    };
    let dim = game
        .native_acting_player()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let held = game.inventory.get(game.selected).cloned().flatten();
    let Some(entries) = held
        .as_ref()
        .filter(|s| &*s.item == "create:schedule")
        .and_then(body::schedule)
    else {
        return "schedule: no schedule in hand".into();
    };
    let text: Vec<String> = entries.iter().map(|(n, w)| format!("{n}@{w}")).collect();
    match super::create_trains::hand_schedule(dim, [x, y, z], &text.join(" ")) {
        Ok(line) => {
            action_bar(game, "create.schedule.applied_to_train");
            let center = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
            game.audio.play("create:confirm", Some(center), 1.0, 1.0);
            let i = game.selected;
            if let Some(s) = game.inventory[i].as_mut() {
                s.count -= 1;
                if s.count == 0 {
                    game.inventory[i] = None;
                }
            }
            line
        }
        Err(key) => {
            action_bar(game, key);
            key.to_owned()
        }
    }
}
