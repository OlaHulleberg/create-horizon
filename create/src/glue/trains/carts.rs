//! Create's minecart contraptions: the cart assembler (`CartAssemblerBlock`,
//! `CartAssemblerBlockEntity`, `CartAssemblerBlockItem`), the contraptions carts carry
//! (`MountedContraption`, `OrientedContraptionEntity`), their items (`MinecartContraptionItem`)
//! and couplings (`MinecartCouplingItem`, `CouplingHandler`, `CouplingPhysics`,
//! `MinecartController`). The rules are `create_engine::cart`'s, `mounted`'s and `coupling`'s.
//! A contraption is typed data keyed by its cart's UUID, followed each level tick through the
//! cart's entity, and kept in the overworld's level attachment (`create:minecart_contraptions`,
//! the legacy `create/minecart_contraptions` saved data's layout) with the couplings and the
//! assemblers' modes. Nothing runs in a level tick while no cart carries a contraption, no
//! carts are coupled and no assembler has a cart to handle.
//!
//! Left out, as the mod API can't do it (beyond drawing, see the parent module):
//!
//! - A carrying cart counting as ridden (`native_set_minecart_carrying`, Horizon API 68): the
//!   cart's own rail physics run as an empty cart's. Needs `MinecartRef::set_carrying`.
//! - The hard coupling step moving a cart off its rail with collisions
//!   (`native_move_minecart`, `Entity.move(MoverType.SELF)`): such a cart only has its motion
//!   damped. Needs `EntityRef::move_by` (HORIZON_API.md's entity motion row).
//! - A redstone conductor beside an assembler turning an accelerated cart
//!   (`BlockState.isRedstoneConductor`): no API reads it, so none is seen. Needs a state fact.
//! - The contraption's storage (`MountedStorageManager`) and actors (drills, harvesters) on a
//!   cart: they are the contraption subsystems'; a block with a block entity of its own is
//!   refused by the structure search, and a furnace cart is not fuelled from storage.

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::subsystem::Subsystem;
use super::super::world::{level_id, level_index, with_world};
use super::structure::{found, turned_state};
use create_engine::Block;
use create_engine::cart::{self, Assembler, RailType};
use create_engine::coupling::{self, Body, Couplings};
use create_engine::kinetics::{Axis, Dir};
use create_engine::mounted::{self, Mode, Mounted, RailShape};
use mod_api::{
    AttachmentCodec, AttachmentType, BlockPart, BlockState, BreakBlockEvent, Compound,
    EntityInteractEvent, EntityMountEvent, EntityRef, EntityUuid, Flow, Holder, Host, HostSide,
    InteractionResult, ItemPart, ItemStack, MinecartPassEvent, Registrar, RegistrationContext,
    ServerHost, SoundSource, Tag, Text, UseItemOnEvent, UseOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The overworld's saved contraptions, couplings and assembler modes.
static SAVED: AttachmentType<Compound> = AttachmentType {
    namespace: "create",
    name: "minecart_contraptions",
    default: Compound::new,
    codec: Some(AttachmentCodec {
        encode: |data| Ok(data.clone()),
        decode: |data| Ok(data.clone()),
    }),
    copy_on_death: false,
    sync: false,
};

/// A cart as a tick reads it.
#[derive(Clone, Debug)]
struct Cart {
    entity: EntityRef,
    uuid: u128,
    kind: String,
    pos: [f64; 3],
    velocity: [f64; 3],
    push: [f64; 3],
    fuel: i32,
    ridden: bool,
}

impl Cart {
    fn read(host: &mut dyn ServerHost, entity: EntityRef) -> Option<Cart> {
        let kind = entity.kind(&*host)?;
        if !kind.ends_with("minecart") {
            return None;
        }
        let minecart = host.minecart(entity)?;
        Some(Cart {
            entity,
            uuid: entity.uuid(host)?,
            pos: entity.position(&*host)?,
            velocity: entity.velocity(&*host).unwrap_or_default(),
            push: minecart.push(&*host).unwrap_or_default(),
            fuel: minecart.fuel(&*host).unwrap_or(0),
            ridden: entity.passengers(&*host).is_some_and(|p| !p.is_empty()),
            kind,
        })
    }
}

/// A cart this module follows: where it is and where it was the tick before.
#[derive(Clone, Copy, Debug)]
struct Known {
    entity: EntityRef,
    pos: [f64; 3],
    prev: [f64; 3],
}

/// A contraption on a cart (its `OrientedContraptionEntity`), by the cart's UUID: its level and
/// whether it spans two coupled carts (`OrientedContraptionEntity.COUPLING`: the coupling a
/// contraption made, whose leading cart is its `couplingId`).
#[derive(Clone, Debug)]
struct OnCart {
    dim: usize,
    m: Mounted,
    coupled: bool,
}

/// A cart assembler's block entity: its cooldown, its mode (`movementMode`) and the cart that
/// passed it since its last tick (`cartToAssemble`).
#[derive(Clone, Copy, Debug, Default)]
struct Station {
    cooldown: Assembler,
    mode: Mode,
    cart: Option<EntityRef>,
}

#[derive(Default)]
struct State {
    /// Carts followed, by UUID.
    known: HashMap<u128, Known>,
    mounted: HashMap<u128, OnCart>,
    couplings: Couplings,
    stations: HashMap<(usize, [i32; 3]), Station>,
    /// Assemblers with a cart to handle or cooling down.
    busy: Vec<(usize, [i32; 3])>,
    /// A player's first click with a coupling (`CouplingHandlerClient.selectedCart`).
    selected: HashMap<u128, EntityRef>,
    /// The saved data was read.
    loaded: bool,
    dirty: bool,
}

impl State {
    /// The one test a level tick makes while nothing is on a cart.
    fn idle(&self) -> bool {
        self.mounted.is_empty() && self.couplings.is_empty() && self.busy.is_empty()
    }
}

/// A rail's shape, and whether it is an activator rail and powered.
type RailFacts = (RailShape, bool, bool);

thread_local! {
    static CARTS: RefCell<State> = RefCell::new(State::default());
    /// Rail facts by block state: its shape, and whether it is an activator rail and powered.
    static RAILS: RefCell<HashMap<BlockState, Option<RailFacts>>> =
        RefCell::new(HashMap::new());
}

pub struct Carts;

impl Subsystem for Carts {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        r.block_part(states::key(Block::CartAssembler), AssemblerBlock);
        let item = r.item_key("create:cart_assembler");
        r.item_part(item, AssemblerItem);
        for name in [
            "create:minecart_contraption",
            "create:furnace_minecart_contraption",
            "create:chest_minecart_contraption",
        ] {
            let item = r.item_key(name);
            r.item_part(item, ContraptionItem);
        }
    }

    fn world_closed(&self) {
        CARTS.with_borrow_mut(|c| *c = State::default());
        RAILS.with_borrow_mut(HashMap::clear);
    }

    /// An assembler gone from the world forgets its mode.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        if assembler(old).is_some() && assembler(new).is_none() {
            CARTS.with_borrow_mut(|c| {
                if c.stations.remove(&(dim, pos)).is_some() {
                    c.busy.retain(|b| *b != (dim, pos));
                    c.dirty = true;
                }
            });
        }
    }

    /// A column left the level: its carts are no longer followed until they are found again.
    fn chunk_unload(&self, _dim: usize, chunk: [i32; 2]) {
        CARTS.with_borrow_mut(|c| {
            let inside = |p: [f64; 3]| {
                (p[0].floor() as i32) >> 4 == chunk[0] && (p[2].floor() as i32) >> 4 == chunk[1]
            };
            c.known.retain(|_, k| !inside(k.pos));
        });
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        level_tick(lv);
    }

    /// A player getting on a cart that carries a contraption is kept off (its seat is the
    /// contraption's: `Entity.canAddPassenger`).
    fn entity_mount(&self, _host: &mut dyn ServerHost, event: &mut EntityMountEvent<'_>) -> Flow {
        if !event.mounting {
            return Flow::Continue;
        }
        let carrying = CARTS.with_borrow(|c| {
            c.known
                .iter()
                .any(|(u, k)| k.entity.id() == event.vehicle && c.mounted.contains_key(u))
        });
        if carrying {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn entity_interact(
        &self,
        host: HostSide,
        event: &mut EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        interact_entity(host, event)
    }

    /// `CartAssemblerBlock.getDrops`: broken by a player not in creative, its rail drops too.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        if event.cancelled {
            return Flow::Continue;
        }
        let Some(info) = states::id(event.state).and_then(assembler) else {
            return Flow::Continue;
        };
        let Some(player) = host.players().acting_player() else {
            return Flow::Continue;
        };
        if player.is_creative(&*host) == Some(true) {
            return Flow::Continue;
        }
        let rail = info.rail_type.unwrap_or(RailType::PoweredRail).item();
        let Some(level) = player.level(&*host) else {
            return Flow::Continue;
        };
        if let Some(stack) = item_stack(&*host, rail, 1) {
            let p = event.pos.map(|v| f64::from(v) + 0.5);
            let _ = level.spawn_item(host, p[0], p[1], p[2], stack, 10);
        }
        Flow::Continue
    }
}

fn assembler(state: u32) -> Option<create_engine::parts::StateInfo> {
    with_world(|w| {
        let (b, info) = w.table.get(state)?;
        (b == Block::CartAssembler).then_some(*info)
    })
    .flatten()
}

/// One of item `id`.
fn item_stack(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    let key = host.item_key(&mod_api::Identifier::parse(id).ok()?)?;
    ItemStack::new(key, count)
}

/// A rail block's shape, and whether it is an activator rail and powered (`BlockTags.RAILS`:
/// vanilla's rails and Create's rail blocks), read once per state.
fn rail_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<RailFacts> {
    let state = lv.block_state(p)?;
    if let Some(known) = RAILS.with_borrow(|r| r.get(&state).copied()) {
        return known;
    }
    let facts = super::state_parts(&*lv.host, state).and_then(|(name, props)| {
        let rail = matches!(
            name.as_str(),
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
        let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let shape = RailShape::read(prop("shape")?)?;
        let powered = prop("powered") == Some("true");
        Some((shape, name == "minecraft:activator_rail", powered))
    });
    RAILS.with_borrow_mut(|r| r.insert(state, facts));
    facts
}

/// `AbstractMinecart.getCurrentRailPosition`: the cart's block, or the one below when that is a
/// rail.
fn rail_under(lv: &ServerLevel<'_>, pos: [f64; 3]) -> Option<RailFacts> {
    let p = pos.map(|v| v.floor() as i32);
    rail_at(lv, [p[0], p[1] - 1, p[2]]).or_else(|| rail_at(lv, p))
}

fn status(host: &mut dyn ServerHost, player: Option<mod_api::PlayerRef>, text: &str) {
    if let Some(p) = player {
        super::action_bar(host, p, Text::literal(text));
    }
}

/// The stack in the hand the click used shrinks by one (not in creative).
fn use_one(host: &mut dyn ServerHost, player: mod_api::PlayerRef) {
    if player.is_creative(&*host) == Some(true) {
        return;
    }
    let Some(slot) = player.selected_slot(&*host) else {
        return;
    };
    let Some(stack) = player
        .inventory(&*host)
        .and_then(|inv| inv.into_iter().nth(slot))
    else {
        return;
    };
    let next = (stack.count() > 1)
        .then(|| ItemStack::new(stack.item(), stack.count() - 1))
        .flatten();
    let _ = player.set_inventory_slot(host, slot as u32, next);
}

// ---------------------------------------------------------------------------------------------
// The assembler block

/// The assembler's part: the carts that pass it, and clicks on it.
struct AssemblerBlock;

#[mod_api::horizon_part]
impl BlockPart for AssemblerBlock {
    /// `onMinecartPass`: a cart Create may load (`canAssembleTo`) is handled at its next tick
    /// (`assembleNextTick`: the first cart to pass since then).
    fn minecart_pass(&self, host: &mut dyn ServerHost, event: &mut MinecartPassEvent) {
        let Some(kind) = event.entity.kind(&*host) else {
            return;
        };
        if !cart::can_assemble_to(&kind) {
            return;
        }
        let Some(dim) = event
            .entity
            .level(&*host)
            .and_then(|l| l.dimension().id(&*host))
            .and_then(|d| level_index(&d))
        else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        CARTS.with_borrow_mut(|c| {
            let s = c.stations.entry((dim, pos)).or_default();
            if s.cart.is_none() {
                s.cart = Some(event.entity);
            }
            if !c.busy.contains(&(dim, pos)) {
                c.busy.push((dim, pos));
            }
        });
    }

    /// `CartAssemblerBlock.useItemOn` (another rail type's item swaps the rail it stands in
    /// for) and the wrench's `onWrenched` / `onSneakWrenched`.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let stack = click.stack?;
        let item = stack.item().id(&*host);
        let player = host.player(click.player)?;
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let info = assembler(lv.block(click.pos))?;
        if item == "create:wrench" {
            wrench_assembler(&mut lv, player, click.pos, event.state, &info);
            return Some(InteractionResult::SuccessServer);
        }
        let kind = RailType::from_item(&item)?;
        let previous = info.rail_type.unwrap_or(RailType::PoweredRail);
        if kind == previous {
            return None;
        }
        let next = super::with_props(&*lv.host, event.state, &[("rail_type", kind.name())])?;
        let at = click.pos.map(|v| f64::from(v) + 0.5);
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:entity.item.pickup",
            at,
            SoundSource::Players,
            1.0,
            1.0,
        );
        lv.set_state(click.pos, next, update_all());
        if player.is_creative(&*lv.host) != Some(true) {
            use_one(lv.host, player);
            if let Some(back) = item_stack(&*lv.host, previous.item(), 1) {
                let _ = player.give_stack(lv.host, back);
            }
        }
        Some(InteractionResult::SuccessServer)
    }
}

fn wrench_assembler(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    state: BlockState,
    info: &create_engine::parts::StateInfo,
) {
    let east_west = info.rail != Some(create_engine::rail::Shape::NorthSouth);
    let shape = |ew: bool| if ew { "east_west" } else { "north_south" };
    if player.entity().is_shift_key_down(&*lv.host) == Some(true) {
        // `onSneakWrenched`: the assembler to the player (`getDropsNoRail`), its rail stays.
        let kind = info.rail_type.unwrap_or(RailType::PoweredRail);
        let Some(base) = super::default_state(&*lv.host, kind.item()) else {
            return;
        };
        let mut props = vec![("shape", shape(east_west))];
        if kind == RailType::ControllerRail {
            props.push(("backwards", if info.backwards { "true" } else { "false" }));
        }
        let Some(rail) = super::with_props(&*lv.host, base, &props) else {
            return;
        };
        if player.is_creative(&*lv.host) != Some(true)
            && let Some(stack) = item_stack(&*lv.host, "create:cart_assembler", 1)
        {
            let _ = player.give_stack(lv.host, stack);
        }
        lv.set_state(pos, rail, update_all());
        return;
    }
    let (ew, backwards) = cart::wrenched(east_west, info.backwards);
    let backwards = if backwards { "true" } else { "false" };
    if let Some(s) = super::with_props(
        &*lv.host,
        state,
        &[("shape", shape(ew)), ("backwards", backwards)],
    ) {
        lv.set_state(pos, s, update_all());
    }
}

/// `CartAssemblerBlockItem.tryPlaceAssembler`: on a straight flat rail of a kind it takes, the
/// rail becomes an assembler holding it.
struct AssemblerItem;

#[mod_api::horizon_part]
impl ItemPart for AssemblerItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let player = host.player(event.player)?;
        let dimension = player.level(&*host)?.dimension().id(&*host)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        place_assembler(&mut lv, player, event.pos);
        Some(InteractionResult::SuccessServer)
    }
}

fn place_assembler(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, pos: [i32; 3]) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let Some((name, props)) = super::state_parts(&*lv.host, state) else {
        return;
    };
    let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let shape = prop("shape").unwrap_or_default();
    let backwards =
        (name == "create:controller_rail").then(|| prop("backwards").is_some_and(|v| v == "true"));
    let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
    let facing = create_engine::place::horizontal_direction(yaw);
    let Some((kind, east_west, backwards)) = cart::placed_on(&name, &shape, backwards, facing)
    else {
        if RailType::from_item(&name).is_none() {
            status(
                lv.host,
                Some(player),
                "Place your Cart Assembler on a rail block",
            );
        }
        return;
    };
    let Some(placed) = lv
        .host
        .block_state_default(states::key(Block::CartAssembler))
        .and_then(|base| {
            super::with_props(
                &*lv.host,
                base,
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
            )
        })
    else {
        return;
    };
    lv.set_state(pos, placed, update_all());
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.stone.place",
        pos.map(|v| f64::from(v) + 0.5),
        SoundSource::Blocks,
        1.0,
        1.0,
    );
    use_one(lv.host, player);
}

/// `CartAssemblerBlockEntity.tick` for assemblers with a cart or cooling down: at most one cart
/// every 8 ticks (`assemblyCooldown`), assembled onto or taken off by the rail type and power
/// (`getActionForCart`), then sent on, braked or left as the action says.
fn tick_stations(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
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
        let Some(entity) = handle else { continue };
        let Some(info) = assembler(lv.block(pos)) else {
            continue;
        };
        let Some(nc) = Cart::read(lv.host, entity) else {
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
            assemble(lv, pos, axis, info.backwards, &nc);
        }
        if action.disassembles() {
            disassemble_at(lv, axis, info.backwards, &nc);
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
        let centre = [
            f64::from(pos[0]) + 0.5 - nc.pos[0],
            f64::from(pos[1]) + 0.5 - nc.pos[1],
            f64::from(pos[2]) + 0.5 - nc.pos[2],
        ];
        // No API reads a redstone conductor (see the module's doc).
        if let Some(motion) =
            cart::cart_motion(action, axis, info.backwards, v, facing, |_| false, centre)
        {
            nc.entity.set_velocity(lv.host, motion);
        }
    }
}

/// The minecarts of `lv` in the block at `p`'s column of space, near it.
fn carts_near(lv: &mut ServerLevel<'_>, p: [i32; 3], skip: u128) -> Vec<Cart> {
    let lo = [
        f64::from(p[0]) - 1.0,
        f64::from(p[1]) - 1.0,
        f64::from(p[2]) - 1.0,
    ];
    let hi = [
        f64::from(p[0]) + 2.0,
        f64::from(p[1]) + 2.0,
        f64::from(p[2]) + 2.0,
    ];
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    found
        .into_iter()
        .filter_map(|e| Cart::read(lv.host, e))
        .filter(|c| c.uuid != skip)
        .collect()
}

/// `MountedContraption.testSecondaryCartAssembler`: an assembler at `p` in line with the anchor
/// (along x or z) with a cart Create may load in its block (the first cart there, as long as
/// it is one of those) stands in for a second anchor; the cart is the contraption's
/// `connectedCart`.
fn secondary_cart(p: [i32; 3], anchor: [i32; 3], carts: &[Cart]) -> Option<&Cart> {
    let in_line = p[1] == anchor[1] && (p[0] == anchor[0] || p[2] == anchor[2]);
    if !in_line {
        return None;
    }
    let half = 0.49;
    let first = carts.iter().find(|c| {
        c.pos[0] + half > f64::from(p[0])
            && c.pos[0] - half < f64::from(p[0] + 1)
            && c.pos[2] + half > f64::from(p[2])
            && c.pos[2] - half < f64::from(p[2] + 1)
            && c.pos[1] + 0.7 > f64::from(p[1])
            && c.pos[1] < f64::from(p[1] + 1)
    })?;
    cart::can_assemble_to(&first.kind).then_some(first)
}

/// The coupling a contraption made that `cart` is in (`hasContraptionCoupling`).
fn contraption_link(c: &State, cart: u128) -> Option<coupling::Link> {
    let ctl = c.couplings.get(cart);
    [ctl.leading, ctl.connected]
        .into_iter()
        .flatten()
        .find(|l| l.contraption)
}

fn anchor_state(host: &dyn Host, axis: &str) -> Option<BlockState> {
    let base = host.block_state_default(states::key(Block::MinecartAnchor))?;
    super::with_props(host, base, &[("axis", axis)])
}

fn is_anchor(id: u32) -> bool {
    super::car_state(id).is_some_and(|s| s.block() == states::key(Block::MinecartAnchor))
}

/// `CartAssemblerBlockEntity.assemble`: the structure above the assembler (`MountedContraption`)
/// leaves the world and rides the cart, facing the assembler's way (`getHorizontalDirection`).
/// With a second assembler in it holding a cart, both carts are set on their assemblers'
/// centres and coupled (`tryToCoupleCarts` with no player: a contraption's coupling), and the
/// contraption spans them, facing from the first to the second.
fn assemble(lv: &mut ServerLevel<'_>, pos: [i32; 3], axis: Axis, backwards: bool, nc: &Cart) {
    let taken = CARTS.with_borrow(|c| {
        c.mounted.contains_key(&nc.uuid) || contraption_link(c, nc.uuid).is_some()
    });
    if nc.ridden || taken {
        return;
    }
    let axis_name = if axis == Axis::Z { "z" } else { "x" };
    let Some(anchor) = anchor_state(&*lv.host, axis_name) else {
        return;
    };
    let anchor_id = super::car_id(anchor);
    let gathered = {
        let lv_ref = &*lv;
        cart::gather(pos, anchor_id, |p| {
            let mut f = found(lv_ref, p)?;
            if assembler(f.state).is_some() {
                // `movementAllowed`: another assembler moves only as a second anchor; whether
                // it holds a cart is tested below.
                f.movable = p != pos;
            }
            Some(f)
        })
    };
    let Ok(Some(mut blocks)) = gathered else {
        return;
    };
    // `capture`: other assemblers become anchors, the first holding a cart gives the
    // connected one.
    let mut connected: Option<(Cart, [i32; 3])> = None;
    for (o, s) in blocks.iter_mut().filter(|(o, _)| *o != [0, 0, 0]) {
        let Some(info) = assembler(*s) else { continue };
        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let carts = carts_near(lv, p, nc.uuid);
        let Some(second) = secondary_cart(p, pos, &carts) else {
            // `testSecondaryCartAssembler` failed: the assembler can't move.
            return;
        };
        if connected.is_none() {
            connected = Some((second.clone(), p));
        }
        let ns = info.rail == Some(create_engine::rail::Shape::NorthSouth);
        if let Some(a) = anchor_state(&*lv.host, if ns { "z" } else { "x" }) {
            *s = super::car_id(a);
        }
    }
    let mut initial = cart::horizontal_direction(axis, backwards);
    if let Some((other, p)) = &connected {
        let centre = |b: [i32; 3]| {
            [
                f64::from(b[0]) + 0.5,
                f64::from(b[1]),
                f64::from(b[2]) + 0.5,
            ]
        };
        let (here, there) = (centre(pos), centre(*p));
        let d = [0, 1, 2].map(|k| there[k] - here[k]);
        let distance = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        let coupled = CARTS.with_borrow_mut(|c| {
            c.couplings
                .try_couple(nc.uuid, other.uuid, distance, true, |_| true)
                .is_ok()
        });
        if !coupled {
            return;
        }
        nc.entity.teleport_to(lv.host, here, None);
        other.entity.teleport_to(lv.host, there, None);
        initial = coupling::dir_from_y_rot(d[2].atan2(d[0]).to_degrees());
        CARTS.with_borrow_mut(|c| {
            c.known.insert(
                other.uuid,
                Known {
                    entity: other.entity,
                    pos: there,
                    prev: there,
                },
            );
        });
    }
    // `removeBlocksFromWorld`: anchors stay.
    let Some(air) = super::default_state(&*lv.host, "minecraft:air") else {
        return;
    };
    for (o, s) in blocks.iter().filter(|(o, _)| *o != [0, 0, 0]) {
        if is_anchor(*s) {
            continue;
        }
        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        lv.set_state(p, air, update_all());
    }
    let dim = lv.dim;
    let mode = CARTS.with_borrow(|c| {
        c.stations
            .get(&(dim, pos))
            .map(|s| s.mode)
            .unwrap_or_default()
    });
    let m = Mounted::new(blocks, mode, initial);
    // A furnace cart's push is zeroed (`PushX`, `PushZ` saved as 0 and loaded back).
    if nc.kind == "minecraft:furnace_minecart"
        && let Some(minecart) = lv.host.minecart(nc.entity)
    {
        minecart.set_push(lv.host, [0.0; 3]);
    }
    let coupled = connected.is_some();
    CARTS.with_borrow_mut(|c| {
        c.known.insert(
            nc.uuid,
            Known {
                entity: nc.entity,
                pos: nc.pos,
                prev: nc.pos,
            },
        );
        c.mounted.insert(nc.uuid, OnCart { dim, m, coupled });
        c.dirty = true;
    });
}

/// `CartAssemblerBlockEntity.disassemble`: the contraption, turned to the assembler's way, goes
/// back into the world (`disassembleCart`: a furnace cart's push set from its motion, `PushZ`
/// from x and `PushX` from z as Create writes them). A contraption on a coupling waits until
/// its other cart is on an assembler that disassembles too, keeps its turn, and its coupling
/// goes.
fn disassemble_at(lv: &mut ServerLevel<'_>, axis: Axis, backwards: bool, nc: &Cart) {
    let Some(coupled) = CARTS.with_borrow(|c| c.mounted.get(&nc.uuid).map(|on| on.coupled)) else {
        return;
    };
    if coupled {
        let Some(link) = CARTS.with_borrow(|c| contraption_link(c, nc.uuid)) else {
            return;
        };
        let other = if link.main == nc.uuid {
            link.connected
        } else {
            link.main
        };
        let other = CARTS
            .with_borrow(|c| c.known.get(&other).map(|k| k.entity))
            .and_then(|e| Cart::read(lv.host, e));
        let Some(other) = other else { return };
        let b = other.pos.map(|v| v.floor() as i32);
        let Some(info) = assembler(lv.block(b)) else {
            return;
        };
        let rail = info.rail_type.unwrap_or(RailType::PoweredRail);
        if !cart::action(rail, info.powered, other.ridden).disassembles() {
            return;
        }
    } else {
        CARTS.with_borrow_mut(|c| {
            if let Some(on) = c.mounted.get_mut(&nc.uuid) {
                on.m.turn.yaw = mounted::y_rot(cart::horizontal_direction(axis, backwards));
            }
        });
    }
    let Some(on) = CARTS.with_borrow_mut(|c| c.mounted.remove(&nc.uuid)) else {
        return;
    };
    let_go(lv, nc.uuid, nc.pos, on);
    if nc.kind == "minecraft:furnace_minecart"
        && let Some(minecart) = lv.host.minecart(nc.entity)
    {
        let v = nc.velocity;
        minecart.set_push(lv.host, [v[2], 0.0, v[0]]);
    }
}

/// A contraption leaves its cart (`stopRiding` → `disassemble`): it goes back into the world
/// where the cart is, and a coupling the contraption made goes.
fn let_go(lv: &mut ServerLevel<'_>, uuid: u128, at: [f64; 3], on: OnCart) {
    put_back(lv, at, &on);
    if on.coupled {
        CARTS.with_borrow_mut(|c| {
            if let Some(l) = contraption_link(c, uuid) {
                c.couplings.remove_connection(l.main, true);
                c.couplings.remove_connection(l.connected, false);
            }
        });
    }
}

/// `AbstractContraptionEntity.disassemble`: each block where the contraption's turn (to the
/// nearest quarter) puts it, above the block the cart is in (`addBlocksToWorld`); a block whose
/// place is taken drops. Anchors are not placed (`customBlockPlacement`).
fn put_back(lv: &mut ServerLevel<'_>, at: [f64; 3], on: &OnCart) {
    let m = &on.m;
    let origin = [
        at[0].floor() as i32,
        (at[1] + 0.5).floor() as i32,
        at[2].floor() as i32,
    ];
    let (quarter, blocks) = mounted::placed_back(&m.blocks, mounted::y_rot(m.initial), m.turn.yaw);
    for (o, s) in blocks {
        if is_anchor(s) {
            continue;
        }
        let Some(state) = super::car_state(s) else {
            continue;
        };
        let p = [origin[0] + o[0], origin[1] + o[1], origin[2] + o[2]];
        let turned = turned_state(&*lv.host, state, quarter, Axis::Y);
        let free = lv
            .block_state(p)
            .and_then(|h| lv.host.block_state_string(h))
            .is_some_and(|t| t == "minecraft:air" || t == "minecraft:cave_air");
        if free {
            lv.set_state(p, turned, update_all());
        } else if let Some(item) = lv.host.block_item(turned.block())
            && let Some(stack) = ItemStack::new(item, 1)
        {
            let c = p.map(|v| f64::from(v) + 0.5);
            let _ = lv.level.spawn_item(lv.host, c[0], c[1], c[2], stack, 10);
        }
    }
    CARTS.with_borrow_mut(|c| c.dirty = true);
}

// ---------------------------------------------------------------------------------------------
// Contraptions on carts, their items and couplings

/// The contraption's NBT, as its item and the save keep it (`Contraption.writeNBT` reduced to
/// what is ported: the blocks, `RotationMode`, `InitialOrientation`, and the save's turn and
/// `Coupling`).
fn contraption_tag(host: &dyn Host, on: &OnCart, with_turn: bool) -> Compound {
    let m = &on.m;
    let blocks = m
        .blocks
        .iter()
        .filter_map(|(o, s)| {
            let text = super::car_text(host, *s)?;
            Some(Tag::Compound(Compound::from([
                ("Pos".into(), Tag::IntArray(o.to_vec())),
                ("State".into(), Tag::String(text.as_str().into())),
            ])))
        })
        .collect();
    let mut c = Compound::new();
    c.insert("Blocks".into(), Tag::List(blocks));
    c.insert("RotationMode".into(), Tag::String(m.mode.name().into()));
    c.insert(
        "InitialOrientation".into(),
        Tag::String(
            super::super::kinetics::dir_name(m.initial)
                .to_uppercase()
                .as_str()
                .into(),
        ),
    );
    if with_turn {
        c.insert("Yaw".into(), Tag::Double(f64::from(m.turn.yaw).to_bits()));
        c.insert("Placed".into(), Tag::IntArray(vec![i32::from(m.placed)]));
        c.insert(
            "Coupling".into(),
            Tag::IntArray(vec![i32::from(on.coupled)]),
        );
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
    c.get(&key.into())
}

/// A contraption back from its NBT, in level `dim`; None without blocks.
fn contraption_of(host: &dyn Host, c: &Compound, dim: usize) -> Option<OnCart> {
    let Some(Tag::List(list)) = get(c, "Blocks") else {
        return None;
    };
    let blocks: Vec<([i32; 3], u32)> = list
        .iter()
        .filter_map(|t| {
            let Tag::Compound(b) = t else { return None };
            let p = ints(get(b, "Pos"));
            let s = super::car_of_text(host, &text(get(b, "State"))?)?;
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
                .find(|d| super::super::kinetics::dir_name(*d).eq_ignore_ascii_case(&v))
        })
        .unwrap_or(Dir::South);
    let mut m = Mounted::new(blocks, mode, initial);
    if let Some(yaw) = double(get(c, "Yaw")) {
        m.turn = mounted::Orientation::at(yaw as f32);
    }
    m.placed = ints(get(c, "Placed")).first() == Some(&1);
    let coupled = ints(get(c, "Coupling")).first() == Some(&1);
    Some(OnCart { dim, m, coupled })
}

/// `minecraft:custom_data` of a contraption item: `MINECRAFT_CONTRAPTION_DATA`.
fn item_contraption(stack: &ItemStack) -> Option<Compound> {
    let components = stack.components()?;
    let custom = components.as_nbt().get(&"minecraft:custom_data".into())?;
    match custom {
        Tag::Compound(c) => match c.get(&mounted::DATA_KEY.into()) {
            Some(Tag::Compound(d)) => Some(d.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `MinecartContraptionItem.useOn`: on a rail, a cart of the item's kind with its contraption,
/// facing the player's way (`createAtYaw`); elsewhere nothing (`FAIL`).
struct ContraptionItem;

#[mod_api::horizon_part]
impl ItemPart for ContraptionItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let player = host.player(event.player)?;
        let dimension = player.level(&*host)?.dimension().id(&*host)?;
        let lv = ServerLevel::new(host, &dimension)?;
        let item = event.stack.item().id(&*lv.host);
        let kind = cart::item_cart(&item)?;
        let Some((shape, ..)) = rail_at(&lv, event.pos) else {
            return Some(InteractionResult::Fail);
        };
        let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
        let facing = create_engine::place::horizontal_direction(yaw);
        let at = mounted::placed_cart(event.pos, Some(shape));
        let mut nbt = Compound::new();
        nbt.insert(
            "Rotation".into(),
            Tag::List(vec![
                Tag::Float(mounted::y_rot(facing).to_bits()),
                Tag::Float(0f32.to_bits()),
            ]),
        );
        let id = lv.host.summon(&dimension, kind, at, nbt)?;
        let data = item_contraption(event.stack);
        use_one(lv.host, player);
        let entity = lv.host.entity(id)?;
        let nc = Cart::read(lv.host, entity)?;
        let dim = lv.dim;
        let Some(mut on) = data.and_then(|c| contraption_of(&*lv.host, &c, dim)) else {
            return Some(InteractionResult::SuccessServer);
        };
        let m = on.m;
        on.m = Mounted::placed_at(m.blocks, m.mode, m.initial, mounted::y_rot(facing));
        on.coupled = false;
        CARTS.with_borrow_mut(|c| {
            c.known.insert(
                nc.uuid,
                Known {
                    entity,
                    pos: nc.pos,
                    prev: nc.pos,
                },
            );
            c.mounted.insert(nc.uuid, on);
            c.dirty = true;
        });
        Some(InteractionResult::SuccessServer)
    }
}

/// A player's click on an entity (`PlayerInteractEvent.EntityInteract`): a coupling on a cart
/// (`MinecartCouplingItem`), a wrench on a coupled cart (`onWrenchInteractOnMinecart`), else a
/// wrench on a cart carrying a contraption picks it up
/// (`wrenchCanBeUsedToPickUpMinecartContraptions`).
fn interact_entity(
    host: &mut dyn ServerHost,
    event: &mut EntityInteractEvent<'_>,
) -> Option<InteractionResult> {
    if !event.kind.ends_with("minecart") {
        return None;
    }
    let item = event.stack?.item().id(&*host);
    let coupling = item == "create:minecart_coupling";
    if !coupling && item != "create:wrench" {
        return None;
    }
    let player = host.players().acting_player()?;
    let dimension = player.level(&*host)?.dimension().id(&*host)?;
    let mut lv = ServerLevel::new(host, &dimension)?;
    let entity = lv.host.entity(event.entity as u32)?;
    let nc = Cart::read(lv.host, entity)?;
    if coupling {
        click_coupling(&mut lv, player, &nc);
        return Some(InteractionResult::SuccessServer);
    }
    // A wrench: couplings come off first.
    let removed = CARTS.with_borrow_mut(|c| {
        let r = c.couplings.decouple(nc.uuid);
        c.dirty |= r.is_some();
        r
    });
    if let Some(items) = removed {
        status(lv.host, Some(player), coupling::REMOVED);
        if player.is_creative(&*lv.host) != Some(true)
            && items > 0
            && let Some(stack) = item_stack(&*lv.host, "create:minecart_coupling", items)
        {
            let _ = player.give_stack(lv.host, stack);
        }
        return Some(InteractionResult::SuccessServer);
    }
    let on = CARTS.with_borrow(|c| c.mounted.get(&nc.uuid).cloned())?;
    let item = cart::contraption_item(&nc.kind)?;
    let mut out = item_stack(&*lv.host, item, 1)?;
    let tag = contraption_tag(&*lv.host, &on, false);
    let custom = Compound::from([(mounted::DATA_KEY.into(), Tag::Compound(tag))]);
    let components = Compound::from([("minecraft:custom_data".into(), Tag::Compound(custom))]);
    if out.set_components(components).is_err() {
        return None;
    }
    // `Inventory.placeItemBackInInventory`: into the inventory, the rest dropped at the player.
    if player.give_stack(lv.host, out.clone()).is_err() {
        let _ = player.drop_stack(lv.host, out, false);
    }
    CARTS.with_borrow_mut(|c| {
        c.mounted.remove(&nc.uuid);
        c.known.remove(&nc.uuid);
        c.dirty = true;
    });
    nc.entity.discard(lv.host);
    Some(InteractionResult::SuccessServer)
}

/// `onCouplingInteractOnMinecart` and `CouplingHandlerClient.onCartClicked`: a fully coupled
/// cart says so; otherwise the first cart clicked is selected and the second is coupled to it
/// (`tryToCoupleCarts`), the coupling used up.
fn click_coupling(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, nc: &Cart) {
    let uuid = player.uuid();
    if CARTS.with_borrow(|c| c.couplings.get(nc.uuid).is_fully_coupled()) {
        let line = coupling::Refused::TwoCouplingsMax
            .message()
            .unwrap_or_default();
        status(lv.host, Some(player), line);
        return;
    }
    let first = CARTS.with_borrow(|c| c.selected.get(&uuid).copied());
    let first = match first {
        Some(e) if e != nc.entity => Cart::read(lv.host, e),
        _ => None,
    };
    let Some(first) = first else {
        CARTS.with_borrow_mut(|c| c.selected.insert(uuid, nc.entity));
        return;
    };
    CARTS.with_borrow_mut(|c| c.selected.remove(&uuid));
    let distance = (0..3)
        .map(|k| (first.pos[k] - nc.pos[k]).powi(2))
        .sum::<f64>()
        .sqrt();
    let result = CARTS.with_borrow_mut(|c| {
        let r = c
            .couplings
            .try_couple(first.uuid, nc.uuid, distance, false, |_| true);
        if r.is_ok() {
            for k in [&first, nc] {
                c.known.insert(
                    k.uuid,
                    Known {
                        entity: k.entity,
                        pos: k.pos,
                        prev: k.pos,
                    },
                );
            }
            c.dirty = true;
        }
        r
    });
    match result {
        Ok(()) => use_one(lv.host, player),
        Err(e) => {
            if let Some(m) = e.message() {
                status(lv.host, Some(player), m);
            }
        }
    }
}

/// After a level's tick: assemblers handle their carts, carts not found yet are looked for
/// (every second), followed carts move their contraptions or, gone, put them back, and coupled
/// carts keep their length; the overworld keeps the save. One emptiness test while nothing is
/// on a cart.
fn level_tick(lv: &mut ServerLevel<'_>) {
    let overworld = lv.dimension == "minecraft:overworld";
    if overworld && !CARTS.with_borrow(|c| c.loaded) {
        load(lv);
    }
    if !CARTS.with_borrow(State::idle) {
        tick_stations(lv);
        let tick = lv.game_time();
        if tick % 20 == 0 {
            find_carts(lv);
        }
        let carts = follow(lv);
        tick_mounted(lv, &carts);
        tick_couplings(lv, &carts);
    }
    if overworld && CARTS.with_borrow(|c| c.dirty) {
        save(lv);
    }
}

/// Carts of saved contraptions and couplings not followed yet, found by UUID.
fn find_carts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let wanted: Vec<u128> = CARTS.with_borrow(|c| {
        let mounted = c
            .mounted
            .iter()
            .filter(|(u, on)| on.dim == dim && !c.known.contains_key(u))
            .map(|(u, _)| *u);
        let coupled = c
            .couplings
            .carts
            .keys()
            .filter(|u| !c.known.contains_key(u))
            .copied();
        mounted.chain(coupled).collect()
    });
    for uuid in wanted {
        let Some(entity) = lv.host.entity_by_uuid(EntityUuid(uuid)) else {
            continue;
        };
        if entity.level(&*lv.host) != Some(lv.level) {
            continue;
        }
        let Some(pos) = entity.position(&*lv.host) else {
            continue;
        };
        CARTS.with_borrow_mut(|c| {
            c.known.insert(
                uuid,
                Known {
                    entity,
                    pos,
                    prev: pos,
                },
            );
        });
    }
}

/// The followed carts of this level as they are now; a cart gone while its column stayed (it
/// broke) lets go of its contraption, which goes back into the world where it was
/// (`OrientedContraptionEntity.stopRiding` → `disassemble`), and of its couplings
/// (`handleKilledMinecart`).
fn follow(lv: &mut ServerLevel<'_>) -> HashMap<u128, Cart> {
    let followed: Vec<(u128, Known)> =
        CARTS.with_borrow(|c| c.known.iter().map(|(u, k)| (*u, *k)).collect());
    let mut out = HashMap::with_capacity(followed.len());
    for (uuid, k) in followed {
        match k.entity.level(&*lv.host) {
            Some(level) if level != lv.level => continue,
            None if !lv.is_loaded(k.pos.map(|v| v.floor() as i32)) => continue,
            _ => {}
        }
        match Cart::read(lv.host, k.entity) {
            Some(nc) => {
                CARTS.with_borrow_mut(|c| {
                    if let Some(e) = c.known.get_mut(&uuid) {
                        e.prev = e.pos;
                        e.pos = nc.pos;
                    }
                });
                out.insert(uuid, nc);
            }
            None => {
                let (on, drops) = CARTS.with_borrow_mut(|c| {
                    c.known.remove(&uuid);
                    let coupled = !c.couplings.get(uuid).is_empty();
                    let drops = c.couplings.killed(uuid);
                    let on = c.mounted.remove(&uuid);
                    c.dirty |= coupled || on.is_some();
                    (on, drops)
                });
                if let Some(on) = on {
                    put_back(lv, k.pos, &on);
                }
                for next in drops {
                    let other = CARTS.with_borrow(|c| c.known.get(&next).map(|k| k.pos));
                    let at = other.map_or(k.pos, |o| [0, 1, 2].map(|a| (o[a] + k.pos[a]) / 2.0));
                    if let Some(stack) = item_stack(&*lv.host, "create:minecart_coupling", 1) {
                        let _ = lv.level.spawn_item(lv.host, at[0], at[1], at[2], stack, 10);
                    }
                }
            }
        }
    }
    out
}

/// Each contraption follows its cart (`OrientedContraptionEntity.tickContraption`): it turns
/// toward the cart's motion by its mode, or on a coupling faces from one of its carts to the
/// other (`updateOrientation`), comes off on an activator rail (a rideable cart's powered one
/// ejects it, `Minecart.activateMinecart`; any lets go of a chest or furnace cart's,
/// `MinecartController.disassemble`) or when its coupling is gone (`removeConnection`), keeps
/// riders off, and keeps its furnace carts going (`powerFurnaceCartWithFuelFromStorage`).
fn tick_mounted(lv: &mut ServerLevel<'_>, carts: &HashMap<u128, Cart>) {
    let dim = lv.dim;
    let list: Vec<(u128, Option<coupling::Link>)> = CARTS.with_borrow(|c| {
        c.mounted
            .iter()
            .filter(|(u, on)| on.dim == dim && carts.contains_key(u))
            .map(|(u, on)| (*u, on.coupled.then(|| contraption_link(c, *u)).flatten()))
            .collect()
    });
    for (uuid, link) in list {
        let nc = &carts[&uuid];
        let Some(prev) = CARTS.with_borrow(|c| c.known.get(&uuid).map(|k| k.prev)) else {
            continue;
        };
        for rider in nc.entity.passengers(&*lv.host).unwrap_or_default() {
            rider.stop_riding(lv.host);
        }
        let rail = rail_under(lv, nc.pos);
        let eject = rail.is_some_and(|(_, activator, powered)| {
            activator && (powered || nc.kind != "minecraft:minecart")
        });
        let unlinked = CARTS.with_borrow(|c| c.mounted.get(&uuid).is_some_and(|on| on.coupled))
            && link.is_none();
        if eject || unlinked {
            if let Some(on) = CARTS.with_borrow_mut(|c| {
                c.dirty = true;
                c.mounted.remove(&uuid)
            }) {
                let_go(lv, uuid, nc.pos, on);
            }
            continue;
        }
        // On a coupling: both carts (`getCoupledCartsIfPresent`), the leading one first.
        let pair = link.and_then(|l| Some((carts.get(&l.main)?, carts.get(&l.connected)?)));
        let moved = [0, 1, 2].map(|k| nc.pos[k] - prev[k]);
        CARTS.with_borrow_mut(|c| {
            let Some(on) = c.mounted.get_mut(&uuid) else {
                return;
            };
            if on.coupled {
                if let Some((main, connected)) = pair {
                    let (yaw, pitch) =
                        coupling::coupled_orientation(main.pos, connected.pos, main.uuid == uuid);
                    on.m.turn.prev_yaw = on.m.turn.yaw;
                    on.m.turn.yaw = yaw;
                    on.m.turn.pitch = pitch;
                }
            } else {
                let size = mounted::x_size(&on.m.blocks);
                mounted::orient(
                    &mut on.m.turn,
                    on.m.mode,
                    nc.velocity,
                    moved,
                    rail.map(|r| r.0),
                    size,
                );
            }
        });
        match pair {
            Some((main, connected)) => {
                for c in [main, connected] {
                    fuel_furnace(lv, c);
                }
            }
            None if link.is_none() => fuel_furnace(lv, nc),
            None => {}
        }
    }
}

/// `powerFurnaceCartWithFuelFromStorage` for `cart`: a furnace cart on a rail with fuel left is
/// sent on at full speed (its motion made a unit vector; the rail holds it to its top speed).
/// Its fuel from the contraption's storage is left out (see the module's doc).
fn fuel_furnace(lv: &mut ServerLevel<'_>, cart: &Cart) {
    if cart.kind != "minecraft:furnace_minecart" {
        return;
    }
    if cart.fuel > 1 && rail_under(lv, cart.pos).is_some() {
        cart.entity
            .set_velocity(lv.host, mounted::normalize(cart.velocity));
    }
}

/// `MinecartSim2020.canAddMotion`: a furnace cart only while it has no push (stalling is not
/// ported).
fn can_add_motion(nc: &Cart) -> bool {
    nc.kind != "minecraft:furnace_minecart"
        || (nc.push[0].abs() < 1.0e-5 && nc.push[2].abs() < 1.0e-5)
}

/// `CouplingPhysics.tick`: each coupling whose carts are both here runs the soft step on their
/// motion, then the hard step pulls their positions back to the coupling's length; a cart whose
/// partner isn't loaded holds still (`MinecartController.tick`'s internal stall).
fn tick_couplings(lv: &mut ServerLevel<'_>, carts: &HashMap<u128, Cart>) {
    let mut pairs = CARTS.with_borrow(|c| {
        c.couplings
            .carts
            .values()
            .filter_map(|x| x.leading)
            .collect::<Vec<_>>()
    });
    pairs.sort_by_key(|l| (l.main, l.connected));
    for link in pairs {
        let (a, b, length) = (link.main, link.connected, link.length);
        let (Some(ca), Some(cb)) = (carts.get(&a), carts.get(&b)) else {
            // `internalStall`: the loaded cart stops while the other is missing.
            for u in [a, b] {
                if let Some(nc) = carts.get(&u) {
                    nc.entity.set_velocity(lv.host, [0.0; 3]);
                }
            }
            continue;
        };
        let body = |nc: &Cart| {
            let next = coupling::predict(nc.pos, nc.velocity);
            Body {
                pos: nc.pos,
                motion: nc.velocity,
                can_add_motion: can_add_motion(nc),
                max_speed: cart::RAIL_MAX_SPEED,
                rail: rail_under(lv, next).map(|r| r.0),
            }
        };
        let bodies = [body(ca), body(cb)];
        let mut motion = [ca.velocity, cb.velocity];
        if let Some(m) = coupling::soft_step(bodies, f64::from(length)) {
            ca.entity.set_velocity(lv.host, m[0]);
            cb.entity.set_velocity(lv.host, m[1]);
            motion = m;
        }
        let carrying = |u: u128| CARTS.with_borrow(|c| c.mounted.contains_key(&u));
        let hard = |nc: &Cart| coupling::Hard {
            pos: nc.pos,
            can_add_motion: can_add_motion(nc),
            max_speed: cart::RAIL_MAX_SPEED,
            vehicle: carrying(nc.uuid) || nc.ridden,
        };
        let moved = {
            let rails = |p: [i32; 3]| rail_at(lv, p).map(|r| r.0);
            coupling::hard_step([hard(ca), hard(cb)], f64::from(length), &rails)
        };
        for (k, nc) in [ca, cb].into_iter().enumerate() {
            let Some(m) = moved[k] else { continue };
            if m.off_rail {
                // Off its rail the cart can't be moved with collisions (see the module's
                // doc): only its motion is damped, as Create damps it after the move.
                nc.entity.set_velocity(lv.host, motion[k].map(|v| v * 0.95));
            } else {
                // On its rail the cart is put there (`moveCartAlongTrack`).
                nc.entity.teleport_to(lv.host, m.pos, None);
            }
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
            .fold(0u128, |acc, x| (acc << 32) | u128::from(*x as u32))
    })
}

/// The save: each contraption with its cart's UUID and level, each coupling from its leading
/// cart, each assembler's mode that isn't the default.
fn save(lv: &mut ServerLevel<'_>) {
    let (carts, couplings, stations) = CARTS.with_borrow_mut(|c| {
        c.dirty = false;
        let mut carts: Vec<(u128, usize, OnCart)> = c
            .mounted
            .iter()
            .map(|(u, on)| (*u, on.dim, on.clone()))
            .collect();
        carts.sort_by_key(|x| x.0);
        let mut couplings: Vec<coupling::Link> = c
            .couplings
            .carts
            .values()
            .filter_map(|x| x.leading)
            .collect();
        couplings.sort_by_key(|l| (l.main, l.connected));
        let mut stations: Vec<(usize, [i32; 3], Mode)> = c
            .stations
            .iter()
            .filter(|(_, s)| s.mode != Mode::Rotate)
            .map(|((d, p), s)| (*d, *p, s.mode))
            .collect();
        stations.sort_by_key(|s| (s.0, s.1));
        (carts, couplings, stations)
    });
    let carts: Vec<Tag> = carts
        .iter()
        .filter_map(|(u, d, on)| {
            let mut c = contraption_tag(&*lv.host, on, true);
            c.insert("Cart".into(), Tag::IntArray(uuid_ints(*u)));
            c.insert(
                "Dimension".into(),
                Tag::String(level_id(*d)?.as_str().into()),
            );
            Some(Tag::Compound(c))
        })
        .collect();
    let links: Vec<Tag> = couplings
        .iter()
        .map(|l| {
            Tag::Compound(Compound::from([
                ("Main".into(), Tag::IntArray(uuid_ints(l.main))),
                ("Connected".into(), Tag::IntArray(uuid_ints(l.connected))),
                ("Length".into(), Tag::Double(f64::from(l.length).to_bits())),
                (
                    "Contraption".into(),
                    Tag::IntArray(vec![i32::from(l.contraption)]),
                ),
            ]))
        })
        .collect();
    let stations: Vec<Tag> = stations
        .iter()
        .filter_map(|(d, p, m)| {
            Some(Tag::Compound(Compound::from([
                (
                    "Dimension".into(),
                    Tag::String(level_id(*d)?.as_str().into()),
                ),
                ("Pos".into(), Tag::IntArray(p.to_vec())),
                ("Mode".into(), Tag::String(m.name().into())),
            ])))
        })
        .collect();
    let data = Compound::from([
        ("Contraptions".into(), Tag::List(carts)),
        ("Couplings".into(), Tag::List(links)),
        ("Assemblers".into(), Tag::List(stations)),
    ]);
    if let Err(e) = SAVED.set(&mut *lv.host, Holder::Level, data) {
        crate::create_log!("[create] saving the minecart contraptions: {e:?}");
    }
}

/// The world opened: its contraptions, couplings and assembler modes back from the save; their
/// carts are found by UUID as they come in.
fn load(lv: &mut ServerLevel<'_>) {
    CARTS.with_borrow_mut(|c| c.loaded = true);
    let data = match SAVED.get_existing(&mut *lv.host, Holder::Level) {
        Ok(Some(data)) => data,
        Ok(None) => return,
        Err(e) => {
            crate::create_log!("[create] reading the minecart contraptions: {e:?}");
            return;
        }
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
    let mut mounted = HashMap::new();
    for c in list("Contraptions") {
        let (Some(uuid), Some(dim)) = (
            uuid_of(&ints(get(&c, "Cart"))),
            text(get(&c, "Dimension")).and_then(|d| level_index(&d)),
        ) else {
            continue;
        };
        if let Some(on) = contraption_of(&*lv.host, &c, dim) {
            mounted.insert(uuid, on);
        }
    }
    let mut couplings = Couplings::default();
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
        couplings.carts.entry(main).or_default().leading = Some(link);
        couplings.carts.entry(connected).or_default().connected = Some(link);
    }
    let mut stations = HashMap::new();
    for c in list("Assemblers") {
        let p = ints(get(&c, "Pos"));
        let (Some(dim), [x, y, z]) = (
            text(get(&c, "Dimension")).and_then(|d| level_index(&d)),
            p.as_slice(),
        ) else {
            continue;
        };
        stations.insert(
            (dim, [*x, *y, *z]),
            Station {
                mode: Mode::read(&text(get(&c, "Mode")).unwrap_or_default()),
                ..Station::default()
            },
        );
    }
    CARTS.with_borrow_mut(|c| {
        c.mounted = mounted;
        c.couplings = couplings;
        c.stations = stations;
    });
}

#[cfg(test)]
mod tests {
    use super::{uuid_ints, uuid_of};

    #[test]
    fn uuids_keep_their_bits_as_four_ints() {
        let u = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210u128;
        assert_eq!(
            uuid_ints(u),
            vec![
                0x0123_4567,
                0x89ab_cdef_u32 as i32,
                0xfedc_ba98_u32 as i32,
                0x7654_3210
            ]
        );
        assert_eq!(uuid_of(&uuid_ints(u)), Some(u));
    }
}
