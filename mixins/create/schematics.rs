// Create's schematics (`content/schematics`): the schematic and quill's save, the schematic
// table's upload, a schematic deployed, and the schematicannon printing it. Rules are the
// engine's (`create_engine::schematic`, `create_engine::schematicannon`); files, stacks and items
// go through `create_schematic_body`.
//
// Cannons are typed state here, ticked only when one needs it (`Cannon::next_wake`); a
// schematic is parsed and its palette resolved to block states once per load; NBT only at a
// chunk's save.

use super::Game;
use super::create::*;
use super::create_logistics::*;
use super::create_schematic_body as body;
use create_engine::Block;
use create_engine::schematic::{
    self as sch, Class, Mirror, Need, PaletteState, Rotation, SchematicData, Structure,
};
use create_engine::schematicannon::{
    self as cn, Aim, Blueprint, Cannon, CannonWorld, Configure, Entry, Loaded, Options, Printer,
    Requirement, Spot,
};
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_core::geometry::Direction;
use horizon_world::level::data::state_flags;
use horizon_world::nbt::{Compound, Tag};
use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const KEY: &str = "create_schematics";
pub(super) const SCHEMATIC: &str = "create:schematic";
const EMPTY_SCHEMATIC: &str = "create:empty_schematic";
const QUILL: &str = "create:schematic_and_quill";
const GUNPOWDER: &str = "minecraft:gunpowder";
/// `SchematicannonInventory`: schematic in, schematic out, book in, book out, gunpowder.
const CANNON_SLOTS: usize = 5;
/// `Block.UPDATE_CLIENTS | Block.UPDATE_KNOWN_SHAPE`, as `placeSchematicBlock` sets a block.
const PLACE_FLAGS: i32 = 2 | 16;

/// A dimension's cannons and when each wants its next tick.
#[derive(Default)]
struct Dim {
    cannons: HashMap<[i32; 3], Cannon>,
    wakes: BTreeSet<(u64, [i32; 3])>,
    wake_of: HashMap<[i32; 3], u64>,
    /// The aim last sent to clients.
    sent: HashMap<[i32; 3], Option<[i32; 3]>>,
}

impl Dim {
    fn wake(&mut self, pos: [i32; 3], at: u64) {
        if let Some(old) = self.wake_of.get(&pos).copied() {
            if old <= at {
                return;
            }
            self.wakes.remove(&(old, pos));
        }
        self.wakes.insert((at, pos));
        self.wake_of.insert(pos, at);
    }
    fn unwake(&mut self, pos: [i32; 3]) {
        if let Some(old) = self.wake_of.remove(&pos) {
            self.wakes.remove(&(old, pos));
        }
    }
}

/// Something that happened to a cannon outside its tick, handled at its level's tick.
enum Event {
    /// A cannon placed (true) or removed (false) at a position.
    Changed(usize, [i32; 3], bool),
    /// Placed from an item by a player looking at this yaw (`setPlacedBy`).
    Placed(usize, [i32; 3], f32),
    /// Its slots or neighbours changed.
    Wake(usize, [i32; 3], bool),
}

type All = Option<Box<[Dim; 3]>>;

thread_local! {
    static CANNONS: RefCell<All> = const { RefCell::new(None) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    /// The client's barrels: drawn cannons, and the pipe's part model by pitch.
    static DRAWN: RefCell<BTreeSet<[i32; 3]>> = const { RefCell::new(BTreeSet::new()) };
    static PIPES: RefCell<HashMap<i32, u32>> = RefCell::new(HashMap::new());
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    CANNONS.with_borrow_mut(|c| f(&mut c.get_or_insert_with(Default::default)[dim]))
}

/// The world closed: everything is forgotten.
pub(super) fn forget() {
    CANNONS.with_borrow_mut(|c| *c = None);
    EVENTS.with_borrow_mut(Vec::clear);
    body::forget();
}

fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

/// A Create block changed (after the hot hook's range test): a cannon placed or gone.
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    let (was, is) = (
        block_of(old) == Some(Block::Schematicannon),
        block_of(new) == Some(Block::Schematicannon),
    );
    if was != is {
        EVENTS.with_borrow_mut(|e| e.push(Event::Changed(dim, pos, is)));
    }
}

/// A neighbour of a Create block changed: a cannon looks round again (`neighbourCheckCooldown`).
pub(super) fn neighbor_changed(dim: usize, pos: [i32; 3], state: u32) {
    if block_of(state) == Some(Block::Schematicannon) {
        EVENTS.with_borrow_mut(|e| e.push(Event::Wake(dim, pos, false)));
    }
}

/// A container's slots changed (a player's click in its menu, a hopper): a cannon looks at its
/// slots again. Its own changes, made while it ticks, don't reach here.
pub(super) fn contents_changed(game: &mut Game<'_>, pos: [i32; 3]) {
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    if CANNONS.with_borrow(|c| {
        c.as_ref()
            .is_none_or(|c| !c[dim].cannons.contains_key(&pos))
    }) {
        return;
    }
    EVENTS.with_borrow_mut(|e| e.push(Event::Wake(dim, pos, true)));
}

/// A block placed from an item: a cannon takes the placer's yaw and the item's options (none:
/// `applyImplicitComponents`' default).
pub(super) fn place_block(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    if block_of(state.0) != Some(Block::Schematicannon) {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let yaw = cn::placed_yaw(game.yaw, sneaking);
    EVENTS.with_borrow_mut(|e| e.push(Event::Placed(dim, pos, yaw)));
}

/// The block entities of the cannon and the table (their slots are a mod container's).
pub(super) fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if entity.is_none() && matches!(block, "create:schematicannon" | "create:schematic_table") {
        *entity = Some((block.into(), Compound::new()));
    }
}

/// Their slots: Create gives them no item handler, so hoppers reach neither; their menus are a
/// hopper's five slots (the table's screen shows its two).
pub(super) fn block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) {
    if container.is_some() || !matches!(block, "create:schematicannon" | "create:schematic_table") {
        return;
    }
    *container = Some(horizon_client::containers::ModContainer {
        slots: CANNON_SLOTS,
        faces: std::array::from_fn(|_| Vec::new()),
        inputs: Vec::new(),
        outputs: Vec::new(),
        menu: Some("minecraft:hopper".into()),
        only_when_empty: false,
    });
}

/// `SchematicTableMenu`: the input and output slots, as the hopper's first and last.
pub(super) fn screen(block: &str, screen: &mut Option<super::native_hooks::NativeScreen>) {
    if screen.is_some() || block != "create:schematic_table" {
        return;
    }
    *screen = Some(super::native_hooks::NativeScreen {
        width: 176.0,
        height: 133.0,
        slots: vec![Some([44.0, 20.0]), Some([116.0, 20.0]), None, None, None],
        inventory_y: 51.0,
        inventory_x: 8.0,
        own_labels: false,
    });
}

/// An inventory a cannon takes from (`attachedInventories`): an item vault, or a container
/// through the face toward the cannon.
#[derive(PartialEq)]
enum Inv {
    Vault(Vec<[i32; 3]>),
    Native([i32; 3], Direction),
}

/// What a cannon reaches of the game while it ticks.
struct World<'a, 'g> {
    game: &'a mut Game<'g>,
    dimension: &'a str,
    dim: usize,
    pos: [i32; 3],
    found: Option<(Vec<Inv>, bool)>,
}

fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// `Iterate.directions`: down, up, north, south, west, east.
const SIDES: [([i32; 3], Direction); 6] = [
    ([0, -1, 0], Direction::Up),
    ([0, 1, 0], Direction::Down),
    ([0, 0, -1], Direction::South),
    ([0, 0, 1], Direction::North),
    ([-1, 0, 0], Direction::East),
    ([1, 0, 0], Direction::West),
];

impl World<'_, '_> {
    fn items(&self) -> Vec<Option<ItemStack>> {
        match self
            .game
            .world
            .block_entities
            .get(&(self.pos[0], self.pos[1], self.pos[2]))
        {
            Some(horizon_client::containers::BlockEntity::Mod(m)) => {
                let mut v = m.items.clone();
                v.resize(CANNON_SLOTS.max(v.len()), None);
                v
            }
            _ => vec![None; CANNON_SLOTS],
        }
    }

    fn set_slot(&mut self, slot: usize, stack: Option<ItemStack>) {
        self.game.native_with_mod_block_entity(self.pos, |be| {
            be.items.resize(be.items.len().max(CANNON_SLOTS), None);
            be.items[slot] = stack;
        });
    }

    /// `findInventories`, once per tick.
    fn found(&mut self) -> &(Vec<Inv>, bool) {
        if self.found.is_none() {
            let mut invs = Vec::new();
            let mut creative = false;
            for (o, face) in SIDES {
                let at = add(self.pos, o);
                if !self.loaded(at) {
                    continue;
                }
                let state = self.game.world.block(at[0], at[1], at[2]).0;
                if block_of(state) == Some(Block::CreativeCrate) {
                    creative = true;
                }
                let inv = match vault_at(self.game, self.dim, at) {
                    Some(parts) => Inv::Vault(parts),
                    None => {
                        match super::machines::create_extract::takeable_slots(self.game, at, face) {
                            Some(_) => Inv::Native(at, face),
                            None => continue,
                        }
                    }
                };
                if !invs.contains(&inv) {
                    invs.push(inv);
                }
            }
            self.found = Some((invs, creative));
        }
        self.found.as_ref().unwrap()
    }

    /// `ItemHelper.extract(inv, matching item, UPTO, max, simulate)`: how many it gave.
    fn extract(&mut self, i: usize, item: &str, max: u32, simulate: bool) -> u32 {
        let inv = match &self.found().0[i] {
            Inv::Vault(p) => Inv::Vault(p.clone()),
            Inv::Native(p, f) => Inv::Native(*p, *f),
        };
        match inv {
            Inv::Vault(parts) => {
                if simulate {
                    let have: u32 = vault_slots(self.game, &parts)
                        .iter()
                        .flatten()
                        .filter(|s| &*s.item == item)
                        .map(|s| s.count)
                        .sum();
                    have.min(max)
                } else {
                    extract_vault(self.game, &parts, |i| i == item, max).map_or(0, |s| s.count)
                }
            }
            Inv::Native(at, face) => {
                let slots = super::machines::create_extract::takeable_slots(self.game, at, face)
                    .unwrap_or_default();
                let mut got = 0;
                for (index, stack) in slots {
                    let Some(s) = stack.filter(|s| &*s.item == item) else {
                        continue;
                    };
                    if got >= max {
                        break;
                    }
                    let n = s.count.min(max - got);
                    if simulate {
                        got += n;
                    } else if let Some(t) =
                        super::machines::create_extract::take_from_slot(self.game, at, index, n)
                    {
                        got += t.count;
                    }
                }
                got
            }
        }
    }
}

impl CannonWorld for World<'_, '_> {
    fn blueprint(&mut self) -> Blueprint {
        match &self.items()[0] {
            None => Blueprint::Empty,
            Some(s) if &*s.item == SCHEMATIC => {
                let d = body::schematic_data(s);
                Blueprint::Item((!d.is_empty()).then_some(d))
            }
            Some(_) => Blueprint::Item(None),
        }
    }
    fn output_empty(&mut self) -> bool {
        self.items()[1].is_none()
    }
    fn finish_blueprint(&mut self) {
        let n = self.items()[1].as_ref().map_or(0, |s| s.count);
        self.set_slot(0, None);
        self.set_slot(1, Some(ItemStack::new(EMPTY_SCHEMATIC, n + 1)));
    }
    fn expire_blueprint(&mut self) {
        self.set_slot(0, None);
        self.set_slot(1, Some(ItemStack::new(EMPTY_SCHEMATIC, 1)));
    }
    fn own_gunpowder(&mut self) -> bool {
        // Create's slot 4 takes only gunpowder (`isItemValid`); Horizon's menu can't refuse
        // others, so only gunpowder is burnt.
        let Some(mut s) = self.items()[4].clone().filter(|s| &*s.item == GUNPOWDER) else {
            return false;
        };
        s.count -= 1;
        self.set_slot(4, (s.count > 0).then_some(s));
        true
    }
    fn external_gunpowder(&mut self) -> bool {
        let n = self.found().0.len();
        (0..n).any(|i| self.extract(i, GUNPOWDER, 1, false) > 0)
    }
    fn creative_crate(&mut self) -> bool {
        self.found().1
    }
    fn load(&mut self, data: &SchematicData) -> Loaded {
        load(self.game, data)
    }
    fn loaded(&mut self, pos: [i32; 3]) -> bool {
        let (min_y, height) = (self.game.world.shape.min_y, self.game.world.shape.height);
        pos[1] >= min_y && pos[1] < min_y + height && self.game.world.loaded(pos[0], pos[2])
    }
    fn spot(&mut self, pos: [i32; 3]) -> Spot {
        let state = self.game.world.block(pos[0], pos[1], pos[2]);
        let f = self.game.res.behaviours().facts(state);
        Spot {
            state: state.0,
            unbreakable: f.hardness == -1.0,
            conductor: f.flags & state_flags::CONDUCTOR != 0,
            has_block_entity: f.flags & state_flags::BLOCK_ENTITY != 0,
        }
    }
    fn grab(&mut self, item: &str, count: u32, simulate: bool) -> bool {
        let n = self.found().0.len();
        let mut found = 0;
        let mut success = false;
        for i in 0..n {
            found += self.extract(i, item, count, true);
            if found >= count {
                success = true;
                break;
            }
        }
        if !simulate && success {
            // As Create, each inventory is asked for the whole count.
            let mut found = 0;
            for i in 0..n {
                found += self.extract(i, item, count, false);
                if found >= count {
                    break;
                }
            }
        }
        success
    }
    fn place(&mut self, pos: [i32; 3], state: u32) {
        self.game
            .native_set_block(self.dimension, pos, BlockStateId(state), PLACE_FLAGS);
    }
    fn fired(&mut self, now: u64) {
        let _ = now;
        let [x, y, z] = self.pos.map(|v| f64::from(v) + 0.5);
        self.game.native_run_command(&format!(
            "execute in {} run playsound minecraft:entity.generic.explode blocks @a {x} {y} {z} 0.1 1.1",
            self.dimension
        ));
    }
    fn dinged(&mut self) {
        let [x, y, z] = self.pos.map(|v| f64::from(v) + 0.5);
        self.game.native_run_command(&format!(
            "execute in {} run playsound minecraft:block.note_block.bell blocks @a {x} {y} {z} 1 0.7",
            self.dimension
        ));
    }
}

/// A block state by name and properties, as `NbtUtils.readBlockState` reads one: its default
/// state with each property it has set where the value exists; air for an unknown block.
fn resolve_state(registry: &BlockRegistry, name: &str, props: &BTreeMap<String, String>) -> u32 {
    let Some(default) = registry
        .default_state(name)
        .or_else(|| registry.default_state(sch::AIR))
    else {
        return 0;
    };
    let Some(mut s) = registry.state(default).cloned() else {
        return default.0;
    };
    for (k, v) in props {
        if let Some(slot) = s.properties.get_mut(k) {
            let previous = std::mem::replace(slot, v.clone());
            if registry.id(&s).is_none() {
                *s.properties.get_mut(k).unwrap() = previous;
            }
        }
    }
    registry.id(&s).unwrap_or(default).0
}

/// A palette entry turned and resolved once per load: what it prints, places and costs.
fn entry(game: &Game<'_>, p: &PaletteState, rotation: Rotation, mirror: Mirror) -> Entry {
    let registry = game.pack.assets.registry;
    let (mut name, mut props) = (p.name.clone(), p.properties.clone());
    if registry.default_state(&name).is_none() {
        name = sch::AIR.into();
        props.clear();
    }
    sch::transform_state(&name, &mut props, mirror, rotation);
    sch::printed_state(&mut name, &mut props);
    let state = resolve_state(registry, &name, &props);
    let need = sch::need(&name, &props);
    let requirement = match &need {
        Need::Nothing => Requirement::Empty,
        // A belt is printed with its connector (`launchBelt`), which isn't ported.
        Need::Item { .. } if name == "create:belt" => Requirement::Invalid,
        Need::Item { count, item } => {
            match item
                .map(str::to_owned)
                .or_else(|| body::item_for_block(game, &name))
            {
                Some(i) => Requirement::Items(vec![(i, *count)]),
                None => Requirement::Invalid,
            }
        }
    };
    let valid = requirement != Requirement::Invalid;
    let ignore = sch::ignored(&name, &props, valid.then_some(&need));
    let defer = sch::deferred(&name, &props);
    let other = sch::other_half(&props);
    let plain_air = name == sch::AIR;
    let (mut placed_name, mut placed_props) = (name, props);
    sch::placed_state(&mut placed_name, &mut placed_props);
    let placed = resolve_state(registry, &placed_name, &placed_props);
    let b = game.res.behaviours();
    Entry {
        state,
        placed,
        air: b.is_air(BlockStateId(state)),
        plain_air,
        conductor: b.facts(BlockStateId(state)).flags & state_flags::CONDUCTOR != 0,
        requirement,
        ignore,
        defer,
        other,
    }
}

/// `owner/file` under `schematics/uploaded/`, when both are names a player may give.
fn uploaded(owner: &str, file: &str) -> Option<String> {
    let owner_ok = !owner.is_empty() && !owner.contains(['/', '\\']) && !owner.starts_with('.');
    (owner_ok && sch::valid_file(file)).then(|| format!("uploaded/{owner}/{file}"))
}

/// A structure file read (an unreadable one is empty, as `loadSchematic`'s fresh template).
fn read(rel: &str) -> Structure {
    body::read_structure(rel)
        .and_then(|t| Structure::from_nbt(&t))
        .unwrap_or_default()
}

/// `SchematicPrinter.loadSchematic` from the server's uploaded copy.
fn load(game: &Game<'_>, data: &SchematicData) -> Loaded {
    let structure = match (&data.owner, &data.file) {
        (Some(o), Some(f)) => uploaded(o, f).map(|r| read(&r)).unwrap_or_default(),
        _ => Structure::default(),
    };
    let anchor = data.anchor.unwrap_or_default();
    let printer = Printer::load(&structure, anchor, data.rotation, data.mirror);
    let entries = structure
        .palette
        .iter()
        .map(|p| entry(game, p, data.rotation, data.mirror))
        .collect();
    Loaded {
        printer,
        entries,
        air: entry(game, &PaletteState::air(), Rotation::None, Mirror::None),
    }
}

fn players_in(game: &Game<'_>, dimension: &str) -> Vec<u128> {
    game.native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect()
}

fn send(game: &mut Game<'_>, players: &[u128], aims: &[Aim]) {
    if aims.is_empty() {
        return;
    }
    // At most 33 bytes each: well inside a message.
    for chunk in aims.chunks(800) {
        let data = cn::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, cn::CHANNEL, &data);
        }
    }
}

fn aim_of(c: &Cannon) -> Aim {
    Aim::Set {
        pos: c.pos,
        target: c.target(),
        default_yaw: c.default_yaw,
    }
}

fn mark(game: &mut Game<'_>, dimension: &str, pos: [i32; 3]) {
    game.native_mark_chunk_unsaved(dimension, [pos[0] >> 4, pos[2] >> 4]);
}

/// The level's tick: cannons placed, removed and woken, then those whose tick is due; clients
/// are told where barrels point now. Nothing runs without a cannon.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if CANNONS.with_borrow(Option::is_none) && EVENTS.with_borrow(Vec::is_empty) {
        return;
    }
    let now = game.world.game_time.max(0) as u64;
    let events: Vec<Event> = EVENTS.with_borrow_mut(|e| {
        let (mine, rest): (Vec<Event>, Vec<Event>) = std::mem::take(e).into_iter().partition(
            |ev| matches!(ev, Event::Changed(d, ..) | Event::Placed(d, ..) | Event::Wake(d, ..) if *d == dim),
        );
        *e = rest;
        mine
    });
    let mut aims = Vec::new();
    for ev in events {
        match ev {
            Event::Changed(_, pos, true) => {
                let added = with(dim, |d| {
                    if d.cannons.contains_key(&pos) {
                        return false;
                    }
                    let mut c = Cannon::new(pos, Options::default());
                    c.last = now;
                    d.cannons.insert(pos, c);
                    d.wake(pos, now);
                    true
                });
                if added {
                    mark(game, dimension, pos);
                }
            }
            Event::Changed(_, pos, false) => {
                let gone = with(dim, |d| {
                    d.unwake(pos);
                    d.sent.remove(&pos);
                    d.cannons.remove(&pos).is_some()
                });
                if gone {
                    aims.push(Aim::Gone { pos });
                }
            }
            Event::Placed(_, pos, yaw) => with(dim, |d| {
                if let Some(c) = d.cannons.get_mut(&pos) {
                    c.default_yaw = yaw;
                    c.options = Options::placed();
                    d.sent.remove(&pos);
                    d.wake(pos, now);
                }
            }),
            Event::Wake(_, pos, slots) => with(dim, |d| {
                if let Some(c) = d.cannons.get_mut(&pos) {
                    if slots {
                        c.slots_changed(now);
                    } else {
                        c.settle(now);
                    }
                    d.wake(pos, now);
                }
            }),
        }
    }
    let due: Vec<[i32; 3]> = with(dim, |d| {
        let mut due = Vec::new();
        while let Some(&(t, pos)) = d.wakes.first() {
            if t > now {
                break;
            }
            d.wakes.pop_first();
            d.wake_of.remove(&pos);
            due.push(pos);
        }
        due
    });
    for pos in due {
        let Some(mut c) = with(dim, |d| d.cannons.remove(&pos)) else {
            continue;
        };
        {
            let mut w = World {
                game: &mut *game,
                dimension,
                dim,
                pos,
                found: None,
            };
            c.tick(now, &mut w);
        }
        let next = c.next_wake(now);
        let aim = aim_of(&c);
        let changed = with(dim, |d| {
            if let Some(t) = next {
                d.wake(pos, t);
            }
            let target = c.target();
            let changed = d.sent.get(&pos) != Some(&target);
            if changed {
                d.sent.insert(pos, target);
            }
            d.cannons.insert(pos, c);
            changed
        });
        if changed {
            aims.push(aim);
        }
        mark(game, dimension, pos);
    }
    // A cannon just placed or loaded is ticked here and, never sent, sent at rest.
    if !aims.is_empty() {
        let players = players_in(game, dimension);
        send(game, &players, &aims);
    }
}

/// A client joining asks for every part (`sync`): every barrel of its dimension.
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != create_engine::client::CHANNEL
        || data != b"sync"
        || CANNONS.with_borrow(Option::is_none)
    {
        return;
    }
    let Some(dim) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
        .and_then(|p| dimension_index(&p.dimension))
    else {
        return;
    };
    let aims: Vec<Aim> = with(dim, |d| d.cannons.values().map(aim_of).collect());
    send(game, &[player], &aims);
}

fn part_key(pos: [i32; 3], part: u32) -> u64 {
    let mut h: u64 = 0x5C4E_3A71_C0FF_EE00;
    for v in pos.into_iter().chain([part as i32]) {
        h ^= v as u32 as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h | 1 << 63
}

/// A partial's unturned state (`facing=up`).
fn partial(game: &Game<'_>, name: &str) -> Option<BlockStateId> {
    let registry = game.pack.assets.registry;
    let default = registry.default_state(name)?;
    let mut s = registry.state(default)?.clone();
    s.properties.insert("facing".to_owned(), "up".to_owned());
    registry.id(&s)
}

/// A barrel from the server, drawn as two GPU parts (`SchematicannonRenderer`): the connector
/// turned to the yaw, the pipe tilted to the pitch (baked per degree) and turned with it. Set
/// again only when the cannon aims elsewhere.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != cn::CHANNEL {
        return;
    }
    for a in cn::decode(data) {
        match a {
            Aim::Gone { pos } => {
                game.native_part_clear(part_key(pos, 0));
                game.native_part_clear(part_key(pos, 1));
                DRAWN.with_borrow_mut(|d| d.remove(&pos));
            }
            Aim::Set {
                pos,
                target,
                default_yaw,
            } => {
                let (yaw, pitch) = cn::aim(pos, target, default_yaw);
                let turn = (yaw + 90.0).rem_euclid(360.0);
                if let Some(model) = partial(game, "create:part_schematicannon_connector")
                    .and_then(|s| game.native_part_model(s))
                {
                    game.native_part_set(part_key(pos, 0), model, pos, 1, 0.0, turn);
                }
                let degrees = pitch.round() as i32;
                let pipe = PIPES.with_borrow(|p| p.get(&degrees).copied()).or_else(|| {
                    let state = partial(game, "create:part_schematicannon_pipe")?;
                    let pivot = glam::Vec3::new(0.5, 15.0 / 16.0, 0.5);
                    let pose = glam::Mat4::from_translation(pivot)
                        * glam::Mat4::from_rotation_z((degrees as f32).to_radians())
                        * glam::Mat4::from_translation(-pivot);
                    let model = game.native_part_model_posed(state, pose)?;
                    PIPES.with_borrow_mut(|p| p.insert(degrees, model));
                    Some(model)
                });
                if let Some(model) = pipe {
                    game.native_part_set(part_key(pos, 1), model, pos, 1, 0.0, turn);
                }
                DRAWN.with_borrow_mut(|d| d.insert(pos));
            }
        }
    }
}

/// Each tick of the window's game: out of a world, the drawn barrels are forgotten.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if game.native_window() && !game.native_playing() {
        DRAWN.with_borrow_mut(BTreeSet::clear);
        PIPES.with_borrow_mut(HashMap::clear);
    }
}

pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    for usage in [
        // The schematic and quill's two corners and its prompt (`instant`: the creative
        // convert button).
        "create_debug schematic save <from:block_pos> <to:block_pos> <name:word> [instant:word]",
        // The schematic table's upload of a file from `schematics/`.
        "create_debug schematic upload <pos:block_pos> <name:word>",
        // The held schematic deployed (`SchematicHandler` deploy and its tools).
        "create_debug schematic deploy <name:word> <anchor:block_pos> [rotation:word] [mirror:word]",
        // A cannon's state and its screen's options.
        "create_debug schematicannon <pos:block_pos> [option:text]",
    ] {
        commands.push(super::native_server::NativeCommand::op(usage, 2));
    }
}

/// Coordinates from the command's words (`~` from the player's position).
fn coords(words: &[&str], here: Option<[f64; 3]>) -> Option<[i32; 3]> {
    let mut out = [0; 3];
    for i in 0..3 {
        let t = words.get(i)?;
        out[i] = match t.strip_prefix('~') {
            Some(off) => here?[i].floor() as i32 + off.parse::<i32>().unwrap_or(0),
            None => t.parse().ok()?,
        };
    }
    Some(out)
}

fn nbt_file(name: &str) -> String {
    if name.ends_with(".nbt") {
        name.to_owned()
    } else {
        format!("{name}.nbt")
    }
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    let Some(rest) = command.strip_prefix("create_debug ") else {
        return;
    };
    let (verb, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if !matches!(verb, "schematic" | "schematicannon") {
        return;
    }
    *ran = true;
    let player = game.native_acting_player();
    let here = player.as_ref().map(|p| p.position);
    let name = player.as_ref().map_or_else(String::new, |p| p.name.clone());
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let lines = match (verb, words.first().copied()) {
        ("schematicannon", _) => match coords(&words, here) {
            Some(pos) => vec![cannon_command(game, dim, pos, &words[3..])],
            None => vec!["usage: create_debug schematicannon <x y z> [option]".into()],
        },
        (_, Some("save")) => match (
            coords(&words[1..], here),
            coords(words.get(4..).unwrap_or(&[]), here),
        ) {
            (Some(a), Some(b)) => save(
                game,
                &name,
                a,
                b,
                words.get(7).copied().unwrap_or(""),
                words.get(8) == Some(&"instant"),
            ),
            _ => vec!["usage: create_debug schematic save <x y z> <x y z> <name> [instant]".into()],
        },
        (_, Some("upload")) => match (coords(&words[1..], here), words.get(4)) {
            (Some(pos), Some(file)) => upload(game, &name, pos, &nbt_file(file)),
            _ => vec!["usage: create_debug schematic upload <x y z> <name>".into()],
        },
        (_, Some("deploy")) => match (words.get(1), coords(&words[2.min(words.len())..], here)) {
            (Some(file), Some(anchor)) => vec![deploy(
                game,
                &nbt_file(file),
                anchor,
                words.get(5).copied(),
                words.get(6).copied(),
            )],
            _ => vec![
                "usage: create_debug schematic deploy <name> <x y z> [rotation] [mirror]".into(),
            ],
        },
        _ => vec!["usage: create_debug schematic save|upload|deploy ...".into()],
    };
    for l in lines {
        game.native_feedback(horizon_client::text::Text::literal(l));
    }
}

/// `SchematicExport.saveSchematic` of the box between two corners: the blocks (`fillFromWorld`;
/// their block entities' data and the entities in it are not read), into `dir`.
fn export(game: &Game<'_>, a: [i32; 3], b: [i32; 3]) -> Structure {
    let min = [0, 1, 2].map(|i| a[i].min(b[i]));
    let max = [0, 1, 2].map(|i| a[i].max(b[i]));
    let size = [0, 1, 2].map(|i| max[i] - min[i] + 1);
    let registry = game.pack.assets.registry;
    let behaviours = game.res.behaviours();
    let mut cells = Vec::new();
    for y in min[1]..=max[1] {
        for x in min[0]..=max[0] {
            for z in min[2]..=max[2] {
                let state = game.world.block(x, y, z);
                let Some(s) = registry.state(state) else {
                    continue;
                };
                let flags = behaviours.facts(state).flags;
                let class = if flags & state_flags::BLOCK_ENTITY != 0 {
                    Class::WithEntity
                } else if flags & (state_flags::SOLID_RENDER | state_flags::FULL_COLLISION) != 0 {
                    Class::Full
                } else {
                    Class::Special
                };
                cells.push((
                    [x - min[0], y - min[1], z - min[2]],
                    PaletteState {
                        name: s.name.clone(),
                        properties: s.properties.clone().into_iter().collect(),
                    },
                    class,
                ));
            }
        }
    }
    Structure::capture(size, cells)
}

/// `SchematicAndQuillHandler.saveSchematic`: saved in `schematics/` under a free name; with
/// `instant` (the convert button), `ServerSchematicLoader.handleInstantSchematic` too: saved
/// again as the player's upload and the held schematic and quill turned into its schematic.
fn save(
    game: &mut Game<'_>,
    player: &str,
    a: [i32; 3],
    b: [i32; 3],
    name: &str,
    instant: bool,
) -> Vec<String> {
    let structure = export(game, a, b);
    let file = sch::export_name(name, false, body::exists);
    let tag = structure.to_nbt();
    if let Err(e) = body::write_structure(&file, &tag) {
        eprintln!("[create] An error occurred while saving schematic [{file}]: {e}");
        return vec!["Failed to save schematic, check logs for details".into()];
    }
    let mut lines = vec![format!("Saved as {file}")];
    if !instant {
        return lines;
    }
    let holding = game
        .inventory
        .get(game.selected)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| &*s.item == QUILL);
    let size = body::size(&file).unwrap_or(0);
    let dir = format!("uploaded/{player}");
    let target = uploaded(player, &file);
    match target {
        Some(target) if holding && sch::size_allowed(size) => {
            // `tryDeleteOldestSchematic`.
            let list = body::entries(&dir);
            if list.len() >= sch::MAX_SCHEMATICS
                && let Some(old) = list.iter().min_by_key(|e| e.2)
            {
                body::remove(&format!("{dir}/{}", old.0));
            }
            match body::write_structure(&target, &tag) {
                Ok(_) => {
                    let mut stack = ItemStack::new(SCHEMATIC, 1);
                    body::set_schematic_data(
                        &mut stack,
                        &SchematicData::written(&file, player, structure.size),
                    );
                    if let Some(slot) = game.inventory.get_mut(game.selected) {
                        *slot = Some(stack);
                    }
                    lines.push(format!("{file} written to the held schematic"));
                }
                Err(e) => {
                    eprintln!("[create] Schematic instant-upload of {file}: {e}");
                    lines.push("Schematic instant-upload failed, check logs for details".into());
                }
            }
        }
        _ if !sch::size_allowed(size) => lines.push(too_large(size)),
        _ => {}
    }
    lines
}

fn too_large(size: u64) -> String {
    format!(
        "Your schematic exceeds limitations specified by the server. ({} KB). The maximum allowed schematic file size is: {} KB",
        size / 1000,
        sch::MAX_TOTAL_SCHEMATIC_SIZE_KB
    )
}

/// The schematic table's upload (`SchematicTableScreen` → `ServerSchematicLoader`): the file
/// from `schematics/` copied as the player's upload (one of the same name replaced, the oldest
/// gone past `maxSchematics`), the input slot emptied (`startUpload`), and a written schematic
/// in the output (`handleFinishedUpload`). The upload is local, so it finishes at once.
fn upload(game: &mut Game<'_>, player: &str, pos: [i32; 3], file: &str) -> Vec<String> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if block_of(state) != Some(Block::SchematicTable) {
        return vec![format!(
            "no schematic table at {} {} {}",
            pos[0], pos[1], pos[2]
        )];
    }
    let items: Vec<Option<ItemStack>> =
        match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
            Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.clone(),
            _ => Vec::new(),
        };
    // `SchematicTableMenu.canWrite` and its input slot's `mayPlace`.
    let input = items.first().cloned().flatten();
    let writable = input
        .as_ref()
        .is_some_and(|s| matches!(&*s.item, EMPTY_SCHEMATIC | QUILL | SCHEMATIC));
    if !writable || items.get(1).cloned().flatten().is_some() {
        return vec!["the table needs a blank schematic in and nothing out".into()];
    }
    let Some(target) = uploaded(player, file) else {
        eprintln!("[create] Attempted Schematic Upload with non-supported Format: {player}/{file}");
        return vec![format!("not a schematic file: {file}")];
    };
    let Some(size) = body::size(file) else {
        return vec![format!("no schematic {file} in schematics/")];
    };
    if !sch::size_allowed(size) {
        return vec![too_large(size)];
    }
    let dir = format!("uploaded/{player}");
    body::remove(&target);
    let list = body::entries(&dir);
    if list.len() >= sch::MAX_SCHEMATICS
        && let Some(old) = list.iter().filter(|e| e.1).min_by_key(|e| e.2)
    {
        body::remove(&format!("{dir}/{}", old.0));
    }
    if let Err(e) = body::copy(file, &target) {
        eprintln!("[create] Exception Thrown when starting Upload: {player}/{file}: {e}");
        return vec![format!("upload of {file} failed")];
    }
    eprintln!("[create] New Schematic Uploaded: {player}/{file}");
    let bounds = read(&target).size;
    let mut stack = ItemStack::new(SCHEMATIC, 1);
    body::set_schematic_data(&mut stack, &SchematicData::written(file, player, bounds));
    game.native_with_mod_block_entity(pos, |be| {
        be.items.resize(be.items.len().max(CANNON_SLOTS), None);
        be.items[0] = None;
        be.items[1] = Some(stack);
    });
    vec![format!(
        "uploaded {file}: {}x{}x{}",
        bounds[0], bounds[1], bounds[2]
    )]
}

/// The held schematic deployed (`DeployTool.handleRightClick`, then `SchematicSyncPacket`):
/// deployed at `anchor`, turned and mirrored as given.
fn deploy(
    game: &mut Game<'_>,
    file: &str,
    anchor: [i32; 3],
    rotation: Option<&str>,
    mirror: Option<&str>,
) -> String {
    let rotation = match rotation.map(Rotation::parse) {
        None => Rotation::None,
        Some(Some(r)) => r,
        Some(None) => {
            return "rotation: none, clockwise_90, clockwise_180 or counterclockwise_90".into();
        }
    };
    let mirror = match mirror.map(Mirror::parse) {
        None => Mirror::None,
        Some(Some(m)) => m,
        Some(None) => return "mirror: none, left_right or front_back".into(),
    };
    let selected = game.selected;
    let Some(Some(stack)) = game.inventory.get_mut(selected) else {
        return format!("hold the schematic of {file}");
    };
    let mut data = body::schematic_data(stack);
    if &*stack.item != SCHEMATIC || data.file.as_deref() != Some(file) {
        return format!("hold the schematic of {file}");
    }
    data.deployed = Some(true);
    data.anchor = Some(anchor);
    data.rotation = rotation;
    data.mirror = mirror;
    body::set_schematic_data(stack, &data);
    format!(
        "deployed {file} at {} {} {} {} {}",
        anchor[0],
        anchor[1],
        anchor[2],
        rotation.name(),
        mirror.name()
    )
}

/// A cannon's options (`ConfigureSchematicannonPacket`) and state.
fn cannon_command(game: &mut Game<'_>, dim: usize, pos: [i32; 3], option: &[&str]) -> String {
    let now = game.world.game_time.max(0) as u64;
    let configure = if option.is_empty() {
        None
    } else {
        match Configure::parse(option) {
            Some(c) => Some(c),
            None => {
                return "options: play, pause, stop, mode <0-3>, skip_missing <bool>, replace_block_entities <bool>".into();
            }
        }
    };
    with(dim, |d| {
        let Some(c) = d.cannons.get_mut(&pos) else {
            return format!("no schematicannon at {} {} {}", pos[0], pos[1], pos[2]);
        };
        if let Some(o) = configure {
            c.configure(now, o);
        }
        let line = describe(c);
        if configure.is_some() {
            d.wake(pos, now);
        }
        line
    })
}

fn describe(c: &Cannon) -> String {
    format!(
        "schematicannon {} {} {}: {} {} fuel={} placed={}/{} target={:?} mode={} skip_missing={} replace_block_entities={} missing={:?} flying={}",
        c.pos[0],
        c.pos[1],
        c.pos[2],
        c.state.name(),
        c.status.name(),
        c.fuel,
        c.blocks_placed,
        c.blocks_to_place,
        c.target(),
        c.options.replace_mode,
        c.options.skip_missing,
        c.options.replace_block_entities,
        c.missing,
        c.flying.len()
    )
}

fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn text(t: Option<&Tag>) -> String {
    match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

/// A column's cannons saved under `create_schematics` (`write`: state, status, fuel, options,
/// progress, the barrel's rest and the blocks in flight; the printer itself starts over when
/// loaded, as Create's server does).
pub(super) fn chunk_data_save(
    game: &Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if CANNONS.with_borrow(Option::is_none) {
        return;
    }
    let now = game.world.game_time.max(0) as u64;
    let registry = game.pack.assets.registry;
    let list: Vec<Tag> = with(dim, |d| {
        d.cannons
            .values()
            .filter(|c| (c.pos[0] >> 4, c.pos[2] >> 4) == (chunk[0], chunk[1]))
            .map(|c| {
                let flying = c
                    .flying
                    .iter()
                    .map(|f| {
                        let left = f.land_at.saturating_sub(now + 1) as i32;
                        Tag::compound([
                            (
                                "Target",
                                Tag::IntArray(vec![
                                    f.target[0],
                                    f.target[1],
                                    f.target[2],
                                    left,
                                    f.total as i32,
                                ]),
                            ),
                            (
                                "BlockState",
                                Tag::String(state_text(registry, f.state).as_str().into()),
                            ),
                        ])
                    })
                    .collect();
                // Fuel, the options, progress and the barrel's resting yaw.
                let nums = vec![
                    c.fuel,
                    i32::from(c.options.replace_mode),
                    i32::from(c.options.skip_missing),
                    i32::from(c.options.replace_block_entities),
                    c.blocks_placed,
                    c.blocks_to_place,
                    c.default_yaw.to_bits() as i32,
                ];
                let mut e = vec![
                    ("Pos", Tag::IntArray(c.pos.to_vec())),
                    ("Nums", Tag::IntArray(nums)),
                    ("State", Tag::String(c.state.name().into())),
                    ("Status", Tag::String(c.status.name().into())),
                    ("FlyingBlocks", Tag::List(flying)),
                ];
                if let Some(m) = &c.missing {
                    e.push(("MissingItem", Tag::String(m.as_str().into())));
                }
                Tag::compound(e)
            })
            .collect()
    });
    if !list.is_empty() {
        data.insert(KEY.into(), Tag::compound([("cannons", Tag::List(list))]));
    }
}

/// A column joined a level: its cannons, ticked at once (`read`; the schematic in slot 0 loads
/// again, the cannon paused and ready).
pub(super) fn chunk_load(
    game: &mut Game<'_>,
    dimension: &str,
    chunk: &horizon_client::chunk::Chunk,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let Some(Tag::List(list)) = data.get(&"cannons".into()) else {
        return;
    };
    let now = game.world.game_time.max(0) as u64;
    let registry = game.pack.assets.registry;
    let cannons: Vec<Cannon> = list
        .iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let p = ints(c.get(&"Pos".into()));
            let pos = [*p.first()?, *p.get(1)?, *p.get(2)?];
            let n = ints(c.get(&"Nums".into()));
            let num = |i: usize| n.get(i).copied().unwrap_or(0);
            let mut cannon = Cannon::new(
                pos,
                Options {
                    replace_mode: num(1).clamp(0, 3) as u8,
                    skip_missing: num(2) != 0,
                    replace_block_entities: num(3) != 0,
                },
            );
            let state = text(c.get(&"State".into()));
            cannon.state = cn::State::ALL
                .into_iter()
                .find(|s| s.name() == state)
                .unwrap_or_default();
            cannon.status = cn::Status::parse(&text(c.get(&"Status".into()))).unwrap_or_default();
            cannon.fuel = num(0);
            cannon.blocks_placed = num(4);
            cannon.blocks_to_place = num(5);
            cannon.default_yaw = f32::from_bits(num(6) as u32);
            let missing = text(c.get(&"MissingItem".into()));
            cannon.missing = (!missing.is_empty()).then_some(missing);
            if let Some(Tag::List(f)) = c.get(&"FlyingBlocks".into()) {
                for t in f {
                    let Tag::Compound(f) = t else { continue };
                    let t = ints(f.get(&"Target".into()));
                    let Some(state) = state_of_text(registry, &text(f.get(&"BlockState".into())))
                    else {
                        continue;
                    };
                    if t.len() != 5 {
                        continue;
                    }
                    cannon.flying.push(cn::Flying {
                        target: [t[0], t[1], t[2]],
                        state,
                        total: t[4].max(0) as u32,
                        land_at: now + t[3].max(0) as u64 + 1,
                    });
                }
            }
            cannon.last = now;
            Some(cannon)
        })
        .collect();
    with(dim, |d| {
        for c in cannons {
            let pos = c.pos;
            d.cannons.insert(pos, c);
            d.sent.remove(&pos);
            d.wake(pos, now);
        }
    });
}

/// A column left a level: its cannons are forgotten (kept in its save).
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if CANNONS.with_borrow(Option::is_none) {
        return;
    }
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk.x, chunk.z);
    with(dim, |d| {
        let gone: Vec<[i32; 3]> = d.cannons.keys().filter(|p| inside(p)).copied().collect();
        for p in gone {
            d.cannons.remove(&p);
            d.unwake(p);
            d.sent.remove(&p);
        }
    });
}
