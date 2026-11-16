//! The wand of symmetry: its click moves the mirror (`SymmetryWandItem.useOn`), its use in the
//! air turns it off (`use`), its sneaking use opens its screen on the client
//! (`symmetry_screen`, `openWandGUI`; the mirror comes back in `ConfigureSymmetryWandPacket`),
//! and every enabled wand in the hotbar copies a player's placements and breaks to their mirror
//! images (`SymmetryHandler`). The mirror is kept in the stack's custom data under Create's
//! component names (`SYMMETRY_WAND`, `SYMMETRY_WAND_ENABLE`).
//!
//! The client draws each enabled mirror of a wand in its hotbar (`SymmetryHandler.onRenderWorld`:
//! the mirror's model bobbing over its block, turned to its orientation) and sends up end rod
//! particles from it every ten ticks (`onClientTick`). A client reads no inventory slots of its
//! own player, so the server tells it the hotbar's mirrors (`MIRRORS`), every ten ticks when they
//! changed and at once when a click or the screen moves one. The model is drawn with the light
//! of the block the mirror stands in when the client can read it (`LevelRef::brightness`
//! answers on the logical server only), else as in open daylight.

use super::super::net;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::symmetry_screen::{self as screen, Configure};
use super::{Item, creative, custom, held, is, set_custom, set_held, sneaking};
use create_engine::symmetry::{self, ENABLE_KEY, Kind, MIRROR_KEY, Wand};
use mod_api::{
    BlockState, BreakBlockEvent, ClientHost, Compound, EntityPlaceEvent, Hand, Host,
    HostBlockStates, HostSide, Identifier, InteractionResult, ItemPart, ItemStack, LightCoords,
    LightLayer, ModelInstance, ModelRef, Overlay, ParticleKey, ParticleOptions, PlayerRef,
    RenderLevelStage, RenderLevelStageEvent, ServerHost, Tag, UseItemEvent, UseOnEvent,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

/// The wand's item (its cooldown group).
const WAND: &str = "create:wand_of_symmetry";

pub(super) struct WandPart;

#[mod_api::horizon_part]
impl ItemPart for WandPart {
    /// `SymmetryWandItem.useOn`: sneaking opens its screen on the client; otherwise (main hand)
    /// the mirror moves before the clicked face.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => {
                // `getCooldowns().addCooldown(this, 5)`.
                if let Some(player) = host.player(event.player) {
                    player.set_item_cooldown(host, WAND, symmetry::COOLDOWN as i32);
                }
                clicked(host, event);
            }
            HostSide::Client(host) => {
                if client_sneaking(&*host) {
                    screen::request(event.stack.clone(), event.hand);
                }
            }
        }
        Some(InteractionResult::Success)
    }

    /// `SymmetryWandItem.use`: sneaking opens its screen on the client; otherwise the mirror
    /// off.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => {
                // `getCooldowns().addCooldown(this, 5)`.
                if let Some(player) = host.player(event.player) {
                    player.set_item_cooldown(host, WAND, symmetry::COOLDOWN as i32);
                }
                if let Some(player) = host.player(event.player)
                    && !sneaking(&*host, player)
                {
                    let mut stack = event.stack.clone();
                    let mut w = wand(&stack);
                    w.used();
                    set_wand(&mut stack, &w);
                    set_held(host, player, event.hand, Some(stack));
                    sync_player(host, player, true);
                }
            }
            HostSide::Client(host) => {
                if client_sneaking(&*host) {
                    screen::request(event.stack.clone(), event.hand);
                }
            }
        }
        Some(InteractionResult::Success)
    }
}

fn client_sneaking(host: &dyn ClientHost) -> bool {
    host.local_player()
        .and_then(|p| p.entity().is_shift_key_down(host))
        == Some(true)
}

fn clicked(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<()> {
    let player = host.player(event.player)?;
    if sneaking(&*host, player) || event.hand == Hand::Off {
        return None;
    }
    let entity = player.entity();
    let yaw = entity
        .head_yaw(&*host)
        .or_else(|| entity.rotation(&*host).map(|r| r[0]))?;
    let (p, f) = (event.pos, event.face);
    let at = [p[0] + f[0], p[1] + f[1], p[2] + f[2]];
    let mut stack = event.stack.clone();
    let mut w = wand(&stack);
    w.clicked(at, symmetry::faces_z(yaw), yaw);
    set_wand(&mut stack, &w);
    set_held(host, player, event.hand, Some(stack));
    sync_player(host, player, true);
    Some(())
}

fn flag(t: Option<&Tag>) -> bool {
    matches!(t, Some(Tag::Byte(b)) if *b != 0)
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The wand's mirror and its `SYMMETRY_WAND_ENABLE` (`checkComponents`: an empty mirror at the
/// origin, disabled, when it has none).
pub(super) fn wand(stack: &ItemStack) -> Wand {
    let c = custom(stack);
    let Some(Tag::Compound(m)) = c.get(&MIRROR_KEY.into()) else {
        return Wand::blank();
    };
    let position = match m.get(&"position".into()) {
        Some(Tag::List(v)) if v.len() == 3 => {
            let f = |t: &Tag| match t {
                Tag::Double(b) => f64::from_bits(*b),
                _ => 0.0,
            };
            [f(&v[0]), f(&v[1]), f(&v[2])]
        }
        _ => [0.0; 3],
    };
    let mut w = Wand {
        kind: Kind::parse(&text(m.get(&"type".into())).unwrap_or_default()),
        orientation: 0,
        position,
        mirror_enable: flag(m.get(&"enable".into())),
        enabled: flag(c.get(&ENABLE_KEY.into())),
    };
    let orientation = match m.get(&"orientation_index".into()) {
        Some(Tag::Int(i)) => (*i).max(0) as usize,
        _ => 0,
    };
    w.set_orientation(orientation);
    w
}

fn set_wand(stack: &mut ItemStack, w: &Wand) {
    let m = Compound::from([
        ("orientation_index".into(), Tag::Int(w.orientation as i32)),
        (
            "position".into(),
            Tag::List(
                w.position
                    .iter()
                    .map(|v| Tag::Double(v.to_bits()))
                    .collect(),
            ),
        ),
        ("type".into(), Tag::String(w.kind.name().into())),
        ("enable".into(), Tag::Byte(i8::from(w.mirror_enable))),
    ]);
    set_custom(
        stack,
        &[
            (MIRROR_KEY, Some(Tag::Compound(m))),
            (ENABLE_KEY, Some(Tag::Byte(i8::from(w.enabled)))),
        ],
    );
}

/// The wands in the hotbar (`SymmetryHandler`: every hotbar slot holding one).
fn hotbar_wands(host: &dyn Host, player: PlayerRef) -> Vec<Wand> {
    player
        .inventory_slots(host)
        .unwrap_or_default()
        .iter()
        .filter(|(slot, s)| *slot < 9 && is(Some(s), Item::WandOfSymmetry))
        .map(|(_, s)| wand(s))
        .collect()
}

/// `state` turned as a mirror turns it.
fn turned(host: &dyn Host, state: BlockState, turns: &[symmetry::Turn]) -> BlockState {
    let Some(text) = host.block_state_string(state) else {
        return state;
    };
    let (name, own) = states::split(&text);
    let before: BTreeMap<String, String> = own
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let mut props = before.clone();
    for t in turns {
        t.apply(name, &mut props);
    }
    let facts = host.block_states();
    let mut next = state;
    for (k, v) in &props {
        if before.get(k) != Some(v) {
            next = next.with(&facts, k, v).unwrap_or(next);
        }
    }
    next
}

/// `player.getInventory()` giving up one item of `item` (`ItemHelper.extract`'s first match).
fn take_one(host: &mut dyn ServerHost, player: PlayerRef, item: mod_api::ItemKey) -> bool {
    let slots = player.inventory_slots(&*host).unwrap_or_default();
    let Some((slot, stack)) = slots.into_iter().find(|(_, s)| s.item() == item) else {
        return false;
    };
    player
        .set_inventory_slot(host, slot, stack.shrink(1))
        .is_ok()
}

/// The player a placement or a break is by.
fn player_of(host: &mut dyn ServerHost, placer: Option<mod_api::EntityRef>) -> Option<PlayerRef> {
    let uuid = placer?.uuid(host)?;
    host.player(uuid)
}

/// `SymmetryHandler.onBlockPlaced` → `SymmetryWandItem.apply`: each enabled wand in the hotbar
/// copies the placed state, turned, to its mirror images (creative: over anything; survival:
/// only into replaceable, breakable blocks, paying one of the block's item each).
pub(super) fn placed(host: &mut dyn ServerHost, event: &EntityPlaceEvent) -> Option<()> {
    let player = player_of(host, event.placer)?;
    let wands = hotbar_wands(&*host, player);
    if wands.is_empty() {
        return None;
    }
    // `BlockItem.BY_BLOCK`: the block has an item of its own.
    let item = host.block_item(event.state.block())?;
    let dimension = player.dimension(&*host)?.to_string();
    let creative = creative(&*host, player);
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    for w in wands {
        for copy in w.copies(pos) {
            let to = turned(&*lv.host, event.state, &copy.turns);
            let p = copy.pos;
            // `world.isUnobstructed(block, position, ..)`: no entity that blocks building stands
            // where the mirrored block would collide.
            if obstructed(&lv, to, p) {
                continue;
            }
            if !creative {
                let Some(here) = lv.block_state(p) else {
                    continue;
                };
                let facts = lv.host.block_states();
                let replaceable = facts.replaceable(here) == Some(true);
                let unbreakable = facts.destroy_speed(here).is_some_and(|h| h < 0.0);
                if !replaceable || unbreakable || !take_one(lv.host, player, item) {
                    continue;
                }
            }
            lv.set_state(p, to, update_all());
        }
    }
    Some(())
}

/// `Level.isUnobstructed`: an entity but an item, an experience orb or a spectator meets the
/// collision shape `state` would have at `pos`.
pub(crate) fn obstructed(lv: &ServerLevel<'_>, state: BlockState, pos: [i32; 3]) -> bool {
    let host = &*lv.host;
    let Some(shape) = lv
        .level
        .block_shape(host, state, pos, mod_api::BlockShapeKind::Collision)
    else {
        return false;
    };
    if shape.is_empty() {
        return false;
    }
    let lo = pos.map(f64::from);
    let hi = pos.map(|v| f64::from(v) + 1.0);
    lv.level
        .entities_in(host, lo, hi)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            !matches!(
                e.kind(host).map(|k| k.id(host)).as_deref(),
                Some("minecraft:item" | "minecraft:experience_orb")
            ) && e.spectator(host) != Some(true)
        })
        .any(|e| {
            e.bounding_box(host).is_some_and(|[min, max]| {
                shape.iter().any(|b| {
                    (0..3).all(|a| {
                        min[a] < f64::from(pos[a]) + b[a + 3] && max[a] > f64::from(pos[a]) + b[a]
                    })
                })
            })
        })
}

/// Whether `state` is air (`BlockState.isAir`).
fn is_air(host: &dyn Host, state: BlockState) -> bool {
    matches!(
        state.block().id(host).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `SymmetryHandler.onBlockDestroyed` → `SymmetryWandItem.remove`: the mirror images of the
/// broken block break too (in survival only where the same block stands, its drops falling at
/// the broken block as Create drops them).
pub(super) fn broken(host: &mut dyn ServerHost, event: &BreakBlockEvent<'_>) -> Option<()> {
    let player = host.players().acting_player()?;
    let wands = hotbar_wands(&*host, player);
    if wands.is_empty() {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let creative = creative(&*host, player);
    let tool = held(&*host, player, Hand::Main);
    let lv = ServerLevel::new(host, &dimension)?;
    let og = event.state.block();
    let at = event.pos.map(|v| f64::from(v) + 0.5);
    for w in wands {
        for copy in w.copies(event.pos) {
            let p = copy.pos;
            let Some(here) = lv.block_state(p) else {
                continue;
            };
            if (!creative && here.block() != og) || is_air(&*lv.host, here) {
                continue;
            }
            let drops = lv
                .level
                .destroy_block(&mut *lv.host, p, !creative, tool.as_ref(), true)
                .ok()
                .flatten()
                .unwrap_or_default();
            for d in drops {
                let _ = lv.level.spawn_item_with_motion(
                    &mut *lv.host,
                    at[0],
                    at[1],
                    at[2],
                    d,
                    [0.0; 3],
                    10,
                );
            }
        }
    }
    Some(())
}

/// `ConfigureSymmetryWandPacket.handle`: the screen's mirror into the wand still in that hand
/// (`configureSettings`), where the wand's mirror stands (the origin for an empty one).
pub(super) fn configure(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(Configure {
        hand,
        kind,
        orientation,
    }) = Configure::decode(data)
    else {
        return;
    };
    let Some(mut stack) = held(&*host, player, hand) else {
        return;
    };
    if !is(Some(&stack), Item::WandOfSymmetry) || kind == Kind::Empty {
        return;
    }
    let mut w = wand(&stack);
    w.kind = kind;
    w.set_orientation(orientation);
    set_wand(&mut stack, &w);
    set_held(host, player, hand, Some(stack));
    sync_player(host, player, true);
}

// ---------------------------------------------------------------------------------------------
// The mirrors a client draws

/// Server to client: the enabled mirrors of the wands in the player's hotbar.
pub(super) const MIRRORS: &str = "create:symmetry_mirrors";

/// A mirror as drawn: its type, orientation and place.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    kind: Kind,
    orientation: usize,
    position: [f64; 3],
}

fn encode(mirrors: &[Shown]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + mirrors.len() * 26);
    out.push(mirrors.len().min(9) as u8);
    for m in mirrors.iter().take(9) {
        out.push(Kind::ALL.iter().position(|k| *k == m.kind).unwrap_or(0) as u8);
        out.push(m.orientation.min(255) as u8);
        for v in m.position {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn decode(data: &[u8]) -> Option<Vec<Shown>> {
    let (&n, mut rest) = data.split_first()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let (head, tail) = rest.split_at_checked(26)?;
        let f = |i: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&head[2 + 8 * i..10 + 8 * i]);
            f64::from_le_bytes(b)
        };
        out.push(Shown {
            kind: *Kind::ALL.get(usize::from(head[0]))?,
            orientation: usize::from(head[1]),
            position: [f(0), f(1), f(2)],
        });
        rest = tail;
    }
    rest.is_empty().then_some(out)
}

thread_local! {
    /// On the server: what each player's client was last sent.
    static SENT: RefCell<HashMap<u128, Vec<Shown>>> = RefCell::new(HashMap::new());
}

/// The enabled, non-empty mirrors of the wands in `player`'s hotbar.
fn shown(host: &dyn Host, player: PlayerRef) -> Vec<Shown> {
    hotbar_wands(host, player)
        .into_iter()
        .filter(|w| w.is_enabled() && w.kind != Kind::Empty)
        .map(|w| Shown {
            kind: w.kind,
            orientation: w.orientation,
            position: w.position,
        })
        .collect()
}

/// Sends `player`'s client its mirrors when they changed (or always, `force`).
fn sync_player(host: &mut dyn ServerHost, player: PlayerRef, force: bool) {
    let now = shown(&*host, player);
    let uuid = player.uuid();
    let changed = SENT.with_borrow_mut(|sent| {
        let before = sent.get(&uuid);
        if !force && before == Some(&now) || before.is_none() && now.is_empty() {
            return false;
        }
        sent.insert(uuid, now.clone());
        true
    });
    if changed {
        net::send_to_player(host, player, MIRRORS, &encode(&now));
    }
}

/// Every ten ticks, the mirrors of the level's players to their clients, when they changed.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    if lv.game_time() % 10 != 0 {
        return;
    }
    for player in lv.players() {
        sync_player(lv.host, player, false);
    }
}

pub(super) fn world_closed() {
    SENT.with_borrow_mut(HashMap::clear);
}

/// The mirrors' models (`AllPartialModels.SYMMETRY_*`), registered as standalone models.
pub(super) const MODELS: [&str; 3] = [
    "create:block/symmetry_effect/plane",
    "create:block/symmetry_effect/crossplane",
    "create:block/symmetry_effect/tripleplane",
];

/// The client's view of the mirrors.
#[derive(Default)]
struct Client {
    mirrors: Vec<Shown>,
    models: HashMap<&'static str, ModelRef>,
    ticks: u64,
    rng: Option<create_engine::processing::Rng>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

static END_ROD: std::sync::OnceLock<ParticleKey> = std::sync::OnceLock::new();

pub(super) fn register(r: &mut dyn mod_api::Registrar) {
    let _ = END_ROD.set(r.particle_key("minecraft:end_rod"));
}

/// `model_baking_completed`: the models looked up once, until the next bake.
pub(super) fn models_baked(host: &dyn ClientHost) {
    let mut found = HashMap::new();
    for id in MODELS {
        let model = Identifier::parse(id)
            .ok()
            .and_then(|i| host.render_assets().model(&i).ok());
        match model {
            Some(m) => {
                found.insert(id, m);
            }
            None => crate::create_log!("[create] the symmetry mirror's model {id} is missing"),
        }
    }
    CLIENT.with_borrow_mut(|c| c.models = found);
}

pub(super) fn reset() {
    CLIENT.with_borrow_mut(|c| {
        c.mirrors.clear();
        c.ticks = 0;
    });
}

pub(super) fn server_message(channel: &str, data: &[u8]) -> bool {
    if channel != MIRRORS {
        return false;
    }
    if let Some(m) = decode(data) {
        CLIENT.with_borrow_mut(|c| c.mirrors = m);
    }
    true
}

/// `SymmetryHandler.onClientTick`: every ten ticks, an end rod particle rising from near the
/// middle of each mirror's block, a quarter up.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let Some(&end_rod) = END_ROD.get() else {
        return;
    };
    let puffs = CLIENT.with_borrow_mut(|c| {
        c.ticks += 1;
        if c.ticks % 10 != 0 || c.mirrors.is_empty() {
            return Vec::new();
        }
        let rng = c
            .rng
            .get_or_insert_with(|| create_engine::processing::Rng::new(0x5EED_5133));
        c.mirrors
            .iter()
            .map(|m| {
                let ox = (f64::from(rng.next_float()) - 0.5) * 0.3;
                let oz = (f64::from(rng.next_float()) - 0.5) * 0.3;
                let up = f64::from(rng.next_float()) / 8.0;
                (
                    [
                        m.position[0] + 0.5 + ox,
                        m.position[1] + 0.25,
                        m.position[2] + 0.5 + oz,
                    ],
                    up,
                )
            })
            .collect()
    });
    for (at, up) in puffs {
        host.add_particle(&ParticleOptions::simple(end_rod), at, [0.0, up, 0.0]);
    }
}

/// `applyModelTransform`'s turn about the block's middle, as a row-major 3×4 transform.
fn model_turn(degrees: f32) -> [f32; 12] {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        c,
        0.0,
        s,
        0.5 - 0.5 * c - 0.5 * s,
        0.0,
        1.0,
        0.0,
        0.0,
        -s,
        0.0,
        c,
        0.5 + 0.5 * s - 0.5 * c,
    ]
}

/// `SymmetryHandler.onRenderWorld`: each mirror's model over its block, `sin(t / 16) / 5`
/// plus 0.2 up, turned to its orientation.
pub(super) fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterOpaqueBlocks {
        return;
    }
    CLIENT.with_borrow(|c| {
        if c.mirrors.is_empty() {
            return;
        }
        let t = event.view.game_time as f64 + f64::from(event.view.partial_tick);
        let bob = (t / 16.0).sin() / 5.0;
        let level = host.level(host.dimension());
        for m in &c.mirrors {
            let Some((id, degrees)) = m.kind.model(m.orientation) else {
                continue;
            };
            let Some(&model) = c.models.get(id) else {
                continue;
            };
            let block = m.position.map(|v| v.floor() as i32);
            let light = level
                .and_then(|l| {
                    let b = l.brightness(host, LightLayer::Block, block)?;
                    let s = l.brightness(host, LightLayer::Sky, block)?;
                    LightCoords::new(b.clamp(0, 15) as u8, s.clamp(0, 15) as u8)
                })
                .unwrap_or(LightCoords::new(0, 15).unwrap_or(LightCoords::FULL_BRIGHT));
            let origin = [
                f64::from(block[0]),
                f64::from(block[1]) + bob + 0.2,
                f64::from(block[2]),
            ];
            let instance = ModelInstance::new(
                origin,
                model_turn(degrees),
                0xFFFF_FFFF,
                light,
                Overlay::NONE,
            );
            if let Err(e) = event.draw.draw_model_instances(model, &[instance]) {
                crate::create_log!("[create] the symmetry mirror could not be drawn: {e:?}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_cross_the_wire() {
        let m = vec![
            Shown {
                kind: Kind::CrossPlane,
                orientation: 1,
                position: [1.0, -64.0, 30_000_000.0],
            },
            Shown {
                kind: Kind::Plane,
                orientation: 0,
                position: [0.0; 3],
            },
        ];
        assert_eq!(decode(&encode(&m)), Some(m));
        assert_eq!(decode(&[1, 0]), None);
        assert_eq!(decode(&encode(&[])), Some(Vec::new()));
    }

    #[test]
    fn a_quarter_turn_keeps_the_block_in_place() {
        let t = model_turn(90.0);
        // The corner (0, 0, 0) goes to (0, 0, 1), (1, 0, 0) to (0, 0, 0).
        let apply = |p: [f32; 3]| {
            [0, 1, 2]
                .map(|r| t[4 * r] * p[0] + t[4 * r + 1] * p[1] + t[4 * r + 2] * p[2] + t[4 * r + 3])
        };
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        assert!(near(apply([0.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(near(apply([1.0, 0.0, 0.0]), [0.0, 0.0, 0.0]));
    }
}
