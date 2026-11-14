//! Create's equipment (`create_engine::equipment` holds the rules): backtanks filled by their
//! shaft, their air kept on the item, the diving helmet's breathing and the extendo grip's reach
//! and wear. Free when unused: a player's tick reads their armour and hands and looks each item
//! up once (the gear each item is, cached by key), and only backtanks with something to do tick.
//!
//! Left out, for want of the API: the diving boots' sinking (`DivingBootsItem
//! .accelerateDescentUnderwater`), which moves the local player on its client from its movement
//! input: a client can neither read the local player's input (`xxa`, `zza`) nor set its motion
//! (`set_velocity` is the server's). The extendo grip wears on a landed hit only when the grip is
//! the stack that hit (`hurt_enemy` is a part of the hitting item), not when it is in the off hand
//! behind another weapon.

use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::{level_index, with_world};
use create_engine::Block;
use create_engine::equipment::{
    self as eq, AIR_KEY, CAPACITY, DOUBLE_RANGE, Gear, MAX_AIR_SUPPLY, SINGLE_RANGE, Tank,
};
use mod_api::{
    AttributeModifier, AttributeOperation, BlockDropsEvent, BreakBlockEvent, Compound,
    EntityPlaceEvent, EquipmentSlot, Flow, GameMode, Hand, HostSide, HurtEnemyEvent, Identifier,
    ItemKey, ItemPart, ItemStack, LevelHost, LivingRef, ModifierLifetime, OnBlockPlacedByEvent,
    PlayerRef, PlayerTickPostEvent, Registrar, RegistrationContext, ServerHost, SoundSource, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The column's own data key: Create's equipment state beside the rest of Create's.
const KEY: &str = "create_equipment";

const BLOCK_RANGE: &str = "minecraft:block_interaction_range";

/// The armour slots in the legacy order the engine's `Tank::slot` counts (head first).
const ARMOR: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];

/// The copper and netherite backtanks' item keys, requested at registration.
static BACKTANK_ITEMS: std::sync::OnceLock<[ItemKey; 2]> = std::sync::OnceLock::new();

thread_local! {
    /// The gear each item seen in a gear slot is.
    static GEAR: RefCell<HashMap<ItemKey, Gear>> = RefCell::new(HashMap::new());
    /// Players given an extendo grip's reach (`EXTENDO_MARKER`): empty while nobody holds one.
    static REACH: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
    /// Backtanks a player is breaking with air in them, by level and position: their drop takes
    /// the air (`CopyComponentsFunction` of `BACKTANK_AIR`).
    static BREAKING: RefCell<HashMap<(usize, [i32; 3]), eq::Backtank>> = RefCell::new(HashMap::new());
}

pub struct Equipment;

impl Subsystem for Equipment {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let keys = [
            r.item_key("create:copper_backtank"),
            r.item_key("create:netherite_backtank"),
        ];
        for key in keys {
            r.item_part(key, PlacedBacktank);
        }
        let _ = BACKTANK_ITEMS.set(keys);
        let grip = r.item_key("create:extendo_grip");
        r.item_part(grip, GripHit);
    }

    fn world_closed(&self) {
        GEAR.with_borrow_mut(HashMap::clear);
        REACH.with_borrow_mut(Vec::clear);
        BREAKING.with_borrow_mut(HashMap::clear);
    }

    /// Backtanks saved with a column (`BacktankBlockEntity.read`).
    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        let Some(Tag::Compound(own)) = data.get(&KEY.into()) else {
            return;
        };
        let Some(Tag::IntArray(v)) = own.get(&"backtanks".into()) else {
            return;
        };
        let saved: Vec<([i32; 3], eq::Backtank)> = v
            .as_chunks::<6>()
            .0
            .iter()
            .map(|c| {
                let b = eq::Backtank {
                    air: c[3].max(0),
                    timer: c[4].clamp(0, 20),
                    capacity: c[5].max(0),
                };
                ([c[0], c[1], c[2]], b)
            })
            .collect();
        let dim = lv.dim;
        with_world(|w| w.load_backtanks(dim, saved.into_iter()));
    }

    /// `BacktankBlockEntity.write`: x, y, z, `Air`, `Timer` and `CapacityEnchantment` of each
    /// backtank holding air.
    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let dim = lv.dim;
        let tanks = with_world(|w| w.column_backtanks(dim, chunk[0], chunk[1])).unwrap_or_default();
        if tanks.is_empty() {
            return;
        }
        let v: Vec<i32> = tanks
            .iter()
            .flat_map(|(p, b)| [p[0], p[1], p[2], b.air, b.timer, b.capacity])
            .collect();
        let own = Compound::from([("backtanks".into(), Tag::IntArray(v))]);
        data.insert(KEY.into(), Tag::Compound(own));
    }

    /// Before the solve: backtanks with something to do fill (`BacktankBlockEntity.tick`).
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        let dim = lv.dim;
        for (pos, speed) in with_world(|w| w.filling_backtanks(dim)).unwrap_or_default() {
            let filled = with_world(|w| {
                let f = w.dims[dim].backtanks.get_mut(&pos)?.tick(speed, false);
                if f.is_some() {
                    w.backtank_changed(dim, pos);
                }
                // `if (getComparatorOutput() != prevComparatorLevel)
                // updateNeighbourForOutputSignal`.
                if f.is_some_and(|f| f.comparator_changed) {
                    w.dims[dim].analog_checks.push(pos);
                }
                f
            })
            .flatten();
            if filled.is_some_and(|f| f.full) {
                // playFilledEffect: CONFIRM (a bell, 0.4 × 0.5 loud, pitch 0.8) where it stands.
                let at = pos.map(|v| f64::from(v) + 0.5);
                lv.level.play_sound(
                    &mut *lv.host,
                    "minecraft:block.note_block.bell",
                    at,
                    SoundSource::Players,
                    0.2,
                    0.8,
                );
            }
        }
    }

    /// The end of a player's server tick (`EntityTickEvent`, `LivingBreatheEvent`): the extendo
    /// grip's reach and the diving helmet's air.
    fn player_tick(&self, host: HostSide, event: &mut PlayerTickPostEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let player = event.player;
        extendo_reach(host, player);
        let Some(living) = host.living(player.entity()) else {
            return;
        };
        if let Gear::Helmet(fire_resistant) = gear(host, living.equipment(&*host, ARMOR[0])) {
            breathe(host, player, fire_resistant);
        }
    }

    /// A player breaks a block (`BreakEvent`): a grip wears; a backtank's drop keeps its air.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        if event.cancelled {
            return Flow::Continue;
        }
        let Some(player) = host.players().acting_player() else {
            return Flow::Continue;
        };
        wear_extendo(host, player);
        if backtank_block(event.state).is_none() {
            return Flow::Continue;
        }
        let Some(dim) = player_dim(&*host, player) else {
            return Flow::Continue;
        };
        let pos = event.pos;
        if let Some(tank) = with_world(|w| w.dims[dim].backtanks.get(&pos).copied()).flatten()
            && (tank.air != 0 || tank.capacity != 0)
        {
            BREAKING.with_borrow_mut(|b| b.insert((dim, pos), tank));
        }
        Flow::Continue
    }

    /// The broken backtank's drop takes its air and Capacity level.
    fn block_drops(&self, _level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) -> Flow {
        let Some((block, _)) = backtank_block(event.state) else {
            return Flow::Continue;
        };
        let Some(dim) = level_index(event.dimension.key().id()) else {
            return Flow::Continue;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let Some(tank) = BREAKING.with_borrow_mut(|b| b.remove(&(dim, pos))) else {
            return Flow::Continue;
        };
        let item = backtank_item(block);
        if let Some(drop) = event
            .drops
            .iter_mut()
            .find(|d| Some(d.stack.item()) == item)
        {
            set_stack_air(&mut drop.stack, tank.air);
            set_capacity(&mut drop.stack, tank.capacity);
        }
        Flow::Continue
    }

    /// Any placement wears a held grip (`EntityPlaceEvent`).
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        if event.cancelled {
            return Flow::Continue;
        }
        if let Some(placer) = event.placer
            && let Some(player) = placer.uuid(host).and_then(|u| host.player(u))
        {
            wear_extendo(host, player);
        }
        Flow::Continue
    }
}

/// `BacktankBlock.setPlacedBy`: a placed backtank takes the stack's air and Capacity level.
struct PlacedBacktank;

#[mod_api::horizon_part]
impl ItemPart for PlacedBacktank {
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        if backtank_block(event.state).is_none() {
            return;
        }
        let Some(dim) = event.placer.and_then(|p| player_dim(&*host, p)) else {
            return;
        };
        let (air, capacity) = (stack_air(event.stack), capacity(event.stack));
        let pos = event.pos;
        with_world(|w| {
            if let Some(b) = w.dims[dim].backtanks.get_mut(&pos) {
                b.air = air;
                b.capacity = capacity;
                w.backtank_changed(dim, pos);
                w.dims[dim].analog_checks.push(pos);
            }
        });
    }
}

/// A player's hit landed holding the grip: it wears.
struct GripHit;

#[mod_api::horizon_part]
impl ItemPart for GripHit {
    fn hurt_enemy(&self, host: &mut dyn ServerHost, _event: &mut HurtEnemyEvent<'_>) {
        if let Some(player) = host.players().acting_player() {
            wear_extendo(host, player);
        }
    }
}

fn player_dim(host: &dyn ServerHost, player: PlayerRef) -> Option<usize> {
    let dimension = player.dimension(host)?;
    level_index(&dimension.to_string())
}

/// The gear a stack is, by its item once seen.
fn gear(host: &dyn ServerHost, stack: Option<ItemStack>) -> Gear {
    let Some(stack) = stack else {
        return Gear::Other;
    };
    let key = stack.item();
    if let Some(g) = GEAR.with_borrow(|c| c.get(&key).copied()) {
        return g;
    }
    let g = Gear::of(&key.id(host));
    GEAR.with_borrow_mut(|c| c.insert(key, g));
    g
}

fn backtank_block(state: mod_api::BlockState) -> Option<(Block, bool)> {
    let id = states::id(state)?;
    with_world(|w| {
        w.table.get(id).and_then(|(b, i)| {
            matches!(b, Block::CopperBacktank | Block::NetheriteBacktank)
                .then_some((b, i.waterlogged))
        })
    })
    .flatten()
}

/// The item key of `block`'s item.
fn backtank_item(block: Block) -> Option<ItemKey> {
    let [copper, netherite] = *BACKTANK_ITEMS.get()?;
    Some(if block == Block::NetheriteBacktank {
        netherite
    } else {
        copper
    })
}

/// The air stored on a backtank stack (0 when none), in `minecraft:custom_data`.
fn stack_air(stack: &ItemStack) -> i32 {
    let data = stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()));
    match data {
        Some(Tag::Compound(c)) => match c.get(&AIR_KEY.into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

fn set_stack_air(stack: &mut ItemStack, air: i32) {
    let mut c = match stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()))
    {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    c.insert(AIR_KEY.into(), Tag::Int(air));
    let patch = Compound::from([("minecraft:custom_data".into(), Tag::Compound(c))]);
    let _ = stack.apply_components(&patch);
}

/// The stack's Capacity enchantment level (`getEnchantmentLevel(CAPACITY)`).
fn capacity(stack: &ItemStack) -> i32 {
    let levels = stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:enchantments".into()));
    match levels {
        Some(Tag::Compound(c)) => match c.get(&CAPACITY.into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

fn set_capacity(stack: &mut ItemStack, level: i32) {
    if level <= 0 {
        return;
    }
    let mut c = match stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:enchantments".into()))
    {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    c.insert(CAPACITY.into(), Tag::Int(level));
    let patch = Compound::from([("minecraft:enchantments".into(), Tag::Compound(c))]);
    let _ = stack.apply_components(&patch);
}

/// The worn tanks (`BacktankUtil`'s supplier: every armour slot holding a backtank).
fn tanks(host: &dyn ServerHost, living: LivingRef) -> Vec<Tank> {
    ARMOR
        .iter()
        .enumerate()
        .filter_map(|(slot, &s)| {
            let stack = living.equipment(host, s)?;
            let air = stack_air(&stack);
            let cap = capacity(&stack);
            match gear(host, Some(stack)) {
                Gear::Backtank(fire_resistant) => Some(Tank {
                    slot,
                    stored: air,
                    capacity: cap,
                    fire_resistant,
                }),
                _ => None,
            }
        })
        .collect()
}

/// `DivingHelmetItem.breatheUnderwater`: one air a second from the emptiest tank, and breath.
fn breathe(host: &mut dyn ServerHost, player: PlayerRef, fire_resistant: bool) {
    let entity = player.entity();
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let tanks = tanks(&*host, living);
    if tanks.is_empty() {
        return;
    }
    // `!isEyeInFluid(WATER) || hasEffect(WATER_BREATHING) || abilities.invulnerable`.
    let eye_in_water = entity
        .eye_fluid(&*host)
        .flatten()
        .is_some_and(|f| f == "minecraft:water" || f == "minecraft:flowing_water");
    let water_breathing = host
        .effect_key("minecraft:water_breathing")
        .is_some_and(|e| living.effect(&*host, e).is_some());
    let invulnerable = matches!(
        player.game_mode(&*host),
        Some(GameMode::Creative | GameMode::Spectator)
    );
    let in_lava = entity.is_in_lava(&*host).unwrap_or(false);
    let now = player
        .level(&*host)
        .and_then(|l| l.game_time(&*host))
        .unwrap_or(0);
    let breath = eq::breathe(
        Some(fire_resistant),
        in_lava,
        !eye_in_water || water_breathing || invulnerable,
        &tanks,
        now,
    );
    if let Some(slot) = breath.consume {
        consume_air(host, player, slot, 1);
    }
    if breath.refill {
        // `setRefillAirAmount(getMaxAirSupply())`.
        entity.set_air_supply(host, MAX_AIR_SUPPLY);
    }
}

/// `BacktankUtil.consumeAir` on the tank in armour slot `slot`, with its warnings.
fn consume_air(host: &mut dyn ServerHost, player: PlayerRef, slot: usize, amount: i32) {
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let Some(mut stack) = living.equipment(&*host, ARMOR[slot]) else {
        return;
    };
    let (after, warnings) = eq::consume_air(stack_air(&stack), capacity(&stack), amount);
    set_stack_air(&mut stack, after);
    let _ = living.set_equipment(host, ARMOR[slot], Some(stack));
    if warnings.is_empty() {
        return;
    }
    let Some(name) = player.name(&*host) else {
        return;
    };
    for w in warnings {
        // sendWarning: DENY and STEAM at the player, then a subtitle under an empty title.
        if let (Some(level), Some(at)) = (player.level(&*host), player.entity().position(&*host)) {
            level.play_sound(
                host,
                "minecraft:block.note_block.bass",
                at,
                SoundSource::Players,
                1.0,
                0.625,
            );
            level.play_sound(host, "create:steam", at, SoundSource::Neutral, 0.5, 0.5);
        }
        let color = if w == eq::Warning::Depleted {
            "red"
        } else {
            "gold"
        };
        let commands = host.commands();
        commands.run_command(&format!("title {name} times 10 40 10"));
        commands.run_command(&format!(
            "title {name} subtitle {{\"text\":\"\u{26A0} \",\"color\":\"{color}\",\"extra\":[{{\"translate\":\"{}\",\"color\":\"gray\"}}]}}",
            w.key()
        ));
        commands.run_command(&format!("title {name} title \"\""));
    }
}

/// `BacktankUtil.canAbsorbDamage`: creative players pay nothing; else the emptiest worn tank
/// pays `cost` air, or nothing absorbs it.
fn absorb(host: &mut dyn ServerHost, player: PlayerRef, uses_per_tank: i32) -> bool {
    if uses_per_tank == 0 || player.is_creative(&*host) == Some(true) {
        return true;
    }
    let Some(living) = host.living(player.entity()) else {
        return false;
    };
    let tanks = tanks(&*host, living);
    let Some(first) = eq::with_air(&tanks).first().copied() else {
        return false;
    };
    consume_air(host, player, first.slot, eq::absorb_cost(uses_per_tank));
    true
}

/// The gear in each hand.
fn hands(host: &dyn ServerHost, living: LivingRef) -> (Gear, Gear) {
    (
        gear(host, living.hand_item(host, Hand::Main)),
        gear(host, living.hand_item(host, Hand::Off)),
    )
}

/// `holdingExtendoGripIncreasesRange`: +3 block reach with a grip in one hand, +5 with two.
fn extendo_reach(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let (main, off) = hands(&*host, living);
    let (main, off) = (main == Gear::ExtendoGrip, off == Gear::ExtendoGrip);
    let uuid = player.uuid();
    if !main && !off && !REACH.with_borrow(|r| r.contains(&uuid)) {
        return;
    }
    let (single, dual) = eq::extendo_reach(main, off);
    for (modifier, on) in [(SINGLE_RANGE, single), (DOUBLE_RANGE, dual)] {
        let Ok(id) = Identifier::parse(modifier.0) else {
            continue;
        };
        let has = living
            .attribute_modifiers(&*host, BLOCK_RANGE)
            .is_some_and(|m| m.iter().any(|m| m.id == id));
        if has == on {
            continue;
        }
        if on {
            let m = AttributeModifier::new(id, modifier.1, AttributeOperation::AddValue);
            living.add_attribute_modifier(host, BLOCK_RANGE, m, ModifierLifetime::Transient);
        } else {
            living.remove_attribute_modifier(host, BLOCK_RANGE, &id);
        }
    }
    REACH.with_borrow_mut(|r| {
        r.retain(|p| *p != uuid);
        if single || dual {
            r.push(uuid);
        }
    });
}

/// `findAndDamageExtendoGrip`: a block broken or placed, or a hit landed, with a grip in hand
/// wears it by one unless a worn backtank pays for it.
fn wear_extendo(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let (main, off) = hands(&*host, living);
    let main = main == Gear::ExtendoGrip;
    if !main && off != Gear::ExtendoGrip {
        return;
    }
    if !absorb(host, player, eq::MAX_EXTENDO_GRIP_ACTIONS) {
        // `hurtAndBreak(1)` (Unbreaking applies; none in creative).
        let slot = if main {
            EquipmentSlot::MainHand
        } else {
            EquipmentSlot::OffHand
        };
        living.hurt_and_break(host, slot, 1);
    }
}
