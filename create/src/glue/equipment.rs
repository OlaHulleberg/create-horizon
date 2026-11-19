//! Create's equipment (`create_engine::equipment` holds the rules): backtanks filled by their
//! shaft, their air kept on the item (and shown as its bar, and on a potato cannon's while it
//! uses it: `backtank_bar`), the diving helmet's breathing, the extendo grip's reach (blocks and,
//! as Create's client pick and `ExtendoGripInteractionPacket` give, entities as far), its +2
//! knockback and its wear on any damage its holder deals, paid with backtank air when there is
//! some. The grip lends the game's entity interaction range the reach Create picks to (the
//! block reach, half a block less out of creative). On the pick used by clicks, a block still
//! wins over an entity hit that needed the added reach, as in Create's supplemental ray.
//! A whole netherite diving set makes its wearer immune to fire (`NetheriteDivingHandler`), and a
//! diving helmet sees further under water, the netherite one in lava (`getFogDensity`). The
//! extendo grip is drawn by a special item model (`GripModel`) posed as Create's: an item model
//! node of the glue's (`create:extendo_grip_pose`) picks, by where it is drawn and the game time,
//! one of the poses `create-data prepare` lists (the cog turned to a tenth of its 45° repeat,
//! the swing in eight steps, the hand pointing, punching or holding). `render_hand` supplies
//! the actual main-hand swing and identifies the hand showing the grip.
//! Free when unused: a player's tick reads their armour and hands and looks each item up once
//! (the gear each item is, cached by key), and only backtanks with something to do tick.
//!
//! The diving boots sink the local player on its client (`DivingBootsItem
//! .accelerateDescentUnderwater`, from its movement input and pushes to its velocity); their
//! hold on swimming (`HeavyBootsOnPlayerMixin`: `isUnderWater` false, so no sprint-swimming)
//! is left out, and the ground (`onGround || verticalCollision`, which a client can't read for
//! its own player) is a block shape just under or over the player's box.
//!
//! Left out, for want of the API: the helmet keeps the wearer's air full on the server only: a client
//! cannot set its own player's air (`set_air_supply` is the server's), and the server's value
//! never changes, so the client's bubbles still run down (no damage is taken). The Capacity
//! enchantment is read from a stack's components, but the game's item enchantment table does not
//! hold a native mod's enchantments, so it is not offered by enchanting tables nor shown in tooltips.
//! A placed backtank is put on by a click with the chest free (`BacktankBlock.useItemOn`).

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
    EntityPlaceEvent, EntityRef, EquipmentSlot, Flow, GameMode, Hand, HostSide, HostTransaction,
    Identifier, ItemKey, ItemPart, ItemPlace, ItemStack, LevelHost, LivingRef, ModifierLifetime,
    OnBlockPlacedByEvent, PlayerRef, PlayerTickPostEvent, Registrar, RegistrationContext,
    ServerHost, SoundSource, StackError, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The column's own data key: Create's equipment state beside the rest of Create's.
/// A backtank's air a moving structure carries (`Subsystem::carry_out`).
const CARRIED_AIR: &str = "BacktankAir";
const KEY: &str = "create_equipment";

const BLOCK_RANGE: &str = "minecraft:block_interaction_range";
const ENTITY_RANGE: &str = "minecraft:entity_interaction_range";
/// Vanilla's entity interaction range bounds (`RangedAttribute::sanitizeValue`).
/// The pinned SDK has no read for the registered bounds.
const ENTITY_RANGE_BOUNDS: (f64, f64) = (0.0, 64.0);
/// The port's entity reach for a held grip (`dontMissEntitiesWhenYouHaveHighReachDistance`).
static ENTITY_REACH: std::sync::LazyLock<Identifier> = std::sync::LazyLock::new(|| {
    Identifier::parse("create:extendo_entity_range").expect("valid grip reach identifier")
});

/// The extendo grip's item key, requested at registration.
static GRIP_ITEM: std::sync::OnceLock<ItemKey> = std::sync::OnceLock::new();

/// The armour slots in the order the engine's `Tank::slot` counts (head first).
const ARMOR: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];
/// `ItemPlace::PlayerSlot` numbers the four armour slots from feet (100) to head.
const HEAD_INVENTORY_SLOT: i32 = 103;

/// The copper and netherite backtanks' item keys, requested at registration.
static BACKTANK_ITEMS: std::sync::OnceLock<[ItemKey; 2]> = std::sync::OnceLock::new();

thread_local! {
    /// The gear each item seen in a gear slot is.
    static GEAR: RefCell<HashMap<ItemKey, Gear>> = RefCell::new(HashMap::new());
    /// Players given an extendo grip's reach (`EXTENDO_MARKER`): empty while nobody holds one.
    static REACH: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
    /// The players (by entity id) whose damage an entity took this tick
    /// (`bufferLivingAttackEvent`): a grip they hold wears.
    static HITTERS: RefCell<Vec<(i32, u8)>> = const { RefCell::new(Vec::new()) };
    /// Players wearing a whole netherite diving set, by entity id: fire can't hurt them
    /// (`NetheriteDivingHandler.setFireImmune`).
    static SERVER_FIRE_GRANTS: RefCell<HashMap<u128, EntityRef>> = RefCell::new(HashMap::new());
    static CLIENT_FIRE_GRANT: std::cell::Cell<Option<EntityRef>> = const { std::cell::Cell::new(None) };
    /// Backtanks a player is breaking with air in them, by level and position: their drop takes
    /// the air (`CopyComponentsFunction` of `BACKTANK_AIR`).
    static BREAKING: RefCell<HashMap<(usize, [i32; 3]), eq::Backtank>> = RefCell::new(HashMap::new());
}

pub struct Equipment;

thread_local! {
    /// The local player's last movement input: forward, strafe, jump, sneak.
    static INPUT: std::cell::Cell<(f32, f32, bool, bool)> =
        const { std::cell::Cell::new((0.0, 0.0, false, false)) };
}

/// `DivingBootsItem.accelerateDescentUnderwater` for the local player, on its client (which
/// moves it): in water, not swimming or flying, it sinks 0.05 a tick faster, jumps 0.5 off the
/// ground, and walks 1.3 times as fast while slower than 0.145.
fn diving_boots(host: &mut dyn mod_api::ClientHost, player: PlayerRef) {
    if host.local_player() != Some(player) {
        return;
    }
    let h: &dyn mod_api::Host = &*host;
    let boots = h
        .living(player.entity())
        .and_then(|l| l.equipment(h, EquipmentSlot::Feet))
        .is_some_and(|s| matches!(gear_of(h, &s), Gear::Boots(_)));
    if !boots {
        return;
    }
    let e = player.entity();
    let in_water = e
        .fluid_heights(h)
        .is_some_and(|f| f.iter().any(|(id, _)| id == "minecraft:water"));
    if !in_water || e.pose(h) == Some("swimming") || player.abilities(h).is_some_and(|a| a.flying) {
        return;
    }
    let Some(v) = e.velocity(h) else {
        return;
    };
    // `onGround() || verticalCollision`: the client has no ground flag to read for its own
    // player, so a block shape just under or over its box counts.
    let on_ground = e.bounding_box(h).is_some_and(|[lo, hi]| {
        let level = h.level(h.dimension());
        let touches = |y0: f64, y1: f64| {
            level.is_some_and(|l| {
                l.has_collisions(h, [lo[0], y0, lo[2], hi[0], y1, hi[2]]) == Some(true)
            })
        };
        touches(lo[1] - 1.0e-3, lo[1]) || touches(hi[1], hi[1] + 1.0e-3)
    });
    let (forward, strafe, jump, sneak) = INPUT.get();
    let mut push = [0.0f32; 3];
    push[1] = if jump && on_ground { 0.5 } else { -0.05 };
    if v[0].hypot(v[2]) < 0.145 && (forward > 0.0 || strafe != 0.0) && !sneak {
        push[0] = (v[0] * 0.3) as f32;
        push[2] = (v[2] * 0.3) as f32;
    }
    host.push_player(push);
}

impl Subsystem for Equipment {
    /// `BacktankBlock.useItemOn`: a player not sneaking, with no block in the main hand and
    /// nothing worn on the chest, puts a placed backtank on (`getCloneItemStack`: its air and
    /// Capacity kept), with an item pickup sound; the block goes without drops.
    fn right_click_block(
        &self,
        host: HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        let HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        let click = event.click;
        let Some(player) = host.player(click.player) else {
            return Flow::Continue;
        };
        let dimension = click.dimension.key().id().to_owned();
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Continue;
        };
        let pos = click.pos;
        let Some((block, _)) = lv.block_state(pos).and_then(backtank_block) else {
            return Flow::Continue;
        };
        let Some(living) = lv.host.living(player.entity()) else {
            return Flow::Continue;
        };
        let host = &*lv.host;
        let sneaking = player.entity().is_shift_key_down(host) == Some(true);
        let block_in_hand = living
            .hand_item(host, Hand::Main)
            .is_some_and(|s| host.item_block(s.item()).is_some());
        let worn = living.equipment(host, EquipmentSlot::Chest).is_some();
        if sneaking || block_in_hand || worn {
            return Flow::Continue;
        }
        let dim = lv.dim;
        let tank = with_world(|w| w.dims[dim].backtanks.get(&pos).copied()).flatten();
        let Some(mut stack) = backtank_item(block).and_then(|k| ItemStack::new(k, 1)) else {
            return Flow::Continue;
        };
        if let Some(tank) = tank {
            if let Err(error) = set_stack_air(&mut stack, tank.air) {
                crate::create_log!(
                    "[create] picked-up backtank air could not be written: {error:?}"
                );
            }
            set_capacity(&mut stack, tank.capacity);
        }
        let at = pos.map(|v| f64::from(v) + 0.5);
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:entity.item.pickup",
            at,
            SoundSource::Players,
            0.75,
            1.0,
            None,
        );
        let _ = living.set_equipment(lv.host, EquipmentSlot::Chest, Some(stack));
        lv.destroy_block(pos, false);
        *event.cancellation_result = mod_api::InteractionResult::Success;
        Flow::Cancel
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let keys = [
            r.item_key("create:copper_backtank"),
            r.item_key("create:netherite_backtank"),
        ];
        for key in keys {
            r.item_part(key, PlacedBacktank);
        }
        let _ = BACKTANK_ITEMS.set(keys);
        let _ = GRIP_ITEM.set(r.item_key("create:extendo_grip"));
    }

    fn world_closed(&self) {
        GEAR.with_borrow_mut(HashMap::clear);
        REACH.with_borrow_mut(Vec::clear);
        HITTERS.with_borrow_mut(Vec::clear);
        SERVER_FIRE_GRANTS.with_borrow_mut(HashMap::clear);
        BREAKING.with_borrow_mut(HashMap::clear);
    }

    /// A backtank's air (`BacktankBlockEntity.write`) rides with it.
    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _b: create_engine::Block,
        out: &mut Compound,
    ) {
        let dim = lv.dim;
        if let Some(b) = with_world(|w| w.dims[dim].backtanks.remove(&pos)).flatten() {
            out.insert(
                CARRIED_AIR.into(),
                Tag::IntArray(vec![b.air, b.timer, b.capacity]),
            );
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _b: create_engine::Block,
        data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
        if let Some(Tag::IntArray(v)) = data.get(&CARRIED_AIR.into())
            && let [air, timer, capacity] = v[..]
        {
            let b = eq::Backtank {
                air: air.max(0),
                timer: timer.clamp(0, 20),
                capacity: capacity.max(0),
            };
            let dim = lv.dim;
            with_world(|w| w.load_backtanks(dim, std::iter::once((pos, b))));
        }
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
        // A hitter not found by its player tick within two ticks was no player (a mob's hit).
        if lv.dim == 0 {
            HITTERS.with_borrow_mut(|h| {
                h.retain_mut(|(_, age)| {
                    *age += 1;
                    *age <= 2
                })
            });
        }
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
                    None,
                );
            }
        }
    }

    /// `ExtendoGripRenderHandler.tick`: retract the grip and refresh its block-holding pose.
    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        let hold = host.local_player().is_some_and(|p| {
            let off = super::items::held(host, p, Hand::Off);
            let main = super::items::held(host, p, Hand::Main);
            off.is_some_and(|s| matches!(gear_of(host, &s), Gear::ExtendoGrip))
                && main.is_some_and(|s| {
                    host.item_block(s.item()).is_some()
                        && host.item_uses_block_light(&s, mod_api::ItemDisplayContext::Gui)
                })
        });
        let mut grip = GRIP_RENDER.get();
        grip.swing.tick();
        grip.hold = hold;
        GRIP_RENDER.set(grip);
    }

    fn client_reset(&self) {
        GRIP_RENDER.set(GripRender::default());
        CLIENT_FIRE_GRANT.set(None);
    }

    /// The start of the local player's tick on its client: diving boots and fire prediction.
    fn player_tick_pre(&self, host: HostSide, event: &mut mod_api::PlayerTickPreEvent) {
        if let HostSide::Client(host) = host {
            let player = event.player;
            diving_boots(host, player);
            if host.local_player() != Some(player) {
                return;
            }
            let entity = player.entity();
            if full_netherite(host, player) {
                if (CLIENT_FIRE_GRANT.get() == Some(entity)
                    || entity.fire_immune(host) != Some(true))
                    && entity.set_client_fire_immunity(host, true)
                {
                    CLIENT_FIRE_GRANT.set(Some(entity));
                }
            } else if CLIENT_FIRE_GRANT.take() == Some(entity) {
                entity.set_client_fire_immunity(host, false);
            }
        }
    }

    /// The local player's movement input, for the diving boots (`zza`, `xxa`, `jumping`).
    fn movement_input(
        &self,
        _host: &dyn mod_api::ClientHost,
        event: &mut mod_api::MovementInputEvent,
    ) {
        INPUT.set((event.forward, event.strafe, event.jump, event.sneak));
    }

    /// The end of a player's server tick (`EntityTickEvent`, `LivingBreatheEvent`): the extendo
    /// grip's reach and the diving helmet's air.
    fn player_tick(&self, host: HostSide, event: &mut PlayerTickPostEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let player = event.player;
        extendo_reach(host, player);
        let id = player.entity().id() as i32;
        let hits = HITTERS.with_borrow_mut(|h| {
            let n = h.len();
            h.retain(|(e, _)| *e != id);
            n - h.len()
        });
        for _ in 0..hits {
            wear_extendo(host, player);
        }
        let Some(living) = host.living(player.entity()) else {
            return;
        };
        if let Gear::Helmet(fire_resistant) = gear(host, living.equipment(&*host, ARMOR[0])) {
            breathe(host, player, fire_resistant);
        }
        netherite_set(host, player);
    }

    /// `bufferLivingAttackEvent`: damage a player caused wears the grip they hold.
    fn living_incoming_damage(
        &self,
        _level: &mut dyn mod_api::HitHost,
        event: &mut mod_api::LivingIncomingDamageEvent<'_>,
    ) -> Flow {
        if event.source.causing != 0 {
            HITTERS.with_borrow_mut(|h| h.push((event.source.causing, 0)));
        }
        Flow::Continue
    }

    /// `attacksByExtendoGripHaveMoreKnockback`: a hit by a player (its direct entity) holding a
    /// grip in either hand pushes 2 harder.
    fn living_knockback(
        &self,
        _level: &mut dyn mod_api::LevelRandomHost,
        event: &mut mod_api::LivingKnockbackEvent<'_>,
    ) {
        let Some(grip) = GRIP_ITEM.get() else {
            return;
        };
        if event.attacker == 0 || event.source.direct != event.attacker {
            return;
        }
        let holds = [event.attacker_main, event.attacker_off]
            .iter()
            .any(|s| s.is_some_and(|s| s.item() == *grip));
        if holds {
            event.strength += 2.0;
        }
    }

    fn special_model_renderers(&self, renderers: &mut dyn mod_api::SpecialModelRenderers) {
        renderers.register(
            "create:extendo_grip".into(),
            mod_api::SpecialModelPolicy::Stack,
            std::sync::Arc::new(GripModel),
        );
    }

    /// `ClientEvents.getFogDensity`: under water a diving helmet sees 6.25 times as far; in lava
    /// the netherite one sees 20 blocks.
    fn render_fog(&self, host: &dyn mod_api::ClientHost, event: &mut mod_api::RenderFogEvent<'_>) {
        use mod_api::FogType;
        if !matches!(event.fog_type, FogType::Water | FogType::Lava) {
            return;
        }
        let Some(player) = host.local_player() else {
            return;
        };
        if matches!(player.game_mode(host), Some(GameMode::Spectator)) {
            return;
        }
        let Some(living) = host.living(player.entity()) else {
            return;
        };
        let Some(helmet) = living.equipment(host, ARMOR[0]) else {
            return;
        };
        match (gear_of(host, &helmet), event.fog_type) {
            (Gear::Helmet(_), FogType::Water) => event.fog.environmental_end *= 6.25,
            (Gear::Helmet(true), FogType::Lava) => {
                event.fog.environmental_start = -4.0;
                event.fog.environmental_end = 20.0;
            }
            _ => {}
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
            if let Err(error) = set_stack_air(&mut drop.stack, tank.air) {
                crate::create_log!("[create] dropped backtank air could not be written: {error:?}");
            }
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

/// `BacktankBlock.setPlacedBy`: a placed backtank takes the stack's air and Capacity level. The
/// item always shows its air as a bar (`BacktankItem.isBarVisible`, `getBarWidth`).
struct PlacedBacktank;

/// `BacktankItem.BAR_COLOR`.
const BAR_COLOR: u32 = 0xEFEFEF;

#[mod_api::sunrise_part]
impl ItemPart for PlacedBacktank {
    fn item_bar(&self, _host: &dyn mod_api::ClientHost, event: &mut mod_api::ItemBarEvent<'_>) {
        let max = eq::max_air(capacity(event.stack));
        let air = eq::air(stack_air(event.stack), capacity(event.stack));
        let width = (13.0 * (air as f32 / max as f32).clamp(0.0, 1.0)).round() as i32;
        *event.bar = Some((width, BAR_COLOR));
    }

    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        if backtank_block(event.state).is_none() {
            return;
        }
        let Some(dim) = event
            .level
            .dimension()
            .id(&*host)
            .and_then(|d| crate::glue::world::level_index(&d))
        else {
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

/// The gear a stack is, on either side.
fn gear_of(host: &dyn mod_api::Host, stack: &ItemStack) -> Gear {
    Gear::of(&stack.item().id(host))
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

fn set_stack_air(stack: &mut ItemStack, air: i32) -> Result<(), StackError> {
    let mut c = match stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()))
    {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    c.insert(AIR_KEY.into(), Tag::Int(air));
    let patch = Compound::from([("minecraft:custom_data".into(), Tag::Compound(c))]);
    stack.apply_components(&patch)
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

/// Whether `player` wears a whole netherite diving set (`NetheriteDivingHandler`'s four bits),
/// on either side.
fn full_netherite(host: &dyn mod_api::Host, player: PlayerRef) -> bool {
    let Some(living) = host.living(player.entity()) else {
        return false;
    };
    let worn = ARMOR.map(|slot| living.equipment(host, slot));
    let is = |s: &Option<ItemStack>, ids: &[&str]| {
        s.as_ref()
            .is_some_and(|s| ids.contains(&s.item().id(host).as_str()))
    };
    is(&worn[0], &["create:netherite_diving_helmet"])
        && is(&worn[1], &["create:netherite_backtank"])
        && worn[1].as_ref().is_some_and(|s| stack_air(s) > 0)
        && is(&worn[2], &["minecraft:netherite_leggings"])
        && is(
            &worn[3],
            &["minecraft:netherite_boots", "create:netherite_diving_boots"],
        )
}

/// `NetheriteDivingHandler`: a netherite diving helmet, a netherite backtank with air, and
/// fire-resistant (netherite) leggings and boots make the wearer immune to fire; while they are,
/// they stop burning.
fn netherite_set(host: &mut dyn ServerHost, player: PlayerRef) {
    let entity = player.entity();
    let uuid = player.uuid();
    if full_netherite(&*host, player) {
        let own = SERVER_FIRE_GRANTS.with_borrow(|grants| grants.get(&uuid) == Some(&entity));
        if (own || entity.fire_immune(&*host) != Some(true)) && entity.set_fire_immunity(host, true)
        {
            SERVER_FIRE_GRANTS.with_borrow_mut(|grants| grants.insert(uuid, entity));
        }
    } else if SERVER_FIRE_GRANTS.with_borrow_mut(|grants| grants.remove(&uuid)) == Some(entity) {
        entity.set_fire_immunity(host, false);
    }
}

pub(crate) fn player_logged_out(uuid: u128) {
    SERVER_FIRE_GRANTS.with_borrow_mut(|grants| grants.remove(&uuid));
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
    super::advancements::diving(host, player, breath.refill, breath.lava_diving);
    if let Some(slot) = breath.consume
        && !consume_air(host, player, slot, 1)
    {
        return;
    }
    if breath.refill {
        // `setRefillAirAmount(getMaxAirSupply())`.
        entity.set_air_supply(host, MAX_AIR_SUPPLY);
    }
}

/// `BacktankUtil.consumeAir` on the tank in armour slot `slot`, with its warnings.
/// A refused write spends no air and grants no protection.
fn consume_air(host: &mut dyn ServerHost, player: PlayerRef, slot: usize, amount: i32) -> bool {
    let place = ItemPlace::PlayerSlot {
        player: player.uuid(),
        // Our armour order starts at the head; inventory slots count from the feet.
        slot: HEAD_INVENTORY_SLOT - slot as i32,
    };
    let Ok(warnings) = host.transaction(|host, tx| {
        let mut warnings = None;
        place
            .edit(host, tx, |stack| {
                let (after, triggered) = eq::consume_air(stack_air(stack), capacity(stack), amount);
                if set_stack_air(stack, after).is_ok() {
                    warnings = Some(triggered);
                }
            })
            .map_err(|_| ())?;
        warnings.ok_or(())
    }) else {
        return false;
    };
    if warnings.is_empty() {
        return true;
    }
    let Some(name) = player.name(&*host) else {
        return true;
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
                None,
            );
            level.play_sound(
                host,
                "create:steam",
                at,
                SoundSource::Neutral,
                0.5,
                0.5,
                None,
            );
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
    true
}

/// `BacktankUtil.canAbsorbDamage`: creative players pay nothing; else the emptiest worn tank
/// pays `cost` air, or nothing absorbs it.
pub(crate) fn absorb(host: &mut dyn ServerHost, player: PlayerRef, uses_per_tank: i32) -> bool {
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
    consume_air(host, player, first.slot, eq::absorb_cost(uses_per_tank))
}

/// The gear in each hand.
fn hands(host: &dyn ServerHost, living: LivingRef) -> (Gear, Gear) {
    (
        gear(host, living.hand_item(host, Hand::Main)),
        gear(host, living.hand_item(host, Hand::Off)),
    )
}

/// `dontMissEntitiesWhenYouHaveHighReachDistance`: keep the ordinary block hit when only the
/// grip made the entity reachable.
pub(crate) fn grip_pick(host: &dyn mod_api::ClientHost, event: &mut mod_api::ClientPickEvent<'_>) {
    use mod_api::CrosshairTarget;

    if *event.result != event.original || event.original != event.entity_candidate {
        return;
    }
    let (Some(CrosshairTarget::Entity { point, .. }), Some(block @ CrosshairTarget::Block { .. })) =
        (event.original, event.block_candidate)
    else {
        return;
    };
    let (Some(&grip), Some(player)) = (GRIP_ITEM.get(), host.local_player()) else {
        return;
    };
    let entity = player.entity();
    if host.camera_entity() != Some(entity) {
        return;
    }
    let Some(living) = host.living(entity) else {
        return;
    };
    if ![Hand::Main, Hand::Off].into_iter().any(|hand| {
        living
            .hand_item(host, hand)
            .is_some_and(|stack| stack.item() == grip)
    }) {
        return;
    }
    let (Some(base), Some(modifiers)) = (
        living.attribute_base(host, ENTITY_RANGE),
        living.attribute_modifiers(host, ENTITY_RANGE),
    ) else {
        return;
    };
    if entity_range_without_grip(base, &modifiers)
        .is_some_and(|range| eq::grip_pick_preserves_block(event.origin, point, range))
    {
        *event.result = Some(block);
    }
}

/// Re-evaluate the synced attribute with only the grip's modifier removed. Subtracting its
/// amount from the effective value would also undo the wrong amount under a multiplier.
fn entity_range_without_grip(base: f64, modifiers: &[AttributeModifier]) -> Option<f64> {
    let id = &*ENTITY_REACH;
    if !modifiers.iter().any(|m| &m.id == id) {
        return None;
    }

    let amounts = |operation| {
        modifiers
            .iter()
            .filter(move |m| m.operation == operation && &m.id != id)
            .map(|m| m.amount)
    };
    let additive = amounts(AttributeOperation::AddValue).fold(base, |sum, amount| sum + amount);
    let scaled = amounts(AttributeOperation::AddMultipliedBase)
        .fold(additive, |sum, amount| sum + additive * amount);
    let range = amounts(AttributeOperation::AddMultipliedTotal)
        .fold(scaled, |value, amount| value * (1.0 + amount));
    let (min, max) = ENTITY_RANGE_BOUNDS;
    Some(if range.is_nan() {
        min
    } else {
        range.clamp(min, max)
    })
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
            // `AllAdvancements.EXTENDO_GRIP` / `EXTENDO_GRIP_DUAL` as the grip is taken up.
            let name = if modifier.0 == SINGLE_RANGE.0 {
                "extendo_grip"
            } else {
                "extendo_grip_dual"
            };
            super::advancements::award(host, player, name);
            let m = AttributeModifier::new(id, modifier.1, AttributeOperation::AddValue);
            living.add_attribute_modifier(host, BLOCK_RANGE, m, ModifierLifetime::Transient);
        } else {
            living.remove_attribute_modifier(host, BLOCK_RANGE, &id);
        }
    }
    // `dontMissEntitiesWhenYouHaveHighReachDistance`: the client picks entities as far as the
    // block reach (half a block less out of creative), and the server takes the hit
    // (`ExtendoGripInteractionPacket`); the port lends the entity reach the same distance
    // (from 3, or creative's 5).
    let creative = player.is_creative(&*host) == Some(true);
    let entity = match (single, dual) {
        (_, true) => DOUBLE_RANGE.1,
        (true, _) => SINGLE_RANGE.1,
        _ => 0.0,
    };
    let entity = if entity > 0.0 && !creative {
        entity + 1.0
    } else {
        entity
    };
    let id = &*ENTITY_REACH;
    let now = living
        .attribute_modifiers(&*host, ENTITY_RANGE)
        .and_then(|m| m.iter().find(|m| &m.id == id).map(|m| m.amount));
    if entity == 0.0 && now.is_some() {
        living.remove_attribute_modifier(host, ENTITY_RANGE, id);
    } else if entity > 0.0 && now != Some(entity) {
        if now.is_some() {
            living.remove_attribute_modifier(host, ENTITY_RANGE, id);
        }
        let m = AttributeModifier::new(id.clone(), entity, AttributeOperation::AddValue);
        living.add_attribute_modifier(host, ENTITY_RANGE, m, ModifierLifetime::Transient);
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

/// The backtanks' drawing on Create's block entity type, registered once kinetics made it.
pub(crate) fn register_cogs(r: &mut dyn Registrar, kind: &mod_api::BlockEntityTypeKey) {
    if let Err(e) = r.block_entity_part(kind, BacktankCogs) {
        crate::create_log!("[create] the backtanks' block entity part was refused: {e:?}");
    }
}

/// `BacktankRenderer`: the cogs on a backtank's back, turned to its facing (as modelled, north)
/// and spinning about their own axis (x, through 6.5 and 11 sixteenths) at a quarter of its
/// speed in degrees a tick.
struct BacktankCogs;

#[mod_api::sunrise_part]
impl mod_api::BlockPart for BacktankCogs {}

#[mod_api::sunrise_part]
impl mod_api::BlockEntityPart for BacktankCogs {
    fn block_entity_submit(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        let name = match block {
            Block::CopperBacktank => "part_copper_backtank_cogs",
            Block::NetheriteBacktank => "part_netherite_backtank_cogs",
            _ => return,
        };
        let (Some(part), Some(facing)) = (states::partial(name), info.facing) else {
            return;
        };
        use create_engine::kinetics::Dir;
        // Quarter turns clockwise (seen from above) from north to the facing: (x, z) about the
        // centre becomes (-z, x) each.
        let turns = match facing {
            Dir::East => 1,
            Dir::South => 2,
            Dir::West => 3,
            _ => 0,
        };
        let turn = |v: [f32; 2]| (0..turns).fold(v, |[x, z], _| [-z, x]);
        let [ax, az] = turn([1.0, 0.0]);
        let [px, pz] = turn([0.0, 11.0 / 16.0 - 0.5]);
        let speed = super::kinetics::speeds_of(&event.entity.nbt)[0];
        let draw = &mut *event.draw;
        draw.spin(
            [ax, 0.0, az],
            [px + 0.5, 6.5 / 16.0, pz + 0.5],
            speed / 4.0,
            0.0,
        );
        draw.block(part, &[("facing", super::kinetics::dir_name(facing))]);
        draw.still();
    }
}

/// `BacktankUtil.isBarVisible` / `getBarWidth` for an air-using tool held by the local player:
/// the worn backtanks' air as a bar while there is any.
pub(crate) fn backtank_bar(host: &dyn mod_api::ClientHost) -> Option<(i32, u32)> {
    let player = host.local_player()?;
    let living = host.living(player.entity())?;
    let (mut air, mut max) = (0, 0);
    for slot in ARMOR {
        if let Some(s) = living.equipment(host, slot)
            && matches!(gear_of(host, &s), Gear::Backtank(_))
        {
            let cap = capacity(&s);
            air += eq::air(stack_air(&s), cap);
            max += eq::max_air(cap);
        }
    }
    (air > 0 && max > 0).then(|| {
        let width = (13.0 * (air as f32 / max as f32).clamp(0.0, 1.0)).round() as i32;
        (width, BAR_COLOR)
    })
}

/// `ExtendoGripItemRenderer` at rest (`animation` .25, as Create draws the grip anywhere but in
/// a first-person hand): the grip, its scissor arms folded to their angle, the pointing deployer
/// hand at their end and the cog. A special model is posed once per stack, so the cog does not
/// turn and the arms do not reach out as the player swings.
struct GripModel;

/// A 4×4 pose, row-major, multiplied on the right as `PoseStack` does.
#[derive(Clone, Copy)]
struct Pose([[f32; 4]; 4]);

impl Pose {
    const IDENTITY: Pose = Pose([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    fn mul(self, o: Pose) -> Pose {
        let mut r = [[0.0; 4]; 4];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Pose(r)
    }
    fn translate(self, v: [f32; 3]) -> Pose {
        let mut t = Pose::IDENTITY;
        t.0[0][3] = v[0];
        t.0[1][3] = v[1];
        t.0[2][3] = v[2];
        self.mul(t)
    }
    fn scale(self, v: [f32; 3]) -> Pose {
        let mut t = Pose::IDENTITY;
        t.0[0][0] = v[0];
        t.0[1][1] = v[1];
        t.0[2][2] = v[2];
        self.mul(t)
    }
    /// `Axis.XP/YP/ZP.rotationDegrees`.
    fn rotate(self, axis: usize, degrees: f32) -> Pose {
        let (s, c) = degrees.to_radians().sin_cos();
        let mut t = Pose::IDENTITY;
        let (a, b) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        t.0[a][a] = c;
        t.0[a][b] = -s;
        t.0[b][a] = s;
        t.0[b][b] = c;
        self.mul(t)
    }
    /// The part's 3×4 transform from the item's corner: Create's pose is about the item's
    /// middle, and draws each model from its own middle (`PartialItemModelRenderer`).
    fn part(self, model: &str) -> mod_api::SpecialModelPart {
        let m = Pose::IDENTITY
            .translate([0.5; 3])
            .mul(self)
            .translate([-0.5; 3]);
        let t = m.0;
        mod_api::SpecialModelPart::new(model).with_transform([
            t[0][0], t[0][1], t[0][2], t[0][3], t[1][0], t[1][1], t[1][2], t[1][3], t[2][0],
            t[2][1], t[2][2], t[2][3],
        ])
    }
}

/// The item model node type that picks the grip's pose (`create-data prepare` writes its
/// models: `rest_<cog>` and `hand_<swing>_<cog>_<hand>`).
pub(crate) const GRIP_POSE: &str = "create:extendo_grip_pose";
/// The cog's angles within its 45° repeat, and the swing's steps, as `create-data` makes them.
const GRIP_COG_STEPS: i64 = 10;
const GRIP_SWING_STEPS: f32 = 8.0;

#[derive(Clone, Copy, Default)]
struct GripRender {
    swing: eq::GripSwing,
    hold: bool,
    /// The main hand's swing this frame (zero if the main hand was not drawn).
    main_swing: f32,
    /// The interpolated extension and pose for the grip being resolved next.
    pose: (f32, bool),
}

thread_local! {
    static GRIP_RENDER: std::cell::Cell<GripRender> = std::cell::Cell::new(GripRender::default());
}

/// Some frames show only the off hand. Don't reuse an earlier frame's main-hand swing.
pub(crate) fn grip_frame() {
    let mut grip = GRIP_RENDER.get();
    grip.main_swing = 0.0;
    grip.pose = (0.0, false);
    GRIP_RENDER.set(grip);
}

/// `onRenderPlayerHand`: a displayed grip follows the main-hand swing. When both hands
/// draw, their hooks run in main/off order, each before that hand's item model is resolved.
pub(crate) fn grip_hand(render: &mod_api::HandRender<'_>) {
    let shows_grip = GRIP_ITEM
        .get()
        .is_some_and(|&item| render.stack.is_some_and(|stack| stack.item() == item));
    let mut grip = GRIP_RENDER.get();
    grip.for_hand(render.hand, render.swing, render.partial, shows_grip);
    GRIP_RENDER.set(grip);
}

impl GripRender {
    fn for_hand(&mut self, hand: Hand, swing: f32, partial: f32, shows_grip: bool) {
        let main = hand == Hand::Main;
        if main {
            self.main_swing = swing;
        }
        self.pose = (0.0, false);
        if shows_grip {
            self.swing.observe(self.main_swing);
            // `hold` uses tick-held stacks, not the old stack still shown as the hand lowers.
            self.pose = (self.swing.at(partial), !main && self.hold);
        }
    }
}

/// `item_model_update` for the grip's node: in a first-person hand, the swing's pose and the
/// cog turned on by it; elsewhere at rest (`animation` 0.25); the cog at `renderTime * -2`.
pub(crate) fn grip_pose(
    _host: &dyn mod_api::ClientHost,
    event: &mut mod_api::ItemModelUpdateEvent<'_>,
) {
    if event.kind != GRIP_POSE {
        return;
    }
    let time = event.level.map_or(0, |l| l.game_time);
    let hand = matches!(
        event.display,
        mod_api::ItemDisplayContext::FirstPersonLeftHand
            | mod_api::ItemDisplayContext::FirstPersonRightHand
    );
    let (swing, hold) = GRIP_RENDER.get().pose;
    let step = (swing * (GRIP_SWING_STEPS - 1.0)).round() as i64;
    let shown = step as f32 / (GRIP_SWING_STEPS - 1.0);
    let mut angle = time as f32 * -2.0;
    if hand {
        angle += 360.0 * shown.powi(3);
    }
    let cog = ((angle.rem_euclid(45.0) / 45.0 * GRIP_COG_STEPS as f32) as i64) % GRIP_COG_STEPS;
    event.layers.push(mod_api::ItemModelLayer::new(if hand {
        let pose = if hold { "holding" } else { "punching" };
        format!("hand_{step}_{cog}_{pose}")
    } else {
        format!("rest_{cog}")
    }));
}

/// A number in a special model's config (its JSON converted to NBT).
fn config_number(config: &Tag, key: &str) -> Option<f32> {
    let Tag::Compound(c) = config else {
        return None;
    };
    Some(match c.get(&key.into())? {
        Tag::Double(v) => f64::from_bits(*v) as f32,
        Tag::Float(v) => f32::from_bits(*v),
        Tag::Int(v) => *v as f32,
        Tag::Long(v) => *v as f32,
        Tag::Short(v) => f32::from(*v),
        Tag::Byte(v) => f32::from(*v),
        _ => return None,
    })
}

impl mod_api::SpecialModelRenderer for GripModel {
    /// `ExtendoGripItemRenderer.render` for the pose its node names: `animation` (before it is
    /// cubed), the cog's turn `cog` and the hand.
    fn extract(
        &self,
        _host: &dyn mod_api::ClientHost,
        _stack: &ItemStack,
        config: &Tag,
        _context: &mod_api::SpecialModelContext,
    ) -> Result<Vec<mod_api::SpecialModelPart>, String> {
        const ROT: [f32; 3] = [0.0, 0.5, 0.5];
        const BACK: [f32; 3] = [0.0, -0.5, -0.5];
        let item = |m: &str| format!("create:item/extendo_grip/{m}");
        let animation: f32 = config_number(config, "animation").unwrap_or(0.25).powi(3);
        let cog = config_number(config, "cog").unwrap_or(0.0);
        let hand = match config {
            Tag::Compound(c) => match c.get(&"hand".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => "pointing".to_owned(),
            },
            _ => "pointing".to_owned(),
        };
        let extension = 24.0 + (156.0 - 24.0) * animation;
        let half = extension / 2.0;
        let opposite = 180.0 - extension;
        let mut parts = vec![Pose::IDENTITY.part(&item("item"))];
        let base = Pose::IDENTITY
            .translate([0.0, 1.0 / 16.0, -7.0 / 16.0])
            .scale([1.0, 1.0, 1.0 + animation]);
        // One scissor arm.
        let mut ms = base.rotate(0, -half).translate(ROT);
        parts.push(ms.part(&item("thin_short")));
        ms = ms.translate(BACK).translate([0.0, 5.5 / 16.0, 0.0]);
        ms = ms.rotate(0, -opposite).translate(ROT);
        parts.push(ms.part(&item("wide_long")));
        ms = ms.translate(BACK).translate([0.0, 11.0 / 16.0, 0.0]);
        ms = ms.rotate(0, opposite).translate(ROT);
        ms = ms.translate([0.0, 0.5 / 16.0, 0.0]);
        parts.push(ms.part(&item("thin_short")));
        // The other, and the hand at its end.
        let mut ms = base.rotate(0, -180.0 + half).translate(ROT);
        parts.push(ms.part(&item("wide_short")));
        ms = ms.translate(BACK).translate([0.0, 5.5 / 16.0, 0.0]);
        ms = ms.rotate(0, opposite).translate(ROT);
        parts.push(ms.part(&item("thin_long")));
        ms = ms.translate(BACK).translate([0.0, 11.0 / 16.0, 0.0]);
        ms = ms.rotate(0, -opposite).translate(ROT);
        ms = ms.translate([0.0, 0.5 / 16.0, 0.0]);
        parts.push(ms.part(&item("wide_short")));
        ms = ms.translate(BACK).translate([0.0, 5.5 / 16.0, 0.0]);
        ms = ms.rotate(0, 180.0 - half).rotate(1, 180.0);
        ms = ms.translate([0.0, 0.0, -4.0 / 16.0]);
        ms = ms.scale([1.0, 1.0, 1.0 / (1.0 + animation)]);
        parts.push(ms.part(&format!("create:block/deployer/hand_{hand}")));
        // The cog, turned about its middle (`COG_ROTATION_OFFSET`).
        let cog_pose = Pose::IDENTITY
            .translate([0.0, 1.0 / 16.0, 0.0])
            .rotate(2, cog)
            .translate([0.0, -1.0 / 16.0, 0.0]);
        parts.push(cog_pose.part(&item("cog")));
        Ok(parts)
    }
}

#[cfg(test)]
mod grip_pick_tests {
    use super::*;

    #[test]
    fn ordinary_entity_range_excludes_only_the_grip_and_keeps_other_modifiers() {
        let modifier = |id, amount, operation| {
            AttributeModifier::new(Identifier::parse(id).unwrap(), amount, operation)
        };
        let other = [
            modifier("test:flat", 1.0, AttributeOperation::AddValue),
            modifier(
                "test:base_scale",
                0.25,
                AttributeOperation::AddMultipliedBase,
            ),
            modifier(
                "test:total_scale",
                0.5,
                AttributeOperation::AddMultipliedTotal,
            ),
        ];
        assert_eq!(entity_range_without_grip(3.0, &other), None);
        let mut all = other.to_vec();
        all.push(AttributeModifier::new(
            ENTITY_REACH.clone(),
            4.0,
            AttributeOperation::AddValue,
        ));
        // (3 + 1) * (1 + 0.25) * (1 + 0.5).
        assert_eq!(entity_range_without_grip(3.0, &all), Some(7.5));
        assert_eq!(entity_range_without_grip(-10.0, &all), Some(0.0));
        assert_eq!(entity_range_without_grip(60.0, &all), Some(64.0));
    }
}

#[cfg(test)]
mod grip_render_tests {
    use super::*;

    #[test]
    fn off_hand_grip_uses_only_the_main_hands_swing_in_this_frame() {
        grip_frame();
        let mut grip = GRIP_RENDER.get();
        grip.for_hand(Hand::Main, 0.25, 0.5, false);
        assert_eq!(grip.pose, (0.0, false));
        grip.for_hand(Hand::Off, 0.0, 0.5, true);
        assert_eq!(grip.pose, (0.475, false));

        // An off-hand-only render must not trigger extension from a hidden main hand.
        GRIP_RENDER.set(GripRender {
            main_swing: 0.25,
            ..GripRender::default()
        });
        grip_frame();
        let mut grip = GRIP_RENDER.get();
        grip.for_hand(Hand::Off, 0.25, 0.5, true);
        assert_eq!(grip.pose, (0.0, false));
    }
}
