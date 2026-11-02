// Create's minecart contraptions in the game, injected as `crate::game::create_carts`: the cart
// assembler (`CartAssemblerBlock`, `CartAssemblerBlockEntity`, `CartAssemblerBlockItem`), the
// contraptions carts carry (`MountedContraption`, `OrientedContraptionEntity`), their items
// (`MinecartContraptionItem`) and couplings (`MinecartCouplingItem`, `CouplingHandler`,
// `CouplingPhysics`, `MinecartController`). The rules are `create_engine::cart`'s, `mounted`'s and
// `coupling`'s. A contraption is typed data keyed by its cart's UUID (Horizon API 62), followed
// each tick through `native_minecart`, drawn by clients as one part model gliding and turning on
// the GPU (sent again only when the cart leaves the prediction), and kept in the world's saved
// data (`create/minecart_contraptions`) with the couplings. Nothing runs in a level tick while no
// cart carries a contraption, no carts are coupled and no assembler has a cart to handle.
use super::Game;
use super::create::*;
use super::create_contraptions::{contraption_found, turned_state};
use super::create_logistics::{depot_key, players_in};
use super::native_minecarts::NativeMinecart;
use create_engine::cart::{self, Assembler, RailType};
use create_engine::coupling::{self, Body, Couplings};
use create_engine::kinetics::{Axis, Dir};
use create_engine::mounted::{self, Mode, Mounted, Pose, RailShape, Sent};
use create_engine::world::dimension_index;
use create_engine::{Block, states};
use horizon_world::nbt::{Compound, JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

const DIMENSIONS: [&str; 3] = [
    "minecraft:overworld",
    "minecraft:the_nether",
    "minecraft:the_end",
];

/// The world's saved data holding the contraptions, couplings and assembler modes.
const SAVED: (&str, &str) = ("create", "minecart_contraptions");

/// A cart this module follows: its UUID, where it is and where it was the tick before.
#[derive(Clone, Copy, Debug)]
struct Known {
    uuid: u128,
    pos: [f64; 3],
    prev: [f64; 3],
}

/// A contraption on a cart (its `OrientedContraptionEntity`), by the cart's UUID: its level,
/// what clients last heard and whether they must learn its blocks.
#[derive(Clone, Debug)]
struct OnCart {
    dim: usize,
    m: Mounted,
    sent: Option<Sent>,
    fresh: bool,
}

/// A cart assembler's block entity: its cooldown, its mode (`movementMode`) and the cart that
/// passed it since its last tick (`cartToAssemble`).
#[derive(Clone, Copy, Debug, Default)]
struct Station {
    cooldown: Assembler,
    mode: Mode,
    cart: Option<u32>,
}

#[derive(Default)]
struct Carts {
    /// Carts followed, by level and id; ids by UUID.
    known: HashMap<(usize, u32), Known>,
    ids: HashMap<u128, (usize, u32)>,
    mounted: HashMap<u128, OnCart>,
    couplings: Couplings,
    stations: HashMap<(usize, [i32; 3]), Station>,
    /// Assemblers with a cart to handle or cooling down.
    busy: Vec<(usize, [i32; 3])>,
    /// A player's first click with a coupling (`CouplingHandlerClient.selectedCart`).
    selected: HashMap<u128, (usize, u32)>,
    /// Carts whose contraption clients must forget (their column unloaded).
    clear: Vec<(usize, u32)>,
    /// Look for carts not found yet (after a load) at this level tick.
    scan: bool,
    dirty: bool,
}

impl Carts {
    /// The one test a level tick makes while nothing is on a cart.
    fn idle(&self) -> bool {
        self.mounted.is_empty()
            && self.couplings.is_empty()
            && self.busy.is_empty()
            && self.clear.is_empty()
    }
}

thread_local! {
    static CARTS: RefCell<Carts> = RefCell::new(Carts::default());
    /// Rail facts by block state: its shape, and whether it is an activator rail and powered.
    static RAILS: RefCell<HashMap<u32, Option<(RailShape, bool, bool)>>> = RefCell::new(HashMap::new());
    /// Contraptions a client draws, by cart id: their part model.
    static CLIENT: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
}

/// The world closed.
pub(super) fn forget() {
    CARTS.with_borrow_mut(|c| *c = Carts::default());
    RAILS.with_borrow_mut(HashMap::clear);
}

fn assembler(state: u32) -> Option<create_engine::parts::StateInfo> {
    states::index(state)?;
    with_world(|w| {
        let (b, info) = w.table.get(state)?;
        (b == Block::CartAssembler).then(|| info.clone())
    })
    .flatten()
}

/// A rail block's shape, and whether it is an activator rail and powered (`BlockTags.RAILS`:
/// vanilla's rails and Create's rail blocks), read once per state.
fn rail_at(game: &Game<'_>, p: [i32; 3]) -> Option<(RailShape, bool, bool)> {
    let state = game.world.block(p[0], p[1], p[2]).0;
    if let Some(known) = RAILS.with_borrow(|r| r.get(&state).copied()) {
        return known;
    }
    let facts = game
        .pack
        .assets
        .registry
        .state(BlockStateId(state))
        .and_then(|s| {
            let rail = matches!(
                s.name.as_str(),
                "minecraft:rail"
                    | "minecraft:powered_rail"
                    | "minecraft:detector_rail"
                    | "minecraft:activator_rail"
                    | "create:controller_rail"
                    | "create:cart_assembler"
            );
            if !rail {
                return None;
            }
            let shape = s.properties.get("shape").and_then(|v| RailShape::read(v))?;
            let powered = s.properties.get("powered").is_some_and(|v| v == "true");
            Some((shape, s.name == "minecraft:activator_rail", powered))
        });
    RAILS.with_borrow_mut(|r| r.insert(state, facts));
    facts
}

/// `AbstractMinecart.getCurrentRailPosition`: the cart's block, or the one below when that is a
/// rail.
fn rail_under(game: &Game<'_>, pos: [f64; 3]) -> Option<(RailShape, bool, bool)> {
    let p = pos.map(|v| v.floor() as i32);
    rail_at(game, [p[0], p[1] - 1, p[2]]).or_else(|| rail_at(game, p))
}

fn status(game: &mut Game<'_>, text: &str) {
    if let Some(p) = game.native_acting_player() {
        game.native_action_bar(p.uuid, horizon_client::text::Text::literal(text));
    }
}

/// The stack in the hand the click uses shrinks by one (not in creative).
fn use_one(game: &mut Game<'_>) {
    if game.creative {
        return;
    }
    let slot = if game.hand == horizon_client::first_person::Hand::Off {
        &mut game.offhand
    } else {
        match game.inventory.get_mut(game.selected) {
            Some(s) => s,
            None => return,
        }
    };
    match slot {
        Some(s) if s.count > 1 => s.count -= 1,
        _ => *slot = None,
    }
}

fn drop_item(game: &mut Game<'_>, dimension: &str, at: [f64; 3], item: &str) {
    game.native_run_command(&format!(
        "execute in {dimension} run summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:1}}}}",
        at[0], at[1], at[2]
    ));
}

// ---------------------------------------------------------------------------------------------
// The assembler block

/// `onMinecartPass`: a cart Create may load (`canAssembleTo`) on an assembler is handled at its
/// next tick (`assembleNextTick`: the first cart to pass since then). Every other rail misses on
/// the state test.
pub(super) fn minecart_pass(
    dimension: &str,
    pos: [i32; 3],
    state: BlockStateId,
    cart: &NativeMinecart,
) {
    if assembler(state.0).is_none() || !cart::can_assemble_to(cart.kind) {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    CARTS.with_borrow_mut(|c| {
        let s = c.stations.entry((dim, pos)).or_default();
        if s.cart.is_none() {
            s.cart = Some(cart.id);
        }
        if !c.busy.contains(&(dim, pos)) {
            c.busy.push((dim, pos));
        }
    });
}

/// `CartAssemblerBlockEntity.tick` for assemblers with a cart or cooling down: at most one cart
/// every 8 ticks (`assemblyCooldown`), assembled onto or taken off by the rail type and power
/// (`getActionForCart`), then sent on, braked or left as the action says.
fn tick_stations(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let busy: Vec<[i32; 3]> = CARTS.with_borrow(|c| {
        c.busy
            .iter()
            .filter(|(d, _)| *d == dim)
            .map(|(_, p)| *p)
            .collect()
    });
    for pos in busy {
        let handle = CARTS.with_borrow_mut(|c| {
            let s = c.stations.entry((dim, pos)).or_default();
            let cart = s.cart.take();
            let act = s.cooldown.tick(cart.is_some());
            if s.cooldown.since >= cart::COOLDOWN && !act {
                c.busy.retain(|b| *b != (dim, pos));
                if s.mode == Mode::Rotate {
                    c.stations.remove(&(dim, pos));
                }
            }
            act.then_some(cart).flatten()
        });
        let Some(id) = handle else { continue };
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some(info) = assembler(state) else {
            continue;
        };
        let Some(nc) = game.native_minecart(dimension, id) else {
            continue;
        };
        let carrying = CARTS.with_borrow(|c| c.mounted.contains_key(&nc.uuid));
        let rail = info.rail_type.unwrap_or(RailType::PoweredRail);
        let action = cart::action(rail, info.powered, nc.ridden || carrying);
        let axis = if info.rail == Some(create_engine::rail::Shape::NorthSouth) {
            Axis::Z
        } else {
            Axis::X
        };
        if action.assembles() {
            assemble(game, dimension, dim, pos, axis, info.backwards, &nc);
        }
        if action.disassembles() {
            disassemble_at(game, dimension, pos, axis, info.backwards, &nc);
        }
        // `getMotionDirection`: the way the cart moves along the rail.
        let v = nc.velocity;
        let facing = if axis == Axis::X {
            if v[0] < 0.0 { Dir::West } else { Dir::East }
        } else if v[2] < 0.0 {
            Dir::North
        } else {
            Dir::South
        };
        let conductor = |d: Dir| {
            let o = d.offset();
            let s = game
                .world
                .block(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
            game.res.behaviours().facts(s).flags
                & horizon_world::level::data::state_flags::CONDUCTOR
                != 0
        };
        let centre = [
            f64::from(pos[0]) + 0.5 - nc.pos[0],
            f64::from(pos[1]) + 0.5 - nc.pos[1],
            f64::from(pos[2]) + 0.5 - nc.pos[2],
        ];
        if let Some(motion) =
            cart::cart_motion(action, axis, info.backwards, v, facing, conductor, centre)
        {
            game.native_set_delta_movement(dimension, id, motion);
        }
    }
}

/// A block as a cart's structure search reads it: as any contraption's, but another cart
/// assembler (`testSecondaryCartAssembler`: carriage contraptions are not ported) and a block
/// with a block entity (no storage rides on a cart here) can't move.
fn found(game: &Game<'_>, p: [i32; 3]) -> Option<create_engine::contraption::Found> {
    let mut f = contraption_found(game, p)?;
    if assembler(f.state).is_some() || game.world.block_entities.contains_key(&(p[0], p[1], p[2])) {
        f.movable = false;
    }
    Some(f)
}

/// `CartAssemblerBlockEntity.assemble`: the structure above the assembler (`MountedContraption`)
/// leaves the world and rides the cart, facing the assembler's way (`getHorizontalDirection`).
fn assemble(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    axis: Axis,
    backwards: bool,
    nc: &NativeMinecart,
) {
    if nc.ridden || CARTS.with_borrow(|c| c.mounted.contains_key(&nc.uuid)) {
        return;
    }
    let registry = game.pack.assets.registry;
    let axis_name = if axis == Axis::Z { "z" } else { "x" };
    let Some(anchor) = state_with(registry, "create:minecart_anchor", &[("axis", axis_name)])
    else {
        return;
    };
    let Ok(Some(blocks)) = cart::gather(pos, anchor.0, |p| found(game, p)) else {
        return;
    };
    for (o, _) in blocks.iter().filter(|(o, _)| *o != [0, 0, 0]) {
        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        game.native_set_block(
            dimension,
            p,
            BlockStateId(0),
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
    let mode = CARTS.with_borrow(|c| {
        c.stations
            .get(&(dim, pos))
            .map(|s| s.mode)
            .unwrap_or_default()
    });
    let initial = cart::horizontal_direction(axis, backwards);
    let m = Mounted::new(blocks, mode, initial);
    CARTS.with_borrow_mut(|c| {
        c.ids.insert(nc.uuid, (dim, nc.id));
        c.known.insert(
            (dim, nc.id),
            Known {
                uuid: nc.uuid,
                pos: nc.pos,
                prev: nc.pos,
            },
        );
        c.mounted.insert(
            nc.uuid,
            OnCart {
                dim,
                m,
                sent: None,
                fresh: true,
            },
        );
        c.dirty = true;
    });
}

/// `CartAssemblerBlockEntity.disassemble`: the contraption, turned to the assembler's way, goes
/// back into the world.
fn disassemble_at(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    axis: Axis,
    backwards: bool,
    nc: &NativeMinecart,
) {
    let Some(mut on) = CARTS.with_borrow_mut(|c| c.mounted.remove(&nc.uuid)) else {
        return;
    };
    let _ = pos;
    on.m.turn.yaw = mounted::y_rot(cart::horizontal_direction(axis, backwards));
    put_back(game, dimension, nc.pos, &on.m);
    gone(game, dimension, nc.id);
}

/// `AbstractContraptionEntity.disassemble`: each block where the contraption's turn (to the
/// nearest quarter) puts it, above the block the cart is in; a block whose place is taken drops.
fn put_back(game: &mut Game<'_>, dimension: &str, at: [f64; 3], m: &Mounted) {
    let origin = [
        at[0].floor() as i32,
        (at[1] + 0.5).floor() as i32,
        at[2].floor() as i32,
    ];
    let (quarter, blocks) = mounted::placed_back(&m.blocks, mounted::y_rot(m.initial), m.turn.yaw);
    let registry = game.pack.assets.registry;
    for (o, s) in blocks {
        let p = [origin[0] + o[0], origin[1] + o[1], origin[2] + o[2]];
        let id = turned_state(game, s, quarter, Axis::Y);
        let here = game.world.block(p[0], p[1], p[2]);
        let free = registry
            .state(here)
            .is_some_and(|s| s.name == "minecraft:air" || s.name == "minecraft:cave_air");
        if free {
            game.native_set_block(
                dimension,
                p,
                BlockStateId(id),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(id)) {
            let name = s.name.clone();
            drop_item(game, dimension, p.map(|v| f64::from(v) + 0.5), &name);
        }
    }
    CARTS.with_borrow_mut(|c| c.dirty = true);
}

/// Clients forget cart `id`'s contraption.
fn gone(game: &mut Game<'_>, dimension: &str, id: u32) {
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, mounted::CHANNEL, &id.to_le_bytes());
    }
}

/// `CartAssemblerBlockItem.tryPlaceAssembler`, the assembler's `useItemOn` with another rail
/// item (`rail_type`), and the wrench's `onWrenched` / `onSneakWrenched`. Whether the click was
/// this module's.
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    claimed: &mut bool,
) -> bool {
    let item = &*stack.item;
    if item == "create:cart_assembler" {
        *claimed = true;
        place_assembler(game, pos);
        return true;
    }
    if cart::item_cart(item).is_some() {
        *claimed = true;
        place_cart(game, stack, pos);
        return true;
    }
    if item == "create:wrench" {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        if let Some(info) = assembler(state.0) {
            *claimed = true;
            wrench_assembler(game, pos, state, &info);
            return true;
        }
    }
    false
}

fn place_assembler(game: &mut Game<'_>, pos: [i32; 3]) {
    let registry = game.pack.assets.registry;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(rail) = registry.state(state).cloned() else {
        return;
    };
    let shape = rail.properties.get("shape").cloned().unwrap_or_default();
    let backwards = (rail.name == "create:controller_rail").then(|| {
        rail.properties
            .get("backwards")
            .is_some_and(|v| v == "true")
    });
    let facing = create_engine::place::horizontal_direction(game.yaw);
    let Some((kind, east_west, backwards)) = cart::placed_on(&rail.name, &shape, backwards, facing)
    else {
        if RailType::from_item(&rail.name).is_none() {
            status(game, "Place your Cart Assembler on a rail block");
        }
        return;
    };
    let Some(id) = state_with(
        registry,
        "create:cart_assembler",
        &[
            (
                "shape",
                if east_west {
                    "east_west"
                } else {
                    "north_south"
                },
            ),
            ("rail_type", kind.name()),
            ("backwards", if backwards { "true" } else { "false" }),
        ],
    ) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    game.audio.play(
        "minecraft:block.stone.place",
        Some(pos.map(|v| f64::from(v) + 0.5)),
        1.0,
        1.0,
    );
    use_one(game);
}

fn wrench_assembler(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    info: &create_engine::parts::StateInfo,
) {
    let registry = game.pack.assets.registry;
    let dimension = game.dim.id().to_owned();
    let east_west = info.rail != Some(create_engine::rail::Shape::NorthSouth);
    let shape = |ew: bool| if ew { "east_west" } else { "north_south" };
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        // `onSneakWrenched`: the assembler to the player (`getDropsNoRail`), its rail stays.
        let kind = info.rail_type.unwrap_or(RailType::PoweredRail);
        let mut props = vec![("shape", shape(east_west))];
        if kind == RailType::ControllerRail {
            props.push(("backwards", if info.backwards { "true" } else { "false" }));
        }
        let Some(rail) = state_with(registry, kind.item(), &props) else {
            return;
        };
        if !game.creative {
            game.native_give_items("create:cart_assembler", 1);
        }
        game.native_set_block(
            &dimension,
            pos,
            rail,
            super::native_hooks::flags::UPDATE_ALL,
        );
        return;
    }
    let (ew, backwards) = cart::wrenched(east_west, info.backwards);
    let Some(mut s) = registry.state(state).cloned() else {
        return;
    };
    s.properties.insert("shape".into(), shape(ew).into());
    s.properties.insert(
        "backwards".into(),
        if backwards { "true" } else { "false" }.into(),
    );
    if let Some(id) = registry.id(&s) {
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// `CartAssemblerBlock.useItemOn`: another rail type's item swaps the rail it stands in for
/// (`ITEM_PICKUP` sound), the old rail's item back to a player not in creative.
pub(super) fn use_assembler(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(info) = assembler(state.0) else {
        return;
    };
    let held = if game.hand == horizon_client::first_person::Hand::Off {
        game.offhand.as_ref()
    } else {
        game.inventory.get(game.selected).and_then(Option::as_ref)
    };
    let Some(kind) = held.and_then(|s| RailType::from_item(&s.item)) else {
        return;
    };
    let previous = info.rail_type.unwrap_or(RailType::PoweredRail);
    if kind == previous {
        return;
    }
    *canceled = true;
    let registry = game.pack.assets.registry;
    let Some(mut s) = registry.state(state).cloned() else {
        return;
    };
    s.properties.insert("rail_type".into(), kind.name().into());
    let Some(id) = registry.id(&s) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    game.audio.play(
        "minecraft:entity.item.pickup",
        Some(pos.map(|v| f64::from(v) + 0.5)),
        1.0,
        1.0,
    );
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    if !game.creative {
        use_one(game);
        game.native_give_items(previous.item(), 1);
    }
}

/// `CartAssemblerBlock.getDrops`: broken by a player not in creative, its rail drops too.
pub(super) fn destroy_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: bool,
) {
    if canceled || game.creative {
        return;
    }
    let Some(info) = assembler(state.0) else {
        return;
    };
    let rail = info.rail_type.unwrap_or(RailType::PoweredRail).item();
    let dimension = game.dim.id().to_owned();
    LATER.with_borrow_mut(|l| {
        l.push(format!(
            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{rail}',count:1}}}}",
            pos[0], pos[1], pos[2]
        ))
    });
}

/// An assembler gone from the world forgets its mode.
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    if assembler(old).is_some() && assembler(new).is_none() {
        CARTS.with_borrow_mut(|c| {
            if c.stations.remove(&(dim, pos)).is_some() {
                c.busy.retain(|b| *b != (dim, pos));
                c.dirty = true;
            }
        });
    }
}

/// `/create_debug cart_mode`: the assembler's mode, as its value box's board sets it
/// (`ScrollOptionBehaviour`; the box sits outside the rail's outline here, where no click
/// reaches it).
pub(super) fn set_mode(dim: usize, pos: [i32; 3], mode: Option<Mode>) -> Mode {
    CARTS.with_borrow_mut(|c| {
        let s = c.stations.entry((dim, pos)).or_default();
        if let Some(m) = mode {
            s.mode = m;
            c.dirty = true;
        }
        s.mode
    })
}

// ---------------------------------------------------------------------------------------------
// Contraptions on carts

/// The contraption's NBT, as its item and the save keep it (`Contraption.writeNBT` reduced to
/// what is ported: the blocks, `RotationMode`, `InitialOrientation`).
fn contraption_tag(game: &Game<'_>, m: &Mounted, with_turn: bool) -> Compound {
    let registry = game.pack.assets.registry;
    let blocks = m
        .blocks
        .iter()
        .map(|(o, s)| {
            Tag::compound([
                ("Pos", Tag::IntArray(o.to_vec())),
                (
                    "State",
                    Tag::String(state_text(registry, *s).as_str().into()),
                ),
            ])
        })
        .collect();
    let mut c = Compound::new();
    c.insert("Blocks".into(), Tag::List(blocks));
    c.insert("RotationMode".into(), Tag::String(m.mode.name().into()));
    c.insert(
        "InitialOrientation".into(),
        Tag::String(dir_name(m.initial).to_uppercase().as_str().into()),
    );
    if with_turn {
        c.insert("Yaw".into(), Tag::Double(f64::from(m.turn.yaw).to_bits()));
        c.insert("Placed".into(), Tag::IntArray(vec![i32::from(m.placed)]));
    }
    c
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn double(t: Option<&Tag>) -> Option<f64> {
    match t {
        Some(Tag::Double(b)) => Some(f64::from_bits(*b)),
        _ => None,
    }
}

fn get<'a>(c: &'a Compound, key: &str) -> Option<&'a Tag> {
    c.get(&JavaString::from(key))
}

/// A contraption back from its NBT; None without blocks.
fn contraption_of(game: &Game<'_>, c: &Compound) -> Option<Mounted> {
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = get(c, "Blocks") else {
        return None;
    };
    let blocks: Vec<([i32; 3], u32)> = list
        .iter()
        .filter_map(|t| {
            let Tag::Compound(b) = t else { return None };
            let p = ints(get(b, "Pos"));
            let s = state_of_text(registry, &text(get(b, "State"))?)?;
            Some(([*p.first()?, *p.get(1)?, *p.get(2)?], s))
        })
        .collect();
    if blocks.is_empty() {
        return None;
    }
    let mode = Mode::read(&text(get(c, "RotationMode")).unwrap_or_default());
    let initial = text(get(c, "InitialOrientation"))
        .and_then(|v| {
            [Dir::North, Dir::South, Dir::West, Dir::East]
                .into_iter()
                .find(|d| dir_name(*d).eq_ignore_ascii_case(&v))
        })
        .unwrap_or(Dir::South);
    let mut m = Mounted::new(blocks, mode, initial);
    if let Some(yaw) = double(get(c, "Yaw")) {
        m.turn = mounted::Orientation::at(yaw as f32);
    }
    m.placed = ints(get(c, "Placed")).first() == Some(&1);
    Some(m)
}

/// `MinecartContraptionItem.useOn`: on a rail, a cart of the item's kind with its contraption,
/// facing the player's way (`createAtYaw`); elsewhere nothing (`FAIL`).
fn place_cart(game: &mut Game<'_>, stack: &horizon_client::items::ItemStack, pos: [i32; 3]) {
    let Some(kind) = cart::item_cart(&stack.item) else {
        return;
    };
    let Some((shape, ..)) = rail_at(game, pos) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    let Some(dim) = dimension_index(&dimension) else {
        return;
    };
    let facing = create_engine::place::horizontal_direction(game.yaw);
    let at = mounted::placed_cart(pos, Some(shape));
    let Some(id) =
        game.native_spawn_minecart(&dimension, kind, at, mounted::y_rot(facing), [0.0; 3])
    else {
        return;
    };
    let data = super::create_items_body::minecart_contraption(stack);
    let contraption = data.as_ref().and_then(|c| contraption_of(game, c));
    use_one(game);
    let (Some(m), Some(nc)) = (contraption, game.native_minecart(&dimension, id)) else {
        return;
    };
    let m = Mounted::placed_at(m.blocks, m.mode, m.initial, mounted::y_rot(facing));
    CARTS.with_borrow_mut(|c| {
        c.ids.insert(nc.uuid, (dim, id));
        c.known.insert(
            (dim, id),
            Known {
                uuid: nc.uuid,
                pos: nc.pos,
                prev: nc.pos,
            },
        );
        c.mounted.insert(
            nc.uuid,
            OnCart {
                dim,
                m,
                sent: None,
                fresh: true,
            },
        );
        c.dirty = true;
    });
}

/// A player's click on an entity (`PlayerInteractEvent.EntityInteract`): a coupling on a cart
/// (`MinecartCouplingItem`), a wrench on a coupled cart (`onWrenchInteractOnMinecart`), else a
/// wrench on a cart carrying a contraption picks it up
/// (`wrenchCanBeUsedToPickUpMinecartContraptions`).
pub(super) fn interact_entity(
    game: &mut Game<'_>,
    entity: i32,
    kind: &str,
    stack: Option<&horizon_client::items::ItemStack>,
    claimed: &mut bool,
) {
    if !kind.ends_with("minecart") {
        return;
    }
    let Some(item) = stack.map(|s| &*s.item) else {
        return;
    };
    let coupling = item == "create:minecart_coupling";
    if !coupling && item != "create:wrench" {
        return;
    }
    let dimension = game.dim.id().to_owned();
    let Some(dim) = dimension_index(&dimension) else {
        return;
    };
    let Some(nc) = game.native_minecart(&dimension, entity as u32) else {
        return;
    };
    if coupling {
        *claimed = true;
        click_coupling(game, &dimension, dim, &nc);
        return;
    }
    // A wrench: couplings come off first.
    let removed = CARTS.with_borrow_mut(|c| {
        let r = c.couplings.decouple(nc.uuid);
        c.dirty |= r.is_some();
        r
    });
    if let Some(items) = removed {
        *claimed = true;
        status(game, coupling::REMOVED);
        if !game.creative && items > 0 {
            game.native_give_items("create:minecart_coupling", items);
        }
        return;
    }
    let Some(on) = CARTS.with_borrow(|c| c.mounted.get(&nc.uuid).cloned()) else {
        return;
    };
    let Some(item) = cart::contraption_item(nc.kind) else {
        return;
    };
    *claimed = true;
    let mut out = horizon_client::items::ItemStack::new(item, 1);
    let tag = contraption_tag(game, &on.m, false);
    super::create_items_body::set_minecart_contraption(&mut out, tag);
    super::create_items_body::give_back(game, out);
    CARTS.with_borrow_mut(|c| {
        c.mounted.remove(&nc.uuid);
        c.ids.remove(&nc.uuid);
        c.known.remove(&(dim, nc.id));
        c.dirty = true;
    });
    gone(game, &dimension, nc.id);
    game.native_remove_minecart(&dimension, nc.id);
}

/// `onCouplingInteractOnMinecart` and `CouplingHandlerClient.onCartClicked`: a fully coupled
/// cart says so; otherwise the first cart clicked is selected and the second is coupled to it
/// (`tryToCoupleCarts`), the coupling used up.
fn click_coupling(game: &mut Game<'_>, dimension: &str, dim: usize, nc: &NativeMinecart) {
    let Some(player) = game.native_acting_player().map(|p| p.uuid) else {
        return;
    };
    if CARTS.with_borrow(|c| c.couplings.get(nc.uuid).is_fully_coupled()) {
        status(
            game,
            coupling::Refused::TwoCouplingsMax
                .message()
                .unwrap_or_default(),
        );
        return;
    }
    let first = CARTS.with_borrow(|c| c.selected.get(&player).copied());
    let first = match first {
        Some((d, id)) if d == dim && id != nc.id => game.native_minecart(dimension, id),
        _ => None,
    };
    let Some(first) = first else {
        CARTS.with_borrow_mut(|c| c.selected.insert(player, (dim, nc.id)));
        return;
    };
    CARTS.with_borrow_mut(|c| c.selected.remove(&player));
    let distance = (0..3)
        .map(|k| (first.pos[k] - nc.pos[k]).powi(2))
        .sum::<f64>()
        .sqrt();
    let result = CARTS.with_borrow_mut(|c| {
        let r = c
            .couplings
            .try_couple(first.uuid, nc.uuid, distance, false, |_| true);
        if r.is_ok() {
            for (u, id, p) in [(first.uuid, first.id, first.pos), (nc.uuid, nc.id, nc.pos)] {
                c.ids.insert(u, (dim, id));
                c.known.insert(
                    (dim, id),
                    Known {
                        uuid: u,
                        pos: p,
                        prev: p,
                    },
                );
            }
            c.dirty = true;
        }
        r
    });
    match result {
        Ok(()) => use_one(game),
        Err(e) => {
            if let Some(m) = e.message() {
                status(game, m);
            }
        }
    }
}

/// A player getting on a cart that carries a contraption is kept off (its seat is the
/// contraption's: `Entity.canAddPassenger`).
pub(super) fn entity_mount(dimension: &str, vehicle: u32, mounting: bool, canceled: &mut bool) {
    if !mounting {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    CARTS.with_borrow(|c| {
        if c.known
            .get(&(dim, vehicle))
            .is_some_and(|k| c.mounted.contains_key(&k.uuid))
        {
            *canceled = true;
        }
    });
}

/// A column left the level: its carts are no longer followed until they are found again.
pub(super) fn chunk_unload(dimension: &str, chunk: [i32; 2]) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    CARTS.with_borrow_mut(|c| {
        if c.known.is_empty() {
            return;
        }
        let inside = |p: [f64; 3]| {
            (p[0].floor() as i32) >> 4 == chunk[0] && (p[2].floor() as i32) >> 4 == chunk[1]
        };
        let left: Vec<(u32, u128)> = c
            .known
            .iter()
            .filter(|((d, _), k)| *d == dim && inside(k.pos))
            .map(|((_, id), k)| (*id, k.uuid))
            .collect();
        for (id, uuid) in left {
            c.known.remove(&(dim, id));
            c.ids.remove(&uuid);
            if let Some(on) = c.mounted.get_mut(&uuid) {
                on.sent = None;
                on.fresh = true;
                c.clear.push((dim, id));
            }
            c.scan = true;
        }
    });
}

/// After a level's tick: assemblers handle their carts, carts not found yet are looked for
/// (every second), followed carts move their contraptions or, gone, put them back, and coupled
/// carts keep their length. One emptiness test while nothing is on a cart.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if CARTS.with_borrow(Carts::idle) {
        return;
    }
    tick_stations(game, dimension, dim);
    let tick = game.world.game_time;
    let clear: Vec<u32> = CARTS.with_borrow_mut(|c| {
        let (here, rest) = c.clear.drain(..).partition(|(d, _)| *d == dim);
        c.clear = rest;
        here.into_iter()
            .map(|(_, id): (usize, u32)| id)
            .collect::<Vec<u32>>()
    });
    for id in clear {
        gone(game, dimension, id);
    }
    let scan = CARTS.with_borrow_mut(|c| std::mem::take(&mut c.scan)) || tick % 20 == 0;
    if scan {
        find_carts(game, dimension, dim);
    }
    let carts = follow(game, dimension, dim);
    tick_mounted(game, dimension, dim, &carts, tick);
    tick_couplings(game, dimension, dim, &carts);
}

/// Carts of saved contraptions and couplings not found yet, found in this level by UUID.
fn find_carts(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let wanted: Vec<u128> = CARTS.with_borrow(|c| {
        let mounted = c
            .mounted
            .iter()
            .filter(|(u, on)| on.dim == dim && !c.ids.contains_key(u))
            .map(|(u, _)| *u);
        let coupled = c
            .couplings
            .carts
            .keys()
            .filter(|u| !c.ids.contains_key(u))
            .copied();
        mounted.chain(coupled).collect()
    });
    if wanted.is_empty() {
        return;
    }
    for nc in game.native_minecarts(dimension) {
        if !wanted.contains(&nc.uuid) {
            continue;
        }
        CARTS.with_borrow_mut(|c| {
            c.ids.insert(nc.uuid, (dim, nc.id));
            c.known.insert(
                (dim, nc.id),
                Known {
                    uuid: nc.uuid,
                    pos: nc.pos,
                    prev: nc.pos,
                },
            );
            if let Some(on) = c.mounted.get_mut(&nc.uuid) {
                on.fresh = true;
                on.sent = None;
            }
        });
    }
}

/// The followed carts of this level as they are now; a cart gone while its column stayed (it
/// broke) lets go of its contraption, which goes back into the world where it was
/// (`OrientedContraptionEntity.stopRiding` → `disassemble`), and of its couplings
/// (`handleKilledMinecart`).
fn follow(game: &mut Game<'_>, dimension: &str, dim: usize) -> HashMap<u128, NativeMinecart> {
    let ids: Vec<(u32, Known)> = CARTS.with_borrow(|c| {
        c.known
            .iter()
            .filter(|((d, _), _)| *d == dim)
            .map(|((_, id), k)| (*id, *k))
            .collect()
    });
    let mut out = HashMap::with_capacity(ids.len());
    for (id, k) in ids {
        match game.native_minecart(dimension, id) {
            Some(nc) => {
                CARTS.with_borrow_mut(|c| {
                    if let Some(e) = c.known.get_mut(&(dim, id)) {
                        e.prev = e.pos;
                        e.pos = nc.pos;
                    }
                });
                out.insert(k.uuid, nc);
            }
            None => {
                let (on, drops) = CARTS.with_borrow_mut(|c| {
                    c.known.remove(&(dim, id));
                    c.ids.remove(&k.uuid);
                    let drops = c.couplings.killed(k.uuid);
                    c.dirty = true;
                    (c.mounted.remove(&k.uuid), drops)
                });
                if let Some(on) = on {
                    put_back(game, dimension, k.pos, &on.m);
                    gone(game, dimension, id);
                }
                for next in drops {
                    let other = CARTS.with_borrow(|c| {
                        let (d, i) = c.ids.get(&next).copied()?;
                        (d == dim)
                            .then(|| c.known.get(&(d, i)).map(|k| k.pos))
                            .flatten()
                    });
                    let at = other.map_or(k.pos, |o| [0, 1, 2].map(|a| (o[a] + k.pos[a]) / 2.0));
                    drop_item(game, dimension, at, "create:minecart_coupling");
                }
            }
        }
    }
    out
}

/// Each contraption follows its cart (`OrientedContraptionEntity.tickContraption`): it turns
/// toward the cart's motion by its mode, comes off on an activator rail (a rideable cart's
/// powered one ejects it, `Minecart.activateMinecart`; any lets go of a chest or furnace cart's,
/// `MinecartController.disassemble`), keeps riders off, and is told to clients when the glide
/// they run drifts.
fn tick_mounted(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    carts: &HashMap<u128, NativeMinecart>,
    tick: i64,
) {
    let list: Vec<u128> = CARTS.with_borrow(|c| {
        c.mounted
            .iter()
            .filter(|(u, on)| on.dim == dim && carts.contains_key(u))
            .map(|(u, _)| *u)
            .collect()
    });
    let players = if list.is_empty() {
        Vec::new()
    } else {
        players_in(game, dimension)
    };
    for uuid in list {
        let nc = &carts[&uuid];
        let Some(prev) = CARTS.with_borrow(|c| c.known.get(&(dim, nc.id)).map(|k| k.prev)) else {
            continue;
        };
        for rider in &nc.riders {
            game.native_stop_riding(dimension, *rider);
        }
        let rail = rail_under(game, nc.pos);
        let eject = rail.is_some_and(|(_, activator, powered)| {
            activator && (powered || nc.kind != "minecraft:minecart")
        });
        if eject {
            if let Some(on) = CARTS.with_borrow_mut(|c| {
                c.dirty = true;
                c.mounted.remove(&uuid)
            }) {
                put_back(game, dimension, nc.pos, &on.m);
                gone(game, dimension, nc.id);
            }
            continue;
        }
        let moved = [0, 1, 2].map(|k| nc.pos[k] - prev[k]);
        let message = CARTS.with_borrow_mut(|c| {
            let on = c.mounted.get_mut(&uuid)?;
            let size = mounted::x_size(&on.m.blocks);
            mounted::orient(
                &mut on.m.turn,
                on.m.mode,
                nc.velocity,
                moved,
                rail.map(|r| r.0),
                size,
            );
            let at = [nc.pos[0], nc.pos[1] + 0.5, nc.pos[2]];
            let angle = on.m.angle();
            let due = on.fresh || on.sent.is_none_or(|s| s.due(at, angle, tick));
            if !due {
                return None;
            }
            let turned = mounted::shortest_angle_diff(
                f64::from(on.m.turn.prev_yaw),
                f64::from(on.m.turn.yaw),
            );
            let pose = Pose {
                id: nc.id,
                at,
                velocity: moved.map(|v| (v * 20.0) as f32),
                angle,
                rate: -turned * 20.0,
                blocks: on.fresh.then(|| on.m.blocks.clone()),
            };
            on.fresh = false;
            on.sent = Some(Sent {
                at,
                velocity: pose.velocity,
                angle,
                rate: pose.rate,
                tick,
            });
            Some(pose.encode())
        });
        if let Some(msg) = message {
            for &p in &players {
                let _ = game.native_net_send_to_player(p, mounted::CHANNEL, &msg);
            }
        }
    }
}

/// `CouplingPhysics.tick`: each coupling whose carts are both here runs the soft step on their
/// motion (the hard step moves carts, which no helper does: not ported); a cart whose partner
/// isn't loaded holds still (`MinecartController.tick`'s internal stall).
fn tick_couplings(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    carts: &HashMap<u128, NativeMinecart>,
) {
    let pairs = CARTS.with_borrow(|c| c.couplings.pairs());
    for (a, b, length) in pairs {
        let (Some(ca), Some(cb)) = (carts.get(&a), carts.get(&b)) else {
            // `internalStall`: the loaded cart stops while the other is missing.
            let missing_here = CARTS.with_borrow(|c| [a, b].map(|u| c.ids.get(&u).map(|x| x.0)));
            for (u, at) in [(a, missing_here[0]), (b, missing_here[1])] {
                if at == Some(dim)
                    && let Some(nc) = carts.get(&u)
                {
                    game.native_set_delta_movement(dimension, nc.id, [0.0; 3]);
                }
            }
            continue;
        };
        let body = |nc: &NativeMinecart| {
            let next = coupling::predict(nc.pos, nc.velocity);
            Body {
                pos: nc.pos,
                motion: nc.velocity,
                can_add_motion: true,
                max_speed: cart::RAIL_MAX_SPEED,
                rail: rail_under(game, next).map(|r| r.0),
            }
        };
        let bodies = [body(ca), body(cb)];
        if let Some(m) = coupling::soft_step(bodies, f64::from(length)) {
            game.native_set_delta_movement(dimension, ca.id, m[0]);
            game.native_set_delta_movement(dimension, cb.id, m[1]);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Saved data

/// `NBTHelper` UUIDs as four ints, most significant first.
fn uuid_ints(u: u128) -> Vec<i32> {
    (0..4).map(|i| (u >> (96 - 32 * i)) as u32 as i32).collect()
}

fn uuid_of(v: &[i32]) -> Option<u128> {
    (v.len() == 4).then(|| {
        v.iter()
            .fold(0u128, |acc, x| acc << 32 | u128::from(*x as u32))
    })
}

/// The world's save: each contraption with its cart's UUID and level, each coupling from its
/// leading cart, each assembler's mode that isn't the default.
pub(super) fn level_save(game: &mut Game<'_>, dimension: &str) {
    if dimension != "minecraft:overworld" {
        return;
    }
    let (dirty, carts, couplings, stations) = CARTS.with_borrow_mut(|c| {
        let dirty = c.dirty || !c.mounted.is_empty();
        c.dirty = false;
        let mut carts: Vec<(u128, usize, Mounted)> = c
            .mounted
            .iter()
            .map(|(u, on)| (*u, on.dim, on.m.clone()))
            .collect();
        carts.sort_by_key(|x| x.0);
        let couplings: Vec<coupling::Link> = c
            .couplings
            .carts
            .values()
            .filter_map(|x| x.leading)
            .collect();
        let mut stations: Vec<(usize, [i32; 3], Mode)> = c
            .stations
            .iter()
            .filter(|(_, s)| s.mode != Mode::Rotate)
            .map(|((d, p), s)| (*d, *p, s.mode))
            .collect();
        stations.sort_by_key(|s| (s.0, s.1));
        (dirty, carts, couplings, stations)
    });
    if !dirty {
        return;
    }
    let carts: Vec<Tag> = carts
        .iter()
        .map(|(u, d, m)| {
            let mut c = contraption_tag(game, m, true);
            c.insert("Cart".into(), Tag::IntArray(uuid_ints(*u)));
            c.insert("Dimension".into(), Tag::String(DIMENSIONS[*d].into()));
            Tag::Compound(c)
        })
        .collect();
    let mut links: Vec<Tag> = Vec::new();
    let mut sorted = couplings;
    sorted.sort_by_key(|l| (l.main, l.connected));
    for l in sorted {
        links.push(Tag::compound([
            ("Main", Tag::IntArray(uuid_ints(l.main))),
            ("Connected", Tag::IntArray(uuid_ints(l.connected))),
            ("Length", Tag::Double(f64::from(l.length).to_bits())),
            ("Contraption", Tag::IntArray(vec![i32::from(l.contraption)])),
        ]));
    }
    let stations: Vec<Tag> = stations
        .iter()
        .map(|(d, p, m)| {
            Tag::compound([
                ("Dimension", Tag::String(DIMENSIONS[*d].into())),
                ("Pos", Tag::IntArray(p.to_vec())),
                ("Mode", Tag::String(m.name().into())),
            ])
        })
        .collect();
    let Some(data) = game.native_saved_data(SAVED.0, SAVED.1) else {
        return;
    };
    data.clear();
    data.insert("Contraptions".into(), Tag::List(carts));
    data.insert("Couplings".into(), Tag::List(links));
    data.insert("Assemblers".into(), Tag::List(stations));
    game.native_set_saved_data_dirty(SAVED.0, SAVED.1);
}

/// The world opened: its contraptions, couplings and assembler modes back from the save; their
/// carts are found by UUID as their columns load.
pub(super) fn level_load(game: &mut Game<'_>, dimension: &str) {
    if dimension != "minecraft:overworld" || !ensure(game.pack.assets.registry) {
        return;
    }
    let Some(data) = game.native_saved_data(SAVED.0, SAVED.1).cloned() else {
        return;
    };
    let list = |key: &str| -> Vec<Compound> {
        match get(&data, key) {
            Some(Tag::List(l)) => l
                .iter()
                .filter_map(|t| match t {
                    Tag::Compound(c) => Some(c.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let mut carts = Carts::default();
    for c in list("Contraptions") {
        let (Some(uuid), Some(dim)) = (
            uuid_of(&ints(get(&c, "Cart"))),
            text(get(&c, "Dimension")).and_then(|d| dimension_index(&d)),
        ) else {
            continue;
        };
        if let Some(m) = contraption_of(game, &c) {
            carts.mounted.insert(
                uuid,
                OnCart {
                    dim,
                    m,
                    sent: None,
                    fresh: true,
                },
            );
        }
    }
    for c in list("Couplings") {
        let (Some(main), Some(connected)) = (
            uuid_of(&ints(get(&c, "Main"))),
            uuid_of(&ints(get(&c, "Connected"))),
        ) else {
            continue;
        };
        let link = coupling::Link {
            main,
            connected,
            length: double(get(&c, "Length")).unwrap_or(2.0) as f32,
            contraption: ints(get(&c, "Contraption")).first() == Some(&1),
        };
        carts.couplings.carts.entry(main).or_default().leading = Some(link);
        carts
            .couplings
            .carts
            .entry(connected)
            .or_default()
            .connected = Some(link);
    }
    for c in list("Assemblers") {
        let p = ints(get(&c, "Pos"));
        let (Some(dim), [x, y, z]) = (
            text(get(&c, "Dimension")).and_then(|d| dimension_index(&d)),
            p.as_slice(),
        ) else {
            continue;
        };
        carts.stations.insert(
            (dim, [*x, *y, *z]),
            Station {
                mode: Mode::read(&text(get(&c, "Mode")).unwrap_or_default()),
                ..Station::default()
            },
        );
    }
    carts.scan = true;
    CARTS.with_borrow_mut(|c| *c = carts);
}

/// `/create_debug carts`: the contraptions on carts and the couplings, for tests.
pub(super) fn debug(dim: usize) -> String {
    CARTS.with_borrow(|c| {
        let mut lines: Vec<String> = c
            .mounted
            .iter()
            .filter(|(_, on)| on.dim == dim)
            .map(|(u, on)| {
                let at = c.ids.get(u).and_then(|k| c.known.get(k));
                format!(
                    "cart {:032x} id={} at={} yaw={:.1} blocks={} mode={}",
                    u,
                    c.ids.get(u).map_or(-1, |x| x.1 as i64),
                    at.map_or("unloaded".into(), |k| format!(
                        "{:.2},{:.2},{:.2}",
                        k.pos[0], k.pos[1], k.pos[2]
                    )),
                    on.m.turn.yaw,
                    on.m.blocks.len(),
                    on.m.mode.name()
                )
            })
            .collect();
        for (a, b, l) in c.couplings.pairs() {
            let pos = |u: u128| c.ids.get(&u).and_then(|k| c.known.get(k)).map(|k| k.pos);
            let d = match (pos(a), pos(b)) {
                (Some(p), Some(q)) => format!(
                    "{:.3} at={:.2},{:.2}/{:.2},{:.2}",
                    (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f64>().sqrt(),
                    p[0],
                    p[2],
                    q[0],
                    q[2]
                ),
                _ => "?".into(),
            };
            lines.push(format!(
                "coupling {a:032x}->{b:032x} length={l} distance={d}"
            ));
        }
        lines.sort();
        if lines.is_empty() {
            "no carts".into()
        } else {
            lines.join("; ")
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Clients

fn part_key(id: u32) -> u64 {
    depot_key([id as i32, i32::MIN + 1, 0], 223, 0)
}

/// A cart's contraption on a client (`OrientedContraptionEntity`'s render): one part model of
/// its blocks (the anchor left out), its anchor block's centre the pivot, gliding at the cart's
/// velocity and turning about up at its rate from its angle now, on the GPU until the next
/// message.
pub(super) fn from_server(game: &mut Game<'_>, d: &[u8]) {
    let pose = match Pose::decode(d) {
        Ok(p) => p,
        Err(id) => {
            if let Some(id) = id
                && let Some(model) = CLIENT.with_borrow_mut(|c| c.remove(&id))
            {
                game.native_part_clear(part_key(id));
                game.native_part_model_free(model);
            }
            return;
        }
    };
    if let Some(blocks) = &pose.blocks {
        let parts: Vec<(BlockStateId, glam::Mat4)> = blocks
            .iter()
            .filter(|(o, _)| *o != [0, 0, 0])
            .map(|(o, s)| {
                (
                    BlockStateId(*s),
                    glam::Mat4::from_translation(glam::Vec3::new(
                        o[0] as f32,
                        o[1] as f32,
                        o[2] as f32,
                    )),
                )
            })
            .collect();
        if let Some(old) = CLIENT.with_borrow_mut(|c| c.remove(&pose.id)) {
            game.native_part_clear(part_key(pose.id));
            game.native_part_model_free(old);
        }
        if let Some(model) = game.native_part_model_compound(&parts) {
            CLIENT.with_borrow_mut(|c| c.insert(pose.id, model));
        }
    }
    let Some(model) = CLIENT.with_borrow(|c| c.get(&pose.id).copied()) else {
        return;
    };
    let now = game.native_parts_seconds();
    let block: [i32; 3] = std::array::from_fn(|k| pose.at[k].floor() as i32);
    let offset = [
        (pose.at[0] - f64::from(block[0]) - 0.5) as f32,
        (pose.at[1] - f64::from(block[1]) - 0.5) as f32,
        (pose.at[2] - f64::from(block[2]) - 0.5) as f32,
        now,
    ];
    let phase = (pose.angle - pose.rate * now).rem_euclid(360.0);
    game.native_part_glide(
        part_key(pose.id),
        model,
        block,
        1,
        pose.rate / 6.0,
        phase,
        offset,
        pose.velocity,
    );
}

/// Out of a world: the client forgets what it drew.
pub(super) fn client_reset() {
    CLIENT.with_borrow_mut(HashMap::clear);
}

/// `/create_debug carts` (the contraptions on carts and the couplings of the player's level)
/// and `/create_debug cart_mode <pos> [rotate|rotate_paused|rotation_locked]` (an assembler's
/// mode, as its value box's board sets it), for tests.
pub(super) fn cart_command(game: &mut Game<'_>, command: &str) -> bool {
    let player = game.native_acting_player();
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    if command.trim_end() == "create_debug carts" {
        let line = debug(dim);
        game.native_feedback(horizon_client::text::Text::literal(line));
        return true;
    }
    let Some(rest) = command.strip_prefix("create_debug cart_mode ") else {
        return false;
    };
    let here = player.as_ref().map(|p| p.position);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let parts: Vec<i32> = words
        .iter()
        .take(3)
        .enumerate()
        .filter_map(|(n, t)| match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        })
        .collect();
    let mode = words.get(3).and_then(|w| {
        Mode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(w))
    });
    let line = match parts.as_slice() {
        [x, y, z] => format!(
            "cart assembler at {x} {y} {z}: {}",
            set_mode(dim, [*x, *y, *z], mode).name()
        ),
        _ => "usage: create_debug cart_mode <x> <y> <z> [rotate|rotate_paused|rotation_locked]"
            .into(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}
