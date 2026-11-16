//! Create's schematics (`content/schematics`): the schematic and quill's save, the schematic
//! table's upload, a schematic deployed, and the schematicannon printing it. Rules are the
//! engine's (`create_engine::schematic`, `create_engine::schematicannon`); files, stacks and items
//! go through `body`.
//!
//! Cannons are typed state here, ticked only when one needs it (`Cannon::next_wake`); a
//! schematic is parsed and its palette resolved to block states once per load; NBT only at a
//! chunk's save. The cannon and the table are block entity types of their own whose five slots
//! are a hopper's menu (Create gives them no item handler, so no face reaches them); the
//! cannon's barrel is drawn from where it aims, kept in its block entity's data (`Aim`).
//!
//! Players work them as in Create: the quill's corners, outline and prompt (`quill`), the held
//! schematic's tools, their selection overlay and its place kept in sync with the server
//! (`tools`, `transform`), its edit screen (`deploy`), and the table's and the cannon's screens
//! (`screens`); the `create_debug` commands remain for scripted tests.
//!
//! The engine numbers block states as `u32`s; the API's `BlockState` is a key and an ordinal, so
//! the states a cannon meets are numbered here as it meets them (`intern`).
//!
//! The cannon prints as Create's does: belts with their connector from the end that lays them
//! (`launchBelt`, each segment cased as the schematic's belt block entity's `Casing`), the
//! second half of doors, tall plants and beds (`setPlacedBy`), and the schematic's entities
//! last (`launchEntity`: item frames and armor stands with what they hold and wear); a book or a
//! clipboard in its book slot becomes the print's material checklist (`tickPaperPrinter`).
//! A printed block loads the schematic's block entity data where Create keeps it
//! (`prepareBlockEntityData`: blocks tagged `create:safe_nbt`, such as signs, banners, placards
//! and links, and copycats; not a chest's items), merged into the placed block entity.
//!
//! Left out, for want of API:
//! - A schematic saved by the port holds no item frames or paintings: `LevelRef::entities_in`
//!   doesn't list them (a box lookup of hanging entities would close it); schematics that hold
//!   them print them.
//! - Past `maxSchematics` uploads, the player's oldest is not deleted
//!   (`tryDeleteOldestSchematic`): a mod's data files can be written but not removed.
//! - `isRedstoneConductor` is read as a full collision cube (vanilla's default for it); the
//!   blocks that override it (glass and the like) are not known to the API.
//! - A schematic's export classes its blocks by their block entity and a full collision cube;
//!   `isSolidRender` is not in the API.
//! - Shift-clicking a whole stack into the cannon's or the table's slot of its own (gunpowder,
//!   a book) only works where the game's routing puts it: `menu_quick_move` cannot empty the
//!   clicked slot (an `ItemStack` is never empty).

pub mod body;
mod deploy;
mod quill;
pub(crate) use quill::face_of;
mod screens;
mod tools;
mod transform;

use super::packages::{self, block_of, debug_verb, feedback, ints, text};
use super::persist;
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::level_index;
use create_engine::Block;
use create_engine::schematic::{
    self as sch, Class, Mirror, Need, PaletteState, Rotation, SchematicData, Structure,
};
use create_engine::schematicannon::{
    self as cn, Blueprint, Cannon, CannonWorld, Configure, Entry, Loaded, Options, Printer,
    Requirement, Spot,
};
use mod_api::{
    BlockEntityOnLoadEvent, BlockEntityPart, BlockEntityRef, BlockEntitySubmitEvent,
    BlockEntityTypeKey, BlockEntityUpdateTagEvent, BlockKey, BlockPart, BlockShapeKind, BlockState,
    BlockUpdateFlags, ClientHost, Command, CommandArgumentValue, Compound, ContentsChangedEvent,
    Direction, Flow, Host, HostBlockStates, HostSide, HostTransaction, ITEMS, ItemPart,
    ItemResource, ItemStack, ModContainer, OnBlockPlacedByEvent, PlayerRef, RegisterCommandsEvent,
    Registrar, RegistrationContext, RunModCommandEvent, Screen, ScreenEvent, ServerHost,
    SoundSource, Tag,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

const KEY: &str = "create_schematics";
const SCHEMATIC: &str = "create:schematic";
const EMPTY_SCHEMATIC: &str = "create:empty_schematic";
const QUILL: &str = "create:schematic_and_quill";
const GUNPOWDER: &str = "minecraft:gunpowder";
/// `SchematicannonInventory`: schematic in, schematic out, book in, book out, gunpowder.
const CANNON_SLOTS: usize = 5;
/// The key in a cannon's block entity data under which clients find where its barrel aims:
/// `[has a target, x, y, z, resting yaw's bits]`.
const AIM: &str = "Aim";

/// `Block.UPDATE_CLIENTS | Block.UPDATE_KNOWN_SHAPE`, as `placeSchematicBlock` sets a block.
fn place_flags() -> BlockUpdateFlags {
    BlockUpdateFlags::CLIENTS | BlockUpdateFlags::KNOWN_SHAPE
}

/// A level's cannons and when each wants its next tick.
#[derive(Default)]
struct Dim {
    cannons: HashMap<[i32; 3], Cannon>,
    wakes: BTreeSet<(u64, [i32; 3])>,
    wake_of: HashMap<[i32; 3], u64>,
    /// The aim last given to clients.
    sent: HashMap<[i32; 3], Option<[i32; 3]>>,
    /// What the cannon's screen was last given (its menu data slots).
    sent_data: HashMap<[i32; 3], Vec<i32>>,
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
    /// Its slots (true) or neighbours changed.
    Wake(usize, [i32; 3], bool),
}

impl Event {
    fn dim(&self) -> usize {
        match self {
            Event::Changed(d, ..) | Event::Placed(d, ..) | Event::Wake(d, ..) => *d,
        }
    }
}

/// The states a cannon met, numbered from 1, both ways.
#[derive(Default)]
struct Interned {
    states: Vec<BlockState>,
    ids: HashMap<BlockState, u32>,
}

thread_local! {
    static CANNONS: RefCell<Option<Vec<Dim>>> = const { RefCell::new(None) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static INTERNED: RefCell<Interned> = RefCell::new(Interned::default());
    /// A cannon's own slot writes while it ticks (or the table's upload) are not a change from
    /// outside (`contents_changed` fires inside them).
    static OWN_WRITE: Cell<bool> = const { Cell::new(false) };
}

/// The cannon's and the table's block entity types, and the barrel's two partial models.
static KINDS: OnceLock<(BlockEntityTypeKey, BlockEntityTypeKey)> = OnceLock::new();
static BARREL: OnceLock<(BlockKey, BlockKey)> = OnceLock::new();

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    CANNONS.with_borrow_mut(|c| {
        let levels = c.get_or_insert_with(Vec::new);
        if levels.len() <= dim {
            levels.resize_with(dim + 1, Dim::default);
        }
        f(&mut levels[dim])
    })
}

fn present() -> bool {
    CANNONS.with_borrow(Option::is_some)
}

/// The engine's number for `state`.
fn intern(state: BlockState) -> u32 {
    INTERNED.with_borrow_mut(|t| {
        if let Some(&id) = t.ids.get(&state) {
            return id;
        }
        t.states.push(state);
        let id = t.states.len() as u32;
        t.ids.insert(state, id);
        id
    })
}

/// The state the engine's number `id` is.
fn interned(id: u32) -> Option<BlockState> {
    let i = id.checked_sub(1)? as usize;
    INTERNED.with_borrow(|t| t.states.get(i).copied())
}

fn now(lv: &ServerLevel<'_>) -> u64 {
    lv.game_time().max(0) as u64
}

/// An upload asked for: level, player, table, file.
type Upload = (usize, String, [i32; 3], String);

thread_local! {
    /// Uploads asked for by tables' buttons: level, player, table, file.
    static UPLOADS: RefCell<Vec<Upload>> = const { RefCell::new(Vec::new()) };
    /// Whether the table's screen drew this frame: the wheel then chooses its schematic.
    static TABLE_OPEN: Cell<bool> = const { Cell::new(false) };
}

pub struct Schematics;

impl Subsystem for Schematics {
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel != quill::CHANNEL {
            return false;
        }
        quill::client_message(host, player, data);
        true
    }

    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        quill::tick(host);
        deploy::tick(host);
        tools::tick(host);
    }

    fn client_reset(&self) {
        quill::reset();
        deploy::reset();
        tools::reset();
        screens::reset();
    }

    fn interaction_key(
        &self,
        host: &mut dyn mod_api::ClientHost,
        event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        if quill::interaction_key(host, event) == Flow::Cancel
            || deploy::interaction_key(host, event) == Flow::Cancel
        {
            return Flow::Cancel;
        }
        tools::interaction_key(host, event)
    }

    fn input_key(
        &self,
        host: &mut dyn mod_api::ClientHost,
        event: &mut mod_api::InputKeyEvent<'_>,
    ) {
        tools::input_key(host, event);
    }

    fn render_gui_post(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::RenderGuiPostEvent<'_>,
    ) {
        tools::render_gui_post(host, event);
    }

    fn mouse_scrolling(
        &self,
        host: &mut dyn mod_api::ClientHost,
        event: &mut mod_api::MouseScrollingEvent,
    ) -> Flow {
        if quill::mouse_scrolling(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        tools::mouse_scrolling(host, event)
    }

    fn render_level_stage(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        quill::render_level_stage(host, event);
        tools::render_level_stage(host, event);
    }

    fn screen_render_post(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        quill::screen_render_post(host, event);
        deploy::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if quill::screen_mouse_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        deploy::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if TABLE_OPEN.get() && event.scroll[1] != 0.0 && host.mod_screen().is_none() {
            screens::scroll_table(host, event.scroll[1] > 0.0);
            return Flow::Cancel;
        }
        deploy::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if quill::screen_key_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        deploy::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if quill::screen_char_typed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        deploy::screen_char_typed(host, event)
    }

    fn screen_closing(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenClosingEvent<'_>,
    ) {
        TABLE_OPEN.set(false);
        quill::screen_closing(host, event);
        deploy::screen_closing(host, event);
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        tools::register(r);
        let cannon = r
            .block_entity_type("create:schematicannon")
            .blocks(&["create:schematicannon"])
            .build();
        let table = r
            .block_entity_type("create:schematic_table")
            .blocks(&["create:schematic_table"])
            .build();
        let _ = KINDS.set((cannon, table));
        // Their slots: Create gives them no item handler, so no face reaches them; their menus
        // are a hopper's five slots (the table's screen shows its two).
        for b in [Block::Schematicannon, Block::SchematicTable] {
            r.block_container(
                &states::key(b),
                ModContainer {
                    slots: CANNON_SLOTS,
                    faces: std::array::from_fn(|_| Vec::new()),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    menu: Some("minecraft:hopper".into()),
                },
            );
        }
        for kind in [cannon, table] {
            if let Err(e) = r.block_entity_part(&kind, SchematicEntity) {
                crate::create_log!(
                    "[create] the schematic block entities' part was refused: {e:?}"
                );
            }
        }
        r.block_part(states::key(Block::Schematicannon), CannonBlock);
        let key = r.item_key("create:schematicannon");
        r.item_part(key, CannonItem);
        let _ = BARREL.set((
            r.block_key("create:part_schematicannon_connector"),
            r.block_key("create:part_schematicannon_pipe"),
        ));
    }

    fn world_closed(&self) {
        CANNONS.with_borrow_mut(|c| *c = None);
        EVENTS.with_borrow_mut(Vec::clear);
        INTERNED.with_borrow_mut(|t| *t = Interned::default());
    }

    /// A cannon placed or gone.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (was, is) = (
            block_of(old) == Some(Block::Schematicannon),
            block_of(new) == Some(Block::Schematicannon),
        );
        if was != is {
            EVENTS.with_borrow_mut(|e| e.push(Event::Changed(dim, pos, is)));
        }
    }

    /// A cannon looks round again (`neighbourCheckCooldown`).
    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        if block_of(state) == Some(Block::Schematicannon) {
            EVENTS.with_borrow_mut(|e| e.push(Event::Wake(dim, pos, false)));
        }
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        level_tick(lv);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        if let Some(Tag::Compound(own)) = data.get(&KEY.into()) {
            chunk_load(lv, own);
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        chunk_data_save(lv, chunk, data);
    }

    /// A column left a level: its cannons are forgotten (kept in its save).
    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        if !present() {
            return;
        }
        let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
        with(dim, |d| {
            let gone: Vec<[i32; 3]> = d.cannons.keys().filter(|p| inside(p)).copied().collect();
            for p in gone {
                d.cannons.remove(&p);
                d.unwake(p);
                d.sent.remove(&p);
            }
        });
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
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
            event.commands.push(Command::op(usage, 2));
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let Some(verb) = debug_verb(event, &["schematic", "schematicannon"]) else {
            return Flow::Continue;
        };
        let player = packages::command_player(event.source);
        let Some(dimension) = event.command.dimension.id(&*host) else {
            return Flow::Cancel;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Cancel;
        };
        let cmd = event.command;
        let pos = |name: &str| match cmd.argument(name) {
            Some(CommandArgumentValue::BlockPos(p)) => Some(*p),
            _ => None,
        };
        let word = |name: &str| match cmd.argument(name) {
            Some(CommandArgumentValue::String(s)) => Some(s.clone()),
            _ => None,
        };
        let name = player.and_then(|p| p.name(&*lv.host)).unwrap_or_default();
        let lines = match (verb, cmd.literals.get(2).map(String::as_str)) {
            ("schematicannon", _) => match pos("pos") {
                Some(p) => {
                    let option = word("option").unwrap_or_default();
                    let words: Vec<&str> = option.split_whitespace().collect();
                    vec![cannon_command(&mut lv, p, &words)]
                }
                None => vec!["usage: create_debug schematicannon <x y z> [option]".into()],
            },
            (_, Some("save")) => match (pos("from"), pos("to"), word("name")) {
                (Some(a), Some(b), Some(file)) => save(
                    &mut lv,
                    player,
                    &name,
                    a,
                    b,
                    &file,
                    word("instant").as_deref() == Some("instant"),
                ),
                _ => vec![
                    "usage: create_debug schematic save <x y z> <x y z> <name> [instant]".into(),
                ],
            },
            (_, Some("upload")) => match (pos("pos"), word("name")) {
                (Some(p), Some(file)) => upload(&mut lv, &name, p, &nbt_file(&file)),
                _ => vec!["usage: create_debug schematic upload <x y z> <name>".into()],
            },
            (_, Some("deploy")) => match (word("name"), pos("anchor")) {
                (Some(file), Some(anchor)) => vec![deploy(
                    &mut lv,
                    player,
                    &nbt_file(&file),
                    anchor,
                    word("rotation").as_deref(),
                    word("mirror").as_deref(),
                )],
                _ => vec![
                    "usage: create_debug schematic deploy <name> <x y z> [rotation] [mirror]"
                        .into(),
                ],
            },
            _ => vec!["usage: create_debug schematic save|upload|deploy ...".into()],
        };
        for l in lines {
            feedback(lv.host, l);
        }
        Flow::Cancel
    }
}

/// An inventory a cannon takes from (`attachedInventories`): any block's `ITEMS` through the
/// face toward the cannon.
type Items = mod_api::Capability<dyn mod_api::ResourceHandler<ItemResource>>;

/// What a cannon reaches of the game while it ticks.
struct World<'a, 'l> {
    lv: &'a mut ServerLevel<'l>,
    pos: [i32; 3],
    found: Option<(Vec<Items>, bool)>,
}

/// `Iterate.directions`: down, up, north, south, west, east; each the face toward the cannon.
const SIDES: [([i32; 3], Direction); 6] = [
    ([0, -1, 0], Direction::Up),
    ([0, 1, 0], Direction::Down),
    ([0, 0, -1], Direction::South),
    ([0, 0, 1], Direction::North),
    ([-1, 0, 0], Direction::East),
    ([1, 0, 0], Direction::West),
];

/// The cannon's or table's block entity.
fn entity(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<BlockEntityRef> {
    lv.host.block_entity(lv.level, pos)
}

/// The five slots of the block entity at `pos`.
fn slots(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<Option<ItemStack>> {
    let be = entity(lv, pos);
    (0..CANNON_SLOTS as u32)
        .map(|i| be.and_then(|be| be.item_slot(&*lv.host, i).ok().flatten()))
        .collect()
}

/// Sets slots of the block entity at `pos`, in one transaction.
fn set_slots(lv: &mut ServerLevel<'_>, pos: [i32; 3], writes: &[(u32, Option<ItemStack>)]) {
    let Some(be) = entity(lv, pos) else {
        return;
    };
    OWN_WRITE.set(true);
    let _ = lv.host.transaction(|h, tx| {
        for (slot, stack) in writes {
            be.set_item_slot(h, tx, *slot, stack.clone())
                .map_err(|_| ())?;
        }
        Ok::<(), ()>(())
    });
    OWN_WRITE.set(false);
}

fn item_is(host: &dyn Host, stack: &ItemStack, id: &str) -> bool {
    packages::body::item_id(host, stack) == id
}

impl World<'_, '_> {
    fn item(&self, slot: usize) -> Option<ItemStack> {
        let be = entity(self.lv, self.pos)?;
        be.item_slot(&*self.lv.host, slot as u32).ok().flatten()
    }

    fn set_slot(&mut self, slot: usize, stack: Option<ItemStack>) {
        set_slots(self.lv, self.pos, &[(slot as u32, stack)]);
    }

    /// `findInventories`, once per tick.
    fn found(&mut self) -> &(Vec<Items>, bool) {
        if self.found.is_none() {
            let mut invs = Vec::new();
            let mut seen_vaults: Vec<[i32; 3]> = Vec::new();
            let mut creative = false;
            for (o, face) in SIDES {
                let at = packages::add(self.pos, o);
                if !CannonWorld::loaded(self, at) {
                    continue;
                }
                let block = block_of(self.lv.block(at));
                if block == Some(Block::CreativeCrate) {
                    creative = true;
                }
                // A vault reached from two sides is one inventory.
                if block == Some(Block::ItemVault) {
                    let dim = self.lv.dim;
                    let first = super::world::with_world(|w| w.dims[dim].vaults.inventory(at))
                        .flatten()
                        .and_then(|parts| parts.iter().min().copied())
                        .unwrap_or(at);
                    if seen_vaults.contains(&first) {
                        continue;
                    }
                    seen_vaults.push(first);
                }
                if let Some(items) =
                    self.lv
                        .level
                        .capability(&mut *self.lv.host, &ITEMS, at, Some(face))
                {
                    invs.push(items);
                }
            }
            self.found = Some((invs, creative));
        }
        self.found.as_ref().expect("found was just set")
    }

    /// `ItemHelper.extract(inv, matching item, UPTO, max, simulate)`: how many it gave.
    fn extract(&mut self, i: usize, item: &str, max: u32, simulate: bool) -> u32 {
        self.found();
        let Some((invs, _)) = self.found.as_ref() else {
            return 0;
        };
        let Some(inv) = invs.get(i) else {
            return 0;
        };
        let host = &mut *self.lv.host;
        let mut matching = Vec::new();
        for index in 0..inv.size(host) {
            if let Some(r) = inv.resource(host, index)
                && r.item().id(&*host) == item
            {
                matching.push((index, r, inv.amount(host, index)));
            }
        }
        if simulate {
            let have: u32 = matching.iter().map(|m| m.2).sum();
            return have.min(max);
        }
        host.transaction(|h, tx| {
            let mut got = 0;
            for (index, r, _) in &matching {
                if got >= max {
                    break;
                }
                got += inv.extract(h, tx, *index, r, max - got);
            }
            Ok::<u32, ()>(got)
        })
        .unwrap_or(0)
    }
}

impl CannonWorld for World<'_, '_> {
    fn blueprint(&mut self) -> Blueprint {
        match self.item(0) {
            None => Blueprint::Empty,
            Some(s) if item_is(&*self.lv.host, &s, SCHEMATIC) => {
                let d = body::schematic_data(&s);
                Blueprint::Item((!d.is_empty()).then_some(d))
            }
            Some(_) => Blueprint::Item(None),
        }
    }
    fn output_empty(&mut self) -> bool {
        self.item(1).is_none()
    }
    fn finish_blueprint(&mut self) {
        let n = self.item(1).map_or(0, |s| s.count());
        let out = packages::body::new_stack(&*self.lv.host, EMPTY_SCHEMATIC, n + 1);
        set_slots(self.lv, self.pos, &[(0, None), (1, out)]);
    }
    fn expire_blueprint(&mut self) {
        let out = packages::body::new_stack(&*self.lv.host, EMPTY_SCHEMATIC, 1);
        set_slots(self.lv, self.pos, &[(0, None), (1, out)]);
    }
    fn own_gunpowder(&mut self) -> bool {
        // Create's slot 4 takes only gunpowder (`isItemValid`); the hopper menu takes anything,
        // so only gunpowder is burnt.
        let Some(s) = self
            .item(4)
            .filter(|s| item_is(&*self.lv.host, s, GUNPOWDER))
        else {
            return false;
        };
        self.set_slot(4, s.shrink(1));
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
        load(self.lv, data)
    }
    fn loaded(&mut self, pos: [i32; 3]) -> bool {
        self.lv.level.in_world_bounds(&*self.lv.host, pos) == Some(true) && self.lv.is_loaded(pos)
    }
    fn spot(&mut self, pos: [i32; 3]) -> Spot {
        let Some(state) = self.lv.block_state(pos) else {
            return Spot {
                state: 0,
                unbreakable: false,
                conductor: false,
                has_block_entity: false,
            };
        };
        let facts = self.lv.host.block_states();
        Spot {
            state: intern(state),
            unbreakable: facts.destroy_speed(state) == Some(-1.0),
            conductor: full_cube(self.lv, state, pos),
            has_block_entity: facts.has_block_entity(state) == Some(true),
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
    /// `ForBelt.place`: a shaft where the belt's laying end is, then the belt to its other end
    /// (`createBelts`), and each segment cased as the schematic's was (`setCasingType`).
    fn place_belt(&mut self, pos: [i32; 3], state: u32, to: [i32; 3], casings: &[u8]) {
        let Some(s) = interned(state) else { return };
        let host = &*self.lv.host;
        let facts = host.block_states();
        let props: BTreeMap<String, String> = ["facing", "slope", "part"]
            .iter()
            .filter_map(|k| Some(((*k).to_owned(), s.property(&facts, k).ok()??.to_owned())))
            .collect();
        let Some(piece) = belt_piece(host, &props) else {
            return;
        };
        let axis = shaft_axis(piece.slope, piece.facing);
        if let Some(shaft) = host.block_state_id(&format!("create:shaft[axis={axis}]")) {
            self.lv.set_state(pos, shaft, super::server::update_all());
        }
        super::items::create_belts(self.lv, pos, to);
        let n = casings.len().max(1) as i32 - 1;
        let step = [0, 1, 2].map(|i| if n > 0 { (to[i] - pos[i]) / n } else { 0 });
        for (i, c) in casings.iter().enumerate() {
            let kind = match c {
                1 => "andesite",
                2 => "brass",
                _ => continue,
            };
            let at = [0, 1, 2].map(|k| pos[k] + step[k] * i as i32);
            super::logistics::set_casing(self.lv, at, Some(kind));
        }
    }

    fn book_input(&mut self) -> bool {
        self.item(2).is_some()
    }

    /// `getSlotLimit` (64), or the stack's own limit where it is less (a written book's 16, a
    /// clipboard's 1), which the game won't let the slot pass.
    fn book_output_full(&mut self) -> bool {
        let host = &*self.lv.host;
        self.item(3)
            .is_some_and(|s| s.count() >= host.max_stack_size(s.item()).unwrap_or(64).min(64))
    }

    /// `updateChecklist`'s gathering: every stack an inventory beside the cannon would give.
    fn inventory_items(&mut self) -> Vec<(String, u32)> {
        self.found();
        let Some((invs, _)) = self.found.as_ref() else {
            return Vec::new();
        };
        let host = &mut *self.lv.host;
        let mut out = Vec::new();
        for inv in invs {
            for index in 0..inv.size(host) {
                if let Some(r) = inv.resource(host, index) {
                    let n = inv.amount(host, index);
                    if n > 0 {
                        out.push((r.item().id(&*host), n));
                    }
                }
            }
        }
        out
    }

    /// `tickPaperPrinter`'s finished page: one book (or clipboard) from slot 2 becomes the
    /// checklist (`createWrittenBook`, `createWrittenClipboard`) in slot 3, one more of it there.
    fn write_checklist(&mut self, checklist: &cn::Checklist) {
        let Some(input) = self.item(2) else { return };
        let host: &dyn Host = &*self.lv.host;
        let clipboard = item_is(host, &input, "create:clipboard");
        let Some(mut out) = (if clipboard {
            checklist_clipboard(host, checklist)
        } else {
            checklist_book(host, checklist)
        }) else {
            return;
        };
        let n = self.item(3).map_or(0, |s| s.count()) + 1;
        let _ = out.set_count(n);
        set_slots(self.lv, self.pos, &[(2, input.shrink(1)), (3, Some(out))]);
    }

    /// `ForEntity.place` (`StructureTemplate.placeEntities`' entity): its data without its
    /// UUID, at its place, turned and mirrored with the template (its yaw; a hanging entity's
    /// facing and block), spawned.
    fn place_entity(&mut self, e: &cn::PlacedEntity) {
        let Tag::Compound(mut nbt) = body::to_game(&e.nbt) else {
            return;
        };
        let kind = text(nbt.get(&"id".into()));
        nbt.remove(&"id".into());
        nbt.remove(&"UUID".into());
        nbt.insert(
            "Pos".into(),
            Tag::List(e.pos.iter().map(|v| Tag::Double(v.to_bits())).collect()),
        );
        if let Some(Tag::List(r)) = nbt.get(&"Rotation".into()).cloned()
            && let Some(Tag::Float(yaw)) = r.first()
        {
            let yaw = sch::transform_yaw(f32::from_bits(*yaw), e.mirror, e.rotation);
            let mut r = r;
            r[0] = Tag::Float(yaw.to_bits());
            nbt.insert("Rotation".into(), Tag::List(r));
        }
        // An item frame's `Facing` (3D data value) and a painting's `facing` (2D).
        const D3: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
        const D2: [&str; 4] = ["south", "west", "north", "east"];
        for (key, names) in [("Facing", &D3[..]), ("facing", &D2[..])] {
            let Some(Tag::Byte(f)) = nbt.get(&key.into()).cloned() else {
                continue;
            };
            let Some(name) = names.get(f as usize) else {
                continue;
            };
            let turned = sch::transform_dir(name, e.mirror, e.rotation);
            if let Some(i) = names.iter().position(|n| *n == turned) {
                nbt.insert(key.into(), Tag::Byte(i as i8));
            }
        }
        if nbt.contains_key(&"block_pos".into()) {
            nbt.insert("block_pos".into(), Tag::IntArray(e.block_pos.to_vec()));
        }
        for (i, key) in ["TileX", "TileY", "TileZ"].iter().enumerate() {
            if nbt.contains_key(&(*key).into()) {
                nbt.insert((*key).into(), Tag::Int(e.block_pos[i]));
            }
        }
        let dimension = self.lv.dimension.to_owned();
        let _ = self.lv.host.summon(&dimension, &kind, e.pos, nbt);
    }

    fn place(&mut self, pos: [i32; 3], state: u32) {
        self.place_data(pos, state, None);
    }
    fn place_data(&mut self, pos: [i32; 3], state: u32, data: Option<&sch::Nbt>) {
        if let Some(s) = interned(state) {
            self.lv.set_state(pos, s, place_flags());
            if let Some(data) = data {
                merge_block_data(self.lv, pos, s, data);
            }
            second_half(self.lv, pos, s);
        }
    }
    fn fired(&mut self, _now: u64) {
        let at = self.pos.map(|v| f64::from(v) + 0.5);
        let _ = self.lv.level.play_sound(
            &mut *self.lv.host,
            "minecraft:entity.generic.explode",
            at,
            SoundSource::Blocks,
            0.1,
            1.1,
        );
    }
    fn dinged(&mut self) {
        let at = self.pos.map(|v| f64::from(v) + 0.5);
        let _ = self.lv.level.play_sound(
            &mut *self.lv.host,
            "minecraft:block.note_block.bell",
            at,
            SoundSource::Blocks,
            1.0,
            0.7,
        );
    }
}

/// `placeSchematicBlock`'s data: the block entity placed at `pos` (`state`) loads the
/// schematic's data (`loadWithComponents`, its `x`, `y`, `z` set to where it is), after
/// `NBTProcessors.process`: a sign whose text runs a click command keeps none.
fn merge_block_data(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: BlockState, data: &sch::Nbt) {
    let Tag::Compound(mut data) = body::to_game(data) else {
        return;
    };
    for k in ["id", "x", "y", "z"] {
        data.remove(&k.into());
    }
    if lv.host.block_in_tag(state.block(), "minecraft:all_signs") == Some(true)
        && sign_has_click(&data)
    {
        return;
    }
    let Some(be) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    if let Err(e) = be.merge_data(&mut *lv.host, data) {
        crate::create_log!("[create] a printed block's data was refused at {pos:?}: {e:?}");
    }
}

/// `NBTProcessors.signProcessor`: a line of either side's text with a click event.
fn sign_has_click(data: &Compound) -> bool {
    fn clicks(t: &Tag) -> bool {
        match t {
            Tag::Compound(c) => {
                c.contains_key(&"click_event".into())
                    || c.contains_key(&"clickEvent".into())
                    || c.values().any(clicks)
            }
            Tag::List(l) => l.iter().any(clicks),
            Tag::String(s) => {
                let s = s.to_string_lossy();
                s.contains("click_event") || s.contains("clickEvent")
            }
            _ => false,
        }
    }
    ["front_text", "back_text"]
        .iter()
        .any(|k| data.get(&(*k).into()).is_some_and(clicks))
}

/// `placeSchematicBlock`'s `setPlacedBy` for the blocks of two (`DoorBlock`, `DoublePlantBlock`
/// and `SmallDripleafBlock`: the upper half above; `BedBlock`: the head toward its facing),
/// written with the game's updates: the printer ignores those halves in the schematic.
fn second_half(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: BlockState) {
    let facts = lv.host.block_states();
    let prop = |k: &str| state.property(&facts, k).ok().flatten();
    let (at, other) = if prop("half") == Some("lower") {
        let Ok(upper) = state.with(&facts, "half", "upper") else {
            return;
        };
        // `copyWaterloggedFrom` the block above (the printer's placed state is dry).
        ([pos[0], pos[1] + 1, pos[2]], upper)
    } else if prop("part") == Some("foot") && prop("occupied").is_some() {
        let o = match prop("facing") {
            Some("north") => [0, 0, -1],
            Some("south") => [0, 0, 1],
            Some("west") => [-1, 0, 0],
            Some("east") => [1, 0, 0],
            _ => return,
        };
        let Ok(head) = state.with(&facts, "part", "head") else {
            return;
        };
        ([pos[0] + o[0], pos[1], pos[2] + o[2]], head)
    } else {
        return;
    };
    let other = match lv.block_name(at).as_deref() {
        Some("minecraft:water") if prop("half").is_some() => {
            other.with(&facts, "waterlogged", "true").unwrap_or(other)
        }
        _ => other,
    };
    lv.set_state(at, other, super::server::update_all());
}

/// Whether `state`'s collision shape at `pos` is a whole block (`isCollisionShapeFullBlock`).
fn full_cube(lv: &ServerLevel<'_>, state: BlockState, pos: [i32; 3]) -> bool {
    lv.level
        .block_shape(&*lv.host, state, pos, BlockShapeKind::Collision)
        .is_some_and(|boxes| boxes.len() == 1 && boxes[0] == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
}

/// Whether `state` is air (`isAir`: air, cave air, void air).
fn is_air(host: &dyn Host, state: BlockState) -> bool {
    matches!(
        state.block().id(host).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// A block state by name and properties, as `NbtUtils.readBlockState` reads one: its default
/// state with each property it has set where the value exists; air for an unknown block.
fn resolve_state(
    host: &dyn Host,
    name: &str,
    props: &BTreeMap<String, String>,
) -> Option<BlockState> {
    let default = host
        .block_state_id(name)
        .or_else(|| host.block_state_id(sch::AIR))?;
    let facts = host.block_states();
    let mut s = default;
    for (k, v) in props {
        if let Ok(next) = s.with(&facts, k, v) {
            s = next;
        }
    }
    Some(s)
}

/// A palette entry turned and resolved once per load: what it prints, places and costs.
fn entry(lv: &ServerLevel<'_>, p: &PaletteState, rotation: Rotation, mirror: Mirror) -> Entry {
    let host: &dyn Host = &*lv.host;
    let (mut name, mut props) = (p.name.clone(), p.properties.clone());
    if host.block_state_id(&name).is_none() {
        name = sch::AIR.into();
        props.clear();
    }
    sch::transform_state(&name, &mut props, mirror, rotation);
    sch::printed_state(&mut name, &mut props);
    let state = resolve_state(host, &name, &props);
    let need = sch::need(&name, &props);
    let requirement = match &need {
        Need::Nothing => Requirement::Empty,
        // `BeltBlock.getRequiredItems`: a shaft at each end, and the connector at the start.
        Need::Item { .. } if name == "create:belt" => {
            let part = props.get("part").map(String::as_str);
            let mut items = Vec::new();
            if part != Some("middle") {
                items.push(("create:shaft".to_owned(), 1));
            }
            if part == Some("start") {
                items.push(("create:belt_connector".to_owned(), 1));
            }
            if items.is_empty() {
                Requirement::Empty
            } else {
                Requirement::Items(items)
            }
        }
        Need::Item { count, item } => {
            match item
                .map(str::to_owned)
                .or_else(|| body::item_for_block(host, &name))
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
    let belt = (name == "create:belt")
        .then(|| belt_piece(host, &props))
        .flatten();
    let plain_air = name == sch::AIR;
    let (mut placed_name, mut placed_props) = (name, props);
    sch::placed_state(&mut placed_name, &mut placed_props);
    let placed = resolve_state(host, &placed_name, &placed_props);
    Entry {
        state: state.map_or(0, intern),
        placed: placed.map_or(0, intern),
        air: state.is_none_or(|s| is_air(host, s)),
        plain_air,
        conductor: state.is_some_and(|s| full_cube(lv, s, [0; 3])),
        requirement,
        ignore,
        defer,
        other,
        belt,
        // `prepareBlockEntityData`: `AllBlockTags.SAFE_NBT`, or a copycat's `writeSafe`.
        safe_nbt: state.is_some_and(|s| {
            host.block_in_tag(s.block(), "create:safe_nbt") == Some(true)
                || matches!(
                    states::id(s).and_then(block_of),
                    Some(Block::CopycatPanel | Block::CopycatStep)
                )
        }),
    }
}

/// A belt state's shape for the printer, and the shaft it leaves where it doesn't lay the belt
/// (`stripBeltIfNotLast`: sideways, upright; else across its facing).
fn belt_piece(host: &dyn Host, props: &BTreeMap<String, String>) -> Option<cn::BeltPiece> {
    let p = |k: &str| props.get(k).map(String::as_str);
    let facing = match p("facing")? {
        "north" => [0, 0, -1],
        "south" => [0, 0, 1],
        "west" => [-1, 0, 0],
        _ => [1, 0, 0],
    };
    let slope = match p("slope")? {
        "upward" => cn::BeltSlope::Upward,
        "downward" => cn::BeltSlope::Downward,
        "vertical" => cn::BeltSlope::Vertical,
        "sideways" => cn::BeltSlope::Sideways,
        _ => cn::BeltSlope::Horizontal,
    };
    let part = match p("part")? {
        "start" => cn::BeltPart::Start,
        "end" => cn::BeltPart::End,
        "pulley" => cn::BeltPart::Pulley,
        _ => cn::BeltPart::Middle,
    };
    let axis = shaft_axis(slope, facing);
    let shaft = host
        .block_state_id(&format!("create:shaft[axis={axis}]"))
        .map_or(0, intern);
    Some(cn::BeltPiece {
        facing,
        slope,
        part,
        shaft,
    })
}

/// `slope == SIDEWAYS ? Y : facing.getClockWise().getAxis()`.
fn shaft_axis(slope: cn::BeltSlope, facing: [i32; 3]) -> &'static str {
    if slope == cn::BeltSlope::Sideways {
        "y"
    } else if facing[0] != 0 {
        "z"
    } else {
        "x"
    }
}

/// `owner/file` under `schematics/uploaded/`, when both are names a player may give.
fn uploaded(owner: &str, file: &str) -> Option<String> {
    let owner_ok = !owner.is_empty() && !owner.contains(['/', '\\']) && !owner.starts_with('.');
    (owner_ok && sch::valid_file(file)).then(|| format!("uploaded/{owner}/{file}"))
}

/// A structure file read (an unreadable one is empty, as `loadSchematic`'s fresh template).
fn read(host: &dyn Host, rel: &str) -> Structure {
    body::read_structure(host, rel)
        .and_then(|t| Structure::from_nbt(&t))
        .unwrap_or_default()
}

/// `SchematicPrinter.loadSchematic` from the server's uploaded copy.
fn load(lv: &ServerLevel<'_>, data: &SchematicData) -> Loaded {
    let structure = match (&data.owner, &data.file) {
        (Some(o), Some(f)) => uploaded(o, f)
            .map(|r| read(&*lv.host, &r))
            .unwrap_or_default(),
        _ => Structure::default(),
    };
    let anchor = data.anchor.unwrap_or_default();
    let printer = Printer::load(&structure, anchor, data.rotation, data.mirror);
    let entries = structure
        .palette
        .iter()
        .map(|p| entry(lv, p, data.rotation, data.mirror))
        .collect();
    let entities = structure
        .entities
        .iter()
        .map(|e| {
            let pos = sch::transform_vec(e.pos, data.mirror, data.rotation, [0; 3]);
            let block = sch::transform(e.block_pos, data.mirror, data.rotation, [0; 3]);
            (
                entity_requirement(e),
                cn::PlacedEntity {
                    nbt: e.nbt.clone(),
                    pos: [0, 1, 2].map(|i| pos[i] + f64::from(anchor[i])),
                    block_pos: [0, 1, 2].map(|i| block[i] + anchor[i]),
                    rotation: data.rotation,
                    mirror: data.mirror,
                },
            )
        })
        .collect();
    Loaded {
        printer,
        entries,
        entities,
        air: entry(lv, &PaletteState::air(), Rotation::None, Mirror::None),
    }
}

/// Where the cannon at `pos` aims, for its clients: into its block entity's data.
fn send_aim(lv: &mut ServerLevel<'_>, c: &Cannon) {
    let Some(be) = entity(lv, c.pos) else {
        return;
    };
    let t = c.target();
    let p = t.unwrap_or_default();
    let aim = vec![
        i32::from(t.is_some()),
        p[0],
        p[1],
        p[2],
        c.default_yaw.to_bits() as i32,
    ];
    let data = Compound::from([(AIM.into(), Tag::IntArray(aim))]);
    OWN_WRITE.set(true);
    let _ = be.merge_data(&mut *lv.host, data);
    OWN_WRITE.set(false);
}

/// What the cannon's screen shows, into its menu data slots when it changed.
fn send_screen_data(lv: &mut ServerLevel<'_>, c: &Cannon) {
    let data = screens::cannon_data(c);
    let dim = lv.dim;
    let same = with(dim, |d| d.sent_data.get(&c.pos) == Some(&data));
    if same {
        return;
    }
    let Some(be) = entity(lv, c.pos) else {
        return;
    };
    OWN_WRITE.set(true);
    let set = be.set_data_slots(&mut *lv.host, data.clone());
    OWN_WRITE.set(false);
    if matches!(set, Ok(true) | Ok(false)) {
        with(dim, |d| d.sent_data.insert(c.pos, data));
    }
}

/// The table's upload button (`SchematicUploadPacket`): the schematic chosen, by its place
/// among the saved ones, uploaded at the level's next tick (the table's block entity is out of
/// the world while its button is handled).
fn table_button(host: &mut dyn ServerHost, event: &mut mod_api::MenuButtonEvent<'_>) {
    let context = event.context;
    let Some(index) = usize::try_from(context.id - screens::UPLOAD).ok() else {
        return;
    };
    let Some(player) = host.player(context.player) else {
        return;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(dim) = level_index(&dimension) else {
        return;
    };
    let Some(file) = body::local_files(&*host).into_iter().nth(index) else {
        return;
    };
    let name = player.name(&*host).unwrap_or_default();
    UPLOADS.with_borrow_mut(|u| u.push((dim, name, context.pos, file)));
}

/// The uploads asked for in `dim` since its last tick.
fn run_uploads(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mine: Vec<(String, [i32; 3], String)> = UPLOADS.with_borrow_mut(|u| {
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(u).into_iter().partition(|x| x.0 == dim);
        *u = rest;
        mine.into_iter().map(|(_, n, p, f)| (n, p, f)).collect()
    });
    for (name, pos, file) in mine {
        upload(lv, &name, pos, &file);
    }
}

/// `ConfigureSchematicannonPacket` from the cannon's screen, and its confirm button.
fn cannon_button(host: &mut dyn ServerHost, event: &mut mod_api::MenuButtonEvent<'_>) {
    let context = event.context;
    if context.id == screens::CONFIRM {
        host.close_menu(context.player);
        return;
    }
    let Some(player) = host.player(context.player) else {
        return;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(dim) = level_index(&dimension) else {
        return;
    };
    let pos = context.pos;
    let data = with(dim, |d| d.sent_data.get(&pos).cloned()).unwrap_or_default();
    let Some(option) = screens::configure(context.id, &data) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let now = now(&lv);
    with(dim, |d| {
        if let Some(c) = d.cannons.get_mut(&pos) {
            c.configure(now, option);
            d.wake(pos, now);
        }
    });
}

/// The level's tick: cannons placed, removed and woken, then those whose tick is due; clients
/// are told where barrels point now. Nothing runs without a cannon.
fn level_tick(lv: &mut ServerLevel<'_>) {
    run_uploads(lv);
    let dim = lv.dim;
    if !present() && EVENTS.with_borrow(Vec::is_empty) {
        return;
    }
    let now = now(lv);
    let events: Vec<Event> = EVENTS.with_borrow_mut(|e| {
        let (mine, rest): (Vec<Event>, Vec<Event>) = std::mem::take(e)
            .into_iter()
            .partition(|ev| ev.dim() == dim);
        *e = rest;
        mine
    });
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
                    persist::mark_pos_unsaved(dim, pos);
                }
            }
            Event::Changed(_, pos, false) => with(dim, |d| {
                d.unwake(pos);
                d.sent.remove(&pos);
                d.sent_data.remove(&pos);
                d.cannons.remove(&pos);
            }),
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
                lv: &mut *lv,
                pos,
                found: None,
            };
            c.tick(now, &mut w);
        }
        let next = c.next_wake(now);
        let target = c.target();
        let changed = with(dim, |d| {
            if let Some(t) = next {
                d.wake(pos, t);
            }
            let changed = d.sent.get(&pos) != Some(&target);
            if changed {
                d.sent.insert(pos, target);
            }
            changed
        });
        // A cannon just placed or loaded is ticked here and, never sent, sent at rest.
        if changed {
            send_aim(lv, &c);
        }
        send_screen_data(lv, &c);
        with(dim, |d| d.cannons.insert(pos, c));
        persist::mark_pos_unsaved(dim, pos);
    }
}

/// The cannon's and the table's block entities: the barrel drawn where it aims
/// (`SchematicannonRenderer`), the table's screen, and their column's data once they load.
struct SchematicEntity;

#[mod_api::horizon_part]
impl BlockPart for SchematicEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for SchematicEntity {
    fn block_entity_on_load(&self, host: HostSide, event: &mut BlockEntityOnLoadEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let entity = event.entity;
        let (Some(level), Some(pos)) = (entity.level(&*host), entity.position(&*host)) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        super::ensure_column(host, &dimension, [pos[0] >> 4, pos[2] >> 4]);
    }

    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let aim = event.entity.nbt.get(&AIM.into())?.clone();
        Some(Compound::from([(AIM.into(), aim)]))
    }

    fn block_entity_submit(&self, _host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.block != "create:schematicannon" {
            return;
        }
        let Some(&(connector, pipe)) = BARREL.get() else {
            return;
        };
        let a = ints(event.entity.nbt.get(&AIM.into()));
        let (target, default_yaw) = match a.as_slice() {
            [has, x, y, z, yaw] => (
                (*has != 0).then_some([*x, *y, *z]),
                f32::from_bits(*yaw as u32),
            ),
            _ => (None, 0.0),
        };
        let (yaw, pitch) = cn::aim(event.pos, target, default_yaw);
        let turn = (yaw + 90.0).rem_euclid(360.0);
        let draw = &mut *event.draw;
        let up = [("facing", "up")];
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], turn, [0.5, 0.0, 0.5]);
        draw.block(connector, &up);
        draw.rotate_about([0.0, 0.0, 1.0], pitch, [0.5, 15.0 / 16.0, 0.5]);
        draw.block(pipe, &up);
        draw.pop();
        draw.keep();
    }

    /// `SchematicTableMenu`: the input and output slots, as the hopper's first and last.
    fn screen_draw(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenDrawEvent<'_>) {
        match event.block {
            "create:schematicannon" => {
                screens::draw_cannon(host, event.draw, event.data, event.origin);
            }
            "create:schematic_table" => {
                TABLE_OPEN.set(true);
                screens::draw_table(host, event.draw, event.data, event.origin);
            }
            _ => {}
        }
    }

    fn screen_click(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenClickEvent<'_>,
    ) -> Option<i32> {
        if event.right {
            return None;
        }
        match event.block {
            "create:schematicannon" => screens::click_cannon(host, event.data, event.at),
            "create:schematic_table" => screens::click_table(host, event.at),
            _ => None,
        }
    }

    fn screen_tooltip(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenTooltipEvent<'_>) {
        match event.block {
            "create:schematicannon" => {
                screens::tooltip_cannon(host, event.data, event.at, event.lines);
            }
            "create:schematic_table" => screens::tooltip_table(host, event.at, event.lines),
            _ => {}
        }
    }

    fn menu_button(&self, host: &mut dyn ServerHost, event: &mut mod_api::MenuButtonEvent<'_>) {
        match event.entity.id() {
            "create:schematicannon" => cannon_button(host, event),
            "create:schematic_table" => table_button(host, event),
            _ => {}
        }
    }

    /// `SchematicannonMenu.quickMoveStack` and `SchematicTableMenu`'s: from the player's
    /// inventory a schematic goes to the schematic slot, a book or clipboard to the checklist's,
    /// gunpowder to the fuel slot (`SchematicannonInventory.isItemValid`); the table takes blank
    /// schematics, quills and schematics in its input. The game's own routing (the first slot
    /// with room) moves it when that is the right slot; a part of a stack is moved here; a whole
    /// stack bound elsewhere stays, as the hook cannot empty the clicked slot.
    fn menu_quick_move(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::MenuQuickMoveEvent<'_>,
    ) -> Flow {
        if event.slot < event.container_slots {
            return Flow::Continue;
        }
        let id = packages::body::item_id(&*host, event.stack);
        let to = match (event.entity.id(), id.as_str()) {
            ("create:schematicannon", SCHEMATIC) => 0,
            ("create:schematicannon", "minecraft:book" | "minecraft:written_book")
            | ("create:schematicannon", "create:clipboard") => 2,
            ("create:schematicannon", GUNPOWDER) => 4,
            ("create:schematic_table", EMPTY_SCHEMATIC | QUILL | SCHEMATIC) => 0,
            _ => return Flow::Cancel,
        };
        let max = host.max_stack_size(event.stack.item()).unwrap_or(64);
        let stack = event.stack.clone();
        let room = |s: &Option<ItemStack>| match s {
            None => max,
            Some(t) if t.is_stackable_with(&stack) => max.saturating_sub(t.count()),
            Some(_) => 0,
        };
        let items = &mut event.entity.items;
        if items.len() <= to {
            return Flow::Cancel;
        }
        if items.iter().position(|s| room(s) > 0) == Some(to) && room(&items[to]) >= stack.count() {
            return Flow::Continue;
        }
        let n = room(&items[to]).min(stack.count());
        if n == 0 || n == stack.count() {
            return Flow::Cancel;
        }
        let there = items[to].as_ref().map_or(0, ItemStack::count);
        items[to] = packages::body::with_count(&stack, there + n);
        if let Some(rest) = packages::body::with_count(&stack, stack.count() - n) {
            *event.stack = rest;
        }
        Flow::Cancel
    }

    fn screen(&self, _host: &dyn Host, event: &mut ScreenEvent<'_>) -> Option<Screen> {
        if event.block == "create:schematicannon" {
            return Some(screens::cannon_screen());
        }
        (event.block == "create:schematic_table").then(screens::table_screen)
    }
}

/// A cannon's slots changed (a player's click in its menu): it looks at them again.
struct CannonBlock;

#[mod_api::horizon_part]
impl BlockPart for CannonBlock {
    fn contents_changed(&self, host: &mut dyn ServerHost, event: &mut ContentsChangedEvent) {
        if OWN_WRITE.get() {
            return;
        }
        let Some(dim) = event
            .level
            .dimension()
            .id(&*host)
            .and_then(|d| level_index(&d))
        else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        if CANNONS.with_borrow(|c| {
            c.as_ref()
                .and_then(|c| c.get(dim))
                .is_none_or(|d| !d.cannons.contains_key(&pos))
        }) {
            return;
        }
        EVENTS.with_borrow_mut(|e| e.push(Event::Wake(dim, pos, true)));
    }
}

/// The cannon placed from its item: it takes the placer's yaw and the item's options (none:
/// `applyImplicitComponents`' default).
struct CannonItem;

#[mod_api::horizon_part]
impl ItemPart for CannonItem {
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(player) = event.placer else {
            return;
        };
        let Some(dim) = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()))
        else {
            return;
        };
        let yaw = player.entity().rotation(&*host).map_or(0.0, |r| r[0]);
        let sneaking = player.is_secondary_use_active(&*host) == Some(true);
        let yaw = cn::placed_yaw(yaw, sneaking);
        EVENTS.with_borrow_mut(|e| e.push(Event::Placed(dim, event.pos, yaw)));
    }
}

fn nbt_file(name: &str) -> String {
    if name.ends_with(".nbt") {
        name.to_owned()
    } else {
        format!("{name}.nbt")
    }
}

/// `SchematicExport.saveSchematic` of the box between two corners: the blocks and the entities
/// in it (`fillFromWorld` with entities: each block entity's data with its `id`, as
/// `saveWithId`; item frames and paintings are not found).
fn export(lv: &mut ServerLevel<'_>, a: [i32; 3], b: [i32; 3]) -> Structure {
    let min = [0, 1, 2].map(|i| a[i].min(b[i]));
    let max = [0, 1, 2].map(|i| a[i].max(b[i]));
    let size = [0, 1, 2].map(|i| max[i] - min[i] + 1);
    let host: &dyn Host = &*lv.host;
    let facts = host.block_states();
    let mut cells = Vec::new();
    for y in min[1]..=max[1] {
        for x in min[0]..=max[0] {
            for z in min[2]..=max[2] {
                let pos = [x, y, z];
                let Some(state) = lv.block_state(pos) else {
                    continue;
                };
                let Some(text) = host.block_state_string(state) else {
                    continue;
                };
                let (name, props) = states::split(&text);
                let class = if facts.has_block_entity(state) == Some(true) {
                    Class::WithEntity
                } else if full_cube(lv, state, pos) {
                    Class::Full
                } else {
                    Class::Special
                };
                cells.push((
                    [x - min[0], y - min[1], z - min[2]],
                    PaletteState {
                        name: name.to_owned(),
                        properties: props
                            .into_iter()
                            .map(|(k, v)| (k.to_owned(), v.to_owned()))
                            .collect(),
                    },
                    class,
                ));
            }
        }
    }
    let mut structure = Structure::capture(size, cells);
    for b in &mut structure.blocks {
        let at = [0, 1, 2].map(|i| b.pos[i] + min[i]);
        let host: &dyn Host = &*lv.host;
        let Some(be) = host.block_entity(lv.level, at) else {
            continue;
        };
        let (Some(data), Some(kind)) = (be.data(host), be.kind(host)) else {
            continue;
        };
        let mut data = data;
        data.insert("id".into(), Tag::String(kind.id(host).as_str().into()));
        b.nbt = Some(body::to_engine(&Tag::Compound(data)));
    }
    structure.entities = export_entities(lv, min, max);
    structure
}

/// `StructureTemplate.fillEntityList`: the entities in the box but players and passengers
/// (`Entity.save` writes none for a passenger), each at its offset from the box's corner, its
/// block a painting's own (`getPos`) else the one it stands in, and its data with its `id`.
fn export_entities(lv: &mut ServerLevel<'_>, min: [i32; 3], max: [i32; 3]) -> Vec<sch::EntityInfo> {
    let lo = min.map(f64::from);
    let hi = max.map(|v| f64::from(v) + 1.0);
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    let mut out = Vec::new();
    for e in found {
        let host: &dyn Host = &*lv.host;
        let Some(kind) = e.kind(host).map(|k| k.id(host)) else {
            continue;
        };
        if kind == "minecraft:player" || e.is_passenger(host) == Some(true) {
            continue;
        }
        let Some(at) = e.position(host) else { continue };
        let Some(mut nbt) = e.nbt(&mut *lv.host) else {
            continue;
        };
        nbt.insert("id".into(), Tag::String(kind.as_str().into()));
        let pos = [0, 1, 2].map(|i| at[i] - f64::from(min[i]));
        let tile = ints(nbt.get(&"block_pos".into()));
        let block_pos = if kind == "minecraft:painting" && tile.len() == 3 {
            [0, 1, 2].map(|i| tile[i] - min[i])
        } else {
            pos.map(|v| v.floor() as i32)
        };
        out.push(sch::EntityInfo {
            pos,
            block_pos,
            nbt: body::to_engine(&Tag::Compound(nbt)),
        });
    }
    out
}

/// A text component as its NBT is written (`text`, a colour, children).
fn text_tag(text: &str, color: Option<&str>, extra: Vec<Tag>) -> Tag {
    let mut c = Compound::new();
    c.insert("text".into(), Tag::String(text.into()));
    if let Some(color) = color {
        c.insert("color".into(), Tag::String(color.into()));
    }
    if !extra.is_empty() {
        c.insert("extra".into(), Tag::List(extra));
    }
    Tag::Compound(c)
}

/// An item's name key (`getDescriptionId`: a block's item is named as its block).
fn description_id(host: &dyn Host, item: &str) -> String {
    let (ns, path) = item.split_once(':').unwrap_or(("minecraft", item));
    let block = packages::body::item_key(host, item).and_then(|k| host.item_block(k));
    if block.is_some() {
        format!("block.{ns}.{path}")
    } else {
        format!("item.{ns}.{path}")
    }
}

/// `MaterialChecklist.entry` for a book: the item's name (hovering shows it) in blue, or green
/// with a tick when done, then the count and the stacks it makes.
fn checklist_entry(host: &dyn Host, item: &str, amount: u32, unfinished: bool) -> Tag {
    let mut name = Compound::new();
    name.insert(
        "translate".into(),
        Tag::String(description_id(host, item).as_str().into()),
    );
    let mut hover = Compound::new();
    hover.insert("action".into(), Tag::String("show_item".into()));
    hover.insert("id".into(), Tag::String(item.into()));
    name.insert("hover_event".into(), Tag::Compound(hover));
    let mut extra = vec![Tag::Compound(name)];
    if !unfinished {
        extra.push(text_tag(" \u{2714}", None, Vec::new()));
    }
    extra.push(text_tag(
        &format!("\n x{amount}"),
        Some("black"),
        Vec::new(),
    ));
    extra.push(text_tag(
        &format!(" | {}\u{25A4} +{}\n", amount / 64, amount % 64),
        Some("gray"),
        Vec::new(),
    ));
    text_tag(
        "",
        Some(if unfinished { "blue" } else { "dark_green" }),
        extra,
    )
}

/// `MaterialChecklist.createWrittenBook`: five entries a page, the missing ones first, each
/// page ending in a blue (or green) ">>>" when another follows.
fn checklist_book(host: &dyn Host, c: &cn::Checklist) -> Option<ItemStack> {
    const PER_PAGE: usize = 5;
    let mut pages: Vec<Tag> = Vec::new();
    if c.not_loaded {
        let mut t = Compound::new();
        t.insert(
            "translate".into(),
            Tag::String("create.materialChecklist.blocksNotLoaded".into()),
        );
        pages.push(text_tag("\n\u{a7}c", None, vec![Tag::Compound(t)]));
    }
    // Sorted by the English name (the server has no other: `display::item_name`).
    let name = super::display::item_name;
    let (missing, done) = c.entries(name);
    let mut page: Vec<Tag> = Vec::new();
    let mut written = 0;
    for (list, unfinished, more) in [(&missing, true, "blue"), (&done, false, "dark_green")] {
        for (item, n) in list {
            if written == PER_PAGE {
                written = 0;
                page.push(text_tag("\n >>>", Some(more), Vec::new()));
                pages.push(text_tag("", None, std::mem::take(&mut page)));
            }
            written += 1;
            page.push(checklist_entry(host, item, *n, unfinished));
        }
    }
    pages.push(text_tag("", None, page));
    let raw = |t: Tag| {
        let mut c = Compound::new();
        c.insert("raw".into(), t);
        Tag::Compound(c)
    };
    let mut content = Compound::new();
    content.insert(
        "title".into(),
        raw(Tag::String("\u{a7}9Material Checklist".into())),
    );
    content.insert("author".into(), Tag::String("Schematicannon".into()));
    content.insert("generation".into(), Tag::Int(0));
    content.insert(
        "pages".into(),
        Tag::List(pages.into_iter().map(raw).collect()),
    );
    content.insert("resolved".into(), Tag::Byte(1));
    let mut book = packages::body::new_stack(host, "minecraft:written_book", 1)?;
    let mut patch = Compound::new();
    patch.insert(
        "minecraft:written_book_content".into(),
        Tag::Compound(content),
    );
    patch.insert("minecraft:custom_name".into(), checklist_name(Some("blue")));
    book.apply_components(&patch).ok()?;
    Some(book)
}

/// `CreateLang.translateDirect("materialChecklist")`, not italic.
fn checklist_name(color: Option<&str>) -> Tag {
    let mut c = Compound::new();
    c.insert(
        "translate".into(),
        Tag::String("create.materialChecklist".into()),
    );
    if let Some(color) = color {
        c.insert("color".into(), Tag::String(color.into()));
    }
    c.insert("italic".into(), Tag::Byte(0));
    Tag::Compound(c)
}

/// `MaterialChecklist.createWrittenClipboard`: seven entries a page, each beside its item; the
/// done ones ticked.
fn checklist_clipboard(host: &dyn Host, c: &cn::Checklist) -> Option<ItemStack> {
    use create_engine::clipboard::{Content, Entry, Kind};
    const PER_PAGE: usize = 7;
    // Sorted by the English name (the server has no other: `display::item_name`).
    let name = super::display::item_name;
    let mut pages = Vec::new();
    let mut page = Vec::new();
    if c.not_loaded {
        page.push(Entry {
            checked: false,
            text: create_engine::display::en_us("create.materialChecklist.blocksNotLoaded")
                .unwrap_or("create.materialChecklist.blocksNotLoaded")
                .to_owned(),
            icon: None,
        });
    }
    let (missing, done) = c.entries(name);
    let mut written = 0;
    for (list, finished) in [(&missing, false), (&done, true)] {
        for (item, n) in list {
            if written == PER_PAGE {
                written = 0;
                page.push(Entry {
                    checked: finished,
                    text: ">>>".into(),
                    icon: None,
                });
                pages.push(std::mem::take(&mut page));
            }
            written += 1;
            page.push(Entry {
                checked: finished,
                text: format!("{}\n x{n} | {}\u{25A4} +{}", name(item), n / 64, n % 64),
                icon: Some((item.clone(), if finished { 0 } else { *n as i32 })),
            });
        }
    }
    pages.push(page);
    let clip = packages::body::new_stack(host, "create:clipboard", 1)?;
    let content = Content {
        kind: Kind::Written,
        pages,
        read_only: true,
        previously_opened_page: 0,
    };
    let mut clip = super::display::clipboard::with_content(&clip, Some(&content));
    let mut patch = Compound::new();
    patch.insert("minecraft:custom_name".into(), checklist_name(None));
    clip.apply_components(&patch).ok()?;
    Some(clip)
}

/// `ItemRequirement.of(Entity)`: an item frame its frame and the item in it, an armor stand
/// itself and what it wears and holds; any other entity can't be printed.
fn entity_requirement(e: &sch::EntityInfo) -> Requirement {
    let Tag::Compound(nbt) = body::to_game(&e.nbt) else {
        return Requirement::Invalid;
    };
    let kind = text(nbt.get(&"id".into()));
    let item_of = |t: &Tag| match t {
        Tag::Compound(c) => {
            let id = text(c.get(&"id".into()));
            (!id.is_empty() && id != "minecraft:air").then_some((id, 1))
        }
        _ => None,
    };
    match kind.as_str() {
        "minecraft:item_frame" | "minecraft:glow_item_frame" => {
            let mut items = vec![(kind.clone(), 1)];
            items.extend(nbt.get(&"Item".into()).and_then(item_of));
            Requirement::Items(items)
        }
        "minecraft:armor_stand" => {
            let mut items = vec![("minecraft:armor_stand".to_owned(), 1)];
            for key in ["ArmorItems", "HandItems"] {
                if let Some(Tag::List(l)) = nbt.get(&key.into()) {
                    items.extend(l.iter().filter_map(item_of));
                }
            }
            if let Some(Tag::Compound(eq)) = nbt.get(&"equipment".into()) {
                items.extend(eq.values().filter_map(item_of));
            }
            Requirement::Items(items)
        }
        _ => Requirement::Invalid,
    }
}

/// The stack in the player's selected slot, and that slot.
fn held(lv: &ServerLevel<'_>, player: PlayerRef) -> Option<(u32, ItemStack)> {
    let slot = player.selected_slot(&*lv.host)? as u32;
    let stack = player
        .inventory_slots(&*lv.host)?
        .into_iter()
        .find(|(s, _)| *s == slot)?
        .1;
    Some((slot, stack))
}

/// `SchematicAndQuillHandler.saveSchematic`: saved in `schematics/` under a free name; with
/// `instant` (the convert button), `ServerSchematicLoader.handleInstantSchematic` too: saved
/// again as the player's upload and the held schematic and quill turned into its schematic.
fn save(
    lv: &mut ServerLevel<'_>,
    player: Option<PlayerRef>,
    name: &str,
    a: [i32; 3],
    b: [i32; 3],
    file: &str,
    instant: bool,
) -> Vec<String> {
    let structure = export(lv, a, b);
    let host: &dyn Host = &*lv.host;
    let file = sch::export_name(file, false, |f| body::exists(host, f));
    let tag = structure.to_nbt();
    if let Err(e) = body::write_structure(host, &file, &tag) {
        crate::create_log!("[create] An error occurred while saving schematic [{file}]: {e}");
        return vec!["Failed to save schematic, check logs for details".into()];
    }
    let mut lines = vec![format!("Saved as {file}")];
    if !instant {
        return lines;
    }
    let holding = player
        .and_then(|p| held(lv, p))
        .filter(|(_, s)| item_is(&*lv.host, s, QUILL));
    let size = body::size(&*lv.host, &file).unwrap_or(0);
    match (uploaded(name, &file), holding) {
        (Some(target), Some((slot, _))) if sch::size_allowed(size) => {
            match body::write_structure(&*lv.host, &target, &tag) {
                Ok(_) => {
                    let Some(mut stack) = packages::body::new_stack(&*lv.host, SCHEMATIC, 1) else {
                        return lines;
                    };
                    body::set_schematic_data(
                        &mut stack,
                        &SchematicData::written(&file, name, structure.size),
                    );
                    if let Some(p) = player {
                        let _ = p.set_inventory_slot(&mut *lv.host, slot, Some(stack));
                    }
                    lines.push(format!("{file} written to the held schematic"));
                }
                Err(e) => {
                    crate::create_log!("[create] Schematic instant-upload of {file}: {e}");
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
/// from `schematics/` copied as the player's upload (one of the same name replaced), the input
/// slot emptied (`startUpload`), and a written schematic in the output
/// (`handleFinishedUpload`). The upload is local, so it finishes at once.
fn upload(lv: &mut ServerLevel<'_>, player: &str, pos: [i32; 3], file: &str) -> Vec<String> {
    if block_of(lv.block(pos)) != Some(Block::SchematicTable) {
        return vec![format!(
            "no schematic table at {} {} {}",
            pos[0], pos[1], pos[2]
        )];
    }
    let items = slots(lv, pos);
    // `SchematicTableMenu.canWrite` and its input slot's `mayPlace`.
    let host: &dyn Host = &*lv.host;
    let writable = items[0].as_ref().is_some_and(|s| {
        let id = packages::body::item_id(host, s);
        matches!(id.as_str(), EMPTY_SCHEMATIC | QUILL | SCHEMATIC)
    });
    if !writable || items[1].is_some() {
        return vec!["the table needs a blank schematic in and nothing out".into()];
    }
    let Some(target) = uploaded(player, file) else {
        crate::create_log!(
            "[create] Attempted Schematic Upload with non-supported Format: {player}/{file}"
        );
        return vec![format!("not a schematic file: {file}")];
    };
    let Some(size) = body::size(host, file) else {
        return vec![format!("no schematic {file} in schematics/")];
    };
    if !sch::size_allowed(size) {
        return vec![too_large(size)];
    }
    if let Err(e) = body::copy(host, file, &target) {
        crate::create_log!("[create] Exception Thrown when starting Upload: {player}/{file}: {e}");
        return vec![format!("upload of {file} failed")];
    }
    crate::create_log!("[create] New Schematic Uploaded: {player}/{file}");
    let bounds = read(host, &target).size;
    let Some(mut stack) = packages::body::new_stack(host, SCHEMATIC, 1) else {
        return vec![format!("upload of {file} failed")];
    };
    body::set_schematic_data(&mut stack, &SchematicData::written(file, player, bounds));
    set_slots(lv, pos, &[(0, None), (1, Some(stack))]);
    vec![format!(
        "uploaded {file}: {}x{}x{}",
        bounds[0], bounds[1], bounds[2]
    )]
}

/// The held schematic deployed (`DeployTool.handleRightClick`, then `SchematicSyncPacket`):
/// deployed at `anchor`, turned and mirrored as given.
fn deploy(
    lv: &mut ServerLevel<'_>,
    player: Option<PlayerRef>,
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
    let Some(player) = player else {
        return format!("hold the schematic of {file}");
    };
    let Some((slot, mut stack)) = held(lv, player) else {
        return format!("hold the schematic of {file}");
    };
    let mut data = body::schematic_data(&stack);
    if !item_is(&*lv.host, &stack, SCHEMATIC) || data.file.as_deref() != Some(file) {
        return format!("hold the schematic of {file}");
    }
    data.deployed = Some(true);
    data.anchor = Some(anchor);
    data.rotation = rotation;
    data.mirror = mirror;
    body::set_schematic_data(&mut stack, &data);
    let _ = player.set_inventory_slot(&mut *lv.host, slot, Some(stack));
    format!(
        "deployed {file} at {} {} {} {} {}",
        anchor[0],
        anchor[1],
        anchor[2],
        rotation.name(),
        mirror.name()
    )
}

/// The player's stack in inventory slot `slot`, if it is a schematic.
fn schematic_in(lv: &ServerLevel<'_>, player: PlayerRef, slot: u32) -> Option<ItemStack> {
    let stack = player
        .inventory_slots(&*lv.host)?
        .into_iter()
        .find(|(s, _)| *s == slot)?
        .1;
    item_is(&*lv.host, &stack, SCHEMATIC).then_some(stack)
}

/// `SchematicSyncPacket.handle`: the schematic in the slot deployed (or not), anchored, turned
/// and mirrored as the client has it. `rest`: slot, deployed, rotation, mirror, anchor.
pub(super) fn synced(host: &mut dyn ServerHost, player: PlayerRef, rest: &[u8]) {
    if rest.len() < 16 {
        return;
    }
    let int = |i: usize| {
        let at = 4 + i * 4;
        i32::from_le_bytes([rest[at], rest[at + 1], rest[at + 2], rest[at + 3]])
    };
    let slot = u32::from(rest[0]);
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let Some(mut stack) = schematic_in(&lv, player, slot) else {
        return;
    };
    let mut data = body::schematic_data(&stack);
    data.deployed = Some(rest[1] != 0);
    data.anchor = Some([int(0), int(1), int(2)]);
    data.rotation = Rotation::ALL[usize::from(rest[2]) % 4];
    data.mirror = Mirror::ALL[usize::from(rest[3]) % 3];
    body::set_schematic_data(&mut stack, &data);
    let _ = player.set_inventory_slot(&mut *lv.host, slot, Some(stack));
}

/// `SchematicPlacePacket.handle`: in creative, the schematic in the slot printed at once, each
/// block the world doesn't have already (and isn't unbreakable there) placed as the cannon
/// places it, air left out (`creativePrintIncludesAir`, false by default), then its entities.
pub(super) fn printed(host: &mut dyn ServerHost, player: PlayerRef, rest: &[u8]) {
    let Some(&slot) = rest.first() else {
        return;
    };
    if player.game_mode(&*host) != Some(mod_api::GameMode::Creative) {
        return;
    }
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let Some(stack) = schematic_in(&lv, player, u32::from(slot)) else {
        return;
    };
    let data = body::schematic_data(&stack);
    if data.anchor.is_none() || data.deployed.is_none() {
        return;
    }
    let Loaded {
        mut printer,
        entries,
        entities,
        air,
    } = load(&lv, &data);
    let include_air = false;
    let mut w = World {
        lv: &mut lv,
        pos: [0; 3],
        found: None,
    };
    while printer.advance(|i| entries[i as usize].defer) {
        let target = printer.target();
        if printer.stage == cn::Stage::Entities {
            if let Some((_, e)) = entities.get(printer.entity_index as usize) {
                w.place_entity(e);
            }
            continue;
        }
        // `shouldPlaceBlock` with every placement allowed.
        if !w.loaded(target) {
            continue;
        }
        let rel = [0, 1, 2].map(|i| target[i] - printer.anchor[i]);
        let entry = printer.at(rel).map_or(&air, |i| &entries[i as usize]);
        let here = w.spot(target);
        if here.state == entry.state || here.unbreakable {
            continue;
        }
        if let Some(o) = entry.other
            && w.spot([0, 1, 2].map(|i| target[i] + o[i])).unbreakable
        {
            continue;
        }
        if entry.air && !include_air {
            continue;
        }
        // What the port's cannon leaves to a block placed with another: a door's upper half, a
        // bed's head, a piston's head, a belt's middle.
        if entry.ignore {
            continue;
        }
        let (state, belt_to, casings) = cn::launch(&printer, &entries, rel, target, entry);
        match belt_to {
            Some(to) => w.place_belt(target, state, to, &casings),
            None => w.place_data(target, state, printer.block_data(rel, entry).as_ref()),
        }
    }
}

/// A cannon's options (`ConfigureSchematicannonPacket`) and state.
fn cannon_command(lv: &mut ServerLevel<'_>, pos: [i32; 3], option: &[&str]) -> String {
    let now = now(lv);
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
    with(lv.dim, |d| {
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

/// A column's cannons saved under `create_schematics` (`write`: state, status, fuel, options,
/// progress, the barrel's rest and the blocks in flight; the printer itself starts over when
/// loaded, as Create's server does).
fn chunk_data_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    if !present() {
        return;
    }
    let now = now(lv);
    let host: &dyn Host = &*lv.host;
    let state_text = |id: u32| {
        interned(id)
            .and_then(|s| host.block_state_string(s))
            .unwrap_or_else(|| sch::AIR.to_owned())
    };
    let list: Vec<Tag> = with(lv.dim, |d| {
        let mut cannons: Vec<&Cannon> = d
            .cannons
            .values()
            .filter(|c| [c.pos[0] >> 4, c.pos[2] >> 4] == chunk)
            .collect();
        cannons.sort_by_key(|c| c.pos);
        cannons
            .into_iter()
            .map(|c| {
                let flying = c
                    .flying
                    .iter()
                    .map(|f| {
                        let left = f.land_at.saturating_sub(now + 1) as i32;
                        let mut entity = Vec::new();
                        if let Some(e) = &f.entity {
                            entity.push((
                                "Entity",
                                Tag::compound([
                                    ("Nbt", body::to_game(&e.nbt)),
                                    (
                                        "Pos",
                                        Tag::List(
                                            e.pos
                                                .iter()
                                                .map(|v| Tag::Double(v.to_bits()))
                                                .collect(),
                                        ),
                                    ),
                                    ("Block", Tag::IntArray(e.block_pos.to_vec())),
                                    ("Rotation", Tag::String(e.rotation.name().into())),
                                    ("Mirror", Tag::String(e.mirror.name().into())),
                                ]),
                            ));
                        }
                        Tag::compound(
                            entity
                                .into_iter()
                                .chain([
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
                                        Tag::String(state_text(f.state).as_str().into()),
                                    ),
                                    (
                                        "BeltTo",
                                        Tag::IntArray(f.belt_to.map_or(Vec::new(), |t| t.to_vec())),
                                    ),
                                    (
                                        "Casing",
                                        Tag::IntArray(
                                            f.casings.iter().map(|c| i32::from(*c)).collect(),
                                        ),
                                    ),
                                ])
                                .chain(f.data.iter().map(|d| ("Data", body::to_game(d)))),
                        )
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
fn chunk_load(lv: &ServerLevel<'_>, data: &Compound) {
    let Some(Tag::List(list)) = data.get(&"cannons".into()) else {
        return;
    };
    let now = now(lv);
    let host: &dyn Host = &*lv.host;
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
                    // A launched entity (`ForEntity`): its data and where it lands.
                    let entity = match f.get(&"Entity".into()) {
                        Some(Tag::Compound(e)) => {
                            let pos = match e.get(&"Pos".into()) {
                                Some(Tag::List(l)) if l.len() == 3 => {
                                    let d = |t: &Tag| match t {
                                        Tag::Double(v) => f64::from_bits(*v),
                                        _ => 0.0,
                                    };
                                    [d(&l[0]), d(&l[1]), d(&l[2])]
                                }
                                _ => [0.0; 3],
                            };
                            let b = ints(e.get(&"Block".into()));
                            e.get(&"Nbt".into()).map(|n| {
                                Box::new(cn::PlacedEntity {
                                    nbt: body::to_engine(n),
                                    pos,
                                    block_pos: [
                                        b.first().copied().unwrap_or(0),
                                        b.get(1).copied().unwrap_or(0),
                                        b.get(2).copied().unwrap_or(0),
                                    ],
                                    rotation: Rotation::parse(&text(e.get(&"Rotation".into())))
                                        .unwrap_or_default(),
                                    mirror: Mirror::parse(&text(e.get(&"Mirror".into())))
                                        .unwrap_or_default(),
                                })
                            })
                        }
                        _ => None,
                    };
                    let state = match host.block_state_id(&text(f.get(&"BlockState".into()))) {
                        Some(s) => intern(s),
                        None if entity.is_some() => 0,
                        None => continue,
                    };
                    if t.len() != 5 {
                        continue;
                    }
                    let to = ints(f.get(&"BeltTo".into()));
                    cannon.flying.push(cn::Flying {
                        target: [t[0], t[1], t[2]],
                        state,
                        belt_to: (to.len() == 3).then(|| [to[0], to[1], to[2]]),
                        casings: ints(f.get(&"Casing".into()))
                            .iter()
                            .map(|c| (*c).clamp(0, 2) as u8)
                            .collect(),
                        data: f.get(&"Data".into()).map(body::to_engine),
                        entity,
                        total: t[4].max(0) as u32,
                        land_at: now + t[3].max(0) as u64 + 1,
                    });
                }
            }
            cannon.last = now;
            Some(cannon)
        })
        .collect();
    with(lv.dim, |d| {
        for c in cannons {
            let pos = c.pos;
            d.cannons.insert(pos, c);
            d.sent.remove(&pos);
            d.wake(pos, now);
        }
    });
}
