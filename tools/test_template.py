#!/usr/bin/env python3
"""Compile the real bundle on a tiny source fixture; no Horizon/assets required."""
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import horizon


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result


def snapshot(root):
    return {str(p.relative_to(root)): (horizon.digest(p.read_bytes()), p.stat().st_mtime_ns)
            for p in root.rglob('*') if p.is_file()}


def audit(original, stage, plan):
    run(['cargo', 'run', '--quiet', '--locked', '--offline', '-p', 'native-mod-source-audit',
         '--', str(original), str(stage), str(plan)], cwd=horizon.ROOT)


def write_fixture(original, root):
    """A small stand-in for Horizon with the hook points, types and paths the bundle uses: each hook
    the bundle names, the helpers its glue calls, and a test that drives every hook into the engine."""
    hooks = [
        ('world_lifecycle', "horizon_client::game::native_hooks::Game<'_>::native_world_lifecycle"),
        ('block_changed', "horizon_client::game::native_hooks::GameLevel<'_>::native_block_changed"),
        ('chunk_load', "horizon_client::game::native_hooks::Game<'_>::native_chunk_load"),
        ('chunk_unload', "horizon_client::game::native_hooks::Game<'_>::native_chunk_unload"),
        ('level_tick_post', "horizon_client::game::native_hooks::Game<'_>::native_level_tick_post"),
        ('register_commands', "horizon_client::game::native_hooks::Game<'_>::native_register_commands"),
        ('perform_command', "horizon_client::game::native_hooks::Game<'_>::native_perform_command"),
        ('net_from_client', "horizon_client::game::native_hooks::Game<'_>::native_net_from_client"),
        ('net_from_server', "horizon_client::game::native_hooks::Game<'_>::native_net_from_server"),
        ('tick', "horizon_client::game::native_hooks::Game<'_>::native_tick"),
        ('wheel', "horizon_client::game::native_hooks::Game<'_>::native_wheel"),
        ('chunk_data_save', "horizon_client::game::native_hooks::Game<'_>::native_chunk_data_save"),
        ('neighbor_changed', "horizon_client::game::native_hooks::GameLevel<'_>::native_neighbor_changed"),
        ('right_click_block', "horizon_client::game::native_hooks::Game<'_>::native_right_click_block"),
        ('block_entity', "horizon_client::game::native_hooks::Game<'_>::native_block_entity"),
        ('block_entity_container', "horizon_client::game::native_hooks::Game<'_>::native_block_entity_container"),
        ('contents_changed', "horizon_client::game::native_hooks::Game<'_>::native_contents_changed"),
        ('update_entity_after_fall_on',
         "horizon_client::game::native_hooks::Game<'_>::native_update_entity_after_fall_on"),
        ('block_signal', "horizon_client::game::native_hooks::GameLevel<'_>::native_block_signal"),
        ('is_signal_source', "horizon_client::game::native_hooks::GameLevel<'_>::native_is_signal_source"),
        ('tick_block', "horizon_client::game::native_hooks::GameLevel<'_>::native_tick_block"),
        ('use_item_on', "horizon_client::game::native_hooks::Game<'_>::native_use_item_on"),
        ('use_item', "horizon_client::game::native_hooks::Game<'_>::native_use_item"),
        ('place_block', "horizon_client::game::native_hooks::Game<'_>::native_place_block"),
        ('hud', "horizon_client::game::native_hooks::Game<'_>::native_hud"),
    ]
    files = {
        'Cargo.toml': ('[workspace]\nmembers = ["crates/horizon-core", "crates/horizon-world", '
                       '"crates/horizon-client", "crates/winit"]\nresolver = "2"\n'),
        'crates/winit/Cargo.toml': '[package]\nname = "winit"\nversion = "0.1.0"\nedition = "2024"\n',
        'crates/winit/src/lib.rs': 'pub mod keyboard { #[derive(Hash, PartialEq, Eq)] pub enum KeyCode { ShiftLeft } }\n',
        'crates/horizon-core/Cargo.toml': '[package]\nname = "horizon-core"\nversion = "0.1.0"\nedition = "2024"\n',
        'crates/horizon-core/src/lib.rs': '''
pub mod position {
    #[derive(Clone, Copy)] pub struct BlockPos { pub x: i32, pub y: i32, pub z: i32 }
    impl BlockPos { pub fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } } }
}
pub mod geometry {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Direction { Down, Up, North, South, West, East }
    impl Direction { pub const ALL: [Self; 6] = [Self::Down, Self::Up, Self::North, Self::South, Self::West, Self::East]; }
    #[derive(Clone, Copy)] pub enum Axis { X, Y, Z }
    pub struct VoxelShape;
    impl VoxelShape {
        pub fn is_empty(&self) -> bool { false }
        pub fn is_block(&self) -> bool { true }
        pub fn min_at(&self, axis: Axis, first: f64, second: f64) -> f64 { let _ = (axis, first, second); 0.0 }
        pub fn max_at(&self, axis: Axis, first: f64, second: f64) -> f64 { let _ = (axis, first, second); 1.0 }
    }
}
''',
        'crates/horizon-world/Cargo.toml': '[package]\nname = "horizon-world"\nversion = "0.1.0"\nedition = "2024"\n',
        'crates/horizon-world/src/lib.rs': '''
pub mod fluid {
    #[derive(Clone, Copy, PartialEq)] pub enum FluidKind { Empty, FlowingWater, Water, FlowingLava, Lava }
    #[derive(Clone, Copy)] pub struct FluidState(pub FluidKind);
    impl FluidState { pub fn kind(self) -> FluidKind { self.0 } pub fn is_source(self) -> bool { self.0 != FluidKind::Empty } }
}
pub mod nbt {
    #[derive(Clone, PartialEq, Debug)] pub enum Tag { IntArray(Vec<i32>), Compound(Compound), List(Vec<Tag>), String(JavaString) }
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct JavaString(String);
    impl From<&str> for JavaString { fn from(s: &str) -> Self { JavaString(s.to_owned()) } }
    impl JavaString { pub fn to_string_lossy(&self) -> String { self.0.clone() } }
    pub type Compound = std::collections::BTreeMap<JavaString, Tag>;
}
pub mod level {
    #[derive(Clone, Copy)] pub enum Dimension { Overworld, Nether, End }
    pub mod data { pub mod state_flags { pub const COLLISION_EMPTY: u32 = 1 << 5; } }
}
pub mod registry {
    use std::collections::BTreeMap;
    #[derive(Clone, Copy, PartialEq, Debug)] pub struct BlockStateId(pub u32);
    #[derive(Clone, PartialEq)] pub struct BlockState { pub name: String, pub properties: BTreeMap<String, String> }
    pub struct BlockRegistry { pub states: Vec<BlockState> }
    impl BlockRegistry {
        pub fn len(&self) -> usize { self.states.len() }
        pub fn is_empty(&self) -> bool { self.states.is_empty() }
        pub fn state(&self, id: BlockStateId) -> Option<&BlockState> { self.states.get(id.0 as usize) }
        pub fn default_state(&self, name: &str) -> Option<BlockStateId> {
            self.states.iter().position(|s| s.name == name).map(|i| BlockStateId(i as u32))
        }
        pub fn id(&self, state: &BlockState) -> Option<BlockStateId> {
            self.states.iter().position(|s| s == state).map(|i| BlockStateId(i as u32))
        }
    }
}
''',
        'crates/horizon-client/Cargo.toml': ('[package]\nname = "horizon-client"\nversion = "0.1.0"\nedition = "2024"\n'
                                             '[dependencies]\nhorizon-world = { path = "../horizon-world" }\n'
                                             'horizon-core = { path = "../horizon-core" }\n'
                                             'winit = { path = "../winit" }\n'
                                             'serde_json = "1"\nglam = "0.30"\n'),
        'crates/horizon-client/src/lib.rs': '''
// The game's module reaches the lib as `horizon_client`, as Horizon's binary does.
extern crate self as horizon_client;
pub mod game;
pub mod items {
    #[derive(Clone)] pub struct ItemStack { pub item: std::sync::Arc<str>, pub count: u32, pub damage: u32, pub extra: Option<Box<()>> }
    pub struct ItemInfo { pub max_stack: u32, pub fire_resistant: bool, pub remainder: Option<String> }
    pub struct Items { pub item_tags: std::collections::HashMap<String, std::collections::HashSet<String>> }
    impl Items { pub fn get(&self, item: &str) -> Option<ItemInfo> { let _ = item; Some(ItemInfo { max_stack: 64, fire_resistant: false, remainder: None }) } }
}
pub mod containers {
    pub struct ModBlockEntity { pub items: Vec<Option<crate::items::ItemStack>> }
    pub enum BlockEntity { Mod(ModBlockEntity) }
    pub struct ModContainer { pub slots: usize, pub faces: [Vec<usize>; 6], pub inputs: Vec<usize>, pub outputs: Vec<usize>, pub menu: Option<String>, pub only_when_empty: bool }
}
pub mod first_person {
    #[derive(Clone, Copy, PartialEq)] pub enum Hand { Main, Off }
}
pub mod crafting {
    pub struct CustomRecipe { pub id: String, pub json: std::sync::Arc<serde_json::Value> }
    pub struct Ingredient(pub std::collections::HashSet<String>, pub Vec<String>);
    pub struct Cooking { pub id: String, pub kind: String, pub ingredient: Ingredient, pub result: crate::items::ItemStack }
    #[derive(Default)] pub struct Recipes { pub cooking: Vec<Cooking> }
}
pub mod hud {
    pub struct HudBatch { pub width: f32, pub height: f32, pub drawn: Vec<String> }
}
pub mod text {
    pub struct Text(pub String);
    impl Text { pub fn literal(s: impl Into<String>) -> Self { Text(s.into()) } }
}
pub mod chunk {
    pub struct Blocks(pub Vec<u32>);
    impl Blocks {
        pub fn uniform(&self) -> Option<u32> { self.0.iter().all(|&v| v == self.0[0]).then(|| self.0[0]) }
    }
    pub struct Section { pub blocks: Blocks }
    impl Section {
        pub fn unpack(&self, out: &mut [u32; 4096]) { out.copy_from_slice(&self.blocks.0); }
    }
    pub struct Chunk { pub x: i32, pub z: i32, pub sections: Vec<std::sync::Arc<Section>> }
    impl Chunk { pub fn min_y(&self) -> i32 { 0 } }
}
''',
        'crates/horizon-client/src/game.rs': '''
use horizon_world::registry::{BlockRegistry, BlockStateId};
use horizon_world::level::Dimension as LevelDimension;
use horizon_core::position::BlockPos;
use horizon_client::chunk::Chunk;
pub mod native_lifecycle {
    pub enum WorldPhase { WorldOpenReady, WorldOpenFailed, WorldCloseFinal, WorldCloseFailed }
}
pub mod native_server {
    pub struct NativeCommand { pub usage: String, pub level: u8 }
    impl NativeCommand {
        pub fn op(usage: &str, level: u8) -> Self { Self { usage: usage.to_owned(), level } }
    }
    pub struct NativeServerPlayer { pub uuid: u128, pub dimension: String, pub position: [f64; 3] }
}
pub mod native_hooks {
    use super::*;
    pub mod flags { pub const UPDATE_CLIENTS: i32 = 2; pub const UPDATE_ALL: i32 = 3; }
    use native_lifecycle::WorldPhase;
    use native_server::{NativeCommand, NativeServerPlayer};
    impl GameLevel<'_> {
        pub fn native_block_changed(&self, dimension: LevelDimension, pos: BlockPos, old: BlockStateId, new: BlockStateId) {
            let _ = (dimension, pos, old, new);
        }
        pub fn native_neighbor_changed(&self, dimension: LevelDimension, pos: BlockPos, state: BlockStateId) {
            let _ = (dimension, pos, state);
        }
        pub fn native_block_signal(&self, dimension: LevelDimension, pos: BlockPos, state: BlockStateId,
                                   face: horizon_core::geometry::Direction, direct: bool, signal: &mut i32) {
            let _ = (dimension, pos, state, face, direct, signal);
        }
        pub fn native_is_signal_source(&self, state: BlockStateId, source: &mut bool) { let _ = (state, source); }
        pub fn native_tick_block(&mut self, ticks: &mut Ticks, dimension: LevelDimension, pos: BlockPos,
                                 state: BlockStateId, handled: &mut bool) {
            let _ = (ticks, dimension, pos, state, handled);
        }
    }
    impl Game<'_> {
        pub fn native_right_click_block(&mut self, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
            let _ = (pos, face, canceled);
        }
        pub fn spectator(&self) -> bool { false }
        pub fn native_block_entity(&mut self, block: &str, entity: &mut Option<(String, horizon_world::nbt::Compound)>) {
            let _ = (block, entity);
        }
        pub fn native_block_entity_container(&self, block: &str, container: &mut Option<horizon_client::containers::ModContainer>) {
            let _ = (block, container);
        }
        pub fn items(&self) -> horizon_client::items::Items { horizon_client::items::Items { item_tags: Default::default() } }
        pub fn native_recipes_of_type(&self, kind: &str) -> &[horizon_client::crafting::CustomRecipe] {
            self.custom_recipes.get(kind).map_or(&[], Vec::as_slice)
        }
        pub fn native_recipe_ingredient(&self, json: &serde_json::Value) -> Option<horizon_client::crafting::Ingredient> {
            let item = json.as_str()?.to_owned();
            Some(horizon_client::crafting::Ingredient([item.clone()].into_iter().collect(), vec![item]))
        }
        pub fn native_recipe_result(&self, json: &serde_json::Value) -> Option<horizon_client::items::ItemStack> {
            Some(horizon_client::items::ItemStack { item: json["id"].as_str()?.into(),
                count: json["count"].as_u64().unwrap_or(1) as u32, damage: 0, extra: None })
        }
        pub fn native_with_mod_block_entity<R>(&mut self, pos: [i32; 3], change: impl FnOnce(&mut horizon_client::containers::ModBlockEntity) -> R) -> Option<R> {
            match self.world.block_entities.get_mut(&(pos[0], pos[1], pos[2]))? {
                horizon_client::containers::BlockEntity::Mod(m) => Some(change(m)),
            }
        }
        pub fn native_held(&self) -> Option<(String, u32, bool)> {
            self.inventory.get(self.selected)?.as_ref().map(|s| (s.item.to_string(), s.count, true))
        }
        pub fn native_count_items(&mut self, item: &str) -> u32 { let _ = item; 0 }
        pub fn native_give_items(&mut self, item: &str, count: u32) -> u32 { let _ = item; count }
        pub fn native_take_items(&mut self, item: &str, count: u32) -> u32 { let _ = item; count }
        pub fn native_fluid_flow(&self, dimension: &str, pos: [i32; 3]) -> Option<[f64; 3]> {
            let _ = (dimension, pos);
            Some([0.0, 0.0, 0.0])
        }
    }
    impl Game<'_> {
        pub fn native_world_lifecycle(&mut self, phase: WorldPhase, error_text: &str) {
            let _ = (phase, error_text);
            *self.calls += 1;
        }
        pub fn native_chunk_load(&mut self, dimension: &str, chunk: &Chunk) { let _ = (dimension, chunk); }
        pub fn native_chunk_unload(&mut self, dimension: &str, chunk: &Chunk) { let _ = (dimension, chunk); }
        pub fn native_level_tick_post(&mut self, dimension: &str) { let _ = dimension; }
        pub fn native_register_commands(&mut self, commands: &mut Vec<NativeCommand>) { let _ = commands; }
        pub fn native_perform_command(&mut self, player: &NativeServerPlayer, command: &str, ran: &mut bool) {
            let _ = (player, command, ran);
        }
        pub fn native_acting_player(&self) -> Option<NativeServerPlayer> {
            Some(NativeServerPlayer { uuid: 7, dimension: "minecraft:overworld".into(), position: [0.5, 2.0, 0.5] })
        }
        pub fn native_server_players(&self) -> Vec<NativeServerPlayer> {
            self.native_acting_player().into_iter().collect()
        }
        pub fn native_net_send_to_player(&mut self, player: u128, channel: &str, data: &[u8]) -> Result<(), String> {
            self.sent.push((player, channel.to_owned(), data.to_vec()));
            Ok(())
        }
        pub fn native_net_send_to_server(&mut self, channel: &str, data: &[u8]) -> Result<(), String> {
            self.sent.push((0, channel.to_owned(), data.to_vec()));
            Ok(())
        }
        pub fn native_net_from_client(&mut self, player: u128, channel: &str, data: &[u8]) { let _ = (player, channel, data); }
        pub fn native_net_from_server(&mut self, channel: &str, data: &[u8]) { let _ = (channel, data); }
        pub fn native_tick(&mut self) {}
        pub fn native_window(&self) -> bool { true }
        pub fn native_playing(&self) -> bool { true }
        pub fn native_part_model(&mut self, state: BlockStateId) -> Option<u32> { Some(state.0) }
        pub fn native_part_set(&mut self, key: u64, model: u32, block: [i32; 3], axis: u8, rpm: f32, phase: f32) {
            let _ = (key, phase);
            self.parts.push((model, block, axis, rpm));
        }
        pub fn native_part_clear(&mut self, key: u64) { let _ = key; }
        pub fn native_parts_seconds(&self) -> f32 { 1.0 }
        pub fn native_part_item_model(&mut self, item: &str, pose: glam::Mat4) -> Option<u32> {
            let _ = pose;
            Some(1000 + item.len() as u32)
        }
        pub fn native_item_uses_block_light(&self, item: &str) -> Option<bool> { let _ = item; Some(false) }
        #[allow(clippy::too_many_arguments)]
        pub fn native_part_glide(&mut self, key: u64, model: u32, block: [i32; 3], axis: u8, rpm: f32, phase: f32,
                                 offset: [f32; 4], velocity: [f32; 3]) {
            let _ = (key, phase, offset);
            self.glides.push((model, block, axis, rpm, velocity));
        }
        pub fn native_contents_changed(&mut self, pos: [i32; 3]) { let _ = pos; }
        pub fn native_use_item_on(&mut self, stack: &horizon_client::items::ItemStack, pos: [i32; 3], face: [i32; 3], claimed: &mut bool) {
            let _ = (stack, pos, face, claimed);
        }
        pub fn native_use_item(&mut self, stack: &horizon_client::items::ItemStack, claimed: &mut bool) {
            let _ = (stack, claimed);
        }
        pub fn native_hud(&mut self, batch: &mut horizon_client::hud::HudBatch) { let _ = batch; }
        pub fn native_text_width(&self, text: &str) -> f32 { text.len() as f32 * 6.0 }
        pub fn native_fill(&self, batch: &mut horizon_client::hud::HudBatch, rect: [f32; 4], argb: u32) {
            let _ = (batch, rect, argb);
        }
        pub fn native_text(&self, batch: &mut horizon_client::hud::HudBatch, text: &str, at: [f32; 2], rgb: u32, shadow: bool) {
            let _ = (at, rgb, shadow);
            batch.drawn.push(text.to_owned());
        }
        pub fn native_insert_item(&mut self, pos: [i32; 3], face: horizon_core::geometry::Direction,
                                  stack: horizon_client::items::ItemStack) -> Option<horizon_client::items::ItemStack> {
            let _ = (pos, face);
            Some(stack)
        }
        pub fn native_items_in(&mut self, dimension: &str, min: [f64; 3], max: [f64; 3],
                               f: impl FnMut(u32, &mut horizon_client::items::ItemStack, [f64; 3], &mut [f64; 3])) {
            let _ = (dimension, min, max, f);
        }
        pub fn native_place_block(&mut self, stack: &horizon_client::items::ItemStack, pos: [i32; 3], state: BlockStateId) {
            let _ = (stack, pos, state);
        }
        pub fn native_set_block_property(&mut self, pos: [i32; 3], key: &str, value: &str) -> bool {
            self.native_set_block_property_with(pos, key, value, 3)
        }
        pub fn native_part_model_posed(&mut self, state: BlockStateId, pose: glam::Mat4) -> Option<u32> {
            let _ = pose;
            Some(5000 + state.0)
        }
        pub fn native_part_model_tinted(&mut self, state: BlockStateId, pose: glam::Mat4, tint: Option<u32>) -> Option<u32> {
            let _ = (pose, tint);
            Some(6000 + state.0)
        }
        pub fn native_part_model_compound(&mut self, blocks: &[(BlockStateId, glam::Mat4)]) -> Option<u32> {
            Some(7000 + blocks.len() as u32)
        }
        pub fn native_part_model_free(&mut self, model: u32) { let _ = model; }
        pub fn native_update_entity_after_fall_on(&mut self, pos: [i32; 3], stack: &mut horizon_client::items::ItemStack, at: [f64; 3]) {
            let _ = (pos, stack, at);
        }
        pub fn native_wheel(&mut self, steps: f32, claimed: &mut bool) { let _ = (steps, claimed); }
        pub fn native_chunk_data_save(&self, dimension: &str, chunk: [i32; 2], data: &mut horizon_world::nbt::Compound) {
            let _ = (dimension, chunk, data);
        }
        pub fn native_chunk_data<'c>(chunk: &'c Chunk, mod_id: &str) -> Option<&'c horizon_world::nbt::Compound> {
            let _ = (chunk, mod_id);
            None
        }
        pub fn native_mark_chunk_unsaved(&mut self, dimension: &str, chunk: [i32; 2]) -> bool {
            let _ = dimension;
            self.unsaved.push(chunk);
            true
        }
        pub fn pick(&self) -> Option<Hit> { self.aim.map(|pos| Hit { pos }) }
        pub fn with_level<R>(&mut self, f: impl FnOnce(&Behaviours, &mut GameLevel<'_>, &mut Ticks) -> R) -> R {
            let mut level = GameLevel { registry: self.pack.assets.registry };
            f(&Behaviours, &mut level, &mut Ticks)
        }
        pub fn native_set_block_property_with(&mut self, pos: [i32; 3], key: &str, value: &str, flags: i32) -> bool {
            let _ = flags;
            self.said.push(format!("set {pos:?} {key}={value}"));
            true
        }

        pub fn native_set_block(&mut self, dimension: &str, pos: [i32; 3], state: BlockStateId, flags: i32) -> bool {
            let _ = (dimension, flags);
            self.world.blocks.insert(pos, state.0);
            true
        }
        pub fn native_update_signal_neighbors(&mut self, dimension: &str, pos: [i32; 3]) -> bool {
            let _ = (dimension, pos);
            true
        }
        pub fn native_feedback(&mut self, line: horizon_client::text::Text) { self.said.push(line.0); }
        pub fn native_run_command(&mut self, line: &str) -> bool { self.said.push(line.to_owned()); true }
    }
}
pub struct Assets { pub registry: &'static BlockRegistry }
pub struct Pack { pub assets: Assets }
pub struct World {
    pub blocks: std::collections::HashMap<[i32; 3], u32>, pub game_time: i64,
    pub block_entities: std::collections::HashMap<(i32, i32, i32), horizon_client::containers::BlockEntity>,
}
impl World {
    pub fn block(&self, x: i32, y: i32, z: i32) -> BlockStateId { BlockStateId(*self.blocks.get(&[x, y, z]).unwrap_or(&0)) }
    pub fn loaded(&self, x: i32, z: i32) -> bool { let _ = (x, z); true }
}
pub struct Hit { pub pos: [i32; 3] }
pub struct Player { pub exhaustion: f32 }
impl Player { pub fn cause_food_exhaustion(&mut self, amount: f32) { self.exhaustion += amount; } }
pub struct Game<'a> {
    pub pack: Pack, pub calls: &'a mut u32, pub said: Vec<String>, pub world: World,
    pub sent: Vec<(u128, String, Vec<u8>)>, pub parts: Vec<(u32, [i32; 3], u8, f32)>,
    pub aim: Option<[i32; 3]>, pub unsaved: Vec<[i32; 2]>,
    pub creative: bool, pub player: Option<Player>, pub keys: std::collections::HashSet<winit::keyboard::KeyCode>,
    pub glides: Vec<(u32, [i32; 3], u8, f32, [f32; 3])>, pub hand: horizon_client::first_person::Hand, pub yaw: f32, pub pitch: f32,
    pub armor: [Option<horizon_client::items::ItemStack>; 4],
    pub inventory: Vec<Option<horizon_client::items::ItemStack>>, pub selected: usize, pub dim: Dim,
    pub res: Res, pub custom_recipes: std::collections::HashMap<String, Vec<horizon_client::crafting::CustomRecipe>>,
    pub cooking: horizon_client::crafting::Recipes,
}
impl Game<'_> {
    pub fn recipes(&self) -> &horizon_client::crafting::Recipes { &self.cooking }
}
pub struct Dim;
impl Dim { pub fn id(&self) -> &str { "minecraft:overworld" } }
pub struct GameLevel<'a> { pub registry: &'a BlockRegistry }
pub struct Behaviours;
pub struct Facts { pub flags: u32, pub hardness: f32 }
#[derive(Clone, Copy)] pub struct BlockId(pub u16);
pub struct BlockSet;
impl BlockSet { pub fn contains(&self, block: BlockId) -> bool { let _ = block; false } }
impl Behaviours {
    /// Air (state 0) and water (state 20) have no collision; every other state of the fixture has.
    pub fn facts(&self, state: BlockStateId) -> Facts {
        let empty = state.0 == 0 || state.0 == 20;
        Facts { flags: if empty { horizon_world::level::data::state_flags::COLLISION_EMPTY } else { 0 }, hardness: 1.5 }
    }
    /// No block tags; no cached shapes (the collision flag decides).
    pub fn tag(&self, name: &str) -> Option<&BlockSet> { let _ = name; None }
    pub fn is_replaceable(&self, state: BlockStateId) -> bool { state.0 == 0 }
    pub fn is_air(&self, state: BlockStateId) -> bool { state.0 == 0 }
    pub fn block(&self, state: BlockStateId) -> BlockId { BlockId(state.0 as u16) }
    pub fn collision_shape(&self, state: BlockStateId) -> Option<&horizon_core::geometry::VoxelShape> { let _ = state; None }
    /// State 20 is a water source.
    pub fn fluid(&self, state: BlockStateId) -> horizon_world::fluid::FluidState {
        use horizon_world::fluid::{FluidKind, FluidState};
        FluidState(if state.0 == 20 { FluidKind::Water } else { FluidKind::Empty })
    }
}
pub struct Res;
pub struct BlockFacts { pub sturdy_full: u8 }
pub struct AllFacts;
impl AllFacts {
    /// Stone (state 1) is sturdy on every face; nothing else is.
    pub fn get(&self, state: BlockStateId) -> BlockFacts { BlockFacts { sturdy_full: if state.0 == 1 { 63 } else { 0 } } }
}
pub struct Providers;
impl Providers {
    /// Coal burns 1600 ticks; nothing else is fuel.
    pub fn burn_time<T>(&self, items: T, stack: &horizon_client::items::ItemStack, fast: bool) -> i32 {
        let _ = (items, fast);
        if &*stack.item == "minecraft:coal" { 1600 } else { 0 }
    }
}
impl Res {
    pub fn providers(&self) -> &Providers { &Providers }
    pub fn behaviours(&self) -> &Behaviours { &Behaviours }
    pub fn facts(&self) -> &AllFacts { &AllFacts }
}
pub struct Ticks;
thread_local! {
    /// Blocks giving a signal of 15 toward every side, for the latch test.
    pub static SIGNALS: std::cell::RefCell<std::collections::HashSet<[i32; 3]>> = Default::default();
}
impl Behaviours {
    pub fn has_neighbor_signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos) -> bool {
        let _ = (level, ticks, pos);
        true
    }
    pub fn signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos,
                  direction: horizon_core::geometry::Direction) -> i32 {
        let _ = (level, ticks, direction);
        if SIGNALS.with_borrow(|s| s.contains(&[pos.x, pos.y, pos.z])) { 15 } else { 0 }
    }
}
#[test] fn every_hook_reaches_the_engine() {
    use horizon_world::registry::BlockState;
    use std::collections::BTreeMap;
    let props = |k: &str, v: &str| BTreeMap::from([(k.to_owned(), v.to_owned())]);
    let mut states = vec![
        BlockState { name: "minecraft:air".into(), properties: BTreeMap::new() },
        BlockState { name: "minecraft:stone".into(), properties: BTreeMap::new() },
    ];
    // 2: shaft along x; 3: creative motor facing east; 4: depot.
    states.push(BlockState { name: "create:shaft".into(), properties: props("axis", "x") });
    states.push(BlockState { name: "create:creative_motor".into(), properties: props("facing", "east") });
    states.push(BlockState { name: "create:depot".into(), properties: BTreeMap::new() });
    // 5: a mechanical press facing east (turning on x).
    states.push(BlockState { name: "create:mechanical_press".into(), properties: props("facing", "east") });
    // 6, 7, 8: a belt going south (turning on x): start, middle, end; 9: a motor facing east.
    for part in ["start", "middle", "end"] {
        states.push(BlockState { name: "create:belt".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "south".to_owned()), ("slope".to_owned(), "horizontal".to_owned()),
            ("part".to_owned(), part.to_owned()), ("casing".to_owned(), "false".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    states.push(BlockState { name: "create:creative_motor".into(), properties: props("facing", "east") });
    // 10..: a toggle latch facing north, every powered/powering pair (10: both off).
    for (powered, powering) in [("false", "false"), ("true", "false"), ("false", "true"), ("true", "true")] {
        states.push(BlockState { name: "create:powered_toggle_latch".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "north".to_owned()), ("powered".to_owned(), powered.to_owned()),
            ("powering".to_owned(), powering.to_owned())]) });
    }
    // 14: a shaft along z; 15: a fluid tank; 16: the water box's partial.
    states.push(BlockState { name: "create:shaft".into(), properties: props("axis", "z") });
    states.push(BlockState { name: "create:fluid_tank".into(), properties: BTreeMap::from([
        ("bottom".to_owned(), "true".to_owned()), ("shape".to_owned(), "window".to_owned()), ("top".to_owned(), "true".to_owned())]) });
    states.push(BlockState { name: "create:part_fluid_water".into(), properties: props("facing", "up") });
    // 17: a windmill bearing facing north; 18: a white sail facing north (in the xy plane).
    states.push(BlockState { name: "create:windmill_bearing".into(), properties: props("facing", "north") });
    states.push(BlockState { name: "create:white_sail".into(), properties: props("facing", "north") });
    // 19: an encased fan facing east; 20: a water source.
    states.push(BlockState { name: "create:encased_fan".into(), properties: props("facing", "east") });
    states.push(BlockState { name: "minecraft:water".into(), properties: props("level", "0") });
    let registry = Box::leak(Box::new(BlockRegistry { states }));
    let mut calls = 0;
    let mut game = Game { pack: Pack { assets: Assets { registry } }, calls: &mut calls, said: vec![],
        world: World { blocks: Default::default(), game_time: 0, block_entities: Default::default() }, sent: vec![], parts: vec![], aim: None, unsaved: vec![],
        creative: false, player: Some(Player { exhaustion: 0.0 }), keys: Default::default(),
        glides: vec![], hand: horizon_client::first_person::Hand::Main, yaw: 180.0, pitch: 0.0, armor: [None, None, None, None],
        inventory: vec![Some(horizon_client::items::ItemStack { item: "minecraft:iron_ingot".into(), count: 5, damage: 0, extra: None })],
        selected: 0, dim: Dim, res: Res, custom_recipes: Default::default(), cooking: Default::default() };
    // data/create/recipe/pressing/iron_ingot.json
    game.custom_recipes.insert("create:pressing".into(), vec![horizon_client::crafting::CustomRecipe {
        id: "create:pressing/iron_ingot".into(),
        json: std::sync::Arc::new(serde_json::json!({
            "type": "create:pressing", "ingredients": ["minecraft:iron_ingot"],
            "results": [{"id": "create:iron_sheet"}]})),
    }]);
    // data/create/recipe/splashing/ice.json, for the fan.
    game.custom_recipes.insert("create:splashing".into(), vec![horizon_client::crafting::CustomRecipe {
        id: "create:splashing/ice".into(),
        json: std::sync::Arc::new(serde_json::json!({
            "type": "create:splashing", "ingredients": ["minecraft:ice"],
            "results": [{"id": "minecraft:packed_ice"}]})),
    }]);
    game.native_world_lifecycle(native_lifecycle::WorldPhase::WorldOpenReady, "");
    assert_eq!(create_engine::states::index(2), Some(0));
    // A column with a motor at (0, 0, 0) and a shaft at (1, 0, 0).
    let mut blocks = vec![0u32; 4096];
    blocks[0] = 3;
    blocks[1] = 2;
    let chunk = Chunk { x: 0, z: 0, sections: vec![std::sync::Arc::new(horizon_client::chunk::Section {
        blocks: horizon_client::chunk::Blocks(blocks) })] };
    game.native_chunk_load("minecraft:overworld", &chunk);
    // A shaft placed later at (2, 0, 0), through the hot hook.
    let level = GameLevel { registry };
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 2, y: 0, z: 0 }, BlockStateId(0), BlockStateId(2));
    game.native_level_tick_post("minecraft:overworld");
    // The solve went to the level's player; as their client, it shows the parts.
    let speeds: Vec<Vec<u8>> = game.sent.iter().filter(|m| m.1 == "create:kinetics").map(|m| m.2.clone()).collect();
    assert!(!speeds.is_empty(), "the server sent the speeds");
    game.world.blocks.insert([2, 0, 0], 2);
    for m in &speeds { game.native_net_from_server("create:kinetics", m); }
    assert!(game.parts.iter().any(|p| p.1 == [2, 0, 0] && p.2 == 0 && p.3 == 16.0), "{:?}", game.parts);
    // The player scrolls up once on the motor: the client claims the wheel and tells the server,
    // which speeds the motor up, re-solves, and saves the setting with its chunk.
    game.world.blocks.insert([0, 0, 0], 3);
    game.aim = Some([0, 0, 0]);
    let mut claimed = false;
    game.native_wheel(1.0, &mut claimed);
    assert!(claimed);
    let scroll = game.sent.iter().rev().find(|m| m.1 == "create:scroll").unwrap().2.clone();
    game.native_net_from_client(7, "create:scroll", &scroll);
    game.native_level_tick_post("minecraft:overworld");
    assert_eq!(game.unsaved, vec![[0, 0]]);
    let me = game.native_acting_player().unwrap();
    let mut ran = false;
    game.native_perform_command(&me, "create_debug speed 2 0 0", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 2 0 0: 17 rpm"));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [0, 0], &mut data);
    let expected = horizon_world::nbt::Tag::Compound(
        [("motors".into(), horizon_world::nbt::Tag::IntArray(vec![0, 0, 0, 17]))].into_iter().collect());
    assert_eq!(data.get(&"create".into()), Some(&expected));
    let mut commands = Vec::new();
    game.native_register_commands(&mut commands);
    assert_eq!(commands.len(), 3);
    let player = game.native_acting_player().unwrap();
    let mut ran = false;
    game.native_perform_command(&player, "create_debug speed 2 0 0", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 2 0 0: 17 rpm"), "after the scroll");
    // A depot at (5, 0, 0): its block entity, its slots, then a player facing north puts iron
    // on it; the server sends its contents and the client glides the stack in.
    let mut entity = None;
    game.native_block_entity("create:depot", &mut entity);
    assert_eq!(entity.map(|e| e.0).as_deref(), Some("create:depot"));
    let mut container = None;
    game.native_block_entity_container("create:depot", &mut container);
    assert!(container.as_ref().is_some_and(|c| c.slots == 9 && c.only_when_empty));
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 5, y: 0, z: 0 }, BlockStateId(0), BlockStateId(4));
    game.world.blocks.insert([5, 0, 0], 4);
    game.world.block_entities.insert((5, 0, 0), horizon_client::containers::BlockEntity::Mod(
        horizon_client::containers::ModBlockEntity { items: vec![] }));
    let mut canceled = false;
    game.native_right_click_block([5, 0, 0], [0, 1, 0], &mut canceled);
    assert!(canceled && game.inventory[0].is_none(), "the stack left the hand");
    game.native_level_tick_post("minecraft:overworld");
    let depot: Vec<Vec<u8>> = game.sent.iter().filter(|m| m.1 == "create:depot").map(|m| m.2.clone()).collect();
    assert_eq!(depot.len(), 1, "one message for the change");
    for m in &depot { game.native_net_from_server("create:depot", m); }
    let glide = game.glides.last().expect("the stack is drawn");
    assert_eq!((glide.1, glide.2), ([5, 0, 0], 1));
    assert!(glide.4[2] < 0.0, "it glides north, toward the centre from the south edge: {:?}", glide.4);
    // A press two above the depot, driven by a motor at 16 rpm: once the stack arrives the press
    // runs a cycle and, at its lowest point, presses one ingot into a sheet (Create's iron_ingot
    // pressing recipe; one at a time, the rest stays held).
    for (pos, state) in [([4, 2, 0], 3), ([5, 2, 0], 5)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    for _ in 0..80 { game.native_level_tick_post("minecraft:overworld"); }
    let horizon_client::containers::BlockEntity::Mod(depot) = &game.world.block_entities[&(5, 0, 0)];
    let slot = |i: usize| depot.items.get(i).and_then(|s| s.as_ref()).map(|s| (s.item.to_string(), s.count));
    assert_eq!(slot(0), Some(("minecraft:iron_ingot".into(), 4)), "one of five pressed");
    assert_eq!(slot(1), Some(("create:iron_sheet".into(), 1)));
    assert!(game.sent.iter().any(|m| m.1 == "create:press"), "clients see the head move");
    // A belt south from (10, 0, 10) to (10, 0, 12), a motor west of its start, and a depot past
    // its end: four iron ingots land on the start, ride the belt and end up on the depot.
    let belt = [([10, 0, 10], 6), ([10, 0, 11], 7), ([10, 0, 12], 8), ([9, 0, 10], 9), ([10, 0, 13], 4)];
    for (pos, state) in belt {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    game.world.block_entities.insert((10, 0, 13), horizon_client::containers::BlockEntity::Mod(
        horizon_client::containers::ModBlockEntity { items: vec![] }));
    game.native_level_tick_post("minecraft:overworld");
    let mut stack = horizon_client::items::ItemStack { item: "minecraft:gold_ingot".into(), count: 4, damage: 0, extra: None };
    game.native_update_entity_after_fall_on([10, 0, 10], &mut stack, [10.5, 0.8125, 10.5]);
    assert_eq!(stack.count, 0, "the belt took the stack");
    for _ in 0..200 { game.native_level_tick_post("minecraft:overworld"); }
    let horizon_client::containers::BlockEntity::Mod(end) = &game.world.block_entities[&(10, 0, 13)];
    assert_eq!(end.items.first().and_then(|s| s.as_ref()).map(|s| (s.item.to_string(), s.count)),
        Some(("minecraft:gold_ingot".into(), 4)), "carried onto the depot");
    let belts: Vec<Vec<u8>> = game.sent.iter().filter(|m| m.1 == "create:belt").map(|m| m.2.clone()).collect();
    assert!(!belts.is_empty(), "clients see the belt's items");
    for m in &belts { game.native_net_from_server("create:belt", m); }
    // A stack still on the belt is saved with the chunk of its controller.
    let mut more = horizon_client::items::ItemStack { item: "minecraft:copper_ingot".into(), count: 2, damage: 0, extra: None };
    game.native_update_entity_after_fall_on([10, 0, 10], &mut more, [10.5, 0.8125, 10.5]);
    for _ in 0..3 { game.native_level_tick_post("minecraft:overworld"); }
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [0, 0], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create".into()) else { panic!("no create data") };
    let Some(horizon_world::nbt::Tag::List(saved)) = own.get(&"belts".into()) else { panic!("no belts saved: {own:?}") };
    assert_eq!(saved.len(), 1);
    // A toggle latch facing north at (20, 0, 20), a signal at its back: a tick later it is on and
    // gives 15 toward the north, nothing elsewhere; its repeater template's tick is the glue's.
    game.world.blocks.insert([20, 0, 20], 10);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 20, y: 0, z: 20 }, BlockStateId(0), BlockStateId(10));
    SIGNALS.with_borrow_mut(|s| s.insert([20, 0, 19]));
    game.native_level_tick_post("minecraft:overworld");
    assert_eq!(game.world.blocks[&[20, 0, 20]], 10, "not before its tick");
    game.world.game_time += 1;
    game.native_level_tick_post("minecraft:overworld");
    let on = game.world.blocks[&[20, 0, 20]];
    assert_eq!(on, 13, "powered and powering");
    let mut level = GameLevel { registry };
    let read = |level: &GameLevel<'_>, face| {
        let mut signal = 0;
        level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 20, y: 0, z: 20 }, BlockStateId(on), face, false, &mut signal);
        signal
    };
    // The hook's face is the latch's side touching the reader: its south face, toward what it
    // powers.
    assert_eq!(read(&level, horizon_core::geometry::Direction::South), 15);
    assert_eq!(read(&level, horizon_core::geometry::Direction::North), 0);
    let mut source = false;
    level.native_is_signal_source(BlockStateId(on), &mut source);
    assert!(source);
    let mut handled = false;
    level.native_tick_block(&mut Ticks, LevelDimension::Overworld, BlockPos { x: 20, y: 0, z: 20 }, BlockStateId(on), &mut handled);
    assert!(handled);
    // A wrench on the top of a shaft along x turns it to z (a quarter turn about y).
    game.world.blocks.insert([30, 0, 30], 2);
    let wrench = horizon_client::items::ItemStack { item: "create:wrench".into(), count: 1, damage: 0, extra: None };
    let mut claimed = false;
    game.native_use_item_on(&wrench, [30, 0, 30], [0, 1, 0], &mut claimed);
    assert!(claimed);
    assert_eq!(game.world.blocks[&[30, 0, 30]], 14, "turned to z");
    // Another item is not the wrench's to take.
    let stick = horizon_client::items::ItemStack { item: "minecraft:stick".into(), count: 1, damage: 0, extra: None };
    let mut claimed = false;
    game.native_use_item_on(&stick, [30, 0, 30], [0, 1, 0], &mut claimed);
    assert!(!claimed);
    // A fluid tank at (40, 0, 40): a creative player empties a water bucket into it; it is saved
    // with its chunk, and the client draws its water.
    game.world.blocks.insert([40, 0, 40], 15);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 40, y: 0, z: 40 }, BlockStateId(0), BlockStateId(15));
    game.creative = true;
    game.inventory[0] = Some(horizon_client::items::ItemStack { item: "minecraft:water_bucket".into(), count: 1, damage: 0, extra: None });
    let mut canceled = false;
    game.native_right_click_block([40, 0, 40], [0, 1, 0], &mut canceled);
    assert!(canceled);
    // The bucket's own use that follows the click is taken: no water placed in the world.
    let bucket = horizon_client::items::ItemStack { item: "minecraft:water_bucket".into(), count: 1, damage: 0, extra: None };
    let mut claimed = false;
    game.native_use_item(&bucket, &mut claimed);
    assert!(claimed);
    let mut claimed = false;
    game.native_use_item(&bucket, &mut claimed);
    assert!(!claimed, "only the one use");
    game.native_level_tick_post("minecraft:overworld");
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [2, 2], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create".into()) else { panic!("no create data") };
    assert_eq!(own.get(&"tanks".into()), Some(&horizon_world::nbt::Tag::IntArray(vec![40, 0, 40, 0, 1000])));
    let tanks: Vec<Vec<u8>> = game.sent.iter().filter(|m| m.1 == "create:tank").map(|m| m.2.clone()).collect();
    assert!(!tanks.is_empty());
    for m in &tanks { game.native_net_from_server("create:tank", m); }
    assert!(game.parts.iter().any(|p| p.0 == 6016 && p.1 == [40, 0, 40]), "the water's box is drawn");
    // A windmill bearing at (50, 0, 50) facing north, eight sails in a row in front of it: an
    // empty hand assembles them the next tick (gone from the world, generating 1 rpm), they are
    // saved with the chunk, and a second click puts them back.
    game.world.blocks.insert([50, 0, 50], 17);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 50, y: 0, z: 50 }, BlockStateId(0), BlockStateId(17));
    for x in 46..54 { game.world.blocks.insert([x, 0, 49], 18); }
    game.inventory[0] = None;
    let mut canceled = false;
    game.native_right_click_block([50, 0, 50], [0, 0, -1], &mut canceled);
    assert!(canceled);
    // The off hand's click that Horizon's client sends after it is taken, not acted on.
    game.hand = horizon_client::first_person::Hand::Off;
    let mut canceled = false;
    game.native_right_click_block([50, 0, 50], [0, 0, -1], &mut canceled);
    assert!(canceled);
    game.hand = horizon_client::first_person::Hand::Main;
    game.native_level_tick_post("minecraft:overworld");
    assert!((46..54).all(|x| game.world.blocks.get(&[x, 0, 49]) == Some(&0)), "the sails left");
    game.native_level_tick_post("minecraft:overworld");
    let me = game.native_acting_player().unwrap();
    let mut ran = false;
    game.native_perform_command(&me, "create_debug speed 50 0 50", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 50 0 50: 1 rpm"));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [3, 3], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create".into()) else { panic!("no create data") };
    assert!(own.contains_key(&"windmills".into()));
    assert!(game.sent.iter().any(|m| m.1 == "create:contraption" && m.2.len() == 17 + 8 * 16));
    game.world.game_time += 5;
    let mut canceled = false;
    game.native_right_click_block([50, 0, 50], [0, 0, -1], &mut canceled);
    assert!((46..54).all(|x| game.world.blocks.get(&[x, 0, 49]) == Some(&18)), "the sails are back");
    // A fan at (60, 0, 60) facing east, its motor behind it, water in front and a depot below the
    // current three blocks out holding two ice: washed into packed ice after 151 ticks.
    game.world.block_entities.insert((63, -1, 60), horizon_client::containers::BlockEntity::Mod(
        horizon_client::containers::ModBlockEntity { items: vec![Some(horizon_client::items::ItemStack {
            item: "minecraft:ice".into(), count: 2, damage: 0, extra: None })] }));
    for (pos, state) in [([59, 0, 60], 3), ([60, 0, 60], 19), ([61, 0, 60], 20), ([63, -1, 60], 4)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    let washed = |game: &Game<'_>| {
        let horizon_client::containers::BlockEntity::Mod(d) = &game.world.block_entities[&(63, -1, 60)];
        d.items.first().and_then(|s| s.as_ref()).map(|s| (s.item.to_string(), s.count))
    };
    for _ in 0..100 { game.native_level_tick_post("minecraft:overworld"); }
    assert_eq!(washed(&game), Some(("minecraft:ice".into(), 2)), "not yet");
    for _ in 0..60 { game.native_level_tick_post("minecraft:overworld"); }
    assert_eq!(washed(&game), Some(("minecraft:packed_ice".into(), 2)));
    // Fans placed from an item, by a player looking north (yaw 180): alone it blows back at them
    // (south); with a motor's shaft west of it, it turns its back to the shaft (east, as placed).
    let stack = horizon_client::items::ItemStack { item: "create:encased_fan".into(), count: 1, damage: 0, extra: None };
    for (pos, state) in [([72, 0, 72], 19), ([69, 0, 75], 3), ([70, 0, 75], 19)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    game.native_place_block(&stack, [72, 0, 72], BlockStateId(19));
    assert_eq!(game.said.last().map(String::as_str), Some("set [72, 0, 72] facing=south"));
    game.native_place_block(&stack, [70, 0, 75], BlockStateId(19));
    assert!(!game.said.iter().any(|l| l.starts_with("set [70, 0, 75]")), "already facing away from the shaft");
    // Goggles on: the creative motor at (0, 0, 0) under the crosshair tells its capacity.
    let mut batch = horizon_client::hud::HudBatch { width: 320.0, height: 240.0, drawn: vec![] };
    game.native_hud(&mut batch);
    assert!(batch.drawn.is_empty(), "nothing without goggles");
    game.armor[0] = Some(horizon_client::items::ItemStack { item: "create:goggles".into(), count: 1, damage: 0, extra: None });
    game.aim = Some([0, 0, 0]);
    game.native_hud(&mut batch);
    assert_eq!(batch.drawn.first().map(|s| s.trim()), Some("Generator Stats:"), "{:?}", batch.drawn);
    game.native_chunk_unload("minecraft:overworld", &chunk);
    let mut ran = false;
    game.native_perform_command(&player, "create_debug speed 1 0 0", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 1 0 0: not kinetic"));
    game.native_world_lifecycle(native_lifecycle::WorldPhase::WorldCloseFinal, "");
    assert_eq!(create_engine::states::index(2), None);
    assert_eq!(calls, 2, "the original body still runs");
}
''',
        'native-hooks.json': json.dumps({'hooks': [{'name': n, 'version': 1, 'target': t} for n, t in hooks]}),
        '.horizon-revision': horizon.PIN + '\n',
    }
    for rel, text in files.items():
        (original/rel).parent.mkdir(parents=True, exist_ok=True)
        (original/rel).write_text(text)
    (original/'rust-toolchain.toml').write_bytes((root/'rust-toolchain.toml').read_bytes())


def main():
    root = horizon.ROOT
    (root/'target').mkdir(exist_ok=True)
    run(['cargo', 'build', '--locked', '--offline', '-p', 'horizon-mixins-cli'], cwd=horizon.MIXINS)
    cli = horizon.MIXINS/'target/debug/horizon-mixins'
    manifests = list((root/'mixins').glob('*/horizon-mixin.toml'))
    assert len(manifests) == 1, 'this test expects the one create bundle'
    manifest = tomllib.loads(manifests[0].read_text())
    with tempfile.TemporaryDirectory(prefix='template-test-', dir=root/'target') as scratch:
        scratch = Path(scratch)
        original, stage = scratch/'original', scratch/'stage'
        write_fixture(original, root)
        run(['cargo', 'generate-lockfile', '--offline'], cwd=original)
        before = snapshot(original)
        base = ['--workspace', str(original), '--mods', str(root/'mixins')]
        report = json.loads(run([str(cli), 'check', *base, '--out', str(stage), '--trust-native-mods']).stdout)
        assert report['injections'] == len(manifest['injection']), report
        assert report['modules'] == len(manifest['module']), report
        assert '1 passed' in report['cargo_stdout'], report['cargo_stdout']
        first = snapshot(stage)
        second = json.loads(run([str(cli), 'check', *base, '--out', str(stage), '--trust-native-mods']).stdout)
        assert not second['stage_changed'] and snapshot(stage) == first
        assert snapshot(original) == before
        plan_path = scratch/'plan.json'
        run([str(cli), 'plan', *base, '--out', str(plan_path)])
        plan = json.loads(plan_path.read_text())
        assert plan['schema'] == horizon.COMPAT['mixin_schema']
        assert plan['injections'][0]['mod_id'] == manifest['mod']['id']
        audit(original, stage, plan_path)
        (original/'.horizon-revision').write_text('incompatible\n')
        bad = subprocess.run([str(cli), 'apply', *base, '--out', str(stage)], capture_output=True, text=True)
        assert bad.returncode and json.loads(bad.stderr)['code'] == 'E_REVISION'
        assert snapshot(stage) == first
        (original/'.horizon-revision').write_text(horizon.PIN+'\n')
        hooks = original/'native-hooks.json'
        hooks.write_text(hooks.read_text().replace('native_world_lifecycle', 'renamed_lifecycle'))
        bad = subprocess.run([str(cli), 'apply', *base, '--out', str(stage)], capture_output=True, text=True)
        assert bad.returncode and json.loads(bad.stderr)['code'] == 'E_TARGET_NOT_FOUND', bad.stderr
        assert snapshot(stage) == first
    print(json.dumps({'status': 'ok', 'compiled_runtime_and_original_body': True,
                      'only_planned_asts_changed': True, 'deterministic_bytes_and_mtimes': True,
                      'incompatible_revision_and_target_rejected': True}, indent=2))


if __name__ == '__main__':
    main()
