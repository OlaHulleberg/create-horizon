//! Create's advancements (`AllAdvancements`, `CreateAdvancement`): the files are Create's data
//! generator's (`data/create/advancement`, converted by `create-data advancements`), each
//! completed by its built-in trigger `create:<name>_builtin` (`SimpleCreateTrigger`), which
//! `award` fires where Create calls `awardTo`. The item advancements (`inventory_changed`) are
//! the game's own. The game gives the toast, the chat line and the advancement screen's tab.
//!
//! A block's own awards (`AdvancementBehaviour`) go to the player who placed it
//! (`AdvancementBehaviour.setPlacedBy`): [`Owners`] notes the placer of every Create block a
//! player places, keeps it per block (saved with the column as `create_owners`, carried on a
//! moving structure as the block entity's `Owner`), and [`award_placer`] awards that player
//! when they are in the block's level.

use super::persist;
use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::{level_index, try_with_world};
use create_engine::kinetics::Dir;
use mod_api::{
    Compound, CriterionContext, CriterionTest, CriterionTestError, EntityPlaceEvent, Flow,
    Identifier, PlayerRef, ServerHost, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// `SimpleCreateTrigger`'s test: a built-in trigger has no conditions.
struct Always;

impl CriterionTest for Always {
    fn test(&self, _criterion: &dyn CriterionContext) -> Result<bool, CriterionTestError> {
        Ok(true)
    }
}

/// `CreateAdvancement.awardTo`: the advancement `name` (`belt`, `potato_cannon`) for `player`.
pub(crate) fn award(host: &mut dyn ServerHost, player: PlayerRef, name: &str) {
    let Ok(trigger) = Identifier::parse(&format!("create:{name}_builtin")) else {
        return;
    };
    if let Err(e) = player.trigger_criterion(host, &trigger, Arc::new(Always)) {
        crate::create_log!("[create] the {name} advancement's trigger was refused: {e:?}");
    }
}

thread_local! {
    /// The placer of each block, by level index and position (`AdvancementBehaviour.playerId`).
    static OWNERS: RefCell<HashMap<usize, HashMap<[i32; 3], Owner>>> =
        RefCell::new(HashMap::new());
    /// A placement heard in `entity_place`, before its block is written: (level, pos, player).
    static PENDING: RefCell<Vec<(usize, [i32; 3], u128)>> = const { RefCell::new(Vec::new()) };
    /// Blocks given an owner since the last tick (placed, carried in, loaded), whose first tick
    /// awards what their points or shape earn (`ArmBlockEntity.initInteractionPoints`).
    static FRESH: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    /// The speed each owned pump last turned at, while it turns.
    static PUMP_SPEEDS: RefCell<HashMap<(usize, [i32; 3]), f32>> = RefCell::new(HashMap::new());
    /// How many whistles each owned boiler had at the last look.
    static BOILER_WHISTLES: RefCell<HashMap<(usize, [i32; 3]), usize>> =
        RefCell::new(HashMap::new());
    /// Awards for blocks whose moment had no host to award from, given at the end of the tick:
    /// (level, pos, advancement).
    static LATER: RefCell<Vec<(usize, [i32; 3], &'static str)>> = const { RefCell::new(Vec::new()) };
    /// Awards for players heard of where no host was at hand: (player, advancement).
    static PLAYER_LATER: RefCell<Vec<(u128, &'static str)>> = const { RefCell::new(Vec::new()) };
    /// The section each train's front was in at the last tick, by level and train.
    static LEAD_SECTIONS: RefCell<HashMap<(usize, u32), usize>> = RefCell::new(HashMap::new());
    /// Package item entities thrown by players: entity id, player.
    static TOSSED: RefCell<HashMap<u32, u128>> = RefCell::new(HashMap::new());
    /// Players hiding in cardboard last tick.
    static HIDING: RefCell<std::collections::HashSet<u128>> =
        RefCell::new(std::collections::HashSet::new());
}

/// A block's owner, and what its awards count toward (kept only while it has one, as Create
/// writes `TracksCreated` only with an owner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owner {
    player: u128,
    /// `MechanicalPressBlockEntity.tracksCreated`.
    tracks: u32,
    /// `SpoutBlockEntity.createdChocolateBerries`, `createdHoneyApple`, `createdSweetRoll`:
    /// bits 0, 1 and 2.
    foods: u8,
}

impl Owner {
    fn new(player: u128) -> Owner {
        Owner {
            player,
            tracks: 0,
            foods: 0,
        }
    }
}

/// The column key of the saved owners.
const KEY: &str = "create_owners";
/// The carried block entity's key (`AdvancementBehaviour.write`'s `Owner`).
const OWNER: &str = "Owner";
/// The carried press's `TracksCreated`.
const TRACKS: &str = "TracksCreated";
/// The carried spout's foods (`NBTHelper.putMarker`).
const FOODS: [(&str, &str); 3] = [
    ("create:chocolate_glazed_berries", "ChocolateBerries"),
    ("create:honeyed_apple", "HoneyApple"),
    ("create:sweet_roll", "SweetRoll"),
];

/// The player who placed the block at `pos` of level `dim`, when Create remembers one.
pub(crate) fn owner(dim: usize, pos: [i32; 3]) -> Option<u128> {
    OWNERS.with_borrow(|o| Some(o.get(&dim)?.get(&pos)?.player))
}

/// The block's owner when they are in its level (`AdvancementBehaviour.getPlayer`:
/// `Level.getPlayerByUUID`).
fn present_owner(host: &dyn ServerHost, dim: usize, pos: [i32; 3]) -> Option<PlayerRef> {
    let player = host.player(owner(dim, pos)?)?;
    let at = player
        .dimension(host)
        .and_then(|d| level_index(&d.to_string()))?;
    (at == dim).then_some(player)
}

/// `AdvancementBehaviour.awardPlayer` / `SmartBlockEntity.award` / `tryAward`: advancement
/// `name` for whoever placed the block at `pos` of `lv`.
pub(crate) fn award_placer(lv: &mut ServerLevel<'_>, pos: [i32; 3], name: &str) {
    award_owner(&mut *lv.host, lv.dim, pos, name);
}

/// [`award_placer`] from a host and a level index.
pub(crate) fn award_owner(host: &mut dyn ServerHost, dim: usize, pos: [i32; 3], name: &str) {
    if let Some(player) = present_owner(&*host, dim, pos) {
        award(host, player, name);
    }
}

/// `AdvancementBehaviour.awardPlayerIfNear`: as [`award_placer`], when the placer is within
/// `range` blocks of the block's middle.
pub(crate) fn award_placer_near(lv: &mut ServerLevel<'_>, pos: [i32; 3], name: &str, range: f64) {
    let Some(player) = present_owner(&*lv.host, lv.dim, pos) else {
        return;
    };
    let Some(at) = player.entity().position(&*lv.host) else {
        return;
    };
    let d2: f64 = (0..3)
        .map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2))
        .sum();
    if d2 <= range * range {
        award(lv.host, player, name);
    }
}

/// `BeltDeployerCallbacks.awardAdvancements` / `ManualApplicationRecipe.awardAdvancements`:
/// the advancement for making casing `item`.
fn casing(item: &str) -> Option<&'static str> {
    Some(match item {
        "create:andesite_casing" => "andesite_casing",
        "create:brass_casing" => "brass_casing",
        "create:copper_casing" => "copper_casing",
        "create:railway_casing" => "train_casing_00",
        _ => return None,
    })
}

/// `ManualApplicationRecipe.awardAdvancements`: `player` turned a block into `placed`.
pub(crate) fn award_casing(host: &mut dyn ServerHost, player: PlayerRef, placed: Option<&str>) {
    if let Some(name) = placed.and_then(casing) {
        award(host, player, name);
    }
}

/// `BeltDeployerCallbacks.activate`: the deployer at `pos` applied a recipe whose first result
/// is `result` (`DEPLOYER`, then the casing it made).
pub(crate) fn deployer_applied(lv: &mut ServerLevel<'_>, pos: [i32; 3], result: Option<&str>) {
    award_placer(lv, pos, "deployer");
    if let Some(name) = result.and_then(casing) {
        award_placer(lv, pos, name);
    }
}

/// `MechanicalPressBlockEntity.onItemPressed`: the press at `pos` made `result` (`PRESS`;
/// every thousand tracks, `TRACK_CRAFTING`).
pub(crate) fn pressed(lv: &mut ServerLevel<'_>, pos: [i32; 3], result: Option<(&str, u8)>) {
    let Some((item, count)) = result else {
        return;
    };
    award_placer(lv, pos, "mechanical_press");
    let host = &*lv.host;
    let track = Identifier::parse(create_engine::arm::item_id(item))
        .ok()
        .and_then(|id| host.item_key(&id))
        .and_then(|k| host.item_block(k))
        .and_then(|b| host.block_in_tag(b, "create:tracks"))
        .unwrap_or(false);
    if !track {
        return;
    }
    let made = OWNERS.with_borrow_mut(|o| {
        let u = o.get_mut(&lv.dim)?.get_mut(&pos)?;
        u.tracks += u32::from(count);
        let made = u.tracks >= 1000;
        if made {
            u.tracks = 0;
        }
        Some(made)
    });
    if made.is_some() {
        persist::mark_pos_unsaved(lv.dim, pos);
    }
    if made == Some(true) {
        award_placer(lv, pos, "track_crafting_factory");
    }
}

/// [`pressed`] for a depot's results, when a press (not a deployer) stands over the depot.
pub(crate) fn pressed_on_depot(
    lv: &mut ServerLevel<'_>,
    depot: [i32; 3],
    result: Option<(&str, u8)>,
) {
    let press = [depot[0], depot[1] + 2, depot[2]];
    let dim = lv.dim;
    if super::world::with_world(|w| w.dims[dim].presses.contains_key(&press)) == Some(true) {
        pressed(lv, press, result);
    }
}

/// `CrushingWheelControllerBlock.updateSpeed`: a valid controller beside the wheel at `wheel`
/// (on its side `side`) awards the first wheel around it (in `Direction` order).
pub(crate) fn crushing(lv: &mut ServerLevel<'_>, wheel: [i32; 3], side: Dir) {
    let other = side.offset().map(|v| v * 2);
    let first = if (side.opposite() as usize) < side as usize {
        wheel
    } else {
        [0, 1, 2].map(|i| wheel[i] + other[i])
    };
    award_placer(lv, first, "crushing_wheel");
    if lv.speed(first).abs() > create_engine::kinetics::MAX_SPEED - 1.0 {
        award_placer(lv, first, "crusher_maxed_0000");
    }
}

/// `WaterWheelBlockEntity.determineAndApplyFlowScore`: a wheel at `pos` that some flow turns
/// (`flowing`), lava among the blocks at `offsets` from it making it `LAVA_WHEEL`.
pub(crate) fn water_wheel(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    offsets: impl Iterator<Item = [i32; 3]>,
    flowing: bool,
) {
    if !flowing || owner(lv.dim, pos).is_none() {
        return;
    }
    let host = &*lv.host;
    let lava = offsets
        .filter_map(|o| lv.level.fluid_state(host, [0, 1, 2].map(|i| pos[i] + o[i])))
        .any(|f| {
            matches!(
                &*f.fluid.id(host),
                "minecraft:lava" | "minecraft:flowing_lava"
            )
        });
    award_placer(
        lv,
        pos,
        if lava {
            "lava_wheel_00000"
        } else {
            "water_wheel"
        },
    );
}

/// `CogWheelBlock.triggerShiftingGearsAdvancement`: `player` places a cog (engine state
/// `placed`) beside a cog of the other size on its axis, a step along each of the other two.
fn shifting_gears(host: &mut dyn ServerHost, player: PlayerRef, pos: [i32; 3], placed: u32) {
    use create_engine::Block;
    let cog = |id: u32| {
        try_with_world(|w| match w.table.get(id) {
            Some((b @ (Block::Cogwheel | Block::LargeCogwheel), i)) => {
                Some((b == Block::LargeCogwheel, i.axis?))
            }
            _ => None,
        })
        .flatten()
    };
    let Some((large, axis)) = cog(placed) else {
        return;
    };
    let Some(level) = player.level(&*host) else {
        return;
    };
    let others: Vec<usize> = (0..3).filter(|&a| a != axis as usize).collect();
    for (o1, o2) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
        let mut at = pos;
        at[others[0]] += o1;
        at[others[1]] += o2;
        let id = level
            .block_state(&*host, at)
            .map_or(0, super::states::id_or_none);
        if cog(id).is_some_and(|(l, a)| a == axis && l != large) {
            award(host, player, "shifting_gears");
        }
    }
}

/// `GaugeObservedPacket`'s channel: a client's goggles read a stressometer.
const OBSERVED: &str = "create:observer_stressometer";

/// The goggles' stressometer this frame, the one last told, and each one's dial target.
type GaugeSeen = (Option<[i32; 3]>, Option<[i32; 3]>, HashMap<[i32; 3], f32>);

thread_local! {
    /// On a client: the stressometer the goggles read this frame, the one last told to the
    /// server (`StressGaugeBlockEntity.lastSent`), and each one's last dial target.
    static GAUGE_SEEN: RefCell<GaugeSeen> =
        RefCell::new((None, None, HashMap::new()));
}

/// `StressGaugeBlockEntity.addToGoggleTooltip`, on a client: the goggles read the stressometer
/// at `pos`.
pub(crate) fn gauge_read(pos: [i32; 3]) {
    GAUGE_SEEN.with_borrow_mut(|g| g.0 = Some(pos));
}

/// A client tick of the stressometer at `pos`, its dial's target `dial`: read by the goggles
/// and not yet told, the server hears of it (`GaugeObservedPacket`); new data from the server
/// (`read` with `clientPacket`) lets it be told again.
pub(crate) fn gauge_tick(host: &mut dyn mod_api::ClientHost, pos: [i32; 3], dial: f32) {
    let send = GAUGE_SEEN.with_borrow_mut(|(seen, sent, dials)| {
        if dials.insert(pos, dial).is_some_and(|d| d != dial) && *sent == Some(pos) {
            *sent = None;
        }
        if *seen != Some(pos) {
            return false;
        }
        *seen = None;
        if *sent == Some(pos) {
            return false;
        }
        *sent = Some(pos);
        true
    });
    if send {
        let data: Vec<u8> = pos.iter().flat_map(|v| v.to_le_bytes()).collect();
        super::net::send_to_server(host, OBSERVED, &data);
    }
}

/// `BlockEntityConfigurationPacket.handle`'s checks on a client's message naming a block (its
/// position as three words): not a spectator or an adventure player, the block loaded and
/// within 20 blocks of reach. The player's level and the position.
fn configured(host: &dyn ServerHost, player: PlayerRef, data: &[u8]) -> Option<(String, [i32; 3])> {
    let w: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    let pos = [*w.first()?, *w.get(1)?, *w.get(2)?];
    if player.is_spectator(host) != Some(false) || player.may_build(host) != Some(true) {
        return None;
    }
    let dimension = player.dimension(host)?.to_string();
    let at = player.entity().position(host)?;
    let d2: f64 = (0..3)
        .map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2))
        .sum();
    let loaded = host.level(&dimension)?.is_loaded(host, pos);
    (loaded && d2 <= 25.0 * 25.0).then_some((dimension, pos))
}

/// `GaugeObservedPacket`: `StressGaugeBlockEntity.onObserved`.
fn gauge_observed(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some((dimension, pos)) = configured(&*host, player, data) else {
        return;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    let dial = super::world::with_world(|w| {
        let g = w.dims[dim].gauges.get(&pos)?;
        (g.kind == create_engine::gauge::Kind::Stress).then_some(g.shown.dial)
    })
    .flatten();
    let Some(dial) = dial else {
        return;
    };
    award_placer(&mut lv, pos, "stressometer");
    if (dial - 1.0).abs() < 1e-5 {
        award_placer(&mut lv, pos, "stressometer_maxed");
    }
}

/// `EjectorAwardPacket`'s channel.
const EJECTOR_AWARD: &str = "create:ejector_award";

/// `EjectorBlockEntity.activateDeferred`, on the client it launches: thrown 25 blocks or more,
/// the player is told `EJECTOR_MAXED` (`EjectorAwardPacket`).
pub(crate) fn ejector_launched(host: &mut dyn mod_api::ClientHost, pos: [i32; 3], h: i32, v: i32) {
    if h * h + v * v >= 25 * 25 {
        let data: Vec<u8> = pos.iter().flat_map(|v| v.to_le_bytes()).collect();
        super::net::send_to_server(host, EJECTOR_AWARD, &data);
    }
}

/// `EjectorAwardPacket.applySettings`.
fn ejector_award(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some((dimension, pos)) = configured(&*host, player, data) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    if super::world::with_world(|w| w.dims[dim].ejectors.contains_key(&pos)) == Some(true) {
        award(lv.host, player, "ejector_maxed");
    }
}

/// `SpeedControllerBlockEntity.updateTargetRotation`: a speed controller at `pos` set to a new
/// speed, turning, under its cog (`isCogwheelPresent`).
pub(crate) fn speed_controller(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    use create_engine::{Block, kinetics::Axis};
    let dim = lv.dim;
    let cog = super::world::with_world(|w| {
        w.block_at(dim, pos) == Some(Block::RotationSpeedController)
            && w.state_at(dim, [pos[0], pos[1] + 1, pos[2]])
                .and_then(|s| w.table.get(s))
                .is_some_and(|(b, i)| {
                    b == Block::LargeCogwheel && i.axis.is_some_and(|a| a != Axis::Y)
                })
    });
    if cog == Some(true) && lv.speed(pos) != 0.0 {
        award_placer(lv, pos, "speed_controller");
    }
}

/// `SteamEngineBlockEntity.tick`: each engine driving its shaft with some efficiency.
pub(crate) fn steam_engines(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let engines: Vec<[i32; 3]> = super::world::with_world(|w| {
        w.dims[dim]
            .shaft_engine
            .iter()
            .filter(|(_, e)| owner(dim, **e).is_some())
            .filter(|(s, _)| w.shaft_steam(dim, **s).efficiency > 0.0)
            .map(|(_, e)| *e)
            .collect()
    })
    .unwrap_or_default();
    for e in engines {
        award_placer(lv, e, "steam_engine");
    }
}

/// `BeltFunnelBlock.onWrenched`: the belt funnel at `pos` that `player` just extended meets an
/// extended belt funnel facing it.
pub(crate) fn funnel_kiss(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let belt_funnel = |lv: &ServerLevel<'_>, p: [i32; 3]| -> Option<Dir> {
        let name = lv.block_name(p)?;
        if !matches!(
            &*name,
            "create:andesite_belt_funnel" | "create:brass_belt_funnel"
        ) || lv.property(p, "shape").as_deref() != Some("extended")
        {
            return None;
        }
        Dir::ALL
            .into_iter()
            .find(|d| Some(super::kinetics::dir_name(*d)) == lv.property(p, "facing").as_deref())
    };
    let Some(facing) = belt_funnel(lv, pos) else {
        return;
    };
    let o = facing.offset();
    let opposite = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    if belt_funnel(lv, opposite) == Some(facing.opposite()) {
        award(lv.host, player, "belt_funnel_kiss");
    }
}

/// `ArmBlockEntity.depositItem`: `MECHANICAL_ARM`, and `MUSICAL_ARM` for a whole stack into a
/// jukebox.
pub(crate) fn arm_deposited(lv: &mut ServerLevel<'_>, arm: [i32; 3], musical: bool) {
    if musical {
        award_placer(lv, arm, "musical_arm");
    }
    award_placer(lv, arm, "mechanical_arm");
}

/// `ArmBlockEntity.initInteractionPoints`, on its first tick after it is placed or loaded: ten
/// outputs or more, a blaze burner among its points.
fn arm_points(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    use create_engine::arm::Kind;
    let dim = lv.dim;
    let Some((many, burner)) = super::world::with_world(|w| {
        let a = w.dims[dim].arms.arms.get(&pos)?;
        let burner = a
            .inputs
            .iter()
            .chain(&a.outputs)
            .any(|p| p.kind == Kind::BlazeBurner);
        Some((a.outputs.len() >= 10, burner))
    })
    .flatten() else {
        return;
    };
    if many {
        award_placer(lv, pos, "arm_many_targets");
    }
    if burner {
        award_placer(lv, pos, "arm_blaze_burner");
    }
}

/// `DivingHelmetItem.breatheUnderwater`: `player` breathes by their tank (`DIVING_SUIT`), or
/// dives in lava with one (`DIVING_SUIT_LAVA`).
pub(crate) fn diving(host: &mut dyn ServerHost, player: PlayerRef, breathes: bool, lava: bool) {
    if lava {
        award(host, player, "diving_suit_lava");
    }
    if breathes {
        award(host, player, "diving_suit");
    }
}

/// `CardboardArmorHandler.testForStealth`: crouching, not flying, in four pieces of cardboard.
fn hiding_in_cardboard(host: &dyn ServerHost, player: PlayerRef) -> bool {
    use mod_api::EquipmentSlot as S;
    if player.entity().is_crouching(host) != Some(true)
        || player.abilities(host).is_some_and(|a| a.flying)
    {
        return false;
    }
    let Some(living) = host.living(player.entity()) else {
        return false;
    };
    let pieces = ["helmet", "chestplate", "leggings", "boots"];
    [S::Head, S::Chest, S::Legs, S::Feet]
        .into_iter()
        .zip(pieces)
        .all(|(slot, piece)| {
            living
                .equipment(host, slot)
                .is_some_and(|s| s.item().id(host) == format!("create:cardboard_{piece}"))
        })
}

/// `SmithingMenuMixin`: a piece of cardboard armour taken from a smithing table (trimmed).
pub(crate) fn item_smithed(
    host: &mut dyn ServerHost,
    event: &mut mod_api::PlayerItemSmithedEvent<'_>,
) {
    let id = event.result.item().id(&*host);
    let cardboard = ["helmet", "chestplate", "leggings", "boots"]
        .iter()
        .any(|p| id == format!("create:cardboard_{p}"));
    if cardboard && let Some(player) = host.player(event.player) {
        award(host, player, "cardboard_armor_trim");
    }
}

/// `SpoutBlockEntity.onItemReceived`'s end: the spout at `pos` filled an item into `out`
/// (`SPOUT`; its three foods made, `FOODS`).
pub(crate) fn spouted(lv: &mut ServerLevel<'_>, pos: [i32; 3], out: &str) {
    award_placer(lv, pos, "spout");
    let item = create_engine::arm::item_id(out);
    let Some(bit) = FOODS.iter().position(|(id, _)| *id == item) else {
        return;
    };
    let all = OWNERS.with_borrow_mut(|o| {
        let u = o.get_mut(&lv.dim)?.get_mut(&pos)?;
        u.foods |= 1 << bit;
        Some(u.foods == 0b111)
    });
    if all.is_some() {
        persist::mark_pos_unsaved(lv.dim, pos);
    }
    if all == Some(true) {
        award_placer(lv, pos, "foods");
    }
}

/// After the boilers' tick: `BoilerData.tick`'s sample at level 18 (`STEAM_ENGINE_MAXED`), and
/// `checkPipeOrganAdvancement` for a boiler whose whistles changed (`updateBoilerState`).
pub(crate) fn boilers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((maxed, organs)) = super::world::with_world(|w| {
        let d = &w.dims[dim];
        let mut maxed = Vec::new();
        let mut organs = Vec::new();
        for (tank, b) in &d.boilers {
            if owner(dim, *tank).is_none() {
                continue;
            }
            if b.data.is_active() && b.data.level(b.size) == 18 {
                maxed.push(*tank);
            }
            let n = b.whistles.len();
            if BOILER_WHISTLES.with_borrow_mut(|m| m.insert((dim, *tank), n)) != Some(n) {
                organs.push(*tank);
            }
        }
        Some((maxed, organs))
    })
    .flatten() else {
        return;
    };
    for t in maxed {
        award_placer(lv, t, "steam_engine_maxed");
    }
    for t in organs {
        pipe_organ(lv, t);
    }
}

/// `BoilerData.checkPipeOrganAdvancement` for the boiler at `tank` (its controller), or the one
/// the whistle at `tank` is on: twelve whistles of different pitches on it.
pub(crate) fn pipe_organ(lv: &mut ServerLevel<'_>, at: [i32; 3]) {
    let dim = lv.dim;
    let found = super::world::with_world(|w| {
        let d = &w.dims[dim];
        let tank = match w.whistle_at(dim, at) {
            Some(a) => {
                let o = a.offset();
                [at[0] + o[0], at[1] + o[1], at[2] + o[2]]
            }
            None => at,
        };
        let c = d.tanks.controller(tank)?;
        let b = d.boilers.get(&c)?;
        let pitches: std::collections::HashSet<i32> = b
            .whistles
            .iter()
            .filter_map(|p| d.plumbing.whistles.get(p).map(|w| w.pitch))
            .collect();
        Some((c, pitches.len()))
    })
    .flatten();
    if let Some((c, n)) = found
        && n >= 12
    {
        award_placer(lv, c, "pipe_organ");
    }
}

/// `Contraption.containsBlockBreakers`: a drill, saw, plough, roller or harvester among the
/// blocks (engine states) of a structure.
fn breakers(blocks: &[([i32; 3], u32)]) -> bool {
    use create_engine::Block as B;
    super::world::with_world(|w| {
        blocks.iter().any(|(_, s)| {
            matches!(
                w.table.get(*s).map(|(b, _)| b),
                Some(
                    B::MechanicalDrill
                        | B::MechanicalSaw
                        | B::MechanicalPlough
                        | B::MechanicalRoller
                        | B::MechanicalHarvester
                )
            )
        })
    })
    .unwrap_or(false)
}

/// The structure the block at `controller` (a bearing, piston, pulley, gantry carriage, cart
/// assembler or station) just put in motion: `CONTRAPTION_ACTORS` when it breaks blocks.
pub(crate) fn assembled(
    lv: &mut ServerLevel<'_>,
    controller: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    if owner(lv.dim, controller).is_some() && breakers(blocks) {
        award_placer(lv, controller, "contraption_actors");
    }
}

/// `MechanicalBearingBlockEntity.assemble` of a windmill bearing: `WINDMILL`, a hundred and
/// twenty-eight sails `WINDMILL_MAXED`, and its actors.
pub(crate) fn windmill_assembled(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    blocks: &[([i32; 3], u32)],
    sails: u32,
) {
    award_placer(lv, pos, "windmill");
    if sails >= 16 * 8 {
        award_placer(lv, pos, "windmill_maxed");
    }
    assembled(lv, pos, blocks);
}

/// `ClockworkBearingBlockEntity.assemble`: each hand's actors, then `CLOCKWORK_BEARING`.
pub(crate) fn clock_assembled(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    hour: &[([i32; 3], u32)],
    minute: Option<&[([i32; 3], u32)]>,
) {
    assembled(lv, pos, hour);
    if let Some(m) = minute {
        assembled(lv, pos, m);
    }
    award_placer(lv, pos, "clockwork_bearing");
}

/// [`award_placer`] from a moment with no host at hand (inside a read of the level): given at
/// the end of this tick.
pub(crate) fn award_later(dim: usize, pos: [i32; 3], name: &'static str) {
    if owner(dim, pos).is_some() {
        LATER.with_borrow_mut(|l| l.push((dim, pos, name)));
    }
}

/// The player's data key of where they sat down on a moving structure.
const MOUNT_LOCATION: &str = "ContraptionMountLocation";

/// `AbstractContraptionEntity.addSittingPassenger`: a player who sits down on a moving
/// structure keeps where they did (`ContraptionMountLocation`).
pub(crate) fn seated(host: &mut dyn ServerHost, rider: u128) {
    let Some(player) = host.player(rider) else {
        return;
    };
    let Some(at) = player.entity().position(&*host) else {
        return;
    };
    let mut data = player.persistent_data(&*host).unwrap_or_default();
    let list = at.iter().map(|v| Tag::Double(v.to_bits())).collect();
    data.insert(MOUNT_LOCATION.into(), Tag::List(list));
    let _ = player.set_persistent_data(host, data);
}

/// `getDismountLocationForPassenger`: a player let off at `at`, 5000 blocks or more from where
/// they sat down, `LONG_TRAVEL`.
pub(crate) fn let_off(host: &mut dyn ServerHost, rider: u128, at: [f64; 3]) {
    let Some(player) = host.player(rider) else {
        return;
    };
    let Some(mut data) = player.persistent_data(&*host) else {
        return;
    };
    let Some(Tag::List(from)) = data.remove(&MOUNT_LOCATION.into()) else {
        return;
    };
    let _ = player.set_persistent_data(host, data);
    let from: Vec<f64> = from
        .iter()
        .filter_map(|t| match t {
            Tag::Double(b) => Some(f64::from_bits(*b)),
            _ => None,
        })
        .collect();
    let [x, y, z] = from[..] else {
        return;
    };
    let d2 = (at[0] - x).powi(2) + (at[1] - y).powi(2) + (at[2] - z).powi(2);
    if d2 >= 5000.0 * 5000.0 {
        award(host, player, "long_travel");
    }
}

/// `StationBlockEntity.assemble`: each carriage's actors, `TRAIN`, and six carriages or more
/// `LONG_TRAIN`.
pub(crate) fn train_assembled(
    lv: &mut ServerLevel<'_>,
    station: [i32; 3],
    cars: &[Vec<([i32; 3], u32)>],
) {
    for car in cars {
        assembled(lv, station, car);
    }
    award_placer(lv, station, "train");
    if cars.len() >= 6 {
        award_placer(lv, station, "long_train");
    }
}

/// `Train.tick`'s signal passing: a train driven by hand (no schedule, or one paused) whose
/// front enters a section held by another train gives its drivers `RED_SIGNAL`.
fn red_signals(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let entered: Vec<u32> = super::world::with_world(|w| {
        let r = &w.dims[dim].railway;
        let mut out = Vec::new();
        LEAD_SECTIONS.with_borrow_mut(|leads| {
            leads.retain(|(d, id), _| *d != dim || r.trains.iter().any(|t| t.id == *id));
            if r.net.signals.is_empty() {
                return;
            }
            for t in r
                .trains
                .iter()
                .filter(|t| t.train.bound && !t.train.points.is_empty())
            {
                let lead = t.train.leading(&r.net.graph, t.train.speed >= 0.0);
                let section = r.net.section(lead.edge, lead.position);
                let was = leads.insert((dim, t.id), section);
                let by_hand = t.schedule.is_none() || t.runtime.paused;
                if was.is_some_and(|s| s != section)
                    && by_hand
                    && r.occupancy.occupied_unless(section, t.id)
                {
                    out.push(t.id);
                }
            }
        });
        out
    })
    .unwrap_or_default();
    for id in entered {
        for uuid in super::trains::drivers(dim, id) {
            if let Some(player) = lv.host.player(uuid) {
                award(lv.host, player, "red_signal");
            }
        }
    }
}

/// `ItemTossEvent`: a player throws a package (`PackageEntity.tossedBy`; a package here is an
/// item): its entity is remembered with them, for [`package_landed`].
pub(crate) fn item_tossed(host: &mut dyn ServerHost, event: &mut mod_api::ItemTossEvent) {
    let package = event
        .item
        .stack(&*host)
        .is_some_and(|s| create_engine::package::is_package(&s.item().id(&*host)));
    if package {
        TOSSED.with_borrow_mut(|t| {
            if t.len() >= 256 {
                t.clear();
            }
            t.insert(event.item.entity().id(), event.player);
        });
    }
}

/// The player who threw the package item entity `entity`, when `stack` is still that package.
pub(crate) fn package_thrower(entity: u32, stack: &Option<mod_api::ItemStack>) -> Option<u128> {
    stack.as_ref()?;
    TOSSED.with_borrow(|t| t.get(&entity).copied())
}

/// `AbstractChuteBlock.updateEntityAfterFallOn`: a thrown package all taken by the chute at
/// `pos` gives its thrower `PACKAGE_CHUTE_THROW`.
pub(crate) fn package_landed(
    lv: &mut ServerLevel<'_>,
    thrower: Option<u128>,
    pos: [i32; 3],
    left: &Option<mod_api::ItemStack>,
) {
    let Some(uuid) = thrower.filter(|_| left.is_none()) else {
        return;
    };
    let chute = matches!(
        lv.block_name(pos).as_deref(),
        Some("create:chute" | "create:smart_chute")
    );
    if chute && let Some(player) = lv.host.player(uuid) {
        award(lv.host, player, "package_chute_throw");
    }
}

fn set_owner(dim: usize, pos: [i32; 3], owner: Option<Owner>) {
    let changed = OWNERS.with_borrow_mut(|o| {
        let d = o.entry(dim).or_default();
        match owner {
            Some(u) => d.insert(pos, u) != Some(u),
            None => d.remove(&pos).is_some(),
        }
    });
    if changed && owner.is_some() {
        FRESH.with_borrow_mut(|f| f.push((dim, pos)));
    }
    if changed {
        persist::mark_pos_unsaved(dim, pos);
    }
}

fn uuid_tag(u: u128) -> [i32; 4] {
    [96, 64, 32, 0].map(|s| (u >> s) as u32 as i32)
}

fn uuid_of(v: &[i32]) -> u128 {
    v.iter().fold(0, |u, w| (u << 32) | u128::from(*w as u32))
}

/// Who placed Create's blocks (`AdvancementBehaviour`'s owner), for [`award_placer`].
pub struct Owners;

impl Subsystem for Owners {
    fn world_closed(&self) {
        OWNERS.with_borrow_mut(HashMap::clear);
        PENDING.with_borrow_mut(Vec::clear);
        FRESH.with_borrow_mut(Vec::clear);
        LATER.with_borrow_mut(Vec::clear);
        PLAYER_LATER.with_borrow_mut(Vec::clear);
        TOSSED.with_borrow_mut(HashMap::clear);
        PUMP_SPEEDS.with_borrow_mut(HashMap::clear);
        BOILER_WHISTLES.with_borrow_mut(HashMap::clear);
        LEAD_SECTIONS.with_borrow_mut(HashMap::clear);
    }

    /// `AdvancementBehaviour.setPlacedBy`: a player places a block (taken when it is written).
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        if event.cancelled {
            return Flow::Continue;
        }
        let Some(player) = event
            .placer
            .and_then(|p| p.uuid(host))
            .and_then(|u| host.player(u))
        else {
            return Flow::Continue;
        };
        let Some(dim) = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()))
        else {
            return Flow::Continue;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        PENDING.with_borrow_mut(|p| p.push((dim, pos, player.uuid())));
        shifting_gears(host, player, pos, super::states::id_or_none(event.state));
        Flow::Continue
    }

    /// A placed block takes its placer; a block replaced by another (its block entity gone)
    /// forgets its owner.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let placed = PENDING.with_borrow_mut(|p| {
            let i = p.iter().position(|e| e.0 == dim && e.1 == pos)?;
            Some(p.swap_remove(i).2)
        });
        if new != 0
            && let Some(player) = placed
        {
            set_owner(dim, pos, Some(Owner::new(player)));
            // `TrackTargetingBlockItem.useOn`: a signal put on the track.
            let signal = try_with_world(|w| {
                w.table.get(new).map(|(b, _)| b) == Some(create_engine::Block::TrackSignal)
            });
            if signal == Some(true) {
                PLAYER_LATER.with_borrow_mut(|l| l.push((player, "track_signal")));
            }
            return;
        }
        if old == 0 || owner(dim, pos).is_none() {
            return;
        }
        let other = new == 0
            || try_with_world(|w| {
                let kind = |id| w.table.get(id).map(|(b, _)| b);
                kind(old) != kind(new)
            })
            .unwrap_or(false);
        if other {
            set_owner(dim, pos, None);
        }
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            OBSERVED => gauge_observed(host, player, data),
            EJECTOR_AWARD => ejector_award(host, player, data),
            _ => return false,
        }
        true
    }

    /// `MechanicalCrafterBlockEntity.onSpeedChanged`: a crafter set turning.
    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], speed: f32) {
        if speed.abs() < 1e-5 {
            PUMP_SPEEDS.with_borrow_mut(|p| p.remove(&(lv.dim, pos)));
            return;
        }
        if owner(lv.dim, pos).is_none() {
            return;
        }
        let dim = lv.dim;
        let block = super::world::with_world(|w| w.block_at(dim, pos)).flatten();
        // `BacktankBlockEntity.onSpeedChanged`.
        if matches!(
            block,
            Some(create_engine::Block::CopperBacktank | create_engine::Block::NetheriteBacktank)
        ) {
            award_placer(lv, pos, "backtank");
        }
        // `PumpBlockEntity.onSpeedChanged`: not for a turn the other way at the same speed.
        if block == Some(create_engine::Block::MechanicalPump) {
            let was = PUMP_SPEEDS.with_borrow_mut(|p| p.insert((dim, pos), speed.abs()));
            if was != Some(speed.abs()) {
                award_placer(lv, pos, "mechanical_pump_0");
            }
        }
        if block == Some(create_engine::Block::MechanicalCrafter) {
            award_placer(lv, pos, "mechanical_crafter");
            if speed.abs() < 5.0 {
                award_placer(lv, pos, "crafter_lazy_000");
            }
        }
    }

    /// `CardboardArmorHandler.playerHitboxChangesWhenHidingAsBox`: a player hiding as a box.
    fn player_tick(&self, host: mod_api::HostSide, event: &mut mod_api::PlayerTickPostEvent) {
        let mod_api::HostSide::Server(host) = host else {
            return;
        };
        // Its size changes as it starts to hide (`EntityEvent.Size` on the pose's change).
        let (player, hiding) = (event.player, hiding_in_cardboard(&*host, event.player));
        let started = HIDING.with_borrow_mut(|h| {
            if hiding {
                h.insert(player.uuid())
            } else {
                h.remove(&player.uuid()) && false
            }
        });
        if started {
            award(host, player, "cardboard_armor");
        }
    }

    /// A placement never written (refused by the game or another mod) is forgotten.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        let dim = lv.dim;
        PENDING.with_borrow_mut(|p| p.retain(|e| e.0 != dim));
        let fresh: Vec<[i32; 3]> = FRESH.with_borrow_mut(|f| {
            let (mine, rest): (Vec<_>, Vec<_>) = f.drain(..).partition(|e| e.0 == dim);
            *f = rest;
            mine.into_iter().map(|e| e.1).collect()
        });
        for pos in fresh {
            arm_points(lv, pos);
        }
        let later: Vec<([i32; 3], &str)> = LATER.with_borrow_mut(|l| {
            let (mine, rest): (Vec<_>, Vec<_>) = l.drain(..).partition(|e| e.0 == dim);
            *l = rest;
            mine.into_iter().map(|e| (e.1, e.2)).collect()
        });
        for (pos, name) in later {
            award_placer(lv, pos, name);
        }
        red_signals(lv);
        for (uuid, name) in PLAYER_LATER.with_borrow_mut(std::mem::take) {
            if let Some(player) = lv.host.player(uuid) {
                award(lv.host, player, name);
            }
        }
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        let Some(Tag::List(list)) = data.get(&KEY.into()) else {
            return;
        };
        OWNERS.with_borrow_mut(|o| {
            let d = o.entry(lv.dim).or_default();
            for t in list.iter() {
                if let Tag::IntArray(v) = t
                    && v.len() == 9
                {
                    let (player, tracks, foods) =
                        (uuid_of(&v[3..7]), v[7].max(0) as u32, v[8] as u8);
                    d.insert(
                        [v[0], v[1], v[2]],
                        Owner {
                            player,
                            tracks,
                            foods,
                        },
                    );
                    FRESH.with_borrow_mut(|f| f.push((lv.dim, [v[0], v[1], v[2]])));
                }
            }
        });
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
        let mut list: Vec<([i32; 3], Owner)> = OWNERS.with_borrow(|o| {
            o.get(&lv.dim).map_or_else(Vec::new, |d| {
                d.iter()
                    .filter(|(p, _)| inside(p))
                    .map(|(p, u)| (*p, *u))
                    .collect()
            })
        });
        if list.is_empty() {
            data.remove(&KEY.into());
            return;
        }
        list.sort_unstable_by_key(|e| e.0);
        let tags = list
            .into_iter()
            .map(|(p, u)| {
                let counts = [u.tracks as i32, i32::from(u.foods)];
                Tag::IntArray([&p[..], &uuid_tag(u.player), &counts].concat())
            })
            .collect();
        data.insert(KEY.into(), Tag::List(tags));
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        OWNERS.with_borrow_mut(|o| {
            if let Some(d) = o.get_mut(&dim) {
                d.retain(|p, _| [p[0] >> 4, p[2] >> 4] != chunk);
            }
        });
    }

    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _block: create_engine::Block,
        out: &mut Compound,
    ) {
        let Some(u) = OWNERS.with_borrow(|o| o.get(&lv.dim)?.get(&pos).copied()) else {
            return;
        };
        set_owner(lv.dim, pos, None);
        out.insert(OWNER.into(), Tag::IntArray(uuid_tag(u.player).to_vec()));
        if u.tracks > 0 {
            out.insert(TRACKS.into(), Tag::Int(u.tracks as i32));
        }
        for (bit, (_, key)) in FOODS.iter().enumerate() {
            if u.foods & (1 << bit) != 0 {
                out.insert((*key).into(), Tag::Byte(1));
            }
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _block: create_engine::Block,
        data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
        let Some(player) = carried_owner(data) else {
            return;
        };
        let tracks = match data.get(&TRACKS.into()) {
            Some(Tag::Int(n)) => (*n).max(0) as u32,
            _ => 0,
        };
        let foods = FOODS
            .iter()
            .enumerate()
            .filter(|(_, (_, key))| data.get(&(*key).into()).is_some())
            .fold(0, |f, (bit, _)| f | (1 << bit));
        set_owner(
            lv.dim,
            pos,
            Some(Owner {
                player,
                tracks,
                foods,
            }),
        );
    }
}

/// The owner carried on a moving structure's block (its `Owner`), for awards a block gives
/// while it rides (`DeployerMovementBehaviour`).
pub(crate) fn carried_owner(data: &Compound) -> Option<u128> {
    match data.get(&OWNER.into()) {
        Some(Tag::IntArray(v)) if v.len() == 4 => Some(uuid_of(v)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_round_trips_as_four_ints() {
        let u = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210_u128;
        assert_eq!(uuid_of(&uuid_tag(u)), u);
        assert_eq!(uuid_tag(u)[0], 0x0123_4567);
    }
}
