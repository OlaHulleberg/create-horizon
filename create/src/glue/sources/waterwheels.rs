//! Water wheels' material and drawing (`WaterWheelBlockEntity.material`, `WaterWheelRenderer`):
//! a wheel is made of spruce until a player uses planks on it (or on a large wheel's frame), and
//! is drawn turning in that wood. Its flow and speed are `kinetics`'.
//!
//! The material is kept in the wheel's block entity data as Create keeps it (`Material`, a block
//! state's `Name`), which is saved with the block entity and sent to its clients; each wood's
//! wheel models are partials (`part_water_wheel_<wood>`, `part_large_water_wheel_<wood>` and
//! `_extension_<wood>`: Create's models with the wood's planks and log textures).

use super::super::items;
use super::super::kinetics::{self, axis_vector};
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::client::{Draw, Model};
use create_engine::kinetics::{Axis, Dir};
use create_engine::waterwheel::{DEFAULT_WOOD, wood_of_planks};
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockPart, BlockState, CanSurviveEvent, ClientHost,
    Compound, Flow, Hand, HostBlockStates, InteractionResult, LevelHost, ParticleOptions,
    Registrar, RightClickBlockEvent, ServerHost, SoundSource, Tag, UseItemOnEvent,
};

/// The key of a wheel's material in its block entity data (`WaterWheelBlockEntity.write`).
pub const MATERIAL: &str = "Material";

use crate::glue::server::BLOCK_PARTICLE;

/// The wheels' parts: their clicks and their drawing on the kinetic block entity type.
pub(super) fn register(r: &mut dyn Registrar) {
    for block in [
        Block::WaterWheel,
        Block::LargeWaterWheel,
        Block::WaterWheelStructure,
    ] {
        r.block_part(states::key(block), Wheel);
    }
    r.block_part(states::key(Block::WaterWheel), Spaced);
    let _ = BLOCK_PARTICLE.set(r.particle_key("minecraft:block"));
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, Drawn) {
        crate::create_log!("[create] the water wheels' block entity part was refused: {e:?}");
    }
}

/// Whether part `part` of `block` is a wheel this module draws.
pub(super) fn draws_itself(block: Block, part: &Draw) -> bool {
    matches!(
        (block, part.model),
        (Block::WaterWheel, Model::WaterWheel(_)) | (Block::LargeWaterWheel, Model::Own)
    )
}

/// Planks used on a wheel are the block's on the client too (`useItemOn` answers SUCCESS
/// there), so they are not placed against it.
pub(super) fn predict(host: &mut dyn ClientHost, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(level) = host.level(host.dimension()) else {
        return Flow::Continue;
    };
    let state = level
        .block_state(&*host, click.pos)
        .map_or(0, states::id_or_none);
    if state == 0 {
        return Flow::Continue;
    }
    let Some((block, facing)) =
        super::super::client_table(&*host, |t| t.get(state).map(|(b, i)| (b, i.facing))).flatten()
    else {
        return Flow::Continue;
    };
    let master = match block {
        Block::WaterWheel | Block::LargeWaterWheel => click.pos,
        // The frame's wheel is where its facing points, or one further for a corner.
        Block::WaterWheelStructure => {
            let Some(master) = facing.and_then(|f| {
                let one = add(click.pos, f.offset());
                let at = |p: [i32; 3]| {
                    let s = level.block_state(&*host, p).map_or(0, states::id_or_none);
                    super::super::client_table(&*host, |t| t.get(s).map(|(b, i)| (b, i.facing)))
                        .flatten()
                };
                match at(one)? {
                    (Block::LargeWaterWheel, _) => Some(one),
                    (Block::WaterWheelStructure, Some(g)) => {
                        let two = add(one, g.offset());
                        matches!(at(two), Some((Block::LargeWaterWheel, _))).then_some(two)
                    }
                    _ => None,
                }
            }) else {
                return Flow::Continue;
            };
            master
        }
        _ => return Flow::Continue,
    };
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    // The block's own use is skipped while sneaking with something in either hand.
    let bypassed = items::sneaking(&*host, player)
        && Hand::BOTH
            .iter()
            .any(|&h| items::held(&*host, player, h).is_some());
    if bypassed {
        return Flow::Continue;
    }
    let Some(wood) = click
        .stack
        .and_then(|s| wood_of_planks(&s.item().id(&*host)))
    else {
        return Flow::Continue;
    };
    let now = host
        .block_entity(level, master)
        .and_then(|e| e.data(&*host))
        .map_or(DEFAULT_WOOD, |d| wood_of(&d));
    if now == wood {
        return Flow::Continue;
    }
    *event.cancellation_result = InteractionResult::Success;
    Flow::Cancel
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// The wood a wheel's block entity data names (spruce when it names none, or one no wheel
/// takes).
pub fn wood_of(data: &Compound) -> &'static str {
    let Some(Tag::Compound(m)) = data.get(&MATERIAL.into()) else {
        return DEFAULT_WOOD;
    };
    match m.get(&"Name".into()) {
        Some(Tag::String(name)) => name
            .to_string()
            .ok()
            .and_then(|n| wood_of_planks(&n))
            .unwrap_or(DEFAULT_WOOD),
        _ => DEFAULT_WOOD,
    }
}

/// `WaterWheelBlock.canSurvive` as one is placed: a water wheel beside it must turn on the same
/// axis, and be beside it along that axis.
struct Spaced;

#[mod_api::horizon_part]
impl BlockPart for Spaced {
    fn can_survive(&self, level: &dyn LevelHost, event: &mut CanSurviveEvent) -> Option<bool> {
        let facing_of = |state: BlockState| {
            let id = states::id(state)?;
            with_world(|w| match w.table.get(id) {
                Some((Block::WaterWheel, i)) => i.facing,
                _ => None,
            })
            .flatten()
        };
        let axis = facing_of(event.state)?.axis();
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        for d in Dir::ALL {
            let at = add(pos, d.offset());
            let Some(other) = level.block_state(at).and_then(facing_of) else {
                continue;
            };
            if other.axis() != axis || axis != d.axis() {
                return Some(false);
            }
        }
        None
    }
}

/// `WaterWheelBlock`, `LargeWaterWheelBlock` and `WaterWheelStructuralBlock.useItemOn`: planks
/// of another wood become the wheel's material (`applyMaterialIfValid`).
struct Wheel;

#[mod_api::horizon_part]
impl BlockPart for Wheel {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let dimension = click.dimension.key().id().to_owned();
        let item = click.stack.map(|s| s.item().id(&*host));
        let mut lv = ServerLevel::new(host, &dimension)?;
        let dim = lv.dim;
        let state = lv.block(click.pos);
        let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()?;
        let master = match block {
            Block::WaterWheel | Block::LargeWaterWheel => click.pos,
            // `stillValid` and `getMaster`; a frame without its wheel fails the click.
            _ => match with_world(|w| w.wheel_master(dim, click.pos, state)).flatten() {
                Some(m) => m,
                None => return Some(InteractionResult::Fail),
            },
        };
        let wood = item.as_deref().and_then(wood_of_planks)?;
        let entity = lv.host.block_entity(lv.level, master)?;
        let now = entity.data(&*lv.host).map_or(DEFAULT_WOOD, |d| wood_of(&d));
        if now == wood {
            return None;
        }
        let name = format!("minecraft:{wood}_planks");
        let material = Compound::from([("Name".into(), Tag::String(name.as_str().into()))]);
        let data = Compound::from([(MATERIAL.into(), Tag::Compound(material))]);
        let _ = entity.merge_data(&mut *lv.host, data);
        // `level.levelEvent(PARTICLES_DESTROY_BLOCK, pos, Block.getId(material))`: the planks'
        // break particles and sound (the game's state ids are not the API's, so the event is
        // played as its parts).
        break_effect(&mut lv, master, &name, wood);
        Some(InteractionResult::Success)
    }
}

/// The planks' break particles over the wheel and their break sound (`LevelRenderer
/// .levelEvent` 2001: `addDestroyBlockEffect`, the sound type's break sound at
/// `(volume + 1) / 2` and `pitch * 0.8`).
fn break_effect(lv: &mut ServerLevel<'_>, pos: [i32; 3], planks: &str, wood: &str) {
    let center = pos.map(|v| f64::from(v) + 0.5);
    let state = lv
        .host
        .block_key(planks)
        .and_then(|k| BlockState::of(&lv.host.block_states(), k, &[]).ok());
    if let (Some(&key), Some(state)) = (BLOCK_PARTICLE.get(), state) {
        let options = ParticleOptions::simple(key);
        let _ = lv.level.send_particles(
            &mut *lv.host,
            &options,
            Some(state),
            center,
            [0.25; 3],
            0.0,
            64,
        );
    }
    let sound = match wood {
        "bamboo" => "minecraft:block.bamboo_wood.break",
        "cherry" => "minecraft:block.cherry_wood.break",
        "crimson" | "warped" => "minecraft:block.nether_wood.break",
        _ => "minecraft:block.wood.break",
    };
    lv.level
        .play_sound(&mut *lv.host, sound, center, SoundSource::Blocks, 1.0, 0.8);
}

/// The partial of a wheel in `wood`: the small wheel, or the large one (its extension when it
/// reaches the next wheel along its axis).
fn partial(block: Block, extension: bool, wood: &str) -> Option<mod_api::BlockKey> {
    let wheel = match (block, extension) {
        (Block::WaterWheel, _) => "water_wheel",
        (_, false) => "large_water_wheel",
        (_, true) => "large_water_wheel_extension",
    };
    states::partial(&format!("part_{wheel}_{wood}"))
}

/// The facing a large wheel's partial is drawn in: its blockstate's turn for each axis.
fn large_facing(axis: Axis) -> Dir {
    match axis {
        Axis::X => Dir::East,
        Axis::Y => Dir::Up,
        Axis::Z => Dir::South,
    }
}

/// `WaterWheelRenderer`: the wheel in its material, turning on its axis.
struct Drawn;

#[mod_api::horizon_part]
impl BlockPart for Drawn {}

#[mod_api::horizon_part]
impl BlockEntityPart for Drawn {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        if !matches!(block, Block::WaterWheel | Block::LargeWaterWheel) {
            return;
        }
        let speeds = kinetics::speeds_of(&event.entity.nbt);
        let wood = wood_of(&event.entity.nbt);
        let text = host.block_state_string(event.state).unwrap_or_default();
        let (_, own) = states::split(&text);
        let extension = own.contains(&("extension", "true"));
        for d in create_engine::client::draws(block, &info, event.pos, speeds) {
            let facing = match d.model {
                Model::WaterWheel(f) => f,
                Model::Own => large_facing(d.axis),
                _ => continue,
            };
            let Some(part) = partial(block, extension, wood) else {
                continue;
            };
            let draw = &mut *event.draw;
            draw.spin(axis_vector(d.axis), [0.5; 3], d.rpm * 0.3, d.phase);
            draw.block(part, &[("facing", kinetics::dir_name(facing))]);
            draw.still();
        }
        event.draw.keep();
    }
}

/// The flow at `at` a wheel reads (`getFlowVectorAtPosition`): the fluid's, or a bubble
/// column's straight up or down (`DRAG_DOWN`).
pub fn flow_at(lv: &ServerLevel<'_>, at: [i32; 3]) -> [f64; 3] {
    if let Some(state) = lv.block_state(at)
        && lv.host.block_key("minecraft:bubble_column") == Some(state.block())
    {
        let down = state
            .property(&lv.host.block_states(), "drag")
            .ok()
            .flatten()
            == Some("true");
        return [0.0, if down { -1.0 } else { 1.0 }, 0.0];
    }
    lv.level
        .fluid_flow(&*lv.host, at)
        .map_or([0.0; 3], |v| [v.x, v.y, v.z])
}
