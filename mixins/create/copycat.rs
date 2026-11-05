// Create's copycat panel and step in the game (`content/decoration/copycat`). A sibling of the
// `create` module (`runtime.rs`); the rules are `create_engine::copycat`'s. A copycat's material
// (a block state) and the item it consumed are typed, per dimension, and saved with its column as
// `create_copycat`. Clients are told a copycat's shape and material on `create:copycat` when it
// changes, when its column loads and when they join, and draw it with a model override (API 59):
// the material cut to the copycat's box, the copycat base when it holds none. Nothing runs per
// tick: the level's tick looks only when a copycat was placed, changed or broken.
use super::Game;
use super::create::*;
use super::create_copycat_body as body;
use create_engine::Block;
use create_engine::copycat::{self as cc, Shape, Shown, Special, Use};
use create_engine::kinetics::{Axis, Dir};
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};
use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const KEY: &str = "create_copycat";

/// A copycat's block entity (`CopycatBlockEntity`): its material (None: the copycat base) and
/// the item it consumed for it (`consumedItem`, one, kept with its components).
#[derive(Clone, Default)]
struct Copycat {
    material: Option<BlockStateId>,
    item: Option<ItemStack>,
}

#[derive(Default)]
struct Dim {
    copycats: HashMap<[i32; 3], Copycat>,
    /// Copycats clients must see again.
    shown: Vec<[i32; 3]>,
}

thread_local! {
    static COPYCATS: RefCell<Option<Box<[Dim; 3]>>> = const { RefCell::new(None) };
    /// Copycats placed, changed or broken: (dimension, position, old state, new state).
    static CHANGES: RefCell<Vec<(usize, [i32; 3], u32, u32)>> = const { RefCell::new(Vec::new()) };
    /// Whether the level's tick has anything to do: the miss is this one flag.
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// The last click with a copycat's item on a block: game time, face, the hit's height.
    static CLICK: Cell<Option<(i64, [i32; 3], f64)>> = const { Cell::new(None) };
}

/// A click with a copycat's item on a block, before it places (`BlockPlaceContext`): its face
/// and where it hit, which the placement reads. The miss is a name test.
pub(super) fn use_item_on(game: &Game<'_>, stack: &ItemStack, pos: [i32; 3], face: [i32; 3]) {
    if !matches!(&*stack.item, "create:copycat_panel" | "create:copycat_step") {
        return;
    }
    // The face's plane when nothing is picked.
    let y = game
        .pick()
        .map_or(f64::from(pos[1] + face[1].max(0)), |h| h.point[1]);
    CLICK.set(Some((game.world.game_time, face, y)));
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Dim) -> R) -> R {
    COPYCATS.with_borrow_mut(|d| f(&mut d.get_or_insert_with(Default::default)[dim]))
}

/// The world closed: everything is forgotten.
pub(super) fn forget() {
    COPYCATS.with_borrow_mut(|d| *d = None);
    CHANGES.with_borrow_mut(Vec::clear);
    BUSY.set(false);
}

fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

fn is_copycat(b: Option<Block>) -> bool {
    matches!(b, Some(Block::CopycatPanel | Block::CopycatStep))
}

/// A copycat's shape from its block state.
fn shape_of(registry: &BlockRegistry, state: BlockStateId) -> Option<Shape> {
    let s = registry.state(state)?;
    let facing = s.properties.get("facing").and_then(|f| cc::dir_named(f));
    match s.name.as_str() {
        "create:copycat_panel" => Some(Shape::Panel(facing.unwrap_or(Dir::Up))),
        "create:copycat_step" => Some(Shape::Step {
            facing: facing.unwrap_or(Dir::South),
            top: s.properties.get("half").map(String::as_str) == Some("top"),
        }),
        _ => None,
    }
}

fn center(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

fn mark_unsaved(game: &mut Game<'_>, pos: [i32; 3]) {
    let dimension = game.dim.id().to_owned();
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
}

fn shown(dim: usize, pos: [i32; 3]) {
    with(dim, |d| d.shown.push(pos));
    BUSY.set(true);
}

/// A committed change on the server: a copycat placed, changed or removed (one table lookup per
/// Create block changed).
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    if is_copycat(block_of(old)) || is_copycat(block_of(new)) {
        CHANGES.with_borrow_mut(|c| c.push((dim, pos, old, new)));
        BUSY.set(true);
    }
}

// ---------------------------------------------------------------------------------------------
// Materials

/// `getAcceptedBlockState` for the item `stack` on a copycat of `shape`: the item's block,
/// turned to the clicked `face` when there is one; None when the copycat doesn't take it.
fn accepted(
    game: &Game<'_>,
    shape: Shape,
    stack: &ItemStack,
    face: Option<Dir>,
) -> Option<(BlockStateId, Special)> {
    let state = body::item_block(game, &stack.item)?;
    let c = body::candidate(game, state);
    if !cc::accepts(shape, &c) {
        return None;
    }
    let mut state = state;
    if let Some(face) = face {
        let registry = game.pack.assets.registry;
        for (k, v) in cc::oriented(face) {
            if let Some(next) = with_prop(registry, state, k, v) {
                state = next;
            }
        }
    }
    Some((state, Special::of(&c)))
}

/// `state` with `key` set to `value`, when it has that property and allows the value.
fn with_prop(
    registry: &BlockRegistry,
    state: BlockStateId,
    key: &str,
    value: &str,
) -> Option<BlockStateId> {
    let mut s = registry.state(state)?.clone();
    if !s.properties.contains_key(key) {
        return None;
    }
    s.properties.insert(key.to_owned(), value.to_owned());
    registry.id(&s)
}

/// What `cycleMaterial` reads of a material.
fn cycle_of(registry: &BlockRegistry, state: BlockStateId) -> Option<(&'static str, &'static str)> {
    let s = registry.state(state)?;
    let prop = |k: &str| s.properties.get(k).map(String::as_str);
    let flag = |k: &str| prop(k).map(|v| v == "true");
    let allows = |k: &str, v: &str| with_prop(registry, state, k, v).is_some();
    let m = cc::Material {
        facing: prop("facing")
            .and_then(cc::dir_named)
            .map(|f| (f, allows("facing", "up"))),
        axis: prop("axis")
            .and_then(|a| match a {
                "x" => Some(Axis::X),
                "y" => Some(Axis::Y),
                "z" => Some(Axis::Z),
                _ => None,
            })
            .map(|a| (a, allows("axis", "y"))),
        half_top: prop("half")
            .filter(|h| matches!(*h, "top" | "bottom"))
            .map(|h| h == "top"),
        open: flag("open"),
        lit: flag("lit"),
        powering: flag("powering").filter(|_| s.name == "create:rose_quartz_lamp"),
    };
    cc::cycle(&m)
}

/// `CopycatBlockEntity.setMaterial`: a new block takes the orientation of a neighbour copycat
/// of the same state already showing that block; then clients are told.
fn set_material(game: &Game<'_>, dim: usize, pos: [i32; 3], material: BlockStateId) {
    let registry = game.pack.assets.registry;
    let own = game.world.block(pos[0], pos[1], pos[2]);
    let name = |s: BlockStateId| registry.state(s).map(|s| s.name.clone());
    let mut material = material;
    let current = with(dim, |d| d.copycats.get(&pos).and_then(|c| c.material));
    if current.and_then(name) != name(material) {
        for side in Dir::ALL {
            let o = side.offset();
            let n = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            if game.world.block(n[0], n[1], n[2]) != own {
                continue;
            }
            let other = with(dim, |d| d.copycats.get(&n).and_then(|c| c.material));
            if let Some(other) = other
                && name(other) == name(material)
            {
                material = other;
                break;
            }
        }
    }
    with(dim, |d| {
        d.copycats.entry(pos).or_default().material = Some(material)
    });
    shown(dim, pos);
}

/// The stack in the hand that clicks.
fn hand_stack<'a>(game: &'a mut Game<'_>) -> Option<&'a mut Option<ItemStack>> {
    if game.hand == horizon_client::first_person::Hand::Off {
        Some(&mut game.offhand)
    } else {
        let i = game.selected;
        game.inventory.get_mut(i)
    }
}

/// `placeItemBackInInventory`: onto a stack of the same item, else an empty slot, else
/// dropped at the player's block.
fn give_back(game: &mut Game<'_>, pos: [i32; 3], stack: ItemStack) {
    let limit = super::create_contraption_body::slot_limit(game, &stack);
    let slot = game
        .inventory
        .iter()
        .position(|s| {
            s.as_ref().is_some_and(|s| {
                super::create_contraption_body::same_stack(s, &stack) && s.count < limit
            })
        })
        .or_else(|| game.inventory.iter().position(Option::is_none));
    match slot {
        Some(i) => match &mut game.inventory[i] {
            Some(s) => s.count += stack.count,
            empty => *empty = Some(stack),
        },
        None => super::create_contraption_body::drop_stack(game, center(pos), [0.0; 3], stack),
    }
}

// ---------------------------------------------------------------------------------------------
// Clicks

/// A right click on a copycat with an item (`CopycatBlock.useItemOn`): a block it takes
/// becomes its material (one consumed, none in creative), the same block again turns it
/// (`cycleMaterial`); anything else passes to the item's own use. Sneaking with an item skips
/// the block, as vanilla's. True when the click was a copycat's.
pub(super) fn use_block(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if !is_copycat(block_of(state.0)) {
        return false;
    }
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let Some(stack) = hand_stack(game).and_then(|s| s.clone()) else {
        return true;
    };
    if sneaking {
        return true;
    }
    let registry = game.pack.assets.registry;
    let Some(shape) = shape_of(registry, state) else {
        return true;
    };
    let clicked = Dir::from_offset(face);
    let Some((mut material, special)) = accepted(game, shape, &stack, clicked) else {
        return true;
    };
    // `CopycatPanelBlock.prepareMaterial`: a trapdoor lies or stands as the panel does.
    if let (Shape::Panel(facing), Special::Trapdoor) = (shape, special) {
        let player = create_engine::place::horizontal_direction(game.yaw);
        let near_top = game
            .pick()
            .is_some_and(|h| h.point[1] - 0.5 > f64::from(pos[1]));
        for (k, v) in cc::trapdoor_material(facing, player, near_top) {
            if let Some(next) = with_prop(registry, material, k, v) {
                material = next;
            }
        }
    }
    let current = with(dim, |d| d.copycats.get(&pos).and_then(|c| c.material));
    let same = current.is_some_and(|c| {
        registry.state(c).map(|s| &s.name) == registry.state(material).map(|s| &s.name)
    });
    let turned = current.and_then(|c| {
        let (k, v) = cycle_of(registry, c)?;
        with_prop(registry, c, k, v)
    });
    match cc::use_with(true, same, current.is_some(), turned.is_some()) {
        Use::Pass => {}
        Use::Cycle => {
            *canceled = true;
            if let Some(t) = turned {
                set_material(game, dim, pos, t);
                mark_unsaved(game, pos);
            }
            game.audio.play(
                "minecraft:entity.item_frame.add_item",
                Some(center(pos)),
                0.75,
                0.95,
            );
        }
        Use::Apply => {
            *canceled = true;
            set_material(game, dim, pos, material);
            let mut one = stack.clone();
            one.count = 1;
            with(dim, |d| d.copycats.entry(pos).or_default().item = Some(one));
            mark_unsaved(game, pos);
            if let Some(sound) = body::place_sound(game, material) {
                game.audio.play(&sound, Some(center(pos)), 1.0, 0.75);
            }
            if !game.creative
                && let Some(slot) = hand_stack(game)
            {
                match slot {
                    Some(s) if s.count > 1 => s.count -= 1,
                    _ => *slot = None,
                }
            }
        }
    }
    true
}

/// `CopycatBlock.onWrenched` (also before a sneaking wrench picks it up): the material comes
/// off, its item back to the player (not in creative), with the copycat's break particles.
/// False when it holds none (`PASS`).
pub(super) fn wrench(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let Some(dim) = dimension_index(game.dim.id()) else {
        return false;
    };
    let Some(had) = with(dim, |d| {
        let c = d.copycats.get_mut(&pos)?;
        c.material?;
        Some(std::mem::take(c))
    }) else {
        return false;
    };
    if !game.creative
        && let Some(item) = had.item
    {
        give_back(game, pos, item);
    }
    // `LevelEvent.PARTICLES_DESTROY_BLOCK` of the copycat's own state.
    let own = game.world.block(pos[0], pos[1], pos[2]);
    body::break_particles(game, pos, own);
    shown(dim, pos);
    mark_unsaved(game, pos);
    true
}

/// `getStateForPlacement` and `setPlacedBy`: a placed panel faces away from the player's view,
/// a step their way (upper half under a block or high on a side); a block in the off hand
/// becomes its material, turned to the side nearest the player's view.
pub(super) fn place_block(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    let block = block_of(state.0);
    if !is_copycat(block) {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let nearest = create_engine::place::ordered_by_nearest(game.yaw, game.pitch)[0];
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    // The click that placed it (`use_item_on`, this tick); `getClickLocation().y -
    // getClickedPos().getY()` is the hit's height over the placed position.
    let (clicked, hit_y) =
        CLICK
            .get()
            .filter(|c| c.0 == game.world.game_time)
            .map_or((Dir::Up, 0.0), |c| {
                (
                    Dir::from_offset(c.1).unwrap_or(Dir::Up),
                    c.2 - f64::from(pos[1]),
                )
            });
    let shape = cc::placed(
        block == Some(Block::CopycatPanel),
        nearest,
        horizontal,
        clicked,
        hit_y,
    );
    let registry = game.pack.assets.registry;
    let mut placed = state;
    let props: Vec<(&str, &str)> = match shape {
        Shape::Panel(f) => vec![("facing", cc::dir_name(f))],
        Shape::Step { facing, top } => vec![
            ("facing", cc::dir_name(facing)),
            ("half", if top { "top" } else { "bottom" }),
        ],
    };
    for (k, v) in props {
        if let Some(next) = with_prop(registry, placed, k, v) {
            placed = next;
        }
    }
    if placed != state {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(
            &dimension,
            pos,
            placed,
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
    // `setPlacedBy`: the off hand's block.
    let Some(offhand) = game.offhand.clone() else {
        return;
    };
    let Some((material, _)) = accepted(game, shape, &offhand, Some(nearest)) else {
        return;
    };
    if with(dim, |d| {
        d.copycats.get(&pos).is_some_and(|c| c.material.is_some())
    }) {
        return;
    }
    set_material(game, dim, pos, material);
    let mut one = offhand;
    one.count = 1;
    with(dim, |d| d.copycats.entry(pos).or_default().item = Some(one));
    mark_unsaved(game, pos);
    if !game.creative {
        match &mut game.offhand {
            Some(s) if s.count > 1 => s.count -= 1,
            slot => *slot = None,
        }
    }
}

/// `playerWillDestroy`: a creative player breaking a copycat keeps its item from dropping.
pub(super) fn destroy_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: bool,
) {
    if canceled || !game.creative || !is_copycat(block_of(state.0)) {
        return;
    }
    if let Some(dim) = dimension_index(game.dim.id()) {
        with(dim, |d| {
            if let Some(c) = d.copycats.get_mut(&pos) {
                c.item = None;
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// The level's tick

/// Copycats placed (an empty one, unless the placement gave it a material), changed (a new
/// shape) or broken (`onRemove`: its item drops); then what changed goes to the level's
/// players. Nothing runs without a change.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    if !BUSY.get() {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let changes: Vec<([i32; 3], u32, u32)> = CHANGES.with_borrow_mut(|c| {
        let mine = c
            .iter()
            .filter(|x| x.0 == dim)
            .map(|x| (x.1, x.2, x.3))
            .collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (pos, old, new) in changes {
        let (was, is) = (block_of(old), block_of(new));
        if is_copycat(was) && was != is {
            // `onRemove` with another block: the consumed item drops.
            if let Some(item) = with(dim, |d| d.copycats.remove(&pos)).and_then(|c| c.item) {
                super::create_contraption_body::drop_stack(game, center(pos), [0.0; 3], item);
                mark_unsaved(game, pos);
            }
        }
        if is_copycat(is) {
            with(dim, |d| {
                d.copycats.entry(pos).or_default();
            });
        }
        with(dim, |d| d.shown.push(pos));
    }
    let list = with(dim, |d| std::mem::take(&mut d.shown));
    if !list.is_empty() {
        let players: Vec<u128> = game
            .native_server_players()
            .into_iter()
            .filter(|p| p.dimension == dimension)
            .map(|p| p.uuid)
            .collect();
        send(game, &players, dim, &list);
    }
    let pending = CHANGES.with_borrow(|c| !c.is_empty())
        || COPYCATS.with_borrow(|d| {
            d.as_ref()
                .is_some_and(|d| d.iter().any(|d| !d.shown.is_empty()))
        });
    BUSY.set(pending);
}

// ---------------------------------------------------------------------------------------------
// Clients

/// What each copycat at `list` shows (nothing when it is gone), to `players`.
fn send(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
    let registry = game.pack.assets.registry;
    let mut out = Vec::new();
    for &pos in list {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let record = with(dim, |d| d.copycats.get(&pos).cloned());
        let shape = record.as_ref().and_then(|_| shape_of(registry, state));
        let material = record.and_then(|c| c.material);
        let special = material.map_or(Special::None, |m| Special::of(&body::candidate(game, m)));
        out.push(Shown {
            pos,
            shape,
            special,
            material: material
                .map(|m| state_text(registry, m.0))
                .unwrap_or_default(),
        });
    }
    for chunk in out.chunks(64) {
        let m = cc::encode(chunk);
        for &p in players {
            send_to_player(game, p, cc::CHANNEL, &m);
        }
    }
}

/// A client asks for what its dimension shows (`sync`, when it joins).
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != create_engine::client::CHANNEL || data != b"sync" {
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
    let list: Vec<[i32; 3]> = with(dim, |d| d.copycats.keys().copied().collect());
    if !list.is_empty() {
        send(game, &[player], dim, &list);
    }
}

/// On a client: each copycat drawn as its material cut to its box (`CopycatModel`), the
/// copycat base when it holds none, a panel's trapdoor whole and its bars as the copycat bars'
/// model; a copycat gone draws its own model again.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != cc::CHANNEL {
        return;
    }
    let registry = game.pack.assets.registry;
    for s in cc::decode(data) {
        let material = if s.material.is_empty() {
            registry.default_state(cc::BASE)
        } else {
            state_of_text(registry, &s.material).map(BlockStateId)
        };
        let model = match (s.drawn(), material) {
            (Some(drawn), Some(material)) => {
                let mut model = horizon_client::mesher::ModelOverride::new(material);
                match drawn {
                    cc::Drawn::Whole => {}
                    cc::Drawn::Bars(f) => {
                        match state_with(
                            registry,
                            "create:copycat_bars",
                            &[("facing", cc::dir_name(f))],
                        ) {
                            Some(bars) => model.state = bars,
                            None => model.boxes = cc::Shape::Panel(f).boxes(),
                        }
                    }
                    cc::Drawn::Cut(boxes) => model.boxes = boxes,
                }
                Some(model)
            }
            _ => None,
        };
        game.native_block_model_override(s.pos, model);
    }
}

// ---------------------------------------------------------------------------------------------
// Saved with the column

/// A column's copycats (`CopycatBlockEntity.write`: `Material` and `Item`), as `create_copycat`:
/// `Pos` (three ints each), `Materials` (state text, empty for none) and `Items` (one compound
/// each, empty for none).
pub(super) fn chunk_data_save(
    game: &Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if COPYCATS.with_borrow(|d| d.is_none()) {
        return;
    }
    let registry = game.pack.assets.registry;
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let mut list: Vec<([i32; 3], Copycat)> = with(dim, |d| {
        d.copycats
            .iter()
            .filter(|(p, _)| inside(p))
            .map(|(p, c)| (*p, c.clone()))
            .collect()
    });
    if list.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    list.sort_by_key(|x| x.0);
    let saved: Vec<cc::Saved> = list
        .iter()
        .map(|(p, c)| cc::Saved {
            pos: *p,
            material: c
                .material
                .map(|m| state_text(registry, m.0))
                .unwrap_or_default(),
        })
        .collect();
    let (pos, materials) = cc::save(&saved);
    let items = list
        .iter()
        .map(|(_, c)| match &c.item {
            Some(s) => super::create_package_body::to_nbt(s),
            None => Tag::Compound(Compound::new()),
        })
        .collect();
    data.insert(
        KEY.into(),
        Tag::compound([
            ("Pos", Tag::IntArray(pos)),
            (
                "Materials",
                Tag::List(
                    materials
                        .iter()
                        .map(|m| Tag::String(m.as_str().into()))
                        .collect(),
                ),
            ),
            ("Items", Tag::List(items)),
        ]),
    );
}

/// A column joined a level: its copycats back (`read`, a server's: a material stays only while
/// its saved item still gives it), each shown to the level's clients.
pub(super) fn chunk_load(game: &Game<'_>, dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let pos = match data.get(&"Pos".into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => return,
    };
    let list = |key: &str| match data.get(&key.into()) {
        Some(Tag::List(l)) => l.to_vec(),
        _ => Vec::new(),
    };
    let materials: Vec<String> = list("Materials")
        .iter()
        .map(|t| match t {
            Tag::String(s) => s.to_string_lossy(),
            _ => String::new(),
        })
        .collect();
    let items = list("Items");
    let registry = game.pack.assets.registry;
    for (i, saved) in cc::load(&pos, &materials).into_iter().enumerate() {
        let item = items
            .get(i)
            .and_then(super::create_package_body::from_nbt)
            .filter(|s| s.count > 0);
        let state = game.world.block(saved.pos[0], saved.pos[1], saved.pos[2]);
        // No copycat there any more: nothing to keep.
        let Some(shape) = shape_of(registry, state) else {
            continue;
        };
        let material = state_of_text(registry, &saved.material).map(BlockStateId);
        let keep = match (material, &item) {
            (Some(m), Some(it)) => {
                let given = accepted(game, shape, it, None);
                let name = |s: BlockStateId| registry.state(s).map(|s| s.name.as_str());
                cc::keeps_material(
                    given.and_then(|g| name(g.0)),
                    given.is_some(),
                    name(m).unwrap_or(""),
                )
            }
            _ => false,
        };
        let c = if keep {
            Copycat { material, item }
        } else {
            Copycat::default()
        };
        with(dim, |d| {
            d.copycats.insert(saved.pos, c);
            d.shown.push(saved.pos);
        });
        BUSY.set(true);
    }
}

/// A column left a level: its copycats are forgotten (kept in its save).
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if COPYCATS.with_borrow(|d| d.is_none()) {
        return;
    }
    let outside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (chunk.x, chunk.z);
    with(dim, |d| {
        d.copycats.retain(|p, _| outside(p));
        d.shown.retain(outside);
    });
}

/// `create_debug copycat <x> <y> <z>`: what a copycat holds, for a headless proof.
pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    commands.push(super::native_server::NativeCommand::op(
        "create_debug copycat <x:integer> <y:integer> <z:integer>",
        2,
    ));
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str) -> bool {
    let Some(rest) = command.strip_prefix("create_debug copycat") else {
        return false;
    };
    let p: Vec<i32> = rest
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    let line = match (
        <[i32; 3]>::try_from(p.as_slice()),
        dimension_index(game.dim.id()),
    ) {
        (Ok(pos), Some(dim)) => {
            let registry = game.pack.assets.registry;
            match with(dim, |d| d.copycats.get(&pos).cloned()) {
                Some(c) => format!(
                    "copycat at {} {} {}: {} ({})",
                    pos[0],
                    pos[1],
                    pos[2],
                    c.material
                        .map(|m| state_text(registry, m.0))
                        .unwrap_or_else(|| cc::BASE.to_owned()),
                    c.item
                        .map_or_else(|| "no item".to_owned(), |s| s.item.to_string())
                ),
                None => format!("no copycat at {} {} {}", pos[0], pos[1], pos[2]),
            }
        }
        _ => "usage: create_debug copycat <x> <y> <z>".to_owned(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}
