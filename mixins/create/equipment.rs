// Create's equipment through the hooks (`create-engine`'s `equipment` holds the rules):
// backtanks filled by their shaft, their air kept on the item, the diving helmet's breathing,
// the diving boots' sinking and the extendo grip's reach and wear. Free when unused: a player's
// tick looks at their armour and hands by item pointer (the item's gear is cached per pointer),
// so nobody wearing or holding Create's gear costs a few compares; only backtanks with
// something to do tick.
use super::Game;
use super::create::{LATER, items_in, with_world};
use super::create_equipment_body as body;
use create_engine::equipment::{self as eq, DOUBLE_RANGE, Gear, SINGLE_RANGE, Tank};
use create_engine::world::dimension_index;
use create_engine::{Block, states};
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// Item pointers seen in gear slots and the gear each is: a tick compares pointers, never
    /// strings. Holding the `Arc` keeps its address from being reused by another item.
    static GEAR: RefCell<Vec<(Arc<str>, Gear)>> = const { RefCell::new(Vec::new()) };
    /// Players given an extendo grip's reach (`EXTENDO_MARKER`): empty while nobody holds one.
    static REACH: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
    /// Backtanks a player broke with air in them: their drop takes the air
    /// (`CopyComponentsFunction` of `BACKTANK_AIR`) at the level's next ticks.
    static DROPS: RefCell<Vec<Drop>> = const { RefCell::new(Vec::new()) };
}

struct Drop {
    dimension: String,
    pos: [i32; 3],
    item: &'static str,
    air: i32,
    capacity: i32,
    ticks: u8,
}

const GEAR_CACHE: usize = 32;

/// The gear a stack is, by its item's pointer once seen.
fn gear(stack: Option<&ItemStack>) -> Gear {
    let Some(stack) = stack else {
        return Gear::Other;
    };
    GEAR.with_borrow_mut(|cache| {
        if let Some((_, g)) = cache.iter().find(|(a, _)| Arc::ptr_eq(a, &stack.item)) {
            return *g;
        }
        let g = Gear::of(&stack.item);
        if cache.len() >= GEAR_CACHE {
            cache.remove(0);
        }
        cache.push((stack.item.clone(), g));
        g
    })
}

fn backtank_block(state: u32) -> Option<(Block, bool)> {
    states::index(state)?;
    with_world(|w| {
        w.table.get(state).and_then(|(b, i)| {
            matches!(b, Block::CopperBacktank | Block::NetheriteBacktank)
                .then_some((b, i.waterlogged))
        })
    })
    .flatten()
}

fn backtank_item(block: Block) -> &'static str {
    if block == Block::NetheriteBacktank {
        "create:netherite_backtank"
    } else {
        "create:copper_backtank"
    }
}

/// World close: forget what was seen.
pub(super) fn world_closed() {
    GEAR.with_borrow_mut(Vec::clear);
    REACH.with_borrow_mut(Vec::clear);
    DROPS.with_borrow_mut(Vec::clear);
}

/// The end of a player's server tick (`EntityTickEvent`, `LivingBreatheEvent`): the extendo
/// grip's reach and the diving helmet's air.
pub(super) fn player_tick(game: &mut Game<'_>) {
    extendo_reach(game);
    if let Gear::Helmet(fire_resistant) = gear(game.armor[0].as_ref()) {
        breathe(game, fire_resistant);
    }
}

/// The worn tanks (`BacktankUtil`'s supplier: every armour slot holding a backtank).
fn tanks(game: &Game<'_>) -> Vec<Tank> {
    (0..game.armor.len())
        .filter_map(|slot| {
            let stack = game.armor[slot].as_ref()?;
            match gear(Some(stack)) {
                Gear::Backtank(fire_resistant) => Some(Tank {
                    slot,
                    stored: body::stack_air(stack),
                    capacity: body::capacity(stack),
                    fire_resistant,
                }),
                _ => None,
            }
        })
        .collect()
}

/// `DivingHelmetItem.breatheUnderwater`: one air a second from the emptiest tank, and breath.
fn breathe(game: &mut Game<'_>, fire_resistant: bool) {
    let Some(b) = body::body(game) else {
        return;
    };
    let tanks = tanks(game);
    if tanks.is_empty() {
        return;
    }
    let breath = eq::breathe(
        Some(fire_resistant),
        b.in_lava,
        b.can_breathe,
        &tanks,
        game.world.game_time,
    );
    if let Some(slot) = breath.consume {
        consume_air(game, slot, 1);
    }
    if breath.refill {
        body::refill_air(game);
    }
}

/// `BacktankUtil.consumeAir` on the tank in armour slot `slot`, with its warnings.
fn consume_air(game: &mut Game<'_>, slot: usize, amount: i32) {
    let Some(stack) = game.armor[slot].as_mut() else {
        return;
    };
    let (after, warnings) = eq::consume_air(body::stack_air(stack), body::capacity(stack), amount);
    body::set_stack_air(stack, after);
    let Some(player) = game.native_acting_player() else {
        return;
    };
    for w in warnings {
        // sendWarning: DENY and STEAM at the player, then a subtitle under an empty title.
        let [x, y, z] = player.position;
        let (dim, name) = (&player.dimension, &player.name);
        let color = if w == eq::Warning::Depleted {
            "red"
        } else {
            "gold"
        };
        LATER.with_borrow_mut(|l| {
            l.push(format!(
                "execute in {dim} run playsound minecraft:block.note_block.bass players @a {x} {y} {z} 1 0.625"
            ));
            l.push(format!(
                "execute in {dim} run playsound create:steam neutral @a {x} {y} {z} 0.5 0.5"
            ));
            l.push(format!("title {name} times 10 40 10"));
            l.push(format!(
                "title {name} subtitle {{\"text\":\"\u{26A0} \",\"color\":\"{color}\",\"extra\":[{{\"translate\":\"{}\",\"color\":\"gray\"}}]}}",
                w.key()
            ));
            l.push(format!("title {name} title \"\""));
        });
    }
}

/// `BacktankUtil.canAbsorbDamage`: creative players pay nothing; else the emptiest worn tank
/// pays `cost` air, or nothing absorbs it.
fn absorb(game: &mut Game<'_>, uses_per_tank: i32) -> bool {
    if uses_per_tank == 0 || body::body(game).is_some_and(|b| b.creative) {
        return true;
    }
    let Some(first) = eq::with_air(&tanks(game)).first().copied() else {
        return false;
    };
    consume_air(game, first.slot, eq::absorb_cost(uses_per_tank));
    true
}

/// `holdingExtendoGripIncreasesRange`: +3 block reach with a grip in one hand, +5 with two.
fn extendo_reach(game: &mut Game<'_>) {
    let main =
        gear(game.inventory.get(game.selected).and_then(Option::as_ref)) == Gear::ExtendoGrip;
    let off = gear(game.offhand.as_ref()) == Gear::ExtendoGrip;
    if !main && !off && REACH.with_borrow(Vec::is_empty) {
        return;
    }
    let Some(player) = game.native_acting_player().map(|p| p.uuid) else {
        return;
    };
    let (single, dual) = eq::extendo_reach(main, off);
    if body::has_reach(game, SINGLE_RANGE.0) != single {
        body::set_reach(game, SINGLE_RANGE, single);
    }
    if body::has_reach(game, DOUBLE_RANGE.0) != dual {
        body::set_reach(game, DOUBLE_RANGE, dual);
    }
    REACH.with_borrow_mut(|r| {
        r.retain(|p| *p != player);
        if single || dual {
            r.push(player);
        }
    });
}

/// `findAndDamageExtendoGrip`: a block broken or placed, or a hit landed, with a grip in hand
/// wears it by one unless a worn backtank pays for it.
pub(super) fn wear_extendo(game: &mut Game<'_>) {
    let main =
        gear(game.inventory.get(game.selected).and_then(Option::as_ref)) == Gear::ExtendoGrip;
    if !main && gear(game.offhand.as_ref()) != Gear::ExtendoGrip {
        return;
    }
    if !absorb(game, eq::MAX_EXTENDO_GRIP_ACTIONS) {
        body::damage_hand(game, !main);
    }
}

/// A player breaks a block (`BreakEvent`): a grip wears; a backtank's drop keeps its air.
pub(super) fn destroy_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    canceled: bool,
) {
    if canceled {
        return;
    }
    wear_extendo(game);
    let Some((block, _)) = backtank_block(state.0) else {
        return;
    };
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let Some(tank) = with_world(|w| w.dims[dim].backtanks.get(&pos).copied()).flatten() else {
        return;
    };
    if tank.air == 0 && tank.capacity == 0 {
        return;
    }
    let drop = Drop {
        dimension: game.dim.id().to_owned(),
        pos,
        item: backtank_item(block),
        air: tank.air,
        capacity: tank.capacity,
        ticks: 0,
    };
    DROPS.with_borrow_mut(|d| d.push(drop));
}

/// A block placed from a stack (`BacktankBlock.setPlacedBy`): a backtank takes the stack's air
/// and Capacity level. Any placement wears a held grip (`EntityPlaceEvent`).
pub(super) fn place_block(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    state: BlockStateId,
) {
    wear_extendo(game);
    if backtank_block(state.0).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let (air, capacity) = (body::stack_air(stack), body::capacity(stack));
    with_world(|w| {
        if let Some(b) = w.dims[dim].backtanks.get_mut(&pos) {
            b.air = air;
            b.capacity = capacity;
            w.backtank_changed(dim, pos);
        }
    });
}

/// `BacktankBlock.useItemOn`: not sneaking, no block in the main hand and nothing on the chest,
/// the backtank goes on the chest with its air and the block is gone without drops.
pub(super) fn use_backtank(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    sneaking: bool,
    canceled: &mut bool,
) {
    if sneaking || game.armor[1].is_some() {
        return;
    }
    let held = game.inventory.get(game.selected).and_then(Option::as_ref);
    // A backtank in hand is Create's armour item there, not a block item.
    if held.is_some_and(|s| {
        !matches!(gear(Some(s)), Gear::Backtank(_))
            && game.items().get(&s.item).is_some_and(|i| i.block.is_some())
    }) {
        return;
    }
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let Some((block, waterlogged)) = backtank_block(state) else {
        return;
    };
    let tank = with_world(|w| w.dims[dim].backtanks.get(&pos).copied())
        .flatten()
        .unwrap_or_default();
    let mut stack = ItemStack::new(backtank_item(block), 1);
    body::set_stack_air(&mut stack, tank.air);
    body::set_capacity(&mut stack, tank.capacity);
    *canceled = true;
    let registry = game.pack.assets.registry;
    let fluid = if waterlogged {
        "minecraft:water"
    } else {
        "minecraft:air"
    };
    let Some(left) = registry.default_state(fluid) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    if !game.native_set_block(
        &dimension,
        pos,
        left,
        super::native_hooks::flags::UPDATE_ALL,
    ) {
        return;
    }
    game.armor[1] = Some(stack);
    let [x, y, z] = pos.map(|v| f64::from(v) + 0.5);
    LATER.with_borrow_mut(|l| {
        l.push(format!(
            "execute in {dimension} run playsound minecraft:entity.item.pickup players @a {x} {y} {z} 0.75 1"
        ))
    });
}

/// Before a server level's tick is solved: backtanks with something to do fill
/// (`BacktankBlockEntity.tick`), and broken ones' drops take their air.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    for (pos, speed) in with_world(|w| w.filling_backtanks(dim)).unwrap_or_default() {
        let filled = with_world(|w| {
            let f = w.dims[dim].backtanks.get_mut(&pos)?.tick(speed, false);
            if f.is_some() {
                w.backtank_changed(dim, pos);
            }
            f
        })
        .flatten();
        if filled.is_some_and(|f| f.full) {
            // playFilledEffect: CONFIRM (a bell, 0.4 × 0.5 loud, pitch 0.8) where it stands.
            let [x, y, z] = pos.map(|v| f64::from(v) + 0.5);
            game.native_run_command(&format!(
                "execute in {dimension} run playsound minecraft:block.note_block.bell players @a {x} {y} {z} 0.2 0.8"
            ));
        }
    }
    if DROPS.with_borrow(Vec::is_empty) {
        return;
    }
    let drops = DROPS.with_borrow_mut(std::mem::take);
    let mut left = Vec::new();
    for mut d in drops {
        if d.dimension != dimension {
            left.push(d);
            continue;
        }
        let lo = d.pos.map(|v| f64::from(v) - 0.5);
        let hi = d.pos.map(|v| f64::from(v) + 1.5);
        let mut done = false;
        items_in(game, dimension, lo, hi, |_, stack, _, _| {
            if !done && &*stack.item == d.item && body::stack_air(stack) == 0 {
                body::set_stack_air(stack, d.air);
                body::set_capacity(stack, d.capacity);
                done = true;
            }
        });
        d.ticks += 1;
        if !done && d.ticks < 3 {
            left.push(d);
        }
    }
    DROPS.with_borrow_mut(|d| d.extend(left));
}

/// Backtanks saved with a column (`BacktankBlockEntity.read`).
pub(super) fn chunk_load(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let saved: Vec<([i32; 3], eq::Backtank)> =
        match Game::native_chunk_data(chunk, KEY).and_then(|d| d.get(&"backtanks".into())) {
            Some(horizon_world::nbt::Tag::IntArray(v)) => v
                .chunks_exact(6)
                .map(|c| {
                    let b = eq::Backtank {
                        air: c[3].max(0),
                        timer: c[4].clamp(0, 20),
                        capacity: c[5].max(0),
                    };
                    ([c[0], c[1], c[2]], b)
                })
                .collect(),
            _ => return,
        };
    with_world(|w| w.load_backtanks(dim, saved.into_iter()));
}

/// The column's own data key: Create's equipment state beside the rest of Create's.
const KEY: &str = "create_equipment";

/// A chunk is saved (`BacktankBlockEntity.write`): x, y, z, `Air`, `Timer` and
/// `CapacityEnchantment` of each backtank holding air.
pub(super) fn chunk_data_save(
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let tanks = with_world(|w| w.column_backtanks(dim, chunk[0], chunk[1])).unwrap_or_default();
    if tanks.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    let v: Vec<i32> = tanks
        .iter()
        .flat_map(|(p, b)| [p[0], p[1], p[2], b.air, b.timer, b.capacity])
        .collect();
    let mut own = horizon_world::nbt::Compound::new();
    own.insert("backtanks".into(), horizon_world::nbt::Tag::IntArray(v));
    data.insert(KEY.into(), horizon_world::nbt::Tag::Compound(own));
}

/// The window's tick (`DivingBootsItem.accelerateDescentUnderwater`, run from the tick's end
/// as the next tick's start): heavy boots sink their wearer in water. Its client moves the
/// player, so this is the client's.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if game.armor[3].is_none() || !game.native_window() {
        return;
    }
    if !matches!(gear(game.armor[3].as_ref()), Gear::Boots(_)) {
        return;
    }
    let Some((diver, in_water, swimming, flying)) = body::diver(game) else {
        return;
    };
    if !eq::boots_affect(true, in_water, swimming, flying) {
        return;
    }
    let (motion, on_ground) = eq::accelerate_descent(&diver);
    body::set_dive(game, motion, on_ground);
}
