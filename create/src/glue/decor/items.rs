//! The vertical gearbox and the empty blaze burner items (`VerticalGearboxItem`,
//! `BlazeBurnerBlockItem.empty`): they place their block as a block item does, and an empty blaze
//! burner catches a blaze, from its spawner or the blaze itself.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::{add, creative, give, player_dimension, stack_of, state_with, use_one};
use create_engine::Block;
use create_engine::decor;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    Compound, Hand, HostBlockStates, HostSide, InteractionResult, ItemInteractMobEvent, ItemPart,
    ItemStack, PlayerRef, Registrar, SoundSource, Tag, UseOnEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    for name in ["create:vertical_gearbox", "create:empty_blaze_burner"] {
        let item = r.item_key(name);
        r.item_part(item, PlacingItem);
    }
}

struct PlacingItem;

#[mod_api::horizon_part]
impl ItemPart for PlacingItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let player = host.player(event.player)?;
        let dimension = player_dimension(&*host, player)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        let item = event.stack.item().id(&*lv.host);
        // `BlazeBurnerBlockItem.useOn`: on a spawner of a capturable mob, the burner catches it.
        if item == "create:empty_blaze_burner" && spawns_capturable(&lv, event.pos) {
            give_burner(
                &mut lv,
                player,
                event.hand,
                event.stack,
                super::center(event.pos),
            );
            return Some(InteractionResult::Success);
        }
        place_item(&mut lv, player, event, item == "create:vertical_gearbox");
        Some(InteractionResult::Success)
    }

    /// `BlazeBurnerBlockItem.interactLivingEntity`: an empty blaze burner used on a blaze
    /// (`create:blaze_burner_capturable`) catches it.
    fn item_interact_mob(
        &self,
        host: HostSide,
        event: &mut ItemInteractMobEvent<'_>,
    ) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        if event.kind != "minecraft:blaze" {
            return None;
        }
        if event.stack.item().id(&*host) != "create:empty_blaze_burner" {
            return None;
        }
        let player = host.players().acting_player()?;
        let at = player.entity().position(&*host)?;
        let entity = host.entity(u32::try_from(event.entity).ok()?)?;
        let dimension = player_dimension(&*host, player)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        // `spawnCaptureEffects` at the blaze; here beside the player who caught it.
        give_burner(&mut lv, player, event.hand, event.stack, at);
        let _ = entity.discard(&mut *lv.host);
        Some(InteractionResult::Success)
    }
}

/// The id a spawner's `SpawnData` compound spawns (`entity.id`).
fn spawn_id(data: &Compound) -> Option<String> {
    let Some(Tag::Compound(entity)) = data.get(&"entity".into()) else {
        return None;
    };
    match entity.get(&"id".into()) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// `BlazeBurnerBlockItem.useOn`: whether the spawner at `pos` spawns a capturable mob, among its
/// potentials or, with none, its next spawn.
fn spawns_capturable(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return false;
    };
    if lv
        .block_state(pos)
        .map(|s| s.block().id(&*lv.host))
        .as_deref()
        != Some("minecraft:spawner")
    {
        return false;
    }
    let Some(data) = entity.data(&*lv.host) else {
        return false;
    };
    let blaze = |d: &Compound| spawn_id(d).as_deref() == Some("minecraft:blaze");
    let potentials = match data.get(&"SpawnPotentials".into()) {
        Some(Tag::List(l)) => l.to_vec(),
        _ => Vec::new(),
    };
    if potentials.is_empty() {
        return matches!(data.get(&"SpawnData".into()), Some(Tag::Compound(d)) if blaze(d));
    }
    potentials.iter().any(|p| match p {
        Tag::Compound(p) => matches!(p.get(&"data".into()), Some(Tag::Compound(d)) if blaze(d)),
        _ => false,
    })
}

/// `spawnCaptureEffects` (its sounds) and `giveBurnerItemTo`: the held stack shrinks, the burner
/// goes to the hand or the inventory.
fn give_burner(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    hand: Hand,
    stack: &ItemStack,
    at: [f64; 3],
) {
    let host = &mut *lv.host;
    lv.level.play_sound(
        host,
        "minecraft:entity.blaze.hurt",
        at,
        SoundSource::Blocks,
        0.25,
        0.75,
    );
    lv.level.play_sound(
        host,
        "minecraft:block.fire.extinguish",
        at,
        SoundSource::Blocks,
        0.5,
        0.75,
    );
    let single = stack.count() <= 1;
    let creative = creative(&*host, player);
    if single && !creative {
        if let (Some(living), Some(burner)) = (
            host.living(player.entity()),
            stack_of(&*host, "create:blaze_burner", 1),
        ) {
            let _ = living.set_hand_item(host, hand, Some(burner));
        }
        return;
    }
    use_one(host, player, hand, stack);
    give(host, player, "create:blaze_burner");
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// `BlockItem.canPlace` (`Level.isUnobstructed`) for a full block at `at`: no entity that
/// blocks building (`Entity.blocksBuilding`: living ones, not spectators; minecarts, boats,
/// falling blocks and primed TNT) overlaps it.
fn obstructed(lv: &ServerLevel<'_>, at: [i32; 3]) -> bool {
    let lo = at.map(f64::from);
    let hi = lo.map(|v| v + 1.0);
    let host = &*lv.host;
    let spectators: Vec<_> = lv
        .players()
        .into_iter()
        .filter(|p| p.game_mode(host) == Some(mod_api::GameMode::Spectator))
        .map(|p| p.entity())
        .collect();
    lv.level
        .entities_in(host, lo, hi)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !spectators.contains(e))
        .filter(|e| {
            e.bounding_box(host)
                .is_some_and(|[min, max]| (0..3).all(|i| min[i] < hi[i] && max[i] > lo[i]))
        })
        .any(|e| {
            host.living(e).is_some()
                || e.kind(host).map(|k| k.id(host)).is_some_and(|k| {
                    k.ends_with("minecart")
                        || k.ends_with("boat")
                        || k.ends_with("raft")
                        || k == "minecraft:falling_block"
                        || k == "minecraft:tnt"
                })
        })
}

/// `VerticalGearboxItem` and `BlazeBurnerBlockItem.empty` place their block as a block item does
/// (into the clicked block when it can be replaced, else beside it): a gearbox on Create's axis,
/// an empty blaze burner facing the player.
fn place_item(lv: &mut ServerLevel<'_>, player: PlayerRef, event: &UseOnEvent<'_>, gearbox: bool) {
    if player.may_build(&*lv.host) != Some(true) {
        return;
    }
    let facts = lv.host.block_states();
    let replaceable = |p: [i32; 3]| {
        lv.block_state(p)
            .is_some_and(|s| facts.replaceable(s) == Some(true))
    };
    let pos = event.pos;
    let f = event.face;
    let at = if replaceable(pos) {
        pos
    } else {
        [pos[0] + f[0], pos[1] + f[1], pos[2] + f[2]]
    };
    if !replaceable(at) || obstructed(lv, at) {
        return;
    }
    let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
    let facing = decor::horizontal_facing(yaw);
    let dim = lv.dim;
    let (state, sound) = if gearbox {
        let shafts: Vec<Dir> = [Dir::South, Dir::West, Dir::North, Dir::East]
            .into_iter()
            .filter(|d| {
                with_world(|w| w.has_shaft_towards(dim, add(at, *d), d.opposite())) == Some(true)
            })
            .collect();
        let axis = decor::vertical_gearbox_axis(&shafts, facing);
        (
            state_with(&*lv.host, Block::Gearbox, &[("axis", axis_name(axis))]),
            // The gearbox's sounds are andesite's (`SharedProperties.stone`).
            "minecraft:block.stone.place",
        )
    } else {
        // `BlazeBurnerBlock.getStateForPlacement`: no blaze, facing the player.
        (
            state_with(
                &*lv.host,
                Block::BlazeBurner,
                &[
                    ("blaze", "none"),
                    (
                        "facing",
                        super::super::kinetics::dir_name(facing.opposite()),
                    ),
                ],
            ),
            "minecraft:block.metal.place",
        )
    };
    let Some(state) = state else {
        return;
    };
    lv.put(at, state);
    lv.block_sound(sound, at, 1.0, 0.8);
    use_one(&mut *lv.host, player, event.hand, event.stack);
}
