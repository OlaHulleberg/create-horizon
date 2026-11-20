//! The potato cannon (`PotatoCannonItem`, `PotatoProjectileEntity`; the rules are
//! `create_engine::potato`'s): a use fires the first ammunition found (the off hand, the main
//! hand, then the inventory, as `Player.getProjectile` looks) from the barrel beside the eyes,
//! split shots fanned round the aim, with the type's reload as the item's cooldown, the
//! cannon's durability or a backtank's air paid, and `FWOOMP`. A saved `create:potato_projectile`
//! owns its ammunition and flight state. Its tick sweeps for blocks and entities before moving,
//! with Create's gravity, drag and hit margin. Hits apply damage, sticking, ammo actions,
//! recovery or leftovers, and sound and crumbs. See `API_GAPS.md` for missing native projectile
//! movement, impact, rendering, and damage operations.

use super::super::equipment;
use super::super::subsystem::Subsystem;
use super::{creative, custom, held, set_custom};
use create_engine::potato::{self, Type};
use mod_api::{
    ActiveEffect, AttachmentCodec, AttachmentError, AttachmentType, BlockEntityDrawContext,
    BlockFace, ClientHost, ClipHit, Compound, EntityPart, EntityRef, EntityRenderState,
    EntityRenderer, EntitySpawn, GeneralEntityCategory, GeneralEntityTickEvent, GeneralSavePolicy,
    Hand, Holder, Host, HostSide, HostTransaction, Identifier, InteractionResult,
    ItemDisplayContext, ItemKey, ItemPart, ItemPlace, ItemStack, LevelRef, ParticleKey,
    ParticleOptions, PlayerRef, Registrar, RegistrationContext, ServerHost, SoundSource, SpawnBody,
    Tag, Text, TooltipEvent, UseItemEvent,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

const CANNON: &str = "create:potato_cannon";
const SHOT: &str = "create:potato_projectile";
/// `CEquipment.maxPotatoCannonShots`: shots a full backtank pays for.
const MAX_USES: i32 = potato::MAX_SHOTS;

/// Load-resolved ammo keys for decoding the saved stack in the renderer, which has no Host.
static AMMO_KEYS: OnceLock<HashMap<&'static str, ItemKey>> = OnceLock::new();
static PICKABLE_TYPES: OnceLock<HashSet<mod_api::EntityTypeKey>> = OnceLock::new();

struct Ammo {
    saved: Compound,
    stack: ItemStack,
    kind: &'static Type,
}

static AMMO: AttachmentType<Option<Arc<Ammo>>> = AttachmentType {
    namespace: "create",
    name: "potato_ammo",
    default: || None,
    codec: Some(AttachmentCodec {
        encode: |ammo| Ok(ammo.as_ref().ok_or(AttachmentError::Decode)?.saved.clone()),
        decode: |saved| {
            let Some(Tag::String(id)) = saved.get(&"id".into()) else {
                return Err(AttachmentError::Decode);
            };
            if !matches!(
                saved.get(&"count".into()),
                None | Some(Tag::Byte(1) | Tag::Short(1) | Tag::Int(1) | Tag::Long(1))
            ) {
                return Err(AttachmentError::Decode);
            }
            let id = id.to_string_lossy();
            let item = AMMO_KEYS
                .get()
                .and_then(|keys| keys.get(id.as_str()))
                .ok_or(AttachmentError::Decode)?;
            let kind = potato::type_for(id.as_ref()).ok_or(AttachmentError::Decode)?;
            let mut stack = ItemStack::of(*item);
            match saved.get(&"components".into()) {
                Some(Tag::Compound(patch)) => stack
                    .set_components(patch.clone())
                    .map_err(|_| AttachmentError::Decode)?,
                None => {}
                _ => return Err(AttachmentError::Decode),
            }
            Ok(Some(Arc::new(Ammo {
                saved: saved.clone(),
                stack,
                kind,
            })))
        },
    }),
    copy_on_death: false,
    sync: true,
};

#[derive(Clone, Copy, Default)]
struct Flight {
    // Zero means ineligible or spent. Save the change before spawning any recovery drop.
    recovery: f32,
    left_owner: bool,
}

#[derive(Clone, Copy)]
struct Stuck {
    target: EntityRef,
    offset: [f64; 3],
    fall: f64,
}

// The target and follow offsets are transient in Create as well. Read and update them in
// place rather than re-encoding the saved flight every tick.
static STUCK: AttachmentType<Arc<Mutex<Option<Stuck>>>> = AttachmentType {
    namespace: "create",
    name: "potato_stuck",
    default: Default::default,
    codec: None,
    copy_on_death: false,
    sync: false,
};

static FLIGHT: AttachmentType<Flight> = AttachmentType {
    namespace: "create",
    name: "potato_flight",
    default: Flight::default,
    codec: Some(AttachmentCodec {
        encode: |flight| {
            Ok(Compound::from([
                ("Recovery".into(), Tag::Float(flight.recovery.to_bits())),
                ("LeftOwner".into(), Tag::Byte(i8::from(flight.left_owner))),
            ]))
        },
        decode: |saved| {
            let (Some(Tag::Float(recovery)), Some(Tag::Byte(left_owner))) = (
                saved.get(&"Recovery".into()),
                saved.get(&"LeftOwner".into()),
            ) else {
                return Err(AttachmentError::Decode);
            };
            let recovery = f32::from_bits(*recovery);
            if !(0.0..=1.0).contains(&recovery) || !matches!(left_owner, 0 | 1) {
                return Err(AttachmentError::Decode);
            }
            Ok(Flight {
                recovery,
                left_owner: *left_owner == 1,
            })
        },
    }),
    copy_on_death: false,
    sync: false,
};

static ITEM_PARTICLE: OnceLock<ParticleKey> = OnceLock::new();
static AIR_PARTICLE: OnceLock<ParticleKey> = OnceLock::new();
/// `AllDataComponents.SHAPER_SWAP`: the main hand's cannon lets the off hand's fire next.
const SWAP: &str = "create:shaper_swap";

thread_local! {
    static CAMERA: Cell<[f32; 2]> = const { Cell::new([0.0; 2]) };
    /// The players whose main arm is the left (`Player.getMainArm`, from their client's
    /// information).
    static LEFT_HANDED: RefCell<std::collections::HashSet<u128>> =
        RefCell::new(std::collections::HashSet::new());
}

/// `Inventory.EQUIPMENT_SLOT_MAPPING`: this is the physical offhand slot in the
/// player's inventory view, not `ItemPlace`'s slot number.
const OFFHAND_SLOT: u32 = 40;

fn marked_for_swap(stack: &ItemStack) -> bool {
    matches!(custom(stack).get(&SWAP.into()), Some(Tag::Byte(b)) if *b != 0)
}

/// The air particle (`AirParticle` with `AirParticleData(1, 1/4)`): from where it spawns to its
/// velocity's end, twirling, 4 ticks a block (at most 60), a quarter visible.
const AIR: &str = "create:air";

struct Air;

impl mod_api::ParticleProvider for Air {
    fn tick(&self, p: &mut mod_api::ModParticle) {
        let velocity = p.velocity;
        let h = |salt: u64| {
            let mut x = salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            for v in velocity {
                x = (x ^ v.to_bits()).wrapping_mul(0x0100_0000_01B3);
            }
            ((x >> 11) % 10_000) as f64 / 10_000.0
        };
        if p.age <= 1 {
            p.state[..3].copy_from_slice(&p.previous);
            let len = p.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
            p.lifetime = ((len / 0.25) as i32).min(60);
            p.size *= 0.75;
            p.rgba[3] = 0.25;
            if len == 0.0 {
                p.removed = true;
                return;
            }
        }
        let life = f64::from(p.lifetime.max(1));
        let progress = (f64::from(p.age) / life).powf(1.0);
        let radius = h(1) / 6.0;
        let angle = ((progress * 2.0 * 360.0 + h(2) * 360.0) % 360.0).to_radians();
        let (sin, cos) = angle.sin_cos();
        // `VecHelper.rotate((0, r, 0), angle, X or Z)`.
        let twirl = if h(3) < 0.5 {
            [0.0, radius * cos, radius * sin]
        } else {
            [-radius * sin, radius * cos, 0.0]
        };
        p.position = std::array::from_fn(|i| p.state[i] + p.velocity[i] * progress + twirl[i]);
        // `setSpriteFromAge` over its 8 sprites.
        p.frame = (p.age * 7 / p.lifetime.max(1)).clamp(0, 7) as u32;
    }
}

pub struct PotatoCannon;

impl Subsystem for PotatoCannon {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = ITEM_PARTICLE.set(r.particle_key("minecraft:item"));
        let _ = AIR_PARTICLE.set(r.particle_key(AIR));
        let key = r.item_key(CANNON);
        r.item_part(key, CannonPart);
        let _ = AMMO_KEYS.set(
            potato::TYPES
                .iter()
                .flat_map(|t| t.items.iter().copied())
                .map(|id| (id, r.item_key(id)))
                .collect(),
        );
        // Nonliving, non-minecart targets of the sweep. Resolve names at registration,
        // not for every nearby entity on every flying tick. General shots aren't damageable.
        let _ = PICKABLE_TYPES.set(
            [
                "acacia", "birch", "cherry", "dark_oak", "jungle", "mangrove", "oak", "pale_oak",
                "spruce",
            ]
            .into_iter()
            .flat_map(|wood| {
                [
                    format!("minecraft:{wood}_boat"),
                    format!("minecraft:{wood}_chest_boat"),
                ]
            })
            .chain(
                [
                    "bamboo_raft",
                    "bamboo_chest_raft",
                    "end_crystal",
                    "item_frame",
                    "glow_item_frame",
                    "painting",
                ]
                .map(|id| format!("minecraft:{id}")),
            )
            .map(|id| r.entity_type_key(&id))
            .collect(),
        );
        for (id, sync) in [
            ("create:potato_ammo", true),
            ("create:potato_flight", false),
            ("create:potato_stuck", false),
        ] {
            if let Err(e) = r.register_attachment_type(id, sync) {
                crate::create_log!("[create] declaring potato projectile data: {e:?}");
            }
        }
        if let Err(e) = r.declare_general_entity(
            "potato_projectile",
            GeneralEntityCategory::Misc,
            f64::from(potato::SIZE),
            f64::from(potato::SIZE),
            None,
            GeneralSavePolicy::Save,
            false,
            false,
            4,
            20,
            1,
            &[],
        ) {
            crate::create_log!("[create] potato projectile type was refused: {e:?}");
        }
        r.entity_part(SHOT, Projectile);
    }

    fn entity_renderers(&self, renderers: &mut dyn mod_api::EntityRenderers) {
        renderers.register(SHOT.to_owned(), Arc::new(Projectile));
    }

    fn camera_pose(&self, _host: &dyn ClientHost, event: &mut mod_api::CameraPoseEvent<'_>) {
        CAMERA.set([event.yaw, event.pitch]);
    }

    /// Each player's main arm, for the barrel's side.
    fn client_info(&self, player: u128, info: &mod_api::ClientInfo) {
        let left = matches!(info.main_hand, mod_api::MainHand::Left);
        LEFT_HANDED.with_borrow_mut(|l| {
            if left {
                l.insert(player);
            } else {
                l.remove(&player);
            }
        });
    }

    fn particle_providers(&self, providers: &mut dyn mod_api::ParticleProviders) {
        providers.register(AIR.to_owned(), std::sync::Arc::new(Air));
    }
}

struct Projectile;

#[mod_api::sunrise_part]
impl EntityPart for Projectile {
    fn general_entity_tick(
        &self,
        host: &mut dyn ServerHost,
        event: &mut GeneralEntityTickEvent<'_>,
    ) {
        let e = event.entity;
        if let Some(level) = e.level(host) {
            fly(host, level, e);
        }
    }
}

impl EntityRenderer for Projectile {
    fn submit(
        &self,
        entity: &EntityRenderState,
        draw: &mut dyn BlockEntityDrawContext,
    ) -> Result<(), String> {
        let Some(ammo) = entity
            .attachments
            .as_ref()
            .and_then(|a| a.get(&AMMO).ok().flatten())
            .and_then(Option::as_ref)
        else {
            return Ok(());
        };
        let [yaw, pitch] = CAMERA.get();
        draw.rotate_about([0.0, 1.0, 0.0], 180.0 - yaw, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], -pitch, [0.0; 3]);
        draw.item(&ammo.stack, ItemDisplayContext::Ground, None, 0);
        Ok(())
    }
}

struct CannonPart;

#[mod_api::sunrise_part]
impl ItemPart for CannonPart {
    /// `PotatoCannonItem.use`: passes without ammunition.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => fire(host, event).then_some(InteractionResult::Success),
            HostSide::Client(host) => {
                let player = host.player(event.player)?;
                if event.hand == Hand::Main
                    && marked_for_swap(event.stack)
                    && held(host, player, Hand::Off).is_some_and(|s| s.item() == event.stack.item())
                {
                    return None;
                }
                ammo(host, player).map(|_| InteractionResult::Success)
            }
        }
    }

    /// `isBarVisible` / `getBarWidth` (`BacktankUtil`): the worn backtanks' air while there is
    /// any, else the cannon's wear.
    fn item_bar(&self, host: &dyn ClientHost, event: &mut mod_api::ItemBarEvent<'_>) {
        if let Some(bar) = super::super::equipment::backtank_bar(host) {
            *event.bar = Some(bar);
        }
    }

    /// `PotatoCannonItem.appendHoverText`: the ammunition found and what it does.
    fn tooltip(&self, host: &dyn ClientHost, event: &mut TooltipEvent<'_>) {
        let Some(player) = host.local_player() else {
            return;
        };
        let Some((_, stack, t)) = ammo(host, player) else {
            return;
        };
        const DARK_GREEN: u32 = 0xFF00_AA00;
        const GRAY: u32 = 0xFFAA_AAAA;
        let line = |key: &str, value: String| {
            let text = host
                .translate(&format!("create.potato_cannon.ammo.{key}"))
                .unwrap_or_default()
                .replace("%1$s", &value);
            (Text::literal(format!(" {text}")), DARK_GREEN)
        };
        event.lines.push((Text::literal(""), u32::MAX));
        let name = host.hover_name(&stack).unwrap_or_default();
        event.lines.push((name.append(Text::literal(":")), GRAY));
        event
            .lines
            .push(line("attack_damage", t.damage.to_string()));
        event
            .lines
            .push(line("reload_ticks", t.reload_ticks.to_string()));
        event.lines.push(line("knockback", t.knockback.to_string()));
    }
}

/// Where the ammunition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Hand(Hand),
    Slot(u32),
}

/// `Player.getProjectile` for the cannon: the off hand, the main hand, then the inventory's
/// slots in order; the first stack Create has a projectile type for.
fn ammo(host: &dyn Host, player: PlayerRef) -> Option<(Source, ItemStack, &'static Type)> {
    let kind = |s: &ItemStack| potato::type_for(&s.item().id(host));
    for hand in [Hand::Off, Hand::Main] {
        if let Some(stack) = held(host, player, hand)
            && let Some(t) = kind(&stack)
        {
            return Some((Source::Hand(hand), stack, t));
        }
    }
    let mut slots = player.inventory_slots(host)?;
    slots.sort_by_key(|(slot, _)| *slot);
    slots
        .into_iter()
        .filter(|(slot, _)| *slot < 36)
        .find_map(|(slot, stack)| kind(&stack).map(|t| (Source::Slot(slot), stack, t)))
}

/// `Entity.isAlive`: a living entity's, or any other's presence in its level.
fn alive(host: &dyn Host, e: EntityRef) -> bool {
    match host.living(e) {
        Some(l) => l.alive(host) == Some(true),
        None => e.added_to_level(host),
    }
}

/// One of `stack`.
fn one(stack: &ItemStack) -> ItemStack {
    let mut s = stack.clone();
    let _ = s.set_count(1);
    s
}

/// `Create.RANDOM.nextFloat()`, from the level's random.
fn random(host: &mut dyn ServerHost, level: LevelRef) -> f32 {
    level.with_random(host, |r| r.next_float()).unwrap_or(0.5)
}

/// The cannon's `potato_recovery` enchantment level.
fn recovery_level(stack: &ItemStack) -> i32 {
    let levels = stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:enchantments".into()).cloned());
    match levels {
        Some(Tag::Compound(c)) => match c.get(&"create:potato_recovery".into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

fn fire(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> bool {
    let Some(player) = host.player(event.player) else {
        return false;
    };
    let main = event.hand == Hand::Main;
    let other_hand = if main { Hand::Off } else { Hand::Main };
    let cannon = event.stack;
    let other_cannon = held(host, player, other_hand).filter(|stack| stack.same_item(cannon));
    if main && other_cannon.is_some() && marked_for_swap(cannon) {
        return false;
    }
    let Some(selected_slot) = player.selected_slot(host).map(|slot| slot as u32) else {
        return false;
    };
    let cannon_slot = if main { selected_slot } else { OFFHAND_SLOT };
    let Some((source, ammo_stack, t)) = ammo(host, player) else {
        return false;
    };
    let ammo_slot = match source {
        Source::Hand(Hand::Main) => selected_slot,
        Source::Hand(Hand::Off) => OFFHAND_SLOT,
        Source::Slot(slot) => slot,
    };
    let entity = player.entity();
    let (Some(eye), Some([yaw, pitch]), Some(look)) = (
        entity.eye_position(&*host),
        entity.rotation(&*host),
        entity.view_vector(&*host),
    ) else {
        return false;
    };
    let Some(level) = entity.level(host) else {
        return false;
    };
    // `mainHand == (getMainArm() == RIGHT)`: the barrel on the side of the hand holding it.
    let right_side = main != LEFT_HANDED.with_borrow(|l| l.contains(&player.uuid()));
    let barrel = potato::barrel(eye, yaw, pitch, right_side, [0.75, -0.15, 1.5]);
    let side = potato::barrel(eye, yaw, pitch, right_side, [-0.05, 0.0, 0.0]);
    let correction = [side[0] - eye[0], side[1] - eye[1], side[2] - eye[2]];
    let motion = potato::motion(look, correction, t);
    let pitch_shift = t.sound_pitch + (random(host, level) - 0.5) / 4.0;
    let base = 360.0 * random(host, level);
    let imperfections: Vec<f32> = (0..t.split)
        .map(|_| 40.0 * (random(host, level) - 0.5))
        .collect();
    let motions = potato::spray(motion, t, base, &imperfections);
    let recovery = potato::recovery_chance(recovery_level(cannon));
    let shot_stack = one(&ammo_stack);
    let Some(item) = shot_stack.to_nbt(host) else {
        return false;
    };
    let ammo = Some(Arc::new(Ammo {
        saved: item,
        stack: shot_stack.clone(),
        kind: t,
    }));
    // The use callback is outside a transaction. Stage initialized shots together so the
    // join hook and first tracking snapshot see their ammo; check admission after commit.
    let Ok(mut shots) = host.transaction(|host, _| {
        let mut shots = Vec::with_capacity(motions.len());
        for motion in motions {
            let mut spawn = EntitySpawn::new(SpawnBody::General);
            spawn.owner = Some(entity);
            spawn.velocity = motion;
            let Some(e) = level.spawn_entity(host, SHOT, barrel, &spawn) else {
                continue;
            };
            let holder = Holder::Entity(e.id());
            AMMO.set(host, holder, ammo.clone())?;
            FLIGHT.set(host, holder, Flight::default())?;
            shots.push(e);
        }
        Ok::<_, AttachmentError>(shots)
    }) else {
        return false;
    };
    shots.retain(|e| e.added_to_level(host));
    let Some(first) = shots.first() else {
        return false;
    };
    if recovery > 0.0
        && FLIGHT
            .set(
                host,
                Holder::Entity(first.id()),
                Flight {
                    recovery,
                    left_owner: false,
                },
            )
            .is_err()
    {
        for shot in shots {
            shot.remove(host, mod_api::RemovalReason::Discarded);
        }
        return false;
    }
    // Join listeners can change the held items. Check the captured stacks and selection
    // before debiting, then use only physical slots, even if an inventory listener changes
    // the selection during payment. Inventory and marker edits are journaled with tank air.
    let payment = host.transaction(|host, tx| {
        let slots = player.inventory_slots(host).ok_or(())?;
        let at = |slot| slots.iter().find(|(i, _)| *i == slot).map(|(_, s)| s);
        let other_slot = if main { OFFHAND_SLOT } else { selected_slot };
        if ((cannon_slot == selected_slot || ammo_slot == selected_slot || other_cannon.is_some())
            && player.selected_slot(host) != Some(selected_slot as usize))
            || at(cannon_slot) != Some(cannon)
            || at(ammo_slot) != Some(&ammo_stack)
            || other_cannon
                .as_ref()
                .is_some_and(|other| at(other_slot) != Some(other))
        {
            return Err(());
        }
        // With no worn air, the pin has no native damage operation for the captured slot.
        // Don't apply wear to a potentially different item in the currently selected hand.
        let warnings = equipment::debit_backtank(host, tx, player, MAX_USES)?;
        let marker = |host: &mut dyn ServerHost, slot, expected: &ItemStack, value| {
            let place = ItemPlace::PlayerSlot {
                player: player.uuid(),
                slot,
            };
            let mut matched = false;
            place
                .edit(host, tx, |stack| {
                    if stack == expected {
                        matched = true;
                        set_custom(stack, &[(SWAP, value)]);
                    }
                })
                .map_err(|_| ())?;
            matched.then_some(()).ok_or(())
        };
        if main && other_cannon.is_some() {
            marker(host, selected_slot as i32, cannon, Some(Tag::Byte(1)))?;
        } else if !main {
            if custom(cannon).contains_key(&SWAP.into()) {
                marker(host, -106, cannon, None)?;
            }
            if let Some(other) = other_cannon.as_ref()
                && custom(other).contains_key(&SWAP.into())
            {
                marker(host, selected_slot as i32, other, None)?;
            }
        }
        if !creative(host, player) {
            // This write can notify synchronously on the pin. Make it the last journal
            // write so that listener sees the completed payment, not just the ammo debit.
            player
                .set_inventory_slot(host, ammo_slot, ammo_stack.shrink(1))
                .map_err(|_| ())?;
        }
        Ok(warnings)
    });
    let Ok(warnings) = payment else {
        for shot in shots {
            shot.remove(host, mod_api::RemovalReason::Discarded);
        }
        return false;
    };
    if let Some(warnings) = warnings {
        equipment::warn_air(host, player, warnings);
    }
    let cooldown = potato::cooldown(t, other_cannon.is_some());
    if cooldown > 0 {
        player.set_item_cooldown(host, CANNON, cooldown);
    }
    // `PotatoCannonPacket` (`beforeShoot`): two crumbs of the ammunition and two puffs of air
    // from the barrel along the look.
    let look_n = {
        let l = (look[0] * look[0] + look[1] * look[1] + look[2] * look[2])
            .sqrt()
            .max(1e-9);
        look.map(|v| v / l)
    };
    for _ in 0..2 {
        let r: [f32; 6] = std::array::from_fn(|_| random(host, level));
        let off = |i: usize, radius: f64| (f64::from(r[i]) - 0.5) * 2.0 * radius;
        if let Some(key) = ITEM_PARTICLE.get() {
            let v = [0, 1, 2].map(|i| look_n[i] * 0.1 + off(i, 0.025));
            let options = mod_api::ParticleOptions::item_stack(*key, shot_stack.clone());
            let _ = level.send_particles(host, &options, None, barrel, v, 1.0, 0);
        }
        if let Some(key) = AIR_PARTICLE.get() {
            let v = [0, 1, 2].map(|i| look_n[i] * 2.0 + off(i + 3, 0.5));
            let options = mod_api::ParticleOptions::simple(*key);
            let _ = level.send_particles(host, &options, None, barrel, v, 1.0, 0);
        }
    }
    // `PotatoCannonPacket`: `FWOOMP` at the barrel for everyone.
    level.play_sound(
        host,
        "create:fwoomp",
        barrel,
        SoundSource::Players,
        1.0,
        pitch_shift,
        None,
    );
    true
}

/// `PotatoProjectileEntity.tick`: follow a stuck target or sweep the flight vector.
fn fly(host: &mut dyn ServerHost, level: LevelRef, e: EntityRef) {
    let holder = Holder::Entity(e.id());
    let (Ok(Some(Some(ammo))), Ok(Some(mut flight))) = (
        AMMO.get_existing(host, holder),
        FLIGHT.get_existing(host, holder),
    ) else {
        e.remove(host, mod_api::RemovalReason::Discarded);
        return;
    };
    let (Some(from), Some(velocity)) = (e.position(host), e.velocity(host)) else {
        return;
    };
    if let Ok(Some(stuck)) = STUCK.get_existing(host, holder) {
        let follow = *stuck.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(mut follow) = follow {
            if alive(host, follow.target)
                && let Some(at) = follow.target.position(host)
            {
                if from[1] < at[1] - 0.1 {
                    pop(host, level, &ammo, from);
                    e.remove(host, mod_api::RemovalReason::Discarded);
                    return;
                }
                follow.fall += 0.007 * f64::from(ammo.kind.gravity);
                follow.offset[1] -= follow.fall;
                let to = std::array::from_fn(|i| at[i] + follow.offset[i]);
                // A refused follow leaves the last successful offset in place; retry next tick.
                if e.teleport_to(host, to, None) {
                    *stuck.lock().unwrap_or_else(|err| err.into_inner()) = Some(follow);
                }
                return;
            }
            *stuck.lock().unwrap_or_else(|err| err.into_inner()) = None;
        }
    }

    let motion = potato::fly(velocity, ammo.kind);
    let to = std::array::from_fn(|i| from[i] + motion[i]);
    if let Some(block) = level.clip(
        host,
        from,
        to,
        mod_api::ClipBlock::Collider,
        mod_api::ClipFluid::None,
    ) {
        let owner = e.owner(host);
        let had_left = flight.left_owner;
        let candidate = ahead(
            host,
            level,
            e,
            [from, block.location],
            motion,
            owner,
            &mut flight.left_owner,
        );
        if flight.left_owner != had_left && FLIGHT.set(host, holder, flight).is_err() {
            return;
        }
        if let Some(hit) = candidate {
            match hit_entity(host, level, e, hit, owner, &ammo, flight) {
                Impact::Gone => {
                    e.remove(host, mod_api::RemovalReason::Discarded);
                    return;
                }
                Impact::Stick(follow) => {
                    if let Ok(stuck) = STUCK.get(host, holder) {
                        *stuck.lock().unwrap_or_else(|err| err.into_inner()) = Some(follow);
                        e.set_velocity(host, [0.0; 3]);
                        return;
                    }
                }
                Impact::Continue => {}
            }
        } else if !block.miss {
            pop(host, level, &ammo, block.location);
            if !block_hit(host, level, &ammo, block) {
                leave_item(host, level, e, &ammo, flight);
            }
            e.remove(host, mod_api::RemovalReason::Discarded);
            return;
        }
    }
    // An unanswered clip isn't evidence of either kind of hit. Keep flying, including
    // above the build ceiling; don't publish any impact on an unanswered segment.
    let _ = e.teleport_to(host, to, None);
    e.set_velocity(host, motion);
}

/// `AbstractHurtingProjectile.tick`'s entity hit, with the ray already clipped against blocks.
fn ahead(
    host: &dyn ServerHost,
    level: LevelRef,
    e: EntityRef,
    [from, to]: [[f64; 3]; 2],
    motion: [f64; 3],
    owner: Option<EntityRef>,
    left_owner: &mut bool,
) -> Option<(EntityRef, [f64; 3])> {
    // `getBoundingBox().expandTowards(motion).inflate(1.0)`: the shot's box is 0.25 a side.
    let mut lo = [from[0] - 0.125, from[1], from[2] - 0.125];
    let mut hi = [from[0] + 0.125, from[1] + 0.25, from[2] + 0.125];
    for i in 0..3 {
        if motion[i] < 0.0 {
            lo[i] += motion[i];
        } else {
            hi[i] += motion[i];
        }
        lo[i] -= 1.0;
        hi[i] += 1.0;
    }
    let owner_root = owner
        .filter(|_| !*left_owner)
        .map(|o| o.root_vehicle(host).unwrap_or(o));
    let mut near_owner = false;
    let candidates = level
        .entities_in(host, lo, hi)?
        .into_iter()
        .filter_map(|o| {
            if o == e || !pickable(host, o) {
                return None;
            }
            if owner_root.is_some_and(|r| o.root_vehicle(host).unwrap_or(o) == r) {
                near_owner = true;
                return None;
            }
            Some((o, o.bounding_box(host)?))
        });
    let hit = potato::entity_hit(from, to, candidates, potato::HIT_MARGIN);
    *left_owner = *left_owner || !near_owner;
    hit
}

fn pickable(host: &dyn ServerHost, e: EntityRef) -> bool {
    if let Some(living) = host.living(e) {
        return living.alive(host) == Some(true) && e.spectator(host) != Some(true);
    }
    host.minecart(e).is_some()
        || e.kind(host).is_some_and(|kind| {
            PICKABLE_TYPES
                .get()
                .is_some_and(|kinds| kinds.contains(&kind))
        })
}

/// `pop`: the item's crumbs and `POTATO_HIT` where it hit.
fn pop(host: &mut dyn ServerHost, level: LevelRef, ammo: &Ammo, at: [f64; 3]) {
    if let Some(key) = ITEM_PARTICLE.get() {
        let options = ParticleOptions::item_stack(*key, ammo.stack.clone());
        level.send_particles(host, &options, None, at, [0.0; 3], 0.25, 7);
    }
    level.play_sound(
        host,
        "create:potato_hit",
        at,
        SoundSource::Neutral,
        1.0,
        1.0,
        None,
    );
}

/// `recoverItem` or `spawnAtLocation(type.dropStack())` where the shot is.
fn leave_item(
    host: &mut dyn ServerHost,
    level: LevelRef,
    projectile: EntityRef,
    ammo: &Ammo,
    mut flight: Flight,
) {
    let chance = std::mem::take(&mut flight.recovery);
    let recovered = chance > 0.0
        && FLIGHT
            .set(host, Holder::Entity(projectile.id()), flight)
            .is_ok()
        && level
            .with_random(host, |r| r.next_double())
            .is_some_and(|roll| potato::recovers(chance, roll));
    let stack = if recovered {
        Some(ammo.stack.clone())
    } else {
        ammo.kind.drop.and_then(|id| {
            let key = host.item_key(&Identifier::parse(id).ok()?)?;
            ItemStack::new(key, 1)
        })
    };
    if let Some(stack) = stack {
        let _ = projectile.spawn_at_location(host, stack, 0.0);
    }
}

enum Impact {
    Gone,
    Continue,
    Stick(Stuck),
}

fn hit_entity(
    host: &mut dyn ServerHost,
    level: LevelRef,
    projectile: EntityRef,
    (target, at): (EntityRef, [f64; 3]),
    owner: Option<EntityRef>,
    ammo: &Ammo,
    flight: Flight,
) -> Impact {
    if !alive(host, target) {
        return Impact::Continue;
    }
    let kind = target.kind(host).map(|k| k.id(host));
    pop(host, level, ammo, at);
    // A powered wither (at half its health or less) takes nothing.
    if kind.as_deref() == Some("minecraft:wither")
        && let Some(l) = host.living(target)
        && let (Some(h), Some(max)) = (l.health(host), l.max_health(host))
        && h <= max / 2.0
    {
        return Impact::Gone;
    }
    if let Some(ticks) = action_arg(ammo.kind, "set_on_fire").and_then(|a| a.parse().ok()) {
        target.set_remaining_fire_ticks(host, ticks);
    }
    let (damage, invulnerable) = potato::hit(ammo.kind);
    if target.hurt(host, CANNON, damage, Some(projectile), owner) != Some(true) {
        return Impact::Gone;
    }
    if kind.as_deref() == Some("minecraft:enderman") {
        return Impact::Continue;
    }
    if !entity_hit(host, level, ammo, target) {
        leave_item(host, level, projectile, ammo, flight);
    }
    if host.living(target).is_none() {
        return Impact::Gone;
    }
    if let Some(ticks) = invulnerable {
        target.set_invulnerable_time(host, ticks);
    }
    // `AllAdvancements.POTATO_CANNON`: a monster killed, or another player hit.
    let monster =
        !alive(host, target) && target.classification(host) == Some(mod_api::MobCategory::Monster);
    let other_player = kind.as_deref() == Some("minecraft:player") && Some(target) != owner;
    if (monster || other_player)
        && let Some(owner) = owner
            .and_then(|o| o.uuid(host))
            .and_then(|uuid| host.player(uuid))
    {
        super::super::advancements::award(host, owner, "potato_cannon");
    }
    if ammo.kind.sticky
        && alive(host, target)
        && let (Some(target_position), Some(position)) =
            (target.position(host), projectile.position(host))
    {
        return Impact::Stick(Stuck {
            target,
            offset: std::array::from_fn(|i| position[i] - target_position[i]),
            fall: 0.0,
        });
    }
    Impact::Gone
}

/// The argument of the type's action named `name` (`"set_on_fire 60"` gives `"60"`).
fn action_arg(t: &Type, name: &str) -> Option<&'static str> {
    let action = t.action?;
    let rest = action.strip_prefix(name)?;
    Some(rest.trim_start())
}

/// `MobEffectInstance(effect, ticks, amplifier)` on `target`.
fn give(host: &mut dyn ServerHost, target: EntityRef, effect: &str, ticks: i32, amplifier: i32) {
    let (Some(living), Some(key)) = (host.living(target), host.effect_key(effect)) else {
        return;
    };
    living.add_effect(
        host,
        &ActiveEffect::new(key, ticks, amplifier, false, true, true),
    );
}

/// `onEntityHit`: true when the type took care of the item (nothing recovered or dropped).
fn entity_hit(host: &mut dyn ServerHost, level: LevelRef, ammo: &Ammo, target: EntityRef) -> bool {
    let t = ammo.kind;
    let Some(action) = t.action else {
        return false;
    };
    let mut words = action.split_whitespace();
    match words.next() {
        // `PotionEffect`: recoverable for all but the glow berry.
        Some("potion_effect") => {
            let (Some(effect), Some(level), Some(ticks)) = (
                words.next(),
                words.next().and_then(|w| w.parse::<i32>().ok()),
                words.next().and_then(|w| w.parse::<i32>().ok()),
            ) else {
                return false;
            };
            give(host, target, effect, ticks, level - 1);
            t.id == "glow_berry"
        }
        // `FoodEffects` of the food, not recoverable.
        Some("food_effects") => {
            let effects: &[(&str, i32, i32)] = match t.id {
                "enchanted_golden_apple" => &[
                    ("minecraft:regeneration", 400, 1),
                    ("minecraft:resistance", 6000, 0),
                    ("minecraft:fire_resistance", 6000, 0),
                    ("minecraft:absorption", 2400, 3),
                ],
                "pufferfish" => &[
                    ("minecraft:poison", 1200, 1),
                    ("minecraft:hunger", 300, 2),
                    ("minecraft:nausea", 300, 0),
                ],
                _ => &[],
            };
            for (effect, ticks, amplifier) in effects {
                give(host, target, effect, *ticks, *amplifier);
            }
            true
        }
        // The regular golden-apple effects are available. The weakened villager's live
        // conversion is not; don't replace it by rewriting a live mob's saved data.
        Some("cure_zombie_villager") => {
            let weak = host.effect_key("minecraft:weakness").is_some_and(|k| {
                host.living(target)
                    .is_some_and(|l| l.effect(host, k).is_some())
            });
            let zombie = target
                .kind(host)
                .is_some_and(|k| k.id(host) == "minecraft:zombie_villager");
            if zombie && weak {
                return false;
            }
            give(host, target, "minecraft:regeneration", 100, 1);
            give(host, target, "minecraft:absorption", 2400, 0);
            true
        }
        Some("chorus_teleport") => {
            let diameter = words
                .next()
                .and_then(|w| w.parse::<f64>().ok())
                .unwrap_or(20.0);
            chorus(host, level, target, diameter)
        }
        Some("suspicious_stew") => {
            for (effect, ticks) in stew_effects(&ammo.stack) {
                give(host, target, &effect, ticks, 0);
            }
            false
        }
        _ => false,
    }
}

/// `SuspiciousStewEffects` of the stack: each effect and its duration.
fn stew_effects(stack: &ItemStack) -> Vec<(String, i32)> {
    let list = stack.components().and_then(|c| {
        c.as_nbt()
            .get(&"minecraft:suspicious_stew_effects".into())
            .cloned()
    });
    let Some(Tag::List(list)) = list else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| {
            let Tag::Compound(c) = e else {
                return None;
            };
            let id = match c.get(&"id".into())? {
                Tag::String(s) => s.to_string_lossy(),
                _ => return None,
            };
            let ticks = match c.get(&"duration".into()) {
                Some(Tag::Int(d)) => *d,
                // `SuspiciousEffectHolder.EFFECT_DURATION`'s default.
                _ => 160,
            };
            Some((id, ticks))
        })
        .collect()
}

/// `ChorusTeleport`: up to sixteen tries of `randomTeleport` within `diameter`.
fn chorus(host: &mut dyn ServerHost, level: LevelRef, target: EntityRef, diameter: f64) -> bool {
    let (Some(living), Some(at)) = (host.living(target), target.position(host)) else {
        return false;
    };
    let fox = target.kind(host).map(|k| k.id(host)).as_deref() == Some("minecraft:fox");
    let sound = if fox {
        "minecraft:entity.fox.teleport"
    } else {
        "minecraft:item.chorus_fruit.teleport"
    };
    let span = diameter as i32;
    for _ in 0..16 {
        let (rx, rz) = (
            f64::from(random(host, level)),
            f64::from(random(host, level)),
        );
        let ry = f64::from((random(host, level) * span.max(1) as f32) as i32);
        let to = [
            at[0] + (rx - 0.5) * diameter,
            (at[1] + ry - f64::from(span / 2)).max(0.0),
            at[2] + (rz - 0.5) * diameter,
        ];
        if living.random_teleport(host, to, None) {
            level.play_sound(host, sound, at, SoundSource::Players, 1.0, 1.0, None);
            target.set_velocity(host, [0.0; 3]);
            return true;
        }
    }
    false
}

/// `onBlockHit`: true when the type took care of the item.
fn block_hit(host: &mut dyn ServerHost, level: LevelRef, ammo: &Ammo, hit: ClipHit) -> bool {
    let Some(action) = ammo.kind.action else {
        return false;
    };
    let mut words = action.split_whitespace();
    let (Some(kind), Some(block)) = (words.next(), words.next()) else {
        return false;
    };
    let face = match hit.face {
        BlockFace::Down => [0, -1, 0],
        BlockFace::Up => [0, 1, 0],
        BlockFace::North => [0, 0, -1],
        BlockFace::South => [0, 0, 1],
        BlockFace::West => [-1, 0, 0],
        BlockFace::East => [1, 0, 0],
        _ => return false,
    };
    let place = std::array::from_fn(|i| hit.block[i] + face[i]);
    let is_air = |host: &dyn Host, pos| {
        level.block(host, pos).is_none_or(|b| {
            matches!(
                b.id(host).as_str(),
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            )
        })
    };
    let replaceable = is_air(host, place)
        || level
            .block(host, place)
            .is_some_and(|b| host.block_in_tag(b, "minecraft:replaceable") == Some(true));
    match kind {
        // `PlantCrop`: on top of farmland.
        "plant_crop" => {
            if face != [0, 1, 0] || !replaceable {
                return false;
            }
            let farmland = level
                .block(host, hit.block)
                .is_some_and(|b| b.id(host) == "minecraft:farmland");
            if farmland && let Some(state) = host.block_state_id(block) {
                let _ = level.set_block(host, place, state, super::super::server::update_all());
            }
            true
        }
        // `PlaceBlockOnGround`: set on top, else falling beside where it hit.
        "place_block_on_ground" => {
            if !replaceable {
                return false;
            }
            if face == [0, 1, 0] {
                if let Some(state) = host.block_state_id(block) {
                    let _ = level.set_block(host, place, state, super::super::server::update_all());
                }
            } else {
                let mut y = hit.location[1] - 0.5;
                if !is_air(host, [place[0], place[1] + 1, place[2]]) {
                    y = y.min(f64::from(place[1]));
                }
                if !is_air(host, [place[0], place[1] - 1, place[2]]) {
                    y = y.max(f64::from(place[1]));
                }
                let nbt = Compound::from([
                    (
                        "BlockState".into(),
                        Tag::Compound(Compound::from([("Name".into(), Tag::String(block.into()))])),
                    ),
                    ("Time".into(), Tag::Int(1)),
                ]);
                let at = [f64::from(place[0]) + 0.5, y, f64::from(place[2]) + 0.5];
                if let Some(dimension) = level.dimension().id(host) {
                    host.summon(&dimension, "minecraft:falling_block", at, nbt);
                }
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::prelude::ItemsHost;

    #[test]
    fn a_saved_flight_keeps_recovery_and_the_owner_exit_flag() {
        let flight = Flight {
            recovery: 0.375,
            left_owner: true,
        };
        let codec = FLIGHT.codec.as_ref().unwrap();
        let saved = (codec.encode)(&flight).unwrap();
        let loaded = (codec.decode)(&saved).unwrap();
        assert_eq!(loaded.recovery, 0.375);
        assert!(loaded.left_owner);
        let spent = (codec.encode)(&Flight {
            recovery: 0.0,
            ..loaded
        })
        .unwrap();
        assert_eq!((codec.decode)(&spent).unwrap().recovery, 0.0);
    }

    #[test]
    fn saved_ammo_must_be_a_known_single_round() {
        let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
        let mut host = mod_api::testing::FakeHost::new();
        let id = Identifier::parse("minecraft:potato").unwrap();
        host.registries.resolve_items([id.clone()]).unwrap();
        let key = host.item_key(&id).unwrap();
        AMMO_KEYS.get_or_init(|| HashMap::from([("minecraft:potato", key)]));
        let codec = AMMO.codec.as_ref().unwrap();
        assert!((codec.decode)(&Compound::new()).is_err());
        let saved = ItemStack::of(key).to_nbt(&host).unwrap();
        let decoded = (codec.decode)(&saved).unwrap().unwrap();
        assert_eq!(decoded.stack, ItemStack::of(key));
        assert_eq!((codec.encode)(&Some(decoded)).unwrap(), saved);
        for bad in [
            Tag::Int(0),
            Tag::Int(-1),
            Tag::Int(2),
            Tag::String("1".into()),
        ] {
            let mut invalid = saved.clone();
            invalid.insert("count".into(), bad);
            assert!((codec.decode)(&invalid).is_err());
        }
        let mut unknown = saved;
        unknown.insert("id".into(), Tag::String("minecraft:stone".into()));
        assert!((codec.decode)(&unknown).is_err());
    }

    #[test]
    fn saved_flight_refuses_bad_recovery_and_owner_exit() {
        let codec = FLIGHT.codec.as_ref().unwrap();
        for chance in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 1.1] {
            let mut saved = (codec.encode)(&Flight::default()).unwrap();
            saved.insert("Recovery".into(), Tag::Float(chance.to_bits()));
            assert!((codec.decode)(&saved).is_err());
        }
        for invalid in [Tag::Byte(2), Tag::Byte(-1), Tag::Int(1)] {
            let mut saved = (codec.encode)(&Flight::default()).unwrap();
            saved.insert("LeftOwner".into(), invalid);
            assert!((codec.decode)(&saved).is_err());
        }
    }

    #[test]
    fn actions_read_their_arguments() {
        let baked = potato::type_for("minecraft:baked_potato").unwrap();
        assert_eq!(action_arg(baked, "set_on_fire"), Some("60"));
        assert_eq!(action_arg(baked, "plant_crop"), None);
        assert_eq!(action_arg(potato::fallback(), "set_on_fire"), None);
    }
}
