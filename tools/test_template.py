#!/usr/bin/env python3
"""Compile the real bundle on a tiny source fixture; no Horizon/assets required."""
import json
import re
import shutil
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
        ('mod_screen_draw', "horizon_client::game::native_hooks::Game<'_>::native_mod_screen_draw"),
        ('mod_screen_text', "horizon_client::game::native_hooks::Game<'_>::native_mod_screen_text"),
        ('mod_screen_mouse', "horizon_client::game::native_hooks::Game<'_>::native_mod_screen_mouse"),
        ('mod_screen_scroll', "horizon_client::game::native_hooks::Game<'_>::native_mod_screen_scroll"),
        ('mod_screen_removed', "horizon_client::game::native_hooks::Game<'_>::native_mod_screen_removed"),
        ('entity_collisions', "horizon_client::game::native_hooks::Game<'_>::native_entity_collisions"),
        ('item_handler_insert', "horizon_client::game::native_hooks::Game<'_>::native_item_handler_insert"),
        ('item_handler_extract', "horizon_client::game::native_hooks::Game<'_>::native_item_handler_extract"),
        ('update_entity_after_fall_on',
         "horizon_client::game::native_hooks::Game<'_>::native_update_entity_after_fall_on"),
        ('block_signal', "horizon_client::game::native_hooks::GameLevel<'_>::native_block_signal"),
        ('analog_output_signal', "horizon_client::game::native_hooks::GameLevel<'_>::native_analog_output_signal"),
        ('is_signal_source', "horizon_client::game::native_hooks::GameLevel<'_>::native_is_signal_source"),
        ('tick_block', "horizon_client::game::native_hooks::GameLevel<'_>::native_tick_block"),
        ('use_item_on', "horizon_client::game::native_hooks::Game<'_>::native_use_item_on"),
        ('use_item', "horizon_client::game::native_hooks::Game<'_>::native_use_item"),
        ('place_block', "horizon_client::game::native_hooks::Game<'_>::native_place_block"),
        ('hud', "horizon_client::game::native_hooks::Game<'_>::native_hud"),
        ('player_tick', "horizon_client::game::native_hooks::Game<'_>::native_player_tick"),
        ('destroy_block', "horizon_client::game::native_hooks::Game<'_>::native_destroy_block"),
        ('hurt_enemy', "horizon_client::game::native_hooks::Game<'_>::native_hurt_enemy"),
        ('attack_entity', "horizon_client::game::native_hooks::Game<'_>::native_attack_entity"),
        ('left_click_block', "horizon_client::game::native_hooks::Game<'_>::native_left_click_block"),
        ('screen', "horizon_client::game::native_hooks::Game<'_>::native_screen"),
        ('menu_closed', "horizon_client::game::native_hooks::Game<'_>::native_menu_closed"),
        ('interact_entity', "horizon_client::game::native_hooks::Game<'_>::native_interact_entity"),
        ('input_key', "horizon_client::game::native_hooks::Game<'_>::native_input_key"),
        ('level_load', "horizon_client::game::native_hooks::Game<'_>::native_level_load"),
        ('level_save', "horizon_client::game::native_hooks::Game<'_>::native_level_save"),
        ('entity_inside', "horizon_client::game::native_hooks::Game<'_>::native_entity_inside"),
        ('minecart_pass', "horizon_client::game::native_hooks::Game<'_>::native_minecart_pass"),
        ('entity_mount', "horizon_client::game::native_hooks::Game<'_>::native_entity_mount"),
        ('interaction_key_mapping_triggered',
         "horizon_client::game::native_hooks::Game<'_>::native_interaction_key_mapping_triggered"),
        ('seat_loaded', "horizon_client::game::native_hooks::Game<'_>::native_seat_loaded"),
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
    #[derive(Clone, Copy, Debug, PartialEq, Default)] pub struct Vec3 { pub x: f64, pub y: f64, pub z: f64 }
    impl Vec3 { pub fn new(x: f64, y: f64, z: f64) -> Self { Self { x, y, z } } }
    #[derive(Clone, Copy, Debug, PartialEq)] pub struct Aabb { min: Vec3, max: Vec3 }
    impl Aabb {
        pub fn new(min: Vec3, max: Vec3) -> Self { Self { min, max } }
        pub fn min_position(self) -> Vec3 { self.min }
        pub fn max_position(self) -> Vec3 { self.max }
    }
    pub struct VoxelShape;
    impl VoxelShape {
        pub fn boxes(&self) -> &[Aabb] { &[] }
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
    #[derive(Clone, PartialEq, Debug)] pub enum Tag { IntArray(Vec<i32>), Compound(Compound), List(Vec<Tag>), String(JavaString), Double(u64) }
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct JavaString(String);
    impl From<&str> for JavaString { fn from(s: &str) -> Self { JavaString(s.to_owned()) } }
    impl JavaString { pub fn to_string_lossy(&self) -> String { self.0.clone() } }
    pub type Compound = std::collections::BTreeMap<JavaString, Tag>;
    impl Tag {
        pub fn compound(entries: impl IntoIterator<Item = (impl Into<JavaString>, Tag)>) -> Self {
            Tag::Compound(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
        }
    }
}
pub mod level {
    #[derive(Clone, Copy)] pub enum Dimension { Overworld, Nether, End }
    pub mod data { pub mod state_flags {
        pub const CONDUCTOR: u32 = 1 << 2; pub const COLLISION_EMPTY: u32 = 1 << 5;
        pub const SOLID_RENDER: u32 = 1 << 9; pub const BLOCK_ENTITY: u32 = 1 << 11; pub const FULL_COLLISION: u32 = 1 << 14;
    } }
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
pub mod entities {
    pub struct V { pub x: f64, pub y: f64, pub z: f64 }
    pub struct ItemEntity { pub id: u32, pub stack: crate::items::ItemStack, pub position: V, pub velocity: V }
}
pub mod items {
    #[derive(Clone)] pub struct ItemStack { pub item: std::sync::Arc<str>, pub count: u32, pub damage: u32, pub extra: Option<Box<()>> }
    impl ItemStack {
        pub fn new(item: &str, count: u32) -> ItemStack { ItemStack { item: item.into(), count, damage: 0, extra: None } }
    }
    pub struct ItemInfo { pub max_stack: u32, pub fire_resistant: bool, pub remainder: Option<String>, pub crafting_remainder: Option<String>, pub max_damage: u32, pub block: Option<horizon_world::registry::BlockStateId> }
    pub struct Items { pub item_tags: std::collections::HashMap<String, std::collections::HashSet<String>> }
    impl Items { pub fn get(&self, item: &str) -> Option<ItemInfo> { let _ = item; Some(ItemInfo { max_stack: 64, fire_resistant: false, remainder: None, crafting_remainder: None, max_damage: 0, block: None }) } }
}
pub mod containers {
    pub struct ModBlockEntity { pub items: Vec<Option<crate::items::ItemStack>> }
    pub enum BlockEntity { Mod(ModBlockEntity), Spawner(Box<crate::chunk::block_entity::Spawner>) }
    pub struct ModContainer { pub slots: usize, pub faces: [Vec<usize>; 6], pub inputs: Vec<usize>, pub outputs: Vec<usize>, pub menu: Option<String>, pub only_when_empty: bool }
}
pub mod first_person {
    #[derive(Clone, Copy, PartialEq)] pub enum Hand { Main, Off }
}
pub mod mesher {
    use horizon_world::registry::BlockStateId;
    /// API 59's per-position model.
    #[derive(Clone, Debug, PartialEq)]
    pub struct ModelOverride { pub state: BlockStateId, pub faces: [Option<BlockStateId>; 6], pub boxes: Vec<[[f32; 3]; 2]> }
    impl ModelOverride {
        pub fn new(state: BlockStateId) -> Self { Self { state, faces: [None; 6], boxes: Vec::new() } }
    }
}
pub mod interaction_input {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Button { Attack, Use, Pick }
}
pub mod debug_keys {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum KeyAction { Press, Repeat, Release }
}
pub mod crafting {
    pub struct CustomRecipe { pub id: String, pub json: std::sync::Arc<serde_json::Value> }
    pub struct Ingredient(pub std::collections::HashSet<String>, pub Vec<String>);
    pub struct Cooking { pub id: String, pub kind: String, pub ingredient: Ingredient, pub result: crate::items::ItemStack }
    #[derive(Default)] pub struct Recipes { pub cooking: Vec<Cooking> }
    impl Recipes {
        /// No crafting recipes in the fixture.
        pub fn craft(&self, grid: &[Option<crate::items::ItemStack>], size: usize) -> Option<crate::items::ItemStack> { let _ = (grid, size); None }
    }
}
pub mod hud {
    pub struct HudBatch { pub width: f32, pub height: f32, pub drawn: Vec<String> }
}
pub mod text {
    #[derive(Clone)]
    pub struct Text(pub String);
    pub struct Style { pub color: Option<u32> }
    pub struct Span { pub text: String, pub style: Style }
    pub struct Spans { pub spans: Vec<Span> }
    impl Text {
        pub fn plain(&self) -> String { self.0.clone() }
        pub fn resolved(&self) -> Spans { Spans { spans: vec![Span { text: self.0.clone(), style: Style { color: None } }] } }
        pub fn literal(s: impl Into<String>) -> Self { Text(s.into()) }
        pub fn translate(key: &str, args: &[Text]) -> Self { let _ = args; Text(key.into()) }
        pub fn colored(s: impl Into<String>, color: u32) -> Self { let _ = color; Text(s.into()) }
        pub fn append(self, other: Text) -> Self { Text(self.0 + &other.0) }
    }
}
pub mod mobs {
    pub mod spawner {
        pub trait SpawnDataExt { fn id(&self) -> Option<String>; }
        impl SpawnDataExt for crate::chunk::block_entity::SpawnData {
            fn id(&self) -> Option<String> { Some(self.0.clone()) }
        }
    }
}
pub mod chunk {
    pub mod block_entity {
        pub struct SpawnData(pub String);
        pub enum Next { Potential(usize), Own(SpawnData) }
        pub struct Spawner { pub potentials: Vec<(SpawnData, i32)>, pub next: Option<Next> }
    }
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
        # The hopper's private container rules, where the funnels' extraction module goes.
        'crates/horizon-client/src/game/machines.rs': '''
use super::Game;
use crate::items::ItemStack;
use horizon_core::geometry::Direction;
type Pos = (i32, i32, i32);
#[allow(dead_code)]
enum Kind { Hopper, Furnace, Brewing, Mod, Composter(u8) }
struct View { kind: Kind }
impl View {
    fn slots_for_face(&self, face: Option<Direction>) -> Vec<usize> { let _ = face; Vec::new() }
    fn len(&self) -> usize { 0 }
}
struct Containers;
impl Containers {
    fn view(&self, pos: Pos, blocked_chests: bool) -> Option<View> { let _ = (pos, blocked_chests); None }
    fn get(&self, view: &View, slot: usize) -> Option<&ItemStack> { let _ = (view, slot); None }
    fn can_take(&self, view: &View, slot: usize, stack: &ItemStack, face: Direction) -> bool {
        let _ = (view, slot, stack, face); false
    }
    fn can_place(&self, view: &View, slot: usize, stack: &ItemStack) -> bool { let _ = (view, slot, stack); false }
    fn max_stack(&self, stack: &ItemStack) -> u32 { let _ = stack; 64 }
}
impl Game<'_> {
    fn containers(&self) -> Containers { Containers }
    fn slot_mut_at(&mut self, view: &View, slot: usize) -> Option<&mut Option<ItemStack>> { let _ = (view, slot); None }
    fn view_changed(&mut self, view: &View) { let _ = view; }
}
''',
        # The dispenser's private behaviours, where the deployer's item-use module goes.
        'crates/horizon-client/src/game/dispenser.rs': '''
use super::Game;
use crate::items::ItemStack;
use horizon_core::geometry::Direction;
type Pos = (i32, i32, i32);
type Dispensed = (Option<ItemStack>, i32);
impl Game<'_> {
    fn dispense_empty_bucket(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_filled_bucket(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_flint_and_steel(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_bone_meal(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_glass_bottle(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_glowstone(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_shears(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_honeycomb(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Result<Dispensed, ()> { let _ = (pos, facing); Ok((Some(stack), 0)) }
    fn dispense_brush(&mut self, pos: Pos, facing: Direction, stack: ItemStack) -> Dispensed { let _ = (pos, facing); (Some(stack), 0) }
}
''',
        'crates/horizon-client/src/game.rs': '''
mod dispenser;
mod machines;
use horizon_world::registry::{BlockRegistry, BlockStateId};
use horizon_world::level::Dimension as LevelDimension;
use horizon_core::position::BlockPos;
use horizon_client::chunk::Chunk;
pub mod native_lifecycle {
    pub enum WorldPhase { WorldOpenReady, WorldOpenFailed, WorldCloseFinal, WorldCloseFailed }
}
pub mod native_text {
    /// API 55's text line; the stand-in records what a mod set.
    pub struct TextLine { pub pose: glam::Mat4, pub text: String, pub color: u32, pub outline: u32, pub full_bright: bool }
}
pub mod native_entities {
    /// API 60's entity as a native mod reads it.
    #[derive(Clone, Debug, PartialEq)]
    pub struct NativeEntity { pub id: u32, pub kind: String, pub pos: [f64; 3], pub velocity: [f64; 3], pub yaw: f32,
                              pub on_ground: bool, pub sneaking: bool, pub flying: bool, pub vehicle: Option<u32>, pub min: [f64; 3], pub max: [f64; 3] }
}
pub mod native_minecarts {
    /// API 62's minecart as a native mod reads it.
    #[derive(Clone, Debug, PartialEq)]
    pub struct NativeMinecart { pub id: u32, pub uuid: u128, pub kind: &'static str, pub pos: [f64; 3], pub velocity: [f64; 3],
                                pub yaw: f32, pub riders: Vec<u32>, pub ridden: bool, pub fuel: i32,
                                pub push: [f64; 3], pub carrying: bool }
}
pub mod native_server {
    pub struct NativeCommand { pub usage: String, pub level: u8 }
    impl NativeCommand {
        pub fn op(usage: &str, level: u8) -> Self { Self { usage: usage.to_owned(), level } }
    }
    pub struct NativeServerPlayer { pub uuid: u128, pub name: String, pub dimension: String, pub position: [f64; 3], pub game_mode: i32 }
}
pub mod native_hooks {
    use super::*;
    pub mod flags { pub const UPDATE_CLIENTS: i32 = 2; pub const UPDATE_ALL: i32 = 3; }
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum NativeRecipeKind { CraftingShaped, CraftingShapeless }
    pub struct NativeRecipeStack { pub item: u32, pub count: u32 }
    pub struct NativeRecipe { pub id: String, pub kind: NativeRecipeKind, pub slots: Vec<Vec<NativeRecipeStack>>, pub results: Vec<NativeRecipeStack> }
    pub struct NativeItem { pub id: String }
    pub struct NativeRecipes { pub items: Vec<NativeItem>, pub recipes: Vec<NativeRecipe> }
    pub struct NativeScreen { pub width: f32, pub height: f32, pub slots: Vec<Option<[f32; 2]>>,
                              pub inventory_y: f32, pub inventory_x: f32, pub own_labels: bool }
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
        pub fn native_analog_output_signal(&self, dimension: LevelDimension, pos: BlockPos, state: BlockStateId,
                                           direction: horizon_core::geometry::Direction, signal: &mut i32) {
            let _ = (dimension, pos, state, direction, signal);
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
        pub fn native_screen(&self, block: &str, screen: &mut Option<NativeScreen>) { let _ = (block, screen); }
        pub fn native_menu_closed(&mut self, uuid: u128, window: i32) { let _ = (uuid, window); }
        /// The fixture's menus: window 1 for any player, its title said.
        pub fn native_open_menu(&mut self, uuid: u128, rows: u8, title: horizon_client::text::Text,
                                items: Vec<Option<horizon_client::items::ItemStack>>) -> Option<i32> {
            let _ = (uuid, rows, items);
            self.said.push(format!("menu {}", title.0));
            Some(1)
        }
        pub fn native_block_entity(&mut self, block: &str, entity: &mut Option<(String, horizon_world::nbt::Compound)>) {
            let _ = (block, entity);
        }
        pub fn native_block_entity_container(&self, block: &str, container: &mut Option<horizon_client::containers::ModContainer>) {
            let _ = (block, container);
        }
        pub fn items(&self) -> horizon_client::items::Items { horizon_client::items::Items { item_tags: Default::default() } }
        pub fn native_recipes(&mut self) -> Option<std::sync::Arc<NativeRecipes>> { None }
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
                _ => None,
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
            Some(NativeServerPlayer { uuid: 7, name: "Alex".into(), dimension: "minecraft:overworld".into(), position: super::PLAYER_AT.with(|p| p.get()), game_mode: 1 })
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
        pub fn native_text_set(&mut self, key: u64, pos: [i32; 3], lines: &[super::native_text::TextLine]) {
            let _ = key;
            let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
            super::TEXTS.with_borrow_mut(|t| t.push(format!("set {pos:?} {}", texts.join("|"))));
        }
        pub fn native_text_clear(&mut self, key: u64) {
            let _ = key;
            super::TEXTS.with_borrow_mut(|t| t.push("clear".to_owned()));
        }
        pub fn native_parts_seconds(&self) -> f32 { 1.0 }
        /// The fixture records the model overrides set (API 59).
        pub fn native_block_model_override(&mut self, pos: [i32; 3], model: Option<horizon_client::mesher::ModelOverride>) {
            super::OVERRIDES.with_borrow_mut(|o| match model {
                Some(m) => { o.insert(pos, m); }
                None => { o.remove(&pos); }
            });
        }
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
        pub fn native_mod_screen_draw(&mut self, name: &str, batch: &mut horizon_client::hud::HudBatch, cursor: [f32; 2]) {
            let _ = (name, batch, cursor);
        }
        pub fn native_mod_screen_text(&mut self, name: &str, text: &str) { let _ = (name, text); }
        pub fn native_mod_screen_mouse(&mut self, name: &str, at: [f32; 2], button: u8, pressed: bool) {
            let _ = (name, at, button, pressed);
        }
        pub fn native_mod_screen_scroll(&mut self, name: &str, at: [f32; 2], delta: f32) { let _ = (name, at, delta); }
        pub fn native_mod_screen_removed(&mut self, name: &str) { let _ = name; }
        pub fn native_entity_collisions(&self, around: horizon_core::geometry::Aabb,
            out: &mut Vec<(horizon_core::geometry::Aabb, horizon_core::geometry::Vec3)>) {
            let _ = (around, out);
        }
        pub fn native_item_handler_insert(&mut self, pos: [i32; 3], side: horizon_core::geometry::Direction,
            stack: &mut Option<horizon_client::items::ItemStack>, simulate: bool, handled: &mut bool) {
            let _ = (pos, side, stack, simulate, handled);
        }
        pub fn native_item_handler_extract(&mut self, pos: [i32; 3], side: horizon_core::geometry::Direction,
            amount: u32, simulate: bool, taken: &mut Option<horizon_client::items::ItemStack>, handled: &mut bool) {
            let _ = (pos, side, amount, simulate, taken, handled);
        }
        pub fn native_use_item_on(&mut self, stack: &horizon_client::items::ItemStack, pos: [i32; 3], face: [i32; 3], claimed: &mut bool) {
            let _ = (stack, pos, face, claimed);
        }
        pub fn aim(&self) -> Option<([f64; 3], [f64; 3])> { None }
        pub fn may_build(&self) -> bool { true }
        pub fn native_mapping(&mut self, name: &str) -> Option<(bool, String)> { let _ = name; None }
        pub fn native_use_item(&mut self, stack: &horizon_client::items::ItemStack, claimed: &mut bool) {
            let _ = (stack, claimed);
        }
        pub fn native_hud(&mut self, batch: &mut horizon_client::hud::HudBatch) { let _ = batch; }
        pub fn native_player_tick(&mut self) {}
        pub fn native_destroy_block(&mut self, pos: [i32; 3], state: BlockStateId, canceled: &mut bool) {
            let _ = (pos, state, canceled);
        }
        pub fn native_left_click_block(&mut self, pos: [i32; 3], face: [i32; 3], state: BlockStateId, canceled: &mut bool) {
            let _ = (pos, face, state, canceled);
        }
        pub fn native_interact_entity(&mut self, entity: i32, kind: &str,
                                      stack: Option<&horizon_client::items::ItemStack>, claimed: &mut bool) {
            let _ = (entity, kind, stack, claimed);
        }
        /// The fixture records the mobs a native mod removed.
        pub fn native_remove_mob(&mut self, dimension: &str, id: u32) -> bool {
            let _ = dimension;
            self.said.push(format!("removed mob {id}"));
            true
        }
        pub fn native_hurt_enemy(&mut self, stack: &horizon_client::items::ItemStack, entity: u32, kind: &str) {
            let _ = (stack, entity, kind);
        }
        pub fn native_attack_entity(&mut self, entity: i32, kind: &str, pos: [f64; 3], canceled: &mut bool) {
            let _ = (entity, kind, pos, canceled);
        }
        /// Entity 9 is a living mob (a cow), as is each of `ENTITIES` but players, items and shulkers' kin
        /// that aren't mobs here.
        pub fn native_mob(&self, dimension: &str, id: u32) -> Option<()> {
            let _ = dimension;
            let mob = ENTITIES.with_borrow(|list| { list.iter().any(|e| e.id == id && e.kind != "minecraft:player" && e.kind != "minecraft:item") });
            (id == 9 || mob).then_some(())
        }
        pub fn native_entity_inside(&mut self, dimension: &str, pos: BlockPos, state: BlockStateId,
                                    entity: &super::native_entities::NativeEntity) {
            let _ = (dimension, pos, state, entity);
        }
        pub fn native_minecart_pass(&mut self, dimension: &str, pos: BlockPos, state: BlockStateId,
                                    cart: &mut super::native_minecarts::NativeMinecart) {
            let _ = (dimension, pos, state, cart);
        }
        pub fn native_entity_mount(&mut self, dimension: &str, rider: u32, vehicle: u32, mounting: bool, canceled: &mut bool) {
            let _ = (dimension, rider, vehicle, mounting, canceled);
        }
        pub fn native_seat_loaded(&mut self, dimension: &str, seat: u32, pos: [f64; 3]) {
            let _ = (dimension, seat, pos);
        }
        /// The fixture's entities (`ENTITIES`), their boxes 0.6 wide and 1.8 high at their feet.
        pub fn native_entity(&self, dimension: &str, id: u32) -> Option<super::native_entities::NativeEntity> {
            let _ = dimension;
            ENTITIES.with_borrow(|list| list.iter().find(|e| e.id == id).cloned()).map(boxed)
        }
        pub fn native_entities_in(&self, dimension: &str, min: [f64; 3], max: [f64; 3]) -> Vec<super::native_entities::NativeEntity> {
            let _ = dimension;
            ENTITIES.with_borrow(|list| { list.iter().cloned().map(boxed)
                .filter(|e| (0..3).all(|a| e.min[a] < max[a] && e.max[a] > min[a])).collect() })
        }
        pub fn native_set_delta_movement(&mut self, dimension: &str, id: u32, velocity: [f64; 3]) -> bool {
            let _ = dimension;
            let moved = CARTS.with_borrow_mut(|list| {
                let found = list.iter_mut().find(|x| x.id == id);
                found.map(|x| x.velocity = velocity).is_some()
            });
            if moved {
                return true;
            }
            ENTITIES.with_borrow_mut(|list| { list.iter_mut().find(|e| e.id == id).map(|e| e.velocity = velocity).is_some() })
        }
        /// Recorded in `FALLS_RESET`.
        pub fn native_reset_fall_distance(&mut self, dimension: &str, id: u32) -> bool {
            let _ = dimension;
            FALLS_RESET.with_borrow_mut(|f| f.push(id));
            true
        }
        pub fn native_set_yaw(&mut self, dimension: &str, id: u32, yaw: f32) -> bool {
            let _ = dimension;
            ENTITIES.with_borrow_mut(|list| { list.iter_mut().find(|e| e.id == id).map(|e| e.yaw = yaw).is_some() })
        }
        /// The fixture's minecarts (`CARTS`); spawned ones take the next id from 6000 and a UUID
        /// of their id + 0xC000.
        pub fn native_minecart(&mut self, dimension: &str, id: u32) -> Option<super::native_minecarts::NativeMinecart> {
            let _ = dimension;
            CARTS.with_borrow(|c| c.iter().find(|c| c.id == id).cloned())
        }
        pub fn native_minecarts(&mut self, dimension: &str) -> Vec<super::native_minecarts::NativeMinecart> {
            let _ = dimension;
            CARTS.with_borrow(|c| c.clone())
        }
        pub fn native_spawn_minecart(&mut self, dimension: &str, kind: &str, pos: [f64; 3], yaw: f32,
                                     velocity: [f64; 3]) -> Option<u32> {
            let _ = dimension;
            let kind: &'static str = match kind {
                "minecraft:minecart" => "minecraft:minecart",
                "minecraft:chest_minecart" => "minecraft:chest_minecart",
                "minecraft:furnace_minecart" => "minecraft:furnace_minecart",
                _ => return None,
            };
            Some(CARTS.with_borrow_mut(|c| {
                let id = c.iter().map(|x| x.id).max().unwrap_or(5999).max(5999) + 1;
                c.push(super::native_minecarts::NativeMinecart { id, uuid: 0xC000 + u128::from(id), kind, pos, velocity,
                    yaw, riders: vec![], ridden: false, fuel: 0, push: [0.0; 3], carrying: false });
                id
            }))
        }
        pub fn native_remove_minecart(&mut self, dimension: &str, id: u32) -> bool {
            let _ = dimension;
            CARTS.with_borrow_mut(|c| { let n = c.len(); c.retain(|x| x.id != id); c.len() != n })
        }
        /// API 68 on the fixture's minecarts (`CARTS`).
        pub fn native_set_minecart_position(&mut self, dimension: &str, id: u32, pos: [f64; 3]) -> bool {
            let _ = dimension;
            CARTS.with_borrow_mut(|c| { let found = c.iter_mut().find(|x| x.id == id); found.map(|x| x.pos = pos).is_some() })
        }
        /// No blocks stop a cart here: it moves the whole way.
        pub fn native_move_minecart(&mut self, dimension: &str, id: u32, delta: [f64; 3]) -> Option<[f64; 3]> {
            let _ = dimension;
            CARTS.with_borrow_mut(|c| { let found = c.iter_mut().find(|x| x.id == id);
                found.map(|x| { for k in 0..3 { x.pos[k] += delta[k]; } x.pos }) })
        }
        pub fn native_set_furnace_fuel(&mut self, dimension: &str, id: u32, fuel: i32, push: [f64; 3]) -> bool {
            let _ = dimension;
            CARTS.with_borrow_mut(|c| { let found = c.iter_mut().find(|x| x.id == id && x.kind == "minecraft:furnace_minecart");
                found.map(|x| { x.fuel = fuel; x.push = push; }).is_some() })
        }
        pub fn native_set_minecart_carrying(&mut self, dimension: &str, id: u32, carrying: bool) -> bool {
            let _ = dimension;
            CARTS.with_borrow_mut(|c| { let found = c.iter_mut().find(|x| x.id == id); found.map(|x| x.carrying = carrying).is_some() })
        }
        /// Seats (`SEAT_ENTITIES`) from id 5000; a player rides 0.6 below one, any other its own
        /// attachment (0 here) below; getting off puts a rider where the seat is.
        pub fn native_spawn_seat(&mut self, dimension: &str, pos: [f64; 3], yaw: f32) -> Option<u32> {
            let _ = (dimension, yaw);
            Some(SEAT_ENTITIES.with_borrow_mut(|s| {
                let id = 5000 + s.len() as u32;
                s.push((id, pos, true));
                id
            }))
        }
        pub fn native_move_seat(&mut self, dimension: &str, id: u32, pos: [f64; 3], yaw: f32) -> bool {
            let _ = (dimension, yaw);
            SEAT_ENTITIES.with_borrow_mut(|s| s.iter_mut().find(|s| s.0 == id && s.2).map(|s| s.1 = pos).is_some())
        }
        pub fn native_remove_seat(&mut self, dimension: &str, id: u32) -> bool {
            if !SEAT_ENTITIES.with_borrow(|s| s.iter().any(|s| s.0 == id && s.2)) {
                return false;
            }
            for rider in self.native_passengers(dimension, id) {
                self.native_stop_riding(dimension, rider);
            }
            SEAT_ENTITIES.with_borrow_mut(|s| s.iter_mut().filter(|s| s.0 == id).for_each(|s| s.2 = false));
            true
        }
        pub fn native_start_riding(&mut self, dimension: &str, rider: u32, vehicle: u32) -> bool {
            let _ = dimension;
            let Some(at) = SEAT_ENTITIES.with_borrow(|s| s.iter().find(|s| s.0 == vehicle && s.2).map(|s| s.1)) else {
                return false;
            };
            ENTITIES.with_borrow_mut(|e| {
                let Some(e) = e.iter_mut().find(|e| e.id == rider && e.vehicle.is_none()) else { return false };
                let below = if e.kind == "minecraft:player" { 0.6 } else { 0.0 };
                e.vehicle = Some(vehicle);
                e.pos = [at[0], at[1] - below, at[2]];
                e.velocity = [0.0; 3];
                true
            })
        }
        pub fn native_stop_riding(&mut self, dimension: &str, rider: u32) -> bool {
            let _ = dimension;
            ENTITIES.with_borrow_mut(|e| {
                let Some(e) = e.iter_mut().find(|e| e.id == rider) else { return false };
                let Some(v) = e.vehicle.take() else { return false };
                if let Some(at) = SEAT_ENTITIES.with_borrow(|s| s.iter().find(|s| s.0 == v).map(|s| s.1)) {
                    e.pos = at;
                }
                true
            })
        }
        /// Players first, then the others.
        pub fn native_passengers(&self, dimension: &str, vehicle: u32) -> Vec<u32> {
            let _ = dimension;
            ENTITIES.with_borrow(|e| {
                let on: Vec<_> = e.iter().filter(|e| e.vehicle == Some(vehicle)).collect();
                on.iter().filter(|e| e.kind == "minecraft:player").chain(on.iter().filter(|e| e.kind != "minecraft:player"))
                    .map(|e| e.id).collect()
            })
        }
        pub fn native_text_width(&self, text: &str) -> f32 { text.len() as f32 * 6.0 }
        pub fn native_sprite_size(&self, name: &str) -> Option<[f32; 2]> { let _ = name; None }
        pub fn native_sprite(&self, batch: &mut horizon_client::hud::HudBatch, name: &str, src: [f32; 4], dst: [f32; 4]) -> bool {
            let _ = (src, dst);
            batch.drawn.push(name.to_owned());
            true
        }
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
        pub fn native_input_key(&mut self, key: &str, action: horizon_client::debug_keys::KeyAction, modifiers: u8,
                                screen: bool, consumed: &mut bool) {
            let _ = (key, action, modifiers, screen, consumed);
        }
        pub fn native_interaction_key_mapping_triggered(&mut self, button: horizon_client::interaction_input::Button,
                                                        hand: horizon_client::first_person::Hand,
                                                        canceled: &mut bool, swing: &mut bool) {
            let _ = (button, hand, canceled, swing);
        }
        /// The keys the test holds down.
        pub fn native_is_key_down(&self, key: &str) -> bool { super::KEYS_DOWN.with_borrow(|k| k.contains(key)) }
        pub fn native_level_load(&mut self, dimension: &str) { let _ = dimension; }
        pub fn native_level_save(&mut self, dimension: &str) { let _ = dimension; }
        /// The fixture's world data, by `mod_id/name`; what was marked dirty is recorded.
        pub fn native_saved_data(&mut self, mod_id: &str, name: &str) -> Option<&mut horizon_world::nbt::Compound> {
            Some(self.saved.entry(format!("{mod_id}/{name}")).or_default())
        }
        pub fn native_set_saved_data_dirty(&mut self, mod_id: &str, name: &str) -> bool {
            self.saved_dirty.push(format!("{mod_id}/{name}"));
            true
        }
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
        pub fn pick(&self) -> Option<Hit> {
            self.aim.map(|pos| Hit { pos, point: self.aim_point.unwrap_or(pos.map(|v| f64::from(v) + 0.5)), face: [0, 1, 0] })
        }
        pub fn with_level<R>(&mut self, f: impl FnOnce(&Behaviours, &mut GameLevel<'_>, &mut Ticks) -> R) -> R {
            let mut level = GameLevel { registry: self.pack.assets.registry, world: &self.world, items: fixture_items() };
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
        /// The fixture records which blocks' comparators were told to read again.
        pub fn native_update_neighbour_for_output_signal(&mut self, dimension: &str, pos: [i32; 3]) -> bool {
            let _ = dimension;
            self.said.push(format!("comparators {pos:?}"));
            true
        }
        pub fn native_update_signal_neighbors(&mut self, dimension: &str, pos: [i32; 3]) -> bool {
            let _ = (dimension, pos);
            true
        }
        pub fn native_feedback(&mut self, line: horizon_client::text::Text) { self.said.push(line.0); }
        pub fn native_action_bar(&mut self, uuid: u128, line: horizon_client::text::Text) -> bool { let _ = uuid; self.said.push(line.0); true }
        pub fn native_run_command(&mut self, line: &str) -> bool { self.said.push(line.to_owned()); true }
    }
}
pub struct Assets { pub registry: &'static BlockRegistry }
pub struct Pack { pub assets: Assets }
pub struct World {
    pub blocks: std::collections::HashMap<[i32; 3], u32>, pub game_time: i64, pub day_time: i64,
    pub block_entities: std::collections::HashMap<(i32, i32, i32), horizon_client::containers::BlockEntity>,
    pub shape: LevelShape,
}
#[derive(Clone, Copy)] pub struct LevelShape { pub min_y: i32, pub height: i32 }
impl World {
    pub fn block(&self, x: i32, y: i32, z: i32) -> BlockStateId { BlockStateId(*self.blocks.get(&[x, y, z]).unwrap_or(&0)) }
    pub fn loaded(&self, x: i32, z: i32) -> bool { let _ = (x, z); true }
}
pub struct Hit { pub pos: [i32; 3], pub point: [f64; 3], pub face: [i32; 3] }
/// An entity of the fixture with its box: 0.6 wide, 1.8 high, at its feet.
pub fn boxed(mut e: native_entities::NativeEntity) -> native_entities::NativeEntity {
    e.min = [e.pos[0] - 0.3, e.pos[1], e.pos[2] - 0.3];
    e.max = [e.pos[0] + 0.3, e.pos[1] + 1.8, e.pos[2] + 0.3];
    e
}
/// A fixture entity of `kind` standing at `pos`.
pub fn fixture_entity(id: u32, kind: &str, pos: [f64; 3]) -> native_entities::NativeEntity {
    boxed(native_entities::NativeEntity { id, kind: kind.into(), pos, velocity: [0.0; 3], yaw: 0.0, on_ground: true, sneaking: false, flying: false,
                                          vehicle: None, min: [0.0; 3], max: [0.0; 3] })
}
pub struct Player { pub exhaustion: f32 }
impl Player { pub fn cause_food_exhaustion(&mut self, amount: f32) { self.exhaustion += amount; } }
pub struct Game<'a> {
    pub pack: Pack, pub calls: &'a mut u32, pub said: Vec<String>, pub world: World,
    pub sent: Vec<(u128, String, Vec<u8>)>, pub parts: Vec<(u32, [i32; 3], u8, f32)>,
    pub aim: Option<[i32; 3]>, pub aim_point: Option<[f64; 3]>, pub unsaved: Vec<[i32; 2]>,
    pub offhand: Option<horizon_client::items::ItemStack>,
    pub dropped: Vec<horizon_client::entities::ItemEntity>, pub right: bool, pub audio: Audio,
    pub creative: bool, pub player: Option<Player>, pub keys: std::collections::HashSet<winit::keyboard::KeyCode>,
    pub glides: Vec<(u32, [i32; 3], u8, f32, [f32; 3])>, pub hand: horizon_client::first_person::Hand, pub yaw: f32, pub pitch: f32,
    pub armor: [Option<horizon_client::items::ItemStack>; 4],
    pub inventory: Vec<Option<horizon_client::items::ItemStack>>, pub selected: usize, pub dim: Dim,
    pub res: Res, pub custom_recipes: std::collections::HashMap<String, Vec<horizon_client::crafting::CustomRecipe>>,
    pub cooking: horizon_client::crafting::Recipes,
    pub saved: std::collections::HashMap<String, horizon_world::nbt::Compound>, pub saved_dirty: Vec<String>,
}
impl Game<'_> {
    pub fn recipes(&self) -> &horizon_client::crafting::Recipes { &self.cooking }
}
pub struct Audio;
impl Audio { pub fn play(&mut self, event: &str, at: Option<[f64; 3]>, volume: f32, pitch: f32) { let _ = (event, at, volume, pitch); } }
pub struct Dim;
impl Dim { pub fn id(&self) -> &str { "minecraft:overworld" } }
pub struct GameLevel<'a> { pub registry: &'a BlockRegistry, pub world: &'a World, pub items: &'a horizon_client::items::Items }
/// The item table GameLevel lends comparators (every item stacks to 64).
pub fn fixture_items() -> &'static horizon_client::items::Items {
    static ITEMS: std::sync::OnceLock<horizon_client::items::Items> = std::sync::OnceLock::new();
    ITEMS.get_or_init(|| horizon_client::items::Items { item_tags: Default::default() })
}
pub struct Behaviours;
pub struct Facts { pub flags: u32, pub hardness: f32 }
#[derive(Clone, Copy)] pub struct BlockId(pub u16);
pub struct BlockSet;
impl BlockSet { pub fn contains(&self, block: BlockId) -> bool { let _ = block; false } }
impl Behaviours {
    /// Air (state 0) and water (state 81) have no collision; every other state of the fixture has.
    pub fn facts(&self, state: BlockStateId) -> Facts {
        let empty = state.0 == 0 || state.0 == 81;
        Facts { flags: if empty { horizon_world::level::data::state_flags::COLLISION_EMPTY } else { 0 }, hardness: 1.5 }
    }
    /// No block tags; no cached shapes (the collision flag decides).
    pub fn tag(&self, name: &str) -> Option<&BlockSet> { let _ = name; None }
    pub fn is_replaceable(&self, state: BlockStateId) -> bool { state.0 == 0 }
    pub fn is_air(&self, state: BlockStateId) -> bool { state.0 == 0 }
    pub fn block(&self, state: BlockStateId) -> BlockId { BlockId(state.0 as u16) }
    pub fn collision_shape(&self, state: BlockStateId) -> Option<&horizon_core::geometry::VoxelShape> { let _ = state; None }
    /// State 76 is a water source.
    pub fn fluid(&self, state: BlockStateId) -> horizon_world::fluid::FluidState {
        use horizon_world::fluid::{FluidKind, FluidState};
        FluidState(if state.0 == 81 { FluidKind::Water } else { FluidKind::Empty })
    }
    /// `Block.updateFromNeighbourShapes`: the fixture's states keep their shape.
    pub fn update_from_neighbour_shapes(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos,
                                        state: BlockStateId) -> BlockStateId {
        let _ = (level, ticks, pos);
        state
    }
}
pub struct Res;
pub struct BlockFacts { pub sturdy_full: u8, pub outline: std::sync::Arc<[[f64; 6]]> }
pub struct AllFacts;
impl AllFacts {
    /// Stone (state 1) is sturdy on every face; nothing else is.
    pub fn get(&self, state: BlockStateId) -> BlockFacts { BlockFacts {
        sturdy_full: if state.0 == 1 { 63 } else { 0 },
        outline: if state.0 == 0 { std::sync::Arc::from([]) } else { std::sync::Arc::from([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]) },
    } }
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
    /// The world text set and cleared (API 55), as `set [x, y, z] a|b` and `clear`.
    pub static TEXTS: std::cell::RefCell<Vec<String>> = Default::default();
    /// The level's entities (API 60), the entities whose fall was reset, and the seats spawned
    /// (id, position, still there).
    pub static ENTITIES: std::cell::RefCell<Vec<native_entities::NativeEntity>> = Default::default();
    pub static FALLS_RESET: std::cell::RefCell<Vec<u32>> = Default::default();
    pub static SEAT_ENTITIES: std::cell::RefCell<Vec<(u32, [f64; 3], bool)>> = Default::default();
    /// The level's minecarts (API 62, 65).
    pub static CARTS: std::cell::RefCell<Vec<native_minecarts::NativeMinecart>> = Default::default();
    /// Blocks giving a signal of 15 toward every side, for the latch test.
    pub static SIGNALS: std::cell::RefCell<std::collections::HashSet<[i32; 3]>> = Default::default();
    /// Where the acting player stands, and the keys held down (`native_is_key_down`).
    pub static PLAYER_AT: std::cell::Cell<[f64; 3]> = const { std::cell::Cell::new([0.5, 2.0, 0.5]) };
    pub static KEYS_DOWN: std::cell::RefCell<std::collections::HashSet<String>> = Default::default();
    /// The model overrides a client set, by position.
    pub static OVERRIDES: std::cell::RefCell<std::collections::HashMap<[i32; 3], horizon_client::mesher::ModelOverride>> = Default::default();
}
impl Behaviours {
    pub fn neighbor_changed_at(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos, block: BlockId) {
        let _ = (level, ticks, pos, block);
    }
    pub fn update_neighbors_at_except(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos, block: BlockId, skip: horizon_core::geometry::Direction) {
        let _ = (level, ticks, pos, block, skip);
    }
    pub fn has_neighbor_signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos) -> bool {
        let _ = (level, ticks, pos);
        true
    }
    pub fn best_neighbor_signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos) -> i32 {
        let _ = (level, ticks, pos);
        15
    }
    pub fn signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos,
                  direction: horizon_core::geometry::Direction) -> i32 {
        let _ = (level, ticks, direction);
        if SIGNALS.with_borrow(|s| s.contains(&[pos.x, pos.y, pos.z])) { 15 } else { 0 }
    }
}
/// Seats, turntables, a fan pushing a player and a controller rail, at x and z 1400 and up.
#[cfg(test)]
fn seats_turntables_fans_and_rails(game: &mut Game<'_>, level: &GameLevel<'_>) {
    use horizon_client::items::ItemStack;
    let dim = "minecraft:overworld";
    let place = |game: &mut Game<'_>, pos: [i32; 3], state: u32| {
        let old = game.world.blocks.insert(pos, state).unwrap_or(0);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(old), BlockStateId(state));
    };
    let get = |id: u32| ENTITIES.with_borrow(|list| list.iter().find(|e| e.id == id).cloned().unwrap());
    let set = |id: u32, change: &dyn Fn(&mut native_entities::NativeEntity)| {
        ENTITIES.with_borrow_mut(|list| list.iter_mut().filter(|e| e.id == id).for_each(|e| change(e)))
    };
    let seat_of = |id: u32| SEAT_ENTITIES.with_borrow(|s| s.iter().find(|s| s.0 == id).copied().unwrap());
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let click = |game: &mut Game<'_>, pos: [i32; 3]| {
        let mut canceled = false;
        game.native_right_click_block(pos, [0, 1, 0], &mut canceled);
        canceled
    };
    game.keys.clear();
    game.selected = 0;
    game.hand = horizon_client::first_person::Hand::Main;
    game.inventory[0] = None;
    // Seats are Create's 16 dyed blocks.
    assert!(create_engine::dye::SEATS.iter().all(|b| b.name().ends_with("_seat")));
    // A white seat at (1400, 0, 1400) clicked by the player beside it: at the level's tick they
    // sit on a seat at its middle, 1/16 above the top of Create's 0.35 high seat entity, their
    // feet its 0.6 below; a second click (a player sits there) passes.
    place(game, [1400, 0, 1400], 66);
    ENTITIES.with_borrow_mut(|e| e.push(fixture_entity(7, "minecraft:player", [1400.5, 0.0, 1401.5])));
    PLAYER_AT.set([1400.5, 0.0, 1401.5]);
    assert!(click(game, [1400, 0, 1400]), "the click is the seat's");
    game.native_level_tick_post(dim);
    let seat = get(7).vehicle.expect("the player sat down");
    let (_, at, there) = seat_of(seat);
    assert!(there && at[0] == 1400.5 && at[2] == 1400.5, "{at:?}");
    assert!(close(at[1], 0.0625 + f64::from(0.35f32)), "{at:?}");
    assert!(close(get(7).pos[1], at[1] - 0.6));
    assert!(!click(game, [1400, 0, 1400]), "a player sits there: passed");
    // Sneaking off: the seat goes to Create's dismount point (its box's top, half a block up) so
    // the player gets off there; riderless, it goes at the next tick.
    let mut canceled = false;
    game.native_entity_mount(dim, 7, seat, false, &mut canceled);
    assert!(!canceled);
    game.native_stop_riding(dim, 7);
    assert!(close(get(7).pos[1], 0.675), "{:?}", get(7).pos);
    game.native_level_tick_post(dim);
    assert!(!seat_of(seat).2, "riderless: gone");
    // A pig landing on a seat at (1410, 0, 1410) sits, a shulker on another doesn't; sneaking, the
    // player's click passes.
    place(game, [1410, 0, 1410], 66);
    place(game, [1412, 0, 1410], 66);
    ENTITIES.with_borrow_mut(|e| {
        e.push(fixture_entity(50, "minecraft:pig", [1410.5, 0.5, 1410.5]));
        e.push(fixture_entity(51, "minecraft:shulker", [1412.5, 0.5, 1410.5]));
    });
    game.native_entity_inside(dim, BlockPos { x: 1410, y: 0, z: 1410 }, BlockStateId(66), &get(50));
    game.native_entity_inside(dim, BlockPos { x: 1412, y: 0, z: 1410 }, BlockStateId(66), &get(51));
    let seat = get(50).vehicle.expect("the pig sat down");
    assert!(close(seat_of(seat).1[1], 0.0625 + f64::from(0.35f32)));
    assert_eq!(get(51).vehicle, None, "shulkers don't sit");
    game.keys.insert(winit::keyboard::KeyCode::ShiftLeft);
    assert!(!click(game, [1412, 0, 1410]));
    game.keys.clear();
    // A red dye recolours the pig's seat; it keeps its pig.
    game.inventory[0] = Some(ItemStack::new("minecraft:red_dye", 1));
    assert!(click(game, [1410, 0, 1410]));
    assert_eq!(game.world.blocks.get(&[1410, 0, 1410]), Some(&67));
    game.inventory[0] = None;
    game.native_level_tick_post(dim);
    assert_eq!(get(50).vehicle, Some(seat));
    // The player's click takes the seat from the pig.
    set(7, &|e| e.pos = [1410.5, 0.0, 1411.5]);
    PLAYER_AT.set([1410.5, 0.0, 1411.5]);
    assert!(click(game, [1410, 0, 1410]));
    game.native_level_tick_post(dim);
    assert_eq!(get(7).vehicle, Some(seat));
    assert_eq!(get(50).vehicle, None);
    // Broken, the seat lets the player off at Create's dismount point and goes.
    place(game, [1410, 0, 1410], 0);
    game.native_level_tick_post(dim);
    assert_eq!(get(7).vehicle, None);
    assert!(close(get(7).pos[1], 0.675) && !seat_of(seat).2);
    ENTITIES.with_borrow_mut(Vec::clear);
    // Saved and loaded again (API 67): the world's seats are forgotten and the stand-ins come
    // back under new ids, their riders on them. A pig's in the seat block at (1414, 0, 1410) is
    // that seat's again (broken, it lets the pig off and goes); a ridden one in no seat block
    // waits for its carriage; a riderless one in none goes.
    place(game, [1414, 0, 1410], 66);
    create_seats::forget();
    let spawn = |game: &mut Game<'_>, at: [f64; 3]| game.native_spawn_seat(dim, at, 0.0).unwrap();
    let back = spawn(game, create_engine::seat::seat_at([1414, 0, 1410], "minecraft:pig"));
    let lone = spawn(game, [1416.5, 3.0, 1410.5]);
    let empty = spawn(game, [1418.5, 3.0, 1410.5]);
    ENTITIES.with_borrow_mut(|e| {
        e.push(fixture_entity(53, "minecraft:pig", [1414.5, 0.5, 1410.5]));
        e.push(fixture_entity(54, "minecraft:pig", [1416.5, 3.0, 1410.5]));
    });
    assert!(game.native_start_riding(dim, 53, back) && game.native_start_riding(dim, 54, lone));
    for id in [back, lone, empty] {
        game.native_seat_loaded(dim, id, seat_of(id).1);
    }
    game.native_level_tick_post(dim);
    game.native_level_tick_post(dim);
    assert!(seat_of(back).2 && seat_of(lone).2, "taken back, or waiting for its carriage");
    assert!(!seat_of(empty).2, "riderless and no carriage's: gone");
    assert_eq!(get(54).vehicle, Some(lone));
    place(game, [1414, 0, 1410], 0);
    game.native_level_tick_post(dim);
    assert_eq!(get(53).vehicle, None);
    assert!(!seat_of(back).2, "the seat block's again");
    create_train_controls::forget();
    ENTITIES.with_borrow_mut(Vec::clear);
    // A turntable at (1420, 0, 1420) on a creative motor facing up: a pig standing on it turns by
    // 3/10 of its speed a tick (not while jumping); a player at its middle by 2/3 of it, halved
    // over a tick's frames; a player on its edge is carried round it.
    place(game, [1420, -1, 1420], 69);
    place(game, [1420, 0, 1420], 68);
    game.native_level_tick_post(dim);
    game.native_level_tick_post(dim);
    let table = BlockPos { x: 1420, y: 0, z: 1420 };
    ENTITIES.with_borrow_mut(|e| {
        e.push(fixture_entity(52, "minecraft:pig", [1420.5, 0.5, 1420.5]));
        e.push(fixture_entity(7, "minecraft:player", [1420.5, 0.5, 1420.5]));
    });
    game.native_entity_inside(dim, table, BlockStateId(68), &get(52));
    let turned = get(52).yaw;
    assert!(close(f64::from(turned.abs()), f64::from(create_engine::turntable::mob_turn(16.0))), "{turned}");
    set(52, &|e| e.velocity = [0.0, 0.42, 0.0]);
    game.native_entity_inside(dim, table, BlockStateId(68), &get(52));
    assert_eq!(get(52).yaw, turned, "jumping: not turned");
    game.native_entity_inside(dim, table, BlockStateId(68), &get(7));
    let player = get(7).yaw;
    assert!(close(f64::from(player.abs()), 16.0 / 3.0) && player.signum() == turned.signum(), "{player}");
    assert_eq!(get(7).velocity, [0.0; 3], "its middle on the turntable: not carried");
    set(7, &|e| { e.pos = [1421.2, 0.5, 1420.5]; e.yaw = 0.0; });
    game.native_entity_inside(dim, table, BlockStateId(68), &get(7));
    let v = get(7).velocity;
    let expected = create_engine::turntable::carry([1420, 0, 1420], [1421.2, 0.5, 1420.5], -turned);
    assert!(v != [0.0; 3] && (0..3).all(|a| close(v[a], expected[a])), "{v:?} {expected:?}");
    assert_eq!(get(7).yaw, 0.0, "off its middle: not turned");
    ENTITIES.with_borrow_mut(Vec::clear);
    // An encased fan facing east at (1431, 0, 1430), its motor behind: a player in its current
    // is pushed along x only and forgets their fall.
    place(game, [1430, 0, 1430], 3);
    place(game, [1431, 0, 1430], 19);
    ENTITIES.with_borrow_mut(|e| e.push(fixture_entity(7, "minecraft:player", [1434.5, 0.0, 1430.5])));
    FALLS_RESET.with_borrow_mut(Vec::clear);
    for _ in 0..8 { game.native_level_tick_post(dim); }
    let v = get(7).velocity;
    assert!(v[0] != 0.0 && v[1] == 0.0 && v[2] == 0.0, "{v:?}");
    assert!(FALLS_RESET.with_borrow(|f| f.contains(&7)));
    ENTITIES.with_borrow_mut(Vec::clear);
    // A controller rail along x at power 15 pointing west, (1440, 0, 1440): a still cart leaves
    // west at the rail's top speed; one going east, against it, is pulled to the rail's centre.
    place(game, [1440, 0, 1440], 70);
    let mut cart = native_minecarts::NativeMinecart { id: 90, uuid: 1, kind: "minecraft:minecart",
        pos: [1440.5, 0.0625, 1440.5], velocity: [0.0; 3], yaw: 0.0, riders: vec![], ridden: false, fuel: 0, push: [0.0; 3], carrying: false };
    let rail = BlockPos { x: 1440, y: 0, z: 1440 };
    game.native_minecart_pass(dim, rail, BlockStateId(70), &mut cart);
    assert_eq!(cart.velocity, [-0.4, 0.0, 0.0]);
    cart.pos[0] = 1440.9;
    cart.velocity = [0.2, 0.0, 0.0];
    game.native_minecart_pass(dim, rail, BlockStateId(70), &mut cart);
    assert!(close(cart.velocity[0], -0.4 / 16.0) && cart.velocity[1] == 0.0, "{:?}", cart.velocity);
}
/// A train's controls on the client: a use click with no train drawn is the game's; once the
/// server gives the player a carriage's controls, W is kept from the game and goes to the server
/// as Create's `ControlsInputPacket` (kept alive every 5 ticks); let go, the key goes up there
/// and W walks again. On the server, words about a train that isn't there do nothing.
#[cfg(test)]
fn train_controls(game: &mut Game<'_>) {
    use create_engine::train_controls::{CHANNEL, Target, ToClient, ToServer};
    use horizon_client::debug_keys::KeyAction::{Press, Release};
    use horizon_client::interaction_input::Button;
    let (mut canceled, mut swing) = (false, true);
    game.native_interaction_key_mapping_triggered(Button::Use, horizon_client::first_person::Hand::Main,
                                                  &mut canceled, &mut swing);
    assert!(!canceled && swing, "no carriage: the game's click");
    let key = |game: &mut Game<'_>, action| {
        let mut consumed = false;
        game.native_input_key("key.keyboard.w", action, 0, false, &mut consumed);
        consumed
    };
    assert!(!key(game, Press), "not controlling: W walks");
    let at = Target { train: 4, car: 0, local: [0, 1, 0] };
    game.native_net_from_server(CHANNEL, &ToClient::Start(at).encode());
    assert!(key(game, Press), "controlling: W is the train's");
    KEYS_DOWN.with_borrow_mut(|k| k.insert("key.keyboard.w".to_owned()));
    let sent = |game: &Game<'_>, from: usize| -> Vec<ToServer> {
        game.sent[from..].iter().filter(|m| m.1 == CHANNEL).filter_map(|m| ToServer::decode(&m.2)).collect()
    };
    let from = game.sent.len();
    for _ in 0..6 {
        game.native_tick();
    }
    let input = |keys, press| ToServer::Input { target: at, keys, press, stop: false };
    assert_eq!(sent(game, from), [input(1, true), input(1, true)]);
    let from = game.sent.len();
    game.native_net_from_server(CHANNEL, &ToClient::Stop.encode());
    assert_eq!(sent(game, from), [input(1, false)]);
    KEYS_DOWN.with_borrow_mut(|k| k.remove("key.keyboard.w"));
    key(game, Release);
    assert!(!key(game, Press), "let go: W walks again");
    key(game, Release);
    // The server: keys and a click for train 4, which isn't there.
    let from = game.sent.len();
    for m in [input(1, true), ToServer::Click { target: at, wrench: false }] {
        game.native_net_from_client(7, CHANNEL, &m.encode());
    }
    game.native_level_tick_post("minecraft:overworld");
    assert!(sent(game, from).is_empty() && game.sent[from..].iter().all(|m| m.1 != CHANNEL));
}
/// Cart assemblers, contraptions on carts, their items, saves and couplings, at x 1500 and up.
#[cfg(test)]
fn minecart_contraptions(game: &mut Game<'_>, level: &GameLevel<'_>) {
    use horizon_client::items::ItemStack;
    let dim = "minecraft:overworld";
    let place = |game: &mut Game<'_>, pos: [i32; 3], state: u32| {
        let old = game.world.blocks.insert(pos, state).unwrap_or(0);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(old), BlockStateId(state));
    };
    let cart = |id: u32| CARTS.with_borrow(|c| c.iter().find(|c| c.id == id).cloned());
    let command = |game: &mut Game<'_>, line: &str| {
        let me = game.native_acting_player().unwrap();
        let mut ran = false;
        game.native_perform_command(&me, line, &mut ran);
        assert!(ran, "{line}");
        game.said.last().cloned().unwrap_or_default()
    };
    // Nothing on a cart: the level tick looks at nothing.
    assert_eq!(command(game, "create_debug carts"), "no carts");
    // The assembler's item on a rail along x at (1500, 0, 1500) replaces it, facing north (not
    // backwards), as a regular rail's.
    game.world.blocks.insert([1500, 0, 1500], 84);
    game.inventory[0] = Some(ItemStack::new("create:cart_assembler", 1));
    game.hand = horizon_client::first_person::Hand::Main;
    game.yaw = 180.0;
    let mut claimed = false;
    let stack = ItemStack::new("create:cart_assembler", 1);
    game.native_use_item_on(&stack, [1500, 0, 1500], [0, 1, 0], &mut claimed);
    assert!(claimed);
    assert_eq!(game.world.blocks.get(&[1500, 0, 1500]), Some(&76));
    assert!(game.inventory[0].is_none(), "used up");
    // Its item on stone says where it goes.
    game.world.blocks.insert([1502, 0, 1500], 1);
    let mut claimed = false;
    game.native_use_item_on(&stack, [1502, 0, 1500], [0, 1, 0], &mut claimed);
    assert!(claimed && game.said.last().is_some_and(|l| l == "Place your Cart Assembler on a rail block"));
    // A neighbour's change has it read its power.
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 1500, y: 0, z: 1500 }, BlockStateId(76));
    game.native_level_tick_post(dim);
    assert!(game.said.iter().any(|l| l == "set [1500, 0, 1500] powered=true"), "powered by its neighbour");
    // Powered (as the property would now say), a stone block and its slime above, and a still
    // rideable cart passing: at the assembler's tick both ride the cart, the assembler an anchor.
    place(game, [1500, 0, 1500], 77);
    place(game, [1500, 1, 1500], 1);
    let first = native_minecarts::NativeMinecart { id: 90, uuid: 0xAB, kind: "minecraft:minecart",
        pos: [1500.5, 0.0625, 1500.5], velocity: [0.0; 3], yaw: 0.0, riders: vec![], ridden: false, fuel: 0, push: [0.0; 3], carrying: false };
    CARTS.with_borrow_mut(|list| list.push(first));
    let mut c90 = cart(90).unwrap();
    game.native_minecart_pass(dim, BlockPos { x: 1500, y: 0, z: 1500 }, BlockStateId(77), &mut c90);
    let sent = game.sent.len();
    game.native_level_tick_post(dim);
    assert_eq!(game.world.blocks.get(&[1500, 1, 1500]), Some(&0), "the stone left the world");
    let line = command(game, "create_debug carts");
    assert!(line.starts_with("cart 000000000000000000000000000000ab id=90 at=1500.50,0.06,1500.50 yaw=90.0 blocks=2 mode=ROTATE"), "{line}");
    // Its clients learn the blocks and how it moves.
    let msg = game.sent[sent..].iter().find(|m| m.1 == create_engine::mounted::CHANNEL).expect("sent").2.clone();
    let pose = create_engine::mounted::Pose::decode(&msg).unwrap();
    assert_eq!(pose.blocks, Some(vec![([0, 1, 0], 1), ([0, 0, 0], 78)]));
    assert_eq!((pose.at, pose.angle), ([1500.5, 0.5625, 1500.5], 0.0));
    let glides = game.glides.len();
    game.native_net_from_server(create_engine::mounted::CHANNEL, &msg);
    assert_eq!(game.glides.len(), glides + 1, "drawn as one gliding part");
    assert_eq!(game.glides.last().map(|g| (g.1, g.2)), Some(([1500, 0, 1500], 1)));
    // Assembled facing west (the assembler not backwards). The cart runs east: the contraption
    // turns to face it, 12 degrees a tick, the half turn the
    // way it moved; told to clients again as it turns.
    CARTS.with_borrow_mut(|c| c[0].velocity = [0.2, 0.0, 0.0]);
    for _ in 0..3 {
        CARTS.with_borrow_mut(|c| c[0].pos[0] += 0.2);
        game.native_level_tick_post(dim);
    }
    let line = command(game, "create_debug carts");
    assert!(line.contains("yaw=54.0"), "{line}");
    // A player can't get on it.
    let mut canceled = false;
    game.native_entity_mount(dim, 7, 90, true, &mut canceled);
    assert!(canceled);
    // A wrench picks it up: a minecart contraption item with its blocks, the cart gone.
    game.inventory[0] = None;
    let wrench = ItemStack::new("create:wrench", 1);
    let mut claimed = false;
    game.native_interact_entity(90, "minecraft:minecart", Some(&wrench), &mut claimed);
    assert!(claimed && cart(90).is_none());
    let item = game.inventory[0].clone().expect("the contraption's item");
    assert_eq!(&*item.item, "create:minecart_contraption");
    assert_eq!(command(game, "create_debug carts"), "no carts");
    // Used on the rail at (1510, 0, 1500): a cart with it, facing the player's way.
    game.world.blocks.insert([1510, 0, 1500], 84);
    let mut claimed = false;
    game.native_use_item_on(&item, [1510, 0, 1500], [0, 1, 0], &mut claimed);
    let spawned = CARTS.with_borrow(|c| c.last().cloned()).expect("a cart");
    assert_eq!((spawned.kind, spawned.pos), ("minecraft:minecart", [1510.5, 0.0625, 1500.5]));
    let line = command(game, "create_debug carts");
    assert!(line.contains("yaw=180.0 blocks=2"), "{line}");
    // Saved with the world and back after a reload, found by its cart's UUID.
    game.native_level_save(dim);
    let saved = game.saved.get("create/minecart_contraptions").cloned().expect("saved");
    assert!(game.saved_dirty.iter().any(|d| d == "create/minecart_contraptions"));
    // (the world closed: what the mod kept of it forgotten)
    create_carts::forget();
    game.saved.insert("create/minecart_contraptions".into(), saved);
    game.native_level_load(dim);
    game.native_level_tick_post(dim);
    let line = command(game, "create_debug carts");
    assert!(line.contains(&format!("id={} at=1510.50", spawned.id)), "{line}");
    // The cart breaks: the contraption goes back where it was, turned as it faced.
    CARTS.with_borrow_mut(|c| c.retain(|x| x.id != spawned.id));
    game.native_level_tick_post(dim);
    assert_eq!(game.world.blocks.get(&[1510, 1, 1500]), Some(&1));
    assert_eq!(command(game, "create_debug carts"), "no carts");
    // A furnace cart with a push on a powered assembler at (1530, 0, 1500) under a storage block
    // holding two coal: the storage's items ride with it, the cart moves as a ridden one (API
    // 68), its push zeroed, and it burns a coal from the storage at once (no fuel left).
    use create_contraption_body::STORAGE;
    place(game, [1530, 0, 1500], 77);
    place(game, [1530, 1, 1500], 1);
    STORAGE.with_borrow_mut(|s| s.insert([1530, 1, 1500], vec![Some(ItemStack::new("minecraft:coal", 2))]));
    let furnace = native_minecarts::NativeMinecart { id: 93, uuid: 0x93,
        kind: "minecraft:furnace_minecart", pos: [1530.5, 0.0625, 1500.5], velocity: [0.0; 3], yaw: 0.0,
        riders: vec![], ridden: false, fuel: 0, push: [0.1, 0.0, 0.0], carrying: false };
    CARTS.with_borrow_mut(|c| c.push(furnace));
    let mut c93 = cart(93).unwrap();
    game.native_minecart_pass(dim, BlockPos { x: 1530, y: 0, z: 1500 }, BlockStateId(77), &mut c93);
    game.native_level_tick_post(dim);
    assert!(STORAGE.with_borrow(|s| !s.contains_key(&[1530, 1, 1500])), "its items left the world with it");
    let c93 = cart(93).unwrap();
    assert!(c93.carrying && c93.push == [0.0; 3] && c93.fuel == 3600, "{c93:?}");
    let line = command(game, "create_debug carts");
    assert!(line.contains("blocks=2 mode=ROTATE items=1"), "{line}");
    // With fuel, on its rail, it is sent on at full speed (a unit vector: the rail holds it).
    CARTS.with_borrow_mut(|c| { for x in c.iter_mut().filter(|x| x.id == 93) { x.velocity = [0.1, 0.0, 0.0]; } });
    game.native_level_tick_post(dim);
    assert_eq!(cart(93).unwrap().velocity, [1.0, 0.0, 0.0]);
    // Saved and back with its items.
    game.native_level_save(dim);
    let saved = game.saved.get("create/minecart_contraptions").cloned().expect("saved");
    create_carts::forget();
    game.saved.insert("create/minecart_contraptions".into(), saved);
    game.native_level_load(dim);
    game.native_level_tick_post(dim);
    let line = command(game, "create_debug carts");
    assert!(line.contains("id=93 at=1530.50") && line.contains("items=1"), "{line}");
    // Its cart breaks: the storage block back (a fresh one where it was) takes the coal left.
    STORAGE.with_borrow_mut(|s| s.insert([1530, 1, 1500], vec![None; 27]));
    CARTS.with_borrow_mut(|c| c.retain(|x| x.id != 93));
    game.native_level_tick_post(dim);
    let back = STORAGE.with_borrow(|s| s.get(&[1530, 1, 1500]).cloned()).unwrap();
    assert_eq!(back.iter().flatten().map(|s| (s.item.to_string(), s.count)).collect::<Vec<_>>(),
        vec![("minecraft:coal".to_owned(), 1)]);
    STORAGE.with_borrow_mut(|s| s.remove(&[1530, 1, 1500]));
    game.world.blocks.insert([1530, 0, 1500], 0);
    game.world.blocks.insert([1530, 1, 1500], 0);
    assert_eq!(command(game, "create_debug carts"), "no carts");
    // Two carts 3 apart on x: a coupling's two clicks link them (one used), and they keep their
    // length: the one pulled away runs on at 0.3 while the other is set to follow, half the gap
    // each (their next positions 3 apart).
    CARTS.with_borrow_mut(|c| {
        c.clear();
        for (id, x) in [(91u32, 1520.5), (92, 1523.5)] {
            c.push(native_minecarts::NativeMinecart { id, uuid: u128::from(id), kind: "minecraft:minecart",
                pos: [x, 0.0625, 1500.5], velocity: [0.0; 3], yaw: 0.0, riders: vec![], ridden: false, fuel: 0, push: [0.0; 3], carrying: false });
        }
    });
    game.inventory[0] = Some(ItemStack::new("create:minecart_coupling", 2));
    let coupling = ItemStack::new("create:minecart_coupling", 2);
    for id in [91, 92] {
        let mut claimed = false;
        game.native_interact_entity(id, "minecraft:minecart", Some(&coupling), &mut claimed);
        assert!(claimed);
    }
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(1));
    let line = command(game, "create_debug carts");
    assert!(line.contains("coupling 0000000000000000000000000000005b->0000000000000000000000000000005c length=3"), "{line}");
    for x in 1519..1528 {
        game.world.blocks.insert([x, 0, 1500], 84);
    }
    CARTS.with_borrow_mut(|c| {
        c[1].pos[0] += 0.3;
        c[1].velocity = [0.3, 0.0, 0.0];
    });
    let sent = game.sent.len();
    game.native_level_tick_post(dim);
    let (a, b) = (cart(91).unwrap().velocity, cart(92).unwrap().velocity);
    assert!((a[0] - 0.3).abs() < 1e-6 && b[0].abs() < 1e-6, "{a:?} {b:?}");
    // The hard step: 3.3 apart on a 3 long coupling, each is set half the 0.3 back
    // along its rail (API 68).
    let (a, b) = (cart(91).unwrap().pos, cart(92).unwrap().pos);
    assert!((a[0] - 1520.65).abs() < 1e-6 && (b[0] - 1523.65).abs() < 1e-6, "{a:?} {b:?}");
    // Clients are told how to draw it.
    let msg = game.sent[sent..].iter().find(|m| m.1 == create_engine::coupling::CHANNEL).expect("drawn").2.clone();
    let drawn = create_engine::coupling::Drawn::decode(&msg).unwrap();
    assert_eq!(drawn.key, 91);
    assert!((drawn.length - (3.0 - 2.0 * 13.0 / 16.0)).abs() < 1e-4, "{drawn:?}");
    // Saved and back.
    game.native_level_save(dim);
    let saved = game.saved.get("create/minecart_contraptions").cloned().expect("saved");
    create_carts::forget();
    game.saved.insert("create/minecart_contraptions".into(), saved);
    game.native_level_load(dim);
    game.native_level_tick_post(dim);
    let line = command(game, "create_debug carts");
    assert!(line.contains("length=3 distance=3.000"), "{line}");
    // A wrench takes both off and gives the coupling back.
    let mut claimed = false;
    game.native_interact_entity(91, "minecraft:minecart", Some(&wrench), &mut claimed);
    assert!(claimed && game.said.iter().any(|l| l == "Removed all couplings from minecart"));
    assert_eq!(command(game, "create_debug carts"), "no carts");
    // A cart assembler's mode, as its board sets it.
    assert_eq!(command(game, "create_debug cart_mode 1500 0 1500 rotation_locked"),
        "cart assembler at 1500 0 1500: ROTATION_LOCKED");
    CARTS.with_borrow_mut(Vec::clear);
    game.native_level_save(dim);
    game.saved_dirty.retain(|d| d != "create/minecart_contraptions");
    game.inventory[0] = None;
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
    // 19: an encased fan facing east; 20: a redstone link facing up, transmitting; 21: one
    // receiving (both unpowered).
    states.push(BlockState { name: "create:encased_fan".into(), properties: props("facing", "east") });
    for receiver in ["false", "true"] {
        states.push(BlockState { name: "create:redstone_link".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "up".to_owned()), ("powered".to_owned(), "false".to_owned()),
            ("receiver".to_owned(), receiver.to_owned())]) });
    }
    // 22: a copper backtank facing north, dry; 23: a creative motor facing down.
    states.push(BlockState { name: "create:copper_backtank".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:creative_motor".into(), properties: props("facing", "down") });
    // 24: a red valve handle facing west, 25: a black one.
    for colour in ["red", "black"] {
        states.push(BlockState { name: format!("create:{colour}_valve_handle"), properties: BTreeMap::from([
            ("facing".to_owned(), "west".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    // 26..29: a train door's lower half facing north, hinge left: closed, open and hidden,
    // closed and hidden (sliding shut), open and visible (as the template opens it).
    for (open, visible) in [("false", "true"), ("true", "false"), ("false", "false"), ("true", "true")] {
        states.push(BlockState { name: "create:train_door".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "north".to_owned()), ("half".to_owned(), "lower".to_owned()),
            ("hinge".to_owned(), "left".to_owned()), ("open".to_owned(), open.to_owned()),
            ("powered".to_owned(), "false".to_owned()), ("visible".to_owned(), visible.to_owned())]) });
    }
    // 30, 31: a white postbox facing north, shut and open.
    for open in ["false", "true"] {
        states.push(BlockState { name: "create:white_postbox".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "north".to_owned()), ("open".to_owned(), open.to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    // 32, 33: a display link facing east, unpowered and powered; 34, 35: an orange and a white
    // nixie tube on the floor facing north.
    for powered in ["false", "true"] {
        states.push(BlockState { name: "create:display_link".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "east".to_owned()), ("powered".to_owned(), powered.to_owned())]) });
    }
    for name in ["create:nixie_tube", "create:white_nixie_tube"] {
        states.push(BlockState { name: name.into(), properties: BTreeMap::from([
            ("double_face".to_owned(), "floor".to_owned()), ("facing".to_owned(), "north".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    // 36, 37: a packager facing east, unpowered and powered; 38: a frogport; 39: a chain
    // conveyor.
    for powered in ["false", "true"] {
        states.push(BlockState { name: "create:packager".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "east".to_owned()), ("linked".to_owned(), "false".to_owned()),
            ("powered".to_owned(), powered.to_owned())]) });
    }
    states.push(BlockState { name: "create:package_frogport".into(), properties: props("waterlogged", "false") });
    states.push(BlockState { name: "create:chain_conveyor".into(), properties: props("waterlogged", "false") });
    // 40: a schematicannon; 41: a schematic table facing north; 42: an item vault along x.
    states.push(BlockState { name: "create:schematicannon".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("open".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:schematic_table".into(), properties: props("facing", "north") });
    states.push(BlockState { name: "create:item_vault".into(), properties: BTreeMap::from([
        ("axis".to_owned(), "x".to_owned()), ("large".to_owned(), "false".to_owned())]) });
    // 43: a kindled blaze burner facing north.
    states.push(BlockState { name: "create:blaze_burner".into(), properties: BTreeMap::from([
        ("blaze".to_owned(), "kindled".to_owned()), ("facing".to_owned(), "north".to_owned())]) });
    // 44: a display board facing south.
    states.push(BlockState { name: "create:display_board".into(), properties: BTreeMap::from([
        ("down".to_owned(), "false".to_owned()), ("facing".to_owned(), "south".to_owned()),
        ("up".to_owned(), "false".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    // 45: a flywheel on x; 46, 47: a desk bell facing up, at rest and pressed; 48: a creative
    // crate.
    states.push(BlockState { name: "create:flywheel".into(), properties: props("axis", "x") });
    for powered in ["false", "true"] {
        states.push(BlockState { name: "create:desk_bell".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "up".to_owned()), ("powered".to_owned(), powered.to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    states.push(BlockState { name: "create:creative_crate".into(), properties: props("facing", "down") });
    // 49: a closed fluid valve facing up (its shaft along x, its pipe along z); 50: a
    // small powered steam whistle on the floor facing north; 51: a creative fluid tank; 52: a
    // hose pulley facing south (its shaft out west, its pipe side east).
    states.push(BlockState { name: "create:fluid_valve".into(), properties: BTreeMap::from([
        ("axis_along_first".to_owned(), "true".to_owned()), ("enabled".to_owned(), "false".to_owned()),
        ("facing".to_owned(), "up".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:steam_whistle".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("powered".to_owned(), "true".to_owned()),
        ("size".to_owned(), "small".to_owned()), ("wall".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:creative_fluid_tank".into(), properties: BTreeMap::from([
        ("bottom".to_owned(), "true".to_owned()), ("shape".to_owned(), "window".to_owned()), ("top".to_owned(), "true".to_owned())]) });
    states.push(BlockState { name: "create:hose_pulley".into(), properties: props("facing", "south") });
    // 53: a smart chute, unpowered.
    states.push(BlockState { name: "create:smart_chute".into(), properties: props("powered", "false") });
    // 54: a stock link on the floor (hanging on the block below), unpowered; 55: a stock ticker
    // facing north; 56: a redstone requester along x, unpowered; 57: a factory gauge on a wall
    // facing south (hanging on the block north of it); 58: a re-packager facing east.
    states.push(BlockState { name: "create:stock_link".into(), properties: BTreeMap::from([
        ("face".to_owned(), "floor".to_owned()), ("facing".to_owned(), "north".to_owned()),
        ("powered".to_owned(), "false".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:stock_ticker".into(), properties: props("facing", "north") });
    states.push(BlockState { name: "create:redstone_requester".into(), properties: BTreeMap::from([
        ("axis".to_owned(), "x".to_owned()), ("powered".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:factory_gauge".into(), properties: BTreeMap::from([
        ("face".to_owned(), "wall".to_owned()), ("facing".to_owned(), "south".to_owned()),
        ("powered".to_owned(), "false".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:repackager".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "east".to_owned()), ("powered".to_owned(), "false".to_owned())]) });
    // 59, 60: a train trapdoor facing north, closed and open; 61: a mechanical roller facing
    // north; 62: a powered track observer.
    for open in ["false", "true"] {
        states.push(BlockState { name: "create:train_trapdoor".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "north".to_owned()), ("half".to_owned(), "bottom".to_owned()),
            ("open".to_owned(), open.to_owned()), ("powered".to_owned(), "false".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    states.push(BlockState { name: "create:mechanical_roller".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "create:track_observer".into(), properties: props("powered", "true") });
    // 63: a nozzle facing east; 64: a belt going south's pulley.
    states.push(BlockState { name: "create:nozzle".into(), properties: props("facing", "east") });
    states.push(BlockState { name: "create:belt".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "south".to_owned()), ("slope".to_owned(), "horizontal".to_owned()),
        ("part".to_owned(), "pulley".to_owned()), ("casing".to_owned(), "false".to_owned()),
        ("waterlogged".to_owned(), "false".to_owned())]) });
    // 65: a lectern controller facing north.
    states.push(BlockState { name: "create:lectern_controller".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("has_book".to_owned(), "true".to_owned()),
        ("powered".to_owned(), "false".to_owned())]) });
    // 66, 67: a white and a red seat; 68: a turntable; 69: a creative motor facing up; 70: a
    // controller rail along x at power 15, pointing west.
    for colour in ["white", "red"] {
        states.push(BlockState { name: format!("create:{colour}_seat"), properties: props("waterlogged", "false") });
    }
    states.push(BlockState { name: "create:turntable".into(), properties: props("axis", "y") });
    states.push(BlockState { name: "create:creative_motor".into(), properties: props("facing", "up") });
    states.push(BlockState { name: "create:controller_rail".into(), properties: BTreeMap::from([
        ("backwards".to_owned(), "false".to_owned()), ("power".to_owned(), "15".to_owned()),
        ("shape".to_owned(), "east_west".to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    // 71, 72: copycat panels facing up and north; 73, 74: copycat steps facing south, bottom
    // and top; 75: the copycat base.
    for facing in ["up", "north"] {
        states.push(BlockState { name: "create:copycat_panel".into(), properties: BTreeMap::from([
            ("facing".to_owned(), facing.to_owned()), ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    for half in ["bottom", "top"] {
        states.push(BlockState { name: "create:copycat_step".into(), properties: BTreeMap::from([
            ("facing".to_owned(), "south".to_owned()), ("half".to_owned(), half.to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    states.push(BlockState { name: "create:copycat_base".into(), properties: BTreeMap::new() });
    // 76, 77: cart assemblers along x standing in for a rail, unpowered and powered; 78: a
    // minecart anchor along x; 79: an unpowered assembler for a powered rail; 80: an unpowered
    // assembler along z.
    for (powered, rail, shape) in [("false", "regular", "east_west"), ("true", "regular", "east_west"),
                                   ("", "", ""), ("false", "powered_rail", "east_west"),
                                   ("false", "regular", "north_south")] {
        if rail.is_empty() {
            states.push(BlockState { name: "create:minecart_anchor".into(), properties: props("axis", "x") });
            continue;
        }
        states.push(BlockState { name: "create:cart_assembler".into(), properties: BTreeMap::from([
            ("backwards".to_owned(), "false".to_owned()), ("powered".to_owned(), powered.to_owned()),
            ("rail_type".to_owned(), rail.to_owned()), ("shape".to_owned(), shape.to_owned()),
            ("waterlogged".to_owned(), "false".to_owned())]) });
    }
    // 81: a water source; 82: a target block at power 7; 83: a lectern facing north, no book; 84:
    // a rail along x.
    states.push(BlockState { name: "minecraft:water".into(), properties: props("level", "0") });
    states.push(BlockState { name: "minecraft:target".into(), properties: props("power", "7") });
    states.push(BlockState { name: "minecraft:lectern".into(), properties: BTreeMap::from([
        ("facing".to_owned(), "north".to_owned()), ("has_book".to_owned(), "false".to_owned()),
        ("powered".to_owned(), "false".to_owned())]) });
    states.push(BlockState { name: "minecraft:rail".into(), properties: props("shape", "east_west") });
    let registry = Box::leak(Box::new(BlockRegistry { states }));
    let mut calls = 0;
    let mut game = Game { pack: Pack { assets: Assets { registry } }, calls: &mut calls, said: vec![],
        world: World { blocks: Default::default(), game_time: 0, day_time: 0, block_entities: Default::default(), shape: LevelShape { min_y: -64, height: 384 } }, sent: vec![], parts: vec![], aim: None, aim_point: None, unsaved: vec![],
        offhand: None,
        dropped: Vec::new(), right: false, audio: Audio,
        creative: false, player: Some(Player { exhaustion: 0.0 }), keys: Default::default(),
        glides: vec![], hand: horizon_client::first_person::Hand::Main, yaw: 180.0, pitch: 0.0, armor: [None, None, None, None],
        inventory: vec![Some(horizon_client::items::ItemStack { item: "minecraft:iron_ingot".into(), count: 5, damage: 0, extra: None })],
        selected: 0, dim: Dim, res: Res, custom_recipes: Default::default(), cooking: Default::default(),
        saved: Default::default(), saved_dirty: Vec::new() };
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
    // Hooks that don't look at the world's blocks are given an empty one.
    let nowhere: &'static World = Box::leak(Box::new(World { blocks: Default::default(), game_time: 0, day_time: 0, block_entities: Default::default(), shape: LevelShape { min_y: -64, height: 384 } }));
    let level = GameLevel { registry, world: nowhere, items: fixture_items() };
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
    assert_eq!(commands.len(), 44);
    let player = game.native_acting_player().unwrap();
    let mut ran = false;
    game.native_perform_command(&player, "create_debug speed 2 0 0", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 2 0 0: 17 rpm"), "after the scroll");
    // A train's controls held by command (riding is not possible): no such train yet.
    let mut ran = false;
    game.native_perform_command(&player, "create_debug drive 9 1 left", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("no train 9"));
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
    let horizon_client::containers::BlockEntity::Mod(depot) = &game.world.block_entities[&(5, 0, 0)] else { panic!("a mod block entity") };
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
    let horizon_client::containers::BlockEntity::Mod(end) = &game.world.block_entities[&(10, 0, 13)] else { panic!("a mod block entity") };
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
    let mut level = GameLevel { registry, world: nowhere, items: fixture_items() };
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
    // Redstone links: a transmitter at (100, 0, 100) and a receiver 20 blocks east, each tuned with
    // the iron ingot in hand by a click on its first slot; a signal beside the transmitter
    // reaches the receiver, which turns `powered` on and gives 15.
    for (pos, state) in [([100, 0, 100], 20), ([120, 0, 100], 21)] {
        game.world.blocks.insert([pos[0], -1, pos[2]], 1);
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: 0, z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    for pos in [[100, 0, 100], [120, 0, 100]] {
        game.aim = Some(pos);
        game.aim_point = Some([f64::from(pos[0]) + 0.5, 3.01 / 16.0, f64::from(pos[2]) + 10.5 / 16.0]);
        let mut canceled = false;
        game.native_right_click_block(pos, [0, 1, 0], &mut canceled);
        assert!(canceled, "the slot took the click");
    }
    game.aim_point = None;
    SIGNALS.with_borrow_mut(|s| s.insert([101, 0, 100]));
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 100, y: 0, z: 100 }, BlockStateId(20));
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "set [100, 0, 100] powered=true"), "{:?}", game.said);
    assert!(game.said.iter().any(|l| l == "set [120, 0, 100] powered=true"), "{:?}", game.said);
    let mut signal = 0;
    level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 120, y: 0, z: 100 }, BlockStateId(21),
        horizon_core::geometry::Direction::East, false, &mut signal);
    assert_eq!(signal, 15);
    assert!(game.sent.iter().any(|(_, ch, _)| ch == "create:link"), "clients see the frequencies");
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [7, 6], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create".into()) else { panic!("no create data") };
    assert!(own.get(&"links".into()).is_some(), "{own:?}");
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
    // Saved by its controller: x, y, z, fluid, amount, width, height, windows.
    assert_eq!(own.get(&"tank_multis".into()), Some(&horizon_world::nbt::Tag::IntArray(vec![40, 0, 40, 0, 1000, 1, 1, 1])));
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
    assert!(game.sent.iter().any(|m| m.1 == "create:contraption" && m.2.len() == 17 + 8 * 16 + 4));
    game.world.game_time += 5;
    let mut canceled = false;
    game.native_right_click_block([50, 0, 50], [0, 0, -1], &mut canceled);
    assert!((46..54).all(|x| game.world.blocks.get(&[x, 0, 49]) == Some(&18)), "the sails are back");
    // A fan at (60, 0, 60) facing east, its motor behind it, water in front and a depot below the
    // current three blocks out holding two ice: washed into packed ice after 151 ticks.
    game.world.block_entities.insert((63, -1, 60), horizon_client::containers::BlockEntity::Mod(
        horizon_client::containers::ModBlockEntity { items: vec![Some(horizon_client::items::ItemStack {
            item: "minecraft:ice".into(), count: 2, damage: 0, extra: None })] }));
    for (pos, state) in [([59, 0, 60], 3), ([60, 0, 60], 19), ([61, 0, 60], 81), ([63, -1, 60], 4)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    let washed = |game: &Game<'_>| {
        let horizon_client::containers::BlockEntity::Mod(d) = &game.world.block_entities[&(63, -1, 60)] else { panic!("a mod block entity") };
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
    game.aim = None;
    game.native_hud(&mut batch);
    assert!(batch.drawn.is_empty(), "nothing without goggles or a value box in sight");
    // The motor's value box: its label over the hotbar, without goggles.
    game.aim = Some([0, 0, 0]);
    game.native_hud(&mut batch);
    assert_eq!(batch.drawn.first().map(|s| s.trim()), Some("Generated Speed in RPM"), "{:?}", batch.drawn);
    assert!(!batch.drawn.iter().any(|s| s.trim() == "Generator Stats:"), "no stats without goggles");
    batch.drawn.clear();
    game.armor[0] = Some(horizon_client::items::ItemStack { item: "create:goggles".into(), count: 1, damage: 0, extra: None });
    game.native_hud(&mut batch);
    assert!(batch.drawn.iter().any(|s| s.trim() == "Generator Stats:"), "{:?}", batch.drawn);
    // A copper backtank at (200, 0, 200) under a motor facing down (16 rpm): it fills one air,
    // then waits 16 ticks (`BacktankBlockEntity.tick`); its air is saved with its column.
    for (pos, state) in [([200, 0, 200], 22), ([200, 1, 200], 23)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    for _ in 0..37 { game.native_level_tick_post("minecraft:overworld"); }
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [12, 12], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create_equipment".into()) else { panic!("no backtank saved: {data:?}") };
    let Some(horizon_world::nbt::Tag::IntArray(tank)) = own.get(&"backtanks".into()) else { panic!("{own:?}") };
    assert_eq!(tank[..4], [200, 0, 200, 3], "1 air at once, then one each 17 ticks: {tank:?}");
    // An empty hand and an empty chest slot take it up: on the chest with its air, the block gone.
    game.inventory[0] = None;
    game.armor = [None, None, None, None];
    let mut canceled = false;
    game.native_right_click_block([200, 0, 200], [0, 1, 0], &mut canceled);
    assert!(canceled);
    assert_eq!(game.world.blocks[&[200, 0, 200]], 0, "picked up");
    let worn = game.armor[1].as_ref().map(|s| (s.item.to_string(), s.damage));
    assert_eq!(worn, Some(("create:copper_backtank".into(), 3)), "air on the item (the stand-in keeps it in damage)");
    // A diving helmet under water: one air from the tank every 20 game ticks, and breath.
    game.armor[0] = Some(horizon_client::items::ItemStack { item: "create:copper_diving_helmet".into(), count: 1, damage: 0, extra: None });
    create_equipment_body::BODY.set((false, false));
    game.world.game_time = 40;
    game.native_player_tick();
    assert_eq!(game.armor[1].as_ref().map(|s| s.damage), Some(2));
    assert_eq!(create_equipment_body::REFILLS.get(), 1);
    game.world.game_time = 41;
    game.native_player_tick();
    assert_eq!(game.armor[1].as_ref().map(|s| s.damage), Some(2), "not between seconds");
    // Above water: nothing.
    create_equipment_body::BODY.set((true, false));
    game.world.game_time = 60;
    game.native_player_tick();
    assert_eq!((game.armor[1].as_ref().map(|s| s.damage), create_equipment_body::REFILLS.get()), (Some(2), 2));
    // An extendo grip in hand reaches 3 further; breaking a block with it costs the tank an air
    // (a survival player's: creative ones pay nothing).
    game.creative = false;
    game.inventory[0] = Some(horizon_client::items::ItemStack { item: "create:extendo_grip".into(), count: 1, damage: 0, extra: None });
    game.native_player_tick();
    assert_eq!(create_equipment_body::REACH.with_borrow(|r| r.clone()), vec!["create:single_range_attribute_modifier".to_owned()]);
    let mut canceled = false;
    game.native_destroy_block([1, 5, 1], BlockStateId(1), &mut canceled);
    assert_eq!(game.armor[1].as_ref().map(|s| s.damage), Some(1), "the backtank paid");
    assert!(create_equipment_body::WORN.with_borrow(Vec::is_empty));
    game.inventory[0] = None;
    game.native_player_tick();
    assert!(create_equipment_body::REACH.with_borrow(Vec::is_empty), "the reach goes with the grip");
    // A display link at (80, 0, 80) facing east, on a target block at power 7 west of it, and two
    // nixie tubes (of two colours: one row) at (80, 0, 84) and (80, 0, 85) facing north: bound by
    // the debug command it writes "7" across the row at once, the left tube (south) first.
    game.world.blocks.insert([79, 0, 80], 82);
    for (pos, state) in [([80, 0, 80], 32), ([80, 0, 84], 34), ([80, 0, 85], 35)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    game.native_level_tick_post("minecraft:overworld");
    let before = game.sent.len();
    let mut ran = false;
    game.native_perform_command(&me, "create_debug display_link 80 0 80 80 0 84", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("Successfully bound to targeted position: Redstone Power"));
    let shown: Vec<create_engine::display::Shown> = game.sent[before..].iter()
        .filter(|m| m.1 == "create:display").flat_map(|m| create_engine::display::decode(&m.2)).collect();
    assert_eq!(shown, vec![
        create_engine::display::Shown::Nixie { pos: [80, 0, 84], text: Some("7".into()), index: 1, redstone: 0 },
        create_engine::display::Shown::Nixie { pos: [80, 0, 85], text: Some("7".into()), index: 0, redstone: 0 },
    ]);
    game.native_perform_command(&me, "create_debug display_state 80 0 85", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some(r#"nixie tube at 80 0 85: text "7", index 0, shows "7" " ""#));
    // Nothing changed: 100 ticks on it reads again and sends nothing.
    let before = game.sent.len();
    for _ in 0..100 { game.world.game_time += 1; game.native_level_tick_post("minecraft:overworld"); }
    assert!(!game.sent[before..].iter().any(|m| m.1 == "create:display"));
    // A signal beside it powers it.
    SIGNALS.with_borrow_mut(|s| s.insert([81, 0, 80]));
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 80, y: 0, z: 80 }, BlockStateId(32));
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "set [80, 0, 80] powered=true"), "{:?}", game.said);
    // Saved with its column: the link and both tubes.
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [5, 5], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create_display".into()) else { panic!("no display data") };
    assert!(own.contains_key(&"links".into()) && own.contains_key(&"nixies".into()), "{own:?}");
    // A red valve handle at (70, 0, 70): a click turns it at 32 RPM; a black dye recolours it.
    let at = |p: [i32; 3]| BlockPos { x: p[0], y: p[1], z: p[2] };
    game.world.blocks.insert([70, 0, 70], 24);
    level.native_block_changed(LevelDimension::Overworld, at([70, 0, 70]), BlockStateId(0), BlockStateId(24));
    let mut canceled = false;
    game.native_right_click_block([70, 0, 70], [1, 0, 0], &mut canceled);
    assert!(canceled, "a valve handle takes the click");
    game.native_level_tick_post("minecraft:overworld");
    let mut ran = false;
    game.native_perform_command(&player, "create_debug speed 70 0 70", &mut ran);
    assert!(game.said.last().is_some_and(|l| l.contains("32")), "turning: {:?}", game.said.last());
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("minecraft:black_dye", 1));
    let mut canceled = false;
    game.native_right_click_block([70, 0, 70], [1, 0, 0], &mut canceled);
    assert!(canceled);
    assert_eq!(game.world.blocks.get(&[70, 0, 70]), Some(&25), "dyed black, still facing west");
    game.inventory[0] = None;
    // A train door at (72, 0, 70): a click opens it hidden, with an iron door's sound; a second
    // closes it, and once its panel has slid shut it shows again.
    game.world.blocks.insert([72, 0, 70], 26);
    level.native_block_changed(LevelDimension::Overworld, at([72, 0, 70]), BlockStateId(0), BlockStateId(26));
    let mut canceled = false;
    game.native_right_click_block([72, 0, 70], [0, 0, 1], &mut canceled);
    assert!(canceled);
    assert_eq!(game.world.blocks.get(&[72, 0, 70]), Some(&27), "open, hidden");
    assert!(game.said.last().is_some_and(|l| l.contains("block.iron_door.open")));
    level.native_block_changed(LevelDimension::Overworld, at([72, 0, 70]), BlockStateId(26), BlockStateId(27));
    game.native_level_tick_post("minecraft:overworld");
    let mut canceled = false;
    game.native_right_click_block([72, 0, 70], [0, 0, 1], &mut canceled);
    assert_eq!(game.world.blocks.get(&[72, 0, 70]), Some(&28), "closed, still sliding");
    level.native_block_changed(LevelDimension::Overworld, at([72, 0, 70]), BlockStateId(27), BlockStateId(28));
    let mut shown = None;
    for t in 1..=12 {
        game.native_level_tick_post("minecraft:overworld");
        if game.world.blocks.get(&[72, 0, 70]) == Some(&26) { shown = Some(t); break; }
    }
    assert_eq!(shown, Some(2), "slid one step open, back in one, settled the next");
    assert!(game.said.last().is_some_and(|l| l.contains("block.iron_door.close") && l.ends_with(" 0.5 1")));
    // Opened by redstone (the template sets `open` only): hidden after the level's tick.
    game.world.blocks.insert([72, 0, 70], 29);
    level.native_block_changed(LevelDimension::Overworld, at([72, 0, 70]), BlockStateId(26), BlockStateId(29));
    game.native_level_tick_post("minecraft:overworld");
    assert_eq!(game.world.blocks.get(&[72, 0, 70]), Some(&27));
    // A postbox at (74, 0, 70): a click opens its menu at the level's tick, and it opens; once
    // the menu closes, it shuts.
    game.world.blocks.insert([74, 0, 70], 30);
    let mut canceled = false;
    game.native_right_click_block([74, 0, 70], [0, 0, 1], &mut canceled);
    assert!(canceled);
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "menu block.create.white_postbox"));
    assert!(game.said.iter().any(|l| l == "set [74, 0, 70] open=true"));
    assert!(game.said.last().is_some_and(|l| l.contains("block.barrel.open")));
    game.native_menu_closed(7, 1);
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "set [74, 0, 70] open=false"));
    // A toolbox: a container of 36 slots with a four-row menu, its screen showing Create's 32.
    let mut entity = None;
    game.native_block_entity("create:red_toolbox", &mut entity);
    assert!(entity.is_some());
    let mut container = None;
    game.native_block_entity_container("create:red_toolbox", &mut container);
    assert!(container.as_ref().is_some_and(|c| c.slots == 36 && c.inputs.len() == 32
        && c.menu.as_deref() == Some("minecraft:generic_9x4")));
    let mut screen = None;
    game.native_screen("create:red_toolbox", &mut screen);
    let screen = screen.expect("a toolbox lays its screen out");
    assert_eq!((screen.slots.len(), screen.slots[31], screen.slots[32]), (36, Some([143.0, 72.0]), None));
    // The threshold switch's thresholds and a lever's, observer's or switch's state by command.
    let mut ran = false;
    game.native_perform_command(&player, "create_debug threshold 1 0 0 64 128", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("no threshold switch there"));
    game.native_perform_command(&player, "create_debug redstone_state 1 0 0", &mut ran);
    assert_eq!(game.said.last().map(String::as_str),
        Some("no analog lever, smart observer or threshold switch there"));
    // Super glue from (90, 0, 90) to (91, 0, 90) over two stones: saved with its column; none
    // over a gap.
    game.world.blocks.insert([90, 0, 90], 1);
    game.world.blocks.insert([91, 0, 90], 1);
    let mut ran = false;
    game.native_perform_command(&player, "create_debug glue 90 0 90 91 0 90", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("glued 90 0 90 to 91 0 90"));
    game.native_perform_command(&player, "create_debug glue 90 0 90 93 0 90", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("glue: the corners don't hold together"));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [5, 5], &mut data);
    let glue = horizon_world::nbt::Tag::Compound([("areas".into(),
        horizon_world::nbt::Tag::IntArray(vec![90, 0, 90, 91, 0, 90]))].into_iter().collect());
    assert_eq!(data.get(&"create_glue".into()), Some(&glue));
    // Controls on a contraption that isn't there.
    game.native_perform_command(&player, "create_debug controls 0 90 0 all", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("no moving contraption at 0 90 0"));
    // A train trapdoor at (900, 0, 900): a click opens it, whatever its power, with an iron
    // trapdoor's sound.
    game.world.blocks.insert([900, 0, 900], 59);
    level.native_block_changed(LevelDimension::Overworld, at([900, 0, 900]), BlockStateId(0), BlockStateId(59));
    let mut canceled = false;
    game.native_right_click_block([900, 0, 900], [0, 1, 0], &mut canceled);
    assert!(canceled);
    assert_eq!(game.world.blocks.get(&[900, 0, 900]), Some(&60), "open");
    assert!(game.said.last().is_some_and(|l| l.contains("block.iron_trapdoor.open")));
    // A roller at (902, 0, 900): its mode by command (its value box), saved with its column.
    game.world.blocks.insert([902, 0, 900], 61);
    level.native_block_changed(LevelDimension::Overworld, at([902, 0, 900]), BlockStateId(0), BlockStateId(61));
    game.native_perform_command(&player, "create_debug roller 902 0 900 wide_fill", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("roller 902 0 900: wide_fill no material"));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [56, 56], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create_glue".into()) else { panic!("no roller saved") };
    assert!(own.contains_key(&"rollers".into()), "{own:?}");
    // A powered track observer gives 15 to every side, no direct signal.
    for direct in [false, true] {
        let mut signal = -1;
        level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 904, y: 0, z: 900 }, BlockStateId(62),
            horizon_core::geometry::Direction::North, direct, &mut signal);
        assert_eq!(signal, if direct { 0 } else { 15 });
    }
    // Controls clicked on a contraption with none aboard: none to click.
    game.native_perform_command(&player, "create_debug controls 0 90 0 block", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("no moving contraption at 0 90 0"));
    // Packages: a packager at (300, 0, 300) facing east with a postbox behind it and a frogport
    // on it; two chain conveyors (302, 1, 300) and (306, 1, 300), the first under a motor
    // facing down; a second frogport by the far conveyor takes "Mine".
    let level = GameLevel { registry, world: nowhere, items: fixture_items() };
    for (pos, state) in [([299, 0, 300], 30), ([300, 0, 300], 36), ([300, 1, 300], 38),
                         ([302, 1, 300], 39), ([306, 1, 300], 39), ([302, 2, 300], 23), ([306, 1, 302], 38)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(0), BlockStateId(state));
    }
    game.native_level_tick_post("minecraft:overworld");
    for (line, said) in [
        ("create_debug chain 302 1 300 306 1 300", "chain [302, 1, 300] - [306, 1, 300]"),
        ("create_debug address 300 0 300 Mine", "packager address \\"Mine\\""),
        ("create_debug address 306 1 302 Mine", "port filter \\"Mine\\""),
        ("create_debug address 299 0 300 Home", "port filter \\"Home\\""),
        ("create_debug frogport 300 1 300 302 1 300 0", "frogport on [302, 1, 300] at 0 None"),
        ("create_debug frogport 306 1 302 306 1 300 0", "frogport on [306, 1, 300] at 0 None"),
    ] {
        game.native_perform_command(&player, line, &mut ran);
        assert_eq!(game.said.last().map(String::as_str), Some(said), "{line}");
    }
    // A package in the postbox (not for it), then the postbox gives out anything.
    let package = horizon_client::items::ItemStack::new("create:cardboard_package_10x8", 1);
    assert!(matches!(create_packages::automation_insert(&mut game, 0, [299, 0, 300], package), Ok(None)));
    game.native_perform_command(&player, "create_debug address 299 0 300 *", &mut ran);
    for _ in 0..2 { game.native_level_tick_post("minecraft:overworld"); }
    game.native_perform_command(&player, "create_debug packages 306 1 300", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.contains("connections=[[-4, 0, 0]]") && !said.contains("speed=0 ") && said.contains("Mine:0->[0, 0, 0]"), "{said}");
    // A pulse: the packager packs the package behind it, addressed "Mine"; out after 20 ticks,
    // the frogport on it sends it along the chain to the frogport taking "Mine".
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 300, y: 0, z: 300 }, BlockStateId(36));
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug packages 300 0 300", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.contains("animation=19") && said.contains("to \\"Mine\\""), "{said}");
    let mut drawn = false;
    for _ in 0..400 {
        game.native_level_tick_post("minecraft:overworld");
        for m in game.sent.iter().filter(|m| m.1 == "create:chain") {
            drawn |= m.2.len() > 14;
        }
    }
    game.native_perform_command(&player, "create_debug packages 306 1 302", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some(
        "frogport filter=\\"Mine\\" target=Some(([306, 1, 300], 0.0, None)) animating=false items=[create:cardboard_package_10x8 to \\"Mine\\"]"));
    assert!(drawn, "the package was sent to clients on the chain");
    // As a client: a package on a chain is drawn as an item part.
    let message = game.sent.iter().filter(|m| m.1 == "create:chain" && m.2.len() > 14).last().unwrap().2.clone();
    let (parts, glides) = (game.parts.len(), game.glides.len());
    game.native_net_from_server("create:chain", &message);
    assert!(game.parts.len() + game.glides.len() > parts + glides);
    // Saved with their columns.
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [18, 18], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(saved)) = data.get(&"create_packages".into()) else { panic!("{data:?}") };
    let Some(horizon_world::nbt::Tag::List(ports)) = saved.get(&"ports".into()) else { panic!() };
    assert_eq!(ports.len(), 2, "the postbox and the first frogport");
    // Schematics: two stones at (400, 0, 400) and (401, 0, 400) saved by the schematic and quill
    // (the air above left out), uploaded at a table at (404, 0, 400) into a written schematic,
    // deployed at (410, 0, 400) and printed by a cannon at (406, 0, 400) from the vault east of
    // it, burning the gunpowder in its slot.
    use horizon_client::containers::{BlockEntity::Mod, ModBlockEntity};
    use horizon_client::items::ItemStack;
    game.world.game_time = 5000;
    game.world.blocks.insert([400, 0, 400], 1);
    game.world.blocks.insert([401, 0, 400], 1);
    game.native_perform_command(&player, "create_debug schematic save 400 0 400 401 1 400 Hut", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("Saved as hut.nbt"));
    let saved = create_schematic_body::FILES.with_borrow(|f| f.get("hut.nbt").cloned()).expect("saved");
    let hut = create_engine::schematic::Structure::from_nbt(&saved).unwrap();
    assert_eq!((hut.size, hut.blocks.len(), hut.palette.len()), ([2, 2, 1], 2, 1));
    game.native_perform_command(&player, "create_debug schematic save 400 0 400 401 1 400 Hut", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("Saved as hut_1.nbt"), "a free name");
    game.world.blocks.insert([404, 0, 400], 41);
    game.world.block_entities.insert((404, 0, 400), Mod(ModBlockEntity {
        items: vec![Some(ItemStack::new("create:empty_schematic", 1)), None, None, None, None] }));
    game.native_perform_command(&player, "create_debug schematic upload 404 0 400 hut", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("uploaded hut.nbt: 2x2x1"));
    let written = match game.world.block_entities.get_mut(&(404, 0, 400)) {
        Some(Mod(m)) => { assert!(m.items[0].is_none(), "the blank was used"); m.items[1].take() }
        _ => None,
    };
    assert_eq!(written.as_ref().map(|s| &*s.item), Some("create:schematic"));
    game.inventory[0] = written;
    game.selected = 0;
    game.native_perform_command(&player, "create_debug schematic deploy hut 410 0 400", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("deployed hut.nbt at 410 0 400 NONE NONE"));
    let schematic = game.inventory[0].take();
    game.world.block_entities.insert((407, 0, 400), Mod(ModBlockEntity {
        items: vec![Some(ItemStack::new("minecraft:stone", 64))] }));
    game.world.block_entities.insert((406, 0, 400), Mod(ModBlockEntity {
        items: vec![schematic, None, None, None, Some(ItemStack::new("minecraft:gunpowder", 1))] }));
    for (pos, state) in [([407, 0, 400], 42), ([406, 0, 400], 40)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(0), BlockStateId(state));
    }
    game.world.game_time += 1;
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug schematicannon 406 0 400", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.starts_with("schematicannon 406 0 400: PAUSED ready fuel=400 placed=0/2 target=Some([409, 0, 400])"), "{said}");
    game.native_perform_command(&player, "create_debug schematicannon 406 0 400 play", &mut ran);
    assert!(game.said.last().is_some_and(|l| l.contains("RUNNING running")));
    let mut shots = Vec::new();
    for _ in 0..40 {
        game.world.game_time += 1;
        let before = game.said.len();
        game.native_level_tick_post("minecraft:overworld");
        if game.said[before..].iter().any(|l| l.contains("entity.generic.explode")) {
            shots.push(game.world.game_time - 5001);
        }
    }
    // A shot every schematicannonDelay + 1 ticks; each block lands 11 ticks after.
    assert_eq!(shots, vec![1, 12]);
    assert_eq!((game.world.blocks.get(&[410, 0, 400]), game.world.blocks.get(&[411, 0, 400])), (Some(&1), Some(&1)));
    assert!(game.said.iter().any(|l| l.contains("note_block.bell")), "it dings when done");
    game.native_perform_command(&player, "create_debug schematicannon 406 0 400", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.starts_with("schematicannon 406 0 400: STOPPED finished fuel=398 placed=0/0 target=None"), "{said}");
    let Some(Mod(vault)) = game.world.block_entities.get(&(407, 0, 400)) else { panic!() };
    assert_eq!(vault.items[0].as_ref().map(|s| s.count), Some(62), "two stones taken");
    let Some(Mod(cannon)) = game.world.block_entities.get(&(406, 0, 400)) else { panic!() };
    assert!(cannon.items[0].is_none() && cannon.items[4].is_none());
    assert_eq!(cannon.items[1].as_ref().map(|s| (&*s.item, s.count)), Some(("create:empty_schematic", 1)));
    // Clients were told where the barrel pointed, and that it rests again.
    let aims: Vec<create_engine::schematicannon::Aim> = game.sent.iter().filter(|m| m.1 == "create:schematicannon")
        .flat_map(|m| create_engine::schematicannon::decode(&m.2)).collect();
    assert!(aims.iter().any(|a| matches!(a, create_engine::schematicannon::Aim::Set { target: Some([411, 0, 400]), .. })));
    assert!(matches!(aims.last(), Some(create_engine::schematicannon::Aim::Set { target: None, .. })));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [25, 25], &mut data);
    assert!(data.contains_key(&"create_schematics".into()), "the cannon is saved with its column");
    // Comparators: a vault of two blocks at (500, 0, 500) and (501, 0, 500) along x, ten stacks
    // of iron put into the second: after the level's tick comparators by both blocks read again,
    // and both read the whole vault's 40 slots (floor(10 / 40 * 14) + 1 = 4, not its own 8).
    for x in [500, 501] {
        game.world.block_entities.insert((x, 0, 500), Mod(ModBlockEntity { items: vec![None; 20] }));
        game.world.blocks.insert([x, 0, 500], 42);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x, y: 0, z: 500 }, BlockStateId(0), BlockStateId(42));
    }
    game.native_level_tick_post("minecraft:overworld");
    if let Some(Mod(m)) = game.world.block_entities.get_mut(&(501, 0, 500)) {
        for slot in &mut m.items[..10] { *slot = Some(ItemStack::new("minecraft:iron_ingot", 64)); }
    }
    game.native_contents_changed([501, 0, 500]);
    let before = game.said.len();
    game.native_level_tick_post("minecraft:overworld");
    let told: Vec<&String> = game.said[before..].iter().filter(|l| l.starts_with("comparators")).collect();
    assert_eq!(told, ["comparators [500, 0, 500]", "comparators [501, 0, 500]"]);
    // Nothing changed: nobody is told again.
    let before = game.said.len();
    game.native_level_tick_post("minecraft:overworld");
    assert!(!game.said[before..].iter().any(|l| l.starts_with("comparators")));
    // A depot at (504, 0, 500) holding 32 iron: floor(32 / 64 * 14) + 1 = 8.
    game.world.block_entities.insert((504, 0, 500), Mod(ModBlockEntity {
        items: vec![Some(ItemStack::new("minecraft:iron_ingot", 32))] }));
    game.world.blocks.insert([504, 0, 500], 4);
    let comparator = |game: &Game<'_>, pos: [i32; 3], state: u32| {
        let level = GameLevel { registry, world: &game.world, items: fixture_items() };
        let mut signal = -1;
        level.native_analog_output_signal(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(state), horizon_core::geometry::Direction::North, &mut signal);
        signal
    };
    assert_eq!(comparator(&game, [500, 0, 500], 42), 4, "the whole vault");
    assert_eq!(comparator(&game, [501, 0, 500], 42), 4);
    assert_eq!(comparator(&game, [504, 0, 500], 4), 8, "the depot");
    // The tank at (40, 0, 40) holds a bucket of 8: floor(1/8 * 14 + 1) = 2; its comparators were
    // told when it was filled.
    assert_eq!(comparator(&game, [40, 0, 40], 15), 2, "the tank");
    assert!(game.said.iter().any(|l| l == "comparators [40, 0, 40]"));
    // A kindled blaze burner reads 2 (`HeatLevel.ordinal() - 1`); stone keeps vanilla's answer.
    assert_eq!(comparator(&game, [506, 0, 500], 43), 2, "the burner");
    assert_eq!(comparator(&game, [508, 0, 500], 1), -1, "not Create's");
    // A display board at (510, 0, 500), one block wide, as a client is told of it: its first line
    // "hi" draws as its two flaps' characters; the next tick, with nothing changed, sets nothing;
    // forgotten, its text is cleared.
    game.world.blocks.insert([510, 0, 500], 44);
    let mut first = create_engine::display::Layout::new(4);
    first.sections[0].text = Some("hi".into());
    let board = create_engine::display::Shown::Board { pos: [510, 0, 500], x_size: 1, y_size: 1,
        lines: vec![first, create_engine::display::Layout::new(4)], colour: vec![None, None], glowing: vec![false, false] };
    game.native_net_from_server("create:display", &create_engine::display::encode(&[board]));
    game.native_tick();
    assert_eq!(TEXTS.with_borrow(|t| t.clone()), ["set [510, 0, 500] H|I"]);
    game.native_tick();
    assert_eq!(TEXTS.with_borrow(Vec::len), 1, "unchanged: not set again");
    let forget = create_engine::display::Shown::Forget { pos: [510, 0, 500] };
    game.native_net_from_server("create:display", &create_engine::display::encode(&[forget]));
    game.native_tick();
    assert_eq!(TEXTS.with_borrow(|t| t.last().cloned()).as_deref(), Some("clear"));
    // A motor facing east at (700, 0, 700) turns a flywheel on x beside it: no stress, 16 rpm.
    for (pos, state) in [([700, 0, 700], 3), ([701, 0, 700], 45)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    game.native_level_tick_post("minecraft:overworld");
    assert_eq!(create::with_world(|w| w.speed(0, [701, 0, 700])), Some(Some(16.0)), "the flywheel turns");
    // A desk bell at (704, 0, 700): a click with an empty hand presses it (`powered`), it powers
    // the block below strongly while pressed, and lets go 20 ticks later.
    game.world.blocks.insert([704, 0, 700], 46);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 704, y: 0, z: 700 }, BlockStateId(0), BlockStateId(46));
    game.inventory[0] = None;
    let mut canceled = false;
    game.native_right_click_block([704, 0, 700], [0, 1, 0], &mut canceled);
    assert!(canceled, "the bell takes the click");
    assert!(game.said.iter().any(|l| l == "set [704, 0, 700] powered=true"));
    let mut signal = 0;
    level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 704, y: 0, z: 700 }, BlockStateId(47),
        horizon_core::geometry::Direction::Down, true, &mut signal);
    assert_eq!(signal, 15, "strong into the block below");
    let mut side = 0;
    level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 704, y: 0, z: 700 }, BlockStateId(47),
        horizon_core::geometry::Direction::North, true, &mut side);
    assert_eq!(side, 0, "not strong sideways");
    game.world.game_time += 20;
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "set [704, 0, 700] powered=false"), "let go after 20 ticks");
    // A creative crate at (706, 0, 700): stone clicked on its top is its supply; it takes
    // whatever is put in and gives stone, a stack at a time.
    game.world.blocks.insert([706, 0, 700], 48);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 706, y: 0, z: 700 }, BlockStateId(0), BlockStateId(48));
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("minecraft:stone", 1));
    game.aim = Some([706, 0, 700]);
    game.aim_point = Some([706.5, 0.875, 700.5]);
    let mut canceled = false;
    game.native_right_click_block([706, 0, 700], [0, 1, 0], &mut canceled);
    assert!(canceled);
    assert_eq!(create_decor::crate_supply(&game, 0, [706, 0, 700]), Some(Some("minecraft:stone".to_owned())));
    let room = create_logistics::insert_room(&game, 0, [706, 0, 700], horizon_core::geometry::Direction::Up,
        &horizon_client::items::ItemStack::new("minecraft:dirt", 40));
    assert_eq!(room, Some(40), "it takes everything");
    let taken = create_decor::crate_extract(&game, Some("minecraft:stone"), |_| true, 64);
    assert_eq!(taken.map(|s| (s.item.to_string(), s.count)), Some(("minecraft:stone".to_owned(), 64)));
    game.aim = None;

    // Phase 5's plumbing, at x 600 and on.
    // A whistle (50, powered) on a tank (15) at (600, 0, 600) over a kindled burner (43): the
    // tank becomes a boiler, the burner heats it and the whistle sounds, high (small) at pitch 0,
    // its chiff first.
    for (pos, state) in [([600, -1, 600], 43), ([600, 0, 600], 15), ([600, 1, 600], 50)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, at(pos), BlockStateId(0), BlockStateId(state));
    }
    game.native_level_tick_post("minecraft:overworld");
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l.contains("playsound create:chiff record @a 600.5 1.5 600.5 0.175 1.75")), "{:?}", game.said);
    assert!(game.said.iter().any(|l| l.contains("playsound create:whistle_high record @a 600.5 1.5 600.5 1 1")), "{:?}", game.said);
    let mut ran = false;
    game.native_perform_command(&player, "create_debug fluid_state 600 1 600", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("whistle at 600 1 600: pitch 0, sounding true"));
    // Sounding, it starts its loop again only when it ran out: nothing more for a while.
    let before = game.said.len();
    for _ in 0..10 { game.native_level_tick_post("minecraft:overworld"); }
    assert!(!game.said[before..].iter().any(|l| l.contains("playsound")), "{:?}", &game.said[before..]);
    // The burner goes: silent, its loop stopped.
    game.world.blocks.insert([600, -1, 600], 0);
    level.native_block_changed(LevelDimension::Overworld, at([600, -1, 600]), BlockStateId(43), BlockStateId(0));
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l.ends_with("stopsound @a record create:whistle_high")), "{:?}", &game.said[before..]);
    // A creative fluid tank (51) at (620, 0, 600): a water bucket sets it to water, kept; it is
    // saved with its column and drawn as a full tank.
    game.world.blocks.insert([620, 0, 600], 51);
    level.native_block_changed(LevelDimension::Overworld, at([620, 0, 600]), BlockStateId(0), BlockStateId(51));
    let was_creative = std::mem::replace(&mut game.creative, true);
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("minecraft:water_bucket", 1));
    let mut canceled = false;
    game.native_right_click_block([620, 0, 600], [0, 1, 0], &mut canceled);
    assert!(canceled, "the creative tank takes the bucket");
    game.inventory[0] = None;
    game.creative = was_creative;
    game.native_perform_command(&player, "create_debug fluid_state 620 0 600", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("creative fluid tank at 620 0 600: minecraft:water"));
    let before = game.sent.len();
    game.native_level_tick_post("minecraft:overworld");
    let drawn: Vec<_> = game.sent[before..].iter().filter(|m| m.1 == "create:tank")
        .flat_map(|m| create_engine::tank::decode(&m.2)).collect();
    assert!(drawn.iter().any(|(p, m)| *p == [620, 0, 600] && m.is_some_and(|m| m.tank.amount == 8000)), "{drawn:?}");
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [38, 37], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(own)) = data.get(&"create_fluids".into()) else { panic!("no fluid data") };
    assert_eq!(own.get(&"creative_tanks".into()), Some(&horizon_world::nbt::Tag::IntArray(vec![620, 0, 600, 0])));
    // A hose pulley (52) at (630, 5, 600) turned by a motor facing east (3) west of it lowers
    // its hose 1/32 block a tick at 16 RPM through the air to the stone at y 0: 4 blocks down.
    game.world.blocks.insert([630, 0, 600], 1);
    for (pos, state) in [([630, 5, 600], 52), ([629, 5, 600], 3)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, at(pos), BlockStateId(0), BlockStateId(state));
    }
    let before = game.sent.len();
    for _ in 0..140 { game.world.game_time += 1; game.native_level_tick_post("minecraft:overworld"); }
    game.native_perform_command(&player, "create_debug fluid_state 630 5 600", &mut ran);
    assert_eq!(game.said.last().map(String::as_str),
        Some("hose pulley at 630 5 600: offset 4.00, moving false, tank none 0, infinite false"));
    assert!(game.sent[before..].iter().any(|m| m.1 == "create:hose"), "clients see the hose");
    // Still, it ticks no more.
    let before = game.sent.len();
    for _ in 0..20 { game.native_level_tick_post("minecraft:overworld"); }
    assert!(!game.sent[before..].iter().any(|m| m.1 == "create:hose"));
    // A valve (49) at (640, 0, 600) turned forwards by a motor facing east (3) west of it: its
    // pointer reaches open in 20 ticks at 16 RPM (0.05 a tick) and its block is enabled.
    for (pos, state) in [([640, 0, 600], 49), ([639, 0, 600], 3)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, at(pos), BlockStateId(0), BlockStateId(state));
    }
    for _ in 0..30 { game.native_level_tick_post("minecraft:overworld"); }
    game.native_perform_command(&player, "create_debug fluid_state 640 0 600", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("valve at 640 0 600: pointer 1.00 toward 1"));
    assert!(game.said.iter().any(|l| l == "set [640, 0, 600] enabled=true"), "{:?}", game.said);
    // Filters: a list filter set to deny dirt by its screen's command, put into a smart chute's
    // filter slot at (800, 0, 800) (its south side, 11 px up): taken from the hand, it lets stone
    // in and keeps dirt out; an empty hand clears the slot and the filter comes back.
    let item = |id: &str| horizon_client::items::ItemStack { item: id.into(), count: 1, damage: 0, extra: None };
    let chute = [800, 0, 800];
    game.world.blocks.insert(chute, 53);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 800, y: 0, z: 800 }, BlockStateId(0), BlockStateId(53));
    game.inventory = vec![Some(item("create:filter")), None, None, None];
    game.selected = 0;
    let mut ran = false;
    game.native_perform_command(&player, "create_debug filter hand list deny dirt", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("filter: deny list of 1 items"));
    game.aim = Some(chute);
    game.aim_point = Some([800.5, 11.0 / 16.0, 800.0 + 15.5 / 16.0]);
    let mut canceled = false;
    game.native_right_click_block(chute, [0, 0, 1], &mut canceled);
    assert!(canceled, "the filter slot takes the click");
    assert!(game.inventory[0].is_none(), "the filter item left the hand");
    let accepts = |game: &Game<'_>, id: &str| create_logistics::chute_accepts(
        game, 0, chute, create_engine::Block::SmartChute, false, &item(id));
    assert!(accepts(&game, "minecraft:stone") && !accepts(&game, "minecraft:dirt"));
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [50, 50], &mut data);
    assert!(format!("{data:?}").contains("create:filter"), "saved with its column: {data:?}");
    let mut canceled = false;
    game.native_right_click_block(chute, [0, 0, 1], &mut canceled);
    assert!(accepts(&game, "minecraft:dirt"), "cleared");
    assert_eq!(game.inventory.iter().flatten().filter(|s| &*s.item == "create:filter").count(), 1, "given back");
    game.aim = None;
    game.aim_point = None;
    // An attribute filter's command refuses what the port can't read.
    game.inventory[1] = Some(item("create:attribute_filter"));
    game.native_perform_command(&player, "create_debug filter 1 attribute any placeable !has_color=red", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("filter: the has_color attribute is not ported"));
    game.native_perform_command(&player, "create_debug filter 1 attribute all placeable added_by=minecraft", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("filter: whitelist_conj of 2 attributes"));
    let attribute = create_items::filter_of(game.inventory[1].as_ref().unwrap());
    assert!(create_items::passes(&game, Some(&attribute), &item("minecraft:stone")));
    assert!(!create_items::passes(&game, Some(&attribute), &item("minecraft:dirt")));
    // A wand of symmetry in the hotbar, its mirror a plane across x at (810, 0, 810): stone placed
    // at (812, 0, 811) appears at (808, 0, 811); broken there, its image breaks too.
    game.inventory[2] = Some(item("create:wand_of_symmetry"));
    game.native_perform_command(&player, "create_debug symmetry 2 810 0 810 plane 1", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("symmetry: plane at 810 0 810 orientation 1"));
    game.world.blocks.insert([812, 0, 811], 1);
    game.native_place_block(&item("minecraft:stone"), [812, 0, 811], BlockStateId(1));
    assert_eq!(game.world.blocks.get(&[808, 0, 811]), Some(&1), "mirrored");
    assert!(game.world.blocks.get(&[812, 0, 809]).is_none(), "a plane across x only");
    // Settings for the worldshaper and the schedule's stops, by command.
    game.inventory[3] = Some(item("create:handheld_worldshaper"));
    game.native_perform_command(&player, "create_debug worldshaper 3 sphere 2 fill attached block=minecraft:stone", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("worldshaper: sphere [2] fill attached solid with minecraft:stone"));
    game.inventory[0] = Some(item("create:schedule"));
    game.native_perform_command(&player, "create_debug schedule_item hand A@5 B", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("schedule: 2 stops"));
    game.native_perform_command(&player, "create_debug hand_schedule 800 0 800", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("no train at that station"));
    // A cardboard sword hurts no cow; an arthropod is hit as usual.
    game.inventory[0] = Some(item("create:cardboard_sword"));
    let mut canceled = false;
    game.native_attack_entity(9, "minecraft:cow", [0.0; 3], &mut canceled);
    assert!(canceled);
    let mut canceled = false;
    game.native_attack_entity(9, "minecraft:spider", [0.0; 3], &mut canceled);
    assert!(!canceled);
    // Stock logistics: a vault at (999, 0, 1000) holding 64 iron and 10 gold behind a packager
    // facing east at (1000, 0, 1000) with a stock link on top; a ticker at (1002, 0, 1000) and a
    // requester at (1004, 0, 1000) on the link's network "Base"; a gauge on the packager's south
    // face (1000, 0, 1001) restocks it.
    let mut vault = vec![None; 20];
    vault[0] = Some(ItemStack::new("minecraft:iron_ingot", 64));
    vault[1] = Some(ItemStack::new("minecraft:gold_ingot", 10));
    game.world.block_entities.insert((999, 0, 1000), Mod(ModBlockEntity { items: vault }));
    for (pos, state) in [([999, 0, 1000], 42), ([1000, 0, 1000], 36), ([1000, 1, 1000], 54),
                         ([1002, 0, 1000], 55), ([1004, 0, 1000], 56), ([1000, 0, 1001], 57)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] },
            BlockStateId(0), BlockStateId(state));
    }
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "set [1000, 0, 1000] linked=true"), "the packager is linked");
    for pos in ["1000 1 1000", "1002 0 1000", "1004 0 1000", "1000 0 1001"] {
        game.native_perform_command(&player, &format!("create_debug network {pos} Base"), &mut ran);
        assert!(game.said.last().is_some_and(|l| l.starts_with("network ")), "{:?}", game.said.last());
    }
    game.native_perform_command(&player, "create_debug stock 1002 0 1000", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.ends_with("from 1 links: 64 minecraft:iron_ingot, 10 minecraft:gold_ingot"), "{said}");
    // Linked, the packager no longer packs on a pulse.
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 1000, y: 0, z: 1000 }, BlockStateId(36));
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug packages 1000 0 1000", &mut ran);
    assert!(game.said.last().is_some_and(|l| l.contains("held=-")), "{:?}", game.said.last());
    // An order of 70 iron (64 there) and 5 gold to "Home": one box of both, sliding out.
    game.native_perform_command(&player, "create_debug order 1002 0 1000 Home iron_ingot 70 gold_ingot 5", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("ordered 2 stacks to \\"Home\\""));
    game.native_perform_command(&player, "create_debug packages 1000 0 1000", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.contains("animation=20") && said.contains("to \\"Home\\"") && said.contains("queued=0"), "{said}");
    let Some(Mod(m)) = game.world.block_entities.get(&(999, 0, 1000)) else { panic!() };
    assert!(m.items[0].is_none() && m.items[1].as_ref().map(|s| s.count) == Some(5), "64 iron and 5 gold taken");
    // The requester asks for 5 gold on a rising signal: in stock, so a second box is queued, and
    // its comparator reads 15.
    game.native_perform_command(&player, "create_debug requester 1004 0 1000 Home all gold_ingot 5", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("requester to \\"Home\\""));
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 1004, y: 0, z: 1000 }, BlockStateId(56));
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug packages 1000 0 1000", &mut ran);
    assert!(game.said.last().is_some_and(|l| l.contains("queued=1")), "{:?}", game.said.last());
    assert_eq!(comparator(&game, [1004, 0, 1000], 56), 15, "the requester succeeded");
    // The gauge keeps 10 gold in the packager's inventory: none left, and nothing elsewhere in the
    // network to restock from; 20 gold put in, it is satisfied at its next look.
    game.native_perform_command(&player, "create_debug gauge 1000 0 1001 gold_ingot 10 items Home", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("gauge gold_ingot x10 items to \\"Home\\""));
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug stock 1000 0 1001", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.contains("restocker=true satisfied=false") && said.contains("level=0"), "{said}");
    if let Some(Mod(m)) = game.world.block_entities.get_mut(&(999, 0, 1000)) {
        m.items[1] = Some(ItemStack::new("minecraft:gold_ingot", 20));
    }
    game.world.game_time += 100;
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug stock 1000 0 1001", &mut ran);
    let said = game.said.last().cloned().unwrap_or_default();
    assert!(said.contains("satisfied=true") && said.contains("level=20"), "{said}");
    // Saved with their column.
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [62, 62], &mut data);
    let Some(horizon_world::nbt::Tag::Compound(saved)) = data.get(&"create_stock".into()) else { panic!("{data:?}") };
    for key in ["links", "tickers", "requesters", "gauges"] {
        assert!(matches!(saved.get(&key.into()), Some(horizon_world::nbt::Tag::List(l)) if l.len() == 1), "{key}");
    }
    // A nozzle on the front of a fan facing east at (1101, 0, 1100), its motor behind: placed
    // there from its item it stays (anywhere else it is refused); a zombie 1.75 blocks east of it is
    // caught at its search (every sixth tick) and pushed or pulled along x only, its fall
    // forgotten. With the fan gone the nozzle breaks.
    for (pos, state) in [([1100, 0, 1100], 3), ([1101, 0, 1100], 19)] {
        game.world.blocks.insert(pos, state);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: pos[0], y: pos[1], z: pos[2] }, BlockStateId(0), BlockStateId(state));
    }
    game.keys.clear();
    game.selected = 0;
    game.hand = horizon_client::first_person::Hand::Main;
    let nozzle = ItemStack::new("create:nozzle", 1);
    let mut claimed = false;
    game.native_use_item_on(&nozzle, [1101, 0, 1130], [1, 0, 0], &mut claimed);
    game.world.blocks.insert([1102, 0, 1130], 63);
    game.native_place_block(&nozzle, [1102, 0, 1130], BlockStateId(63));
    assert_eq!(game.world.blocks.get(&[1102, 0, 1130]), Some(&0), "no fan behind: refused");
    let mut claimed = false;
    game.native_use_item_on(&nozzle, [1101, 0, 1100], [1, 0, 0], &mut claimed);
    game.world.blocks.insert([1102, 0, 1100], 63);
    game.native_place_block(&nozzle, [1102, 0, 1100], BlockStateId(63));
    assert_eq!(game.world.blocks.get(&[1102, 0, 1100]), Some(&63), "on the fan's front");
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1102, y: 0, z: 1100 }, BlockStateId(0), BlockStateId(63));
    ENTITIES.with_borrow_mut(|e| e.push(fixture_entity(41, "minecraft:zombie", [1104.25, 0.5, 1100.5])));
    for _ in 0..8 { game.native_level_tick_post("minecraft:overworld"); }
    let pushed = ENTITIES.with_borrow(|e| e[0].velocity);
    assert!(pushed[0] != 0.0 && pushed[1] == 0.0 && pushed[2] == 0.0, "{pushed:?}");
    assert!(FALLS_RESET.with_borrow(|f| f.contains(&41)));
    ENTITIES.with_borrow_mut(Vec::clear);
    game.world.blocks.insert([1101, 0, 1100], 1);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1101, y: 0, z: 1100 }, BlockStateId(19), BlockStateId(1));
    level.native_neighbor_changed(LevelDimension::Overworld, BlockPos { x: 1102, y: 0, z: 1100 }, BlockStateId(63));
    game.native_level_tick_post("minecraft:overworld");
    assert!(game.said.iter().any(|l| l == "execute in minecraft:overworld run setblock 1102 0 1100 air destroy"));
    // The belt connector: shafts along x at (1100, 0, 1110), (1100, 0, 1112) and (1100, 0, 1113);
    // clicked at the first and the last, a belt runs south between them (its middle shaft a
    // pulley) and one connector is used. Shafts in a line along their own axis refuse.
    for z in [1110, 1112, 1113, 1120] {
        game.world.blocks.insert([1100, 0, z], 2);
        level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1100, y: 0, z }, BlockStateId(0), BlockStateId(2));
    }
    game.world.blocks.insert([1105, 0, 1120], 2);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1105, y: 0, z: 1120 }, BlockStateId(0), BlockStateId(2));
    let connector = ItemStack::new("create:belt_connector", 2);
    game.inventory[0] = Some(connector.clone());
    let mut claimed = false;
    game.native_use_item_on(&connector, [1100, 0, 1110], [0, 1, 0], &mut claimed);
    assert!(claimed);
    let mut claimed = false;
    game.native_use_item_on(&connector, [1100, 0, 1113], [0, 1, 0], &mut claimed);
    let row: Vec<u32> = (1110..=1113).map(|z| game.world.blocks.get(&[1100, 0, z]).copied().unwrap_or(0)).collect();
    assert_eq!(row, vec![6, 7, 64, 8], "start, middle, pulley, end");
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(1));
    let mut claimed = false;
    game.native_use_item_on(&connector, [1100, 0, 1120], [0, 1, 0], &mut claimed);
    let mut claimed = false;
    game.native_use_item_on(&connector, [1105, 0, 1120], [0, 1, 0], &mut claimed);
    assert_eq!(game.world.blocks.get(&[1102, 0, 1120]), None, "along their axis: no belt");
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(1));
    // Linked controllers. Messages between the client and server halves on `create:controller`
    // are handed over as the network would (to player 7, and from the client as player 7).
    fn relay(game: &mut Game<'_>, from: usize, to_server: bool) {
        let who = if to_server { 0 } else { 7 };
        let messages: Vec<Vec<u8>> = game.sent[from..].iter()
            .filter(|m| m.0 == who && m.1 == "create:controller").map(|m| m.2.clone()).collect();
        for m in messages {
            if to_server { game.native_net_from_client(7, "create:controller", &m); }
            else { game.native_net_from_server("create:controller", &m); }
        }
    }
    // The client's tick, its packets to the server, the player's tick, the level's tick.
    fn controller_tick(game: &mut Game<'_>) {
        let from = game.sent.len();
        game.native_tick();
        relay(game, from, true);
        game.native_player_tick();
        game.native_level_tick_post("minecraft:overworld");
    }
    let key = |game: &mut Game<'_>, name: &str, action: horizon_client::debug_keys::KeyAction| {
        KEYS_DOWN.with_borrow_mut(|k| {
            if action == horizon_client::debug_keys::KeyAction::Release { k.remove(name); } else { k.insert(name.to_owned()); }
        });
        let mut consumed = false;
        game.native_input_key(name, action, 0, false, &mut consumed);
        consumed
    };
    use horizon_client::debug_keys::KeyAction::{Press, Release};
    // What the receiver gives (its block here stays as placed; its signal is the network's).
    let rx = |level: &GameLevel<'_>| {
        let mut signal = 0;
        level.native_block_signal(LevelDimension::Overworld, BlockPos { x: 1200, y: 0, z: 1200 }, BlockStateId(21),
            horizon_core::geometry::Direction::East, false, &mut signal);
        signal
    };
    // A receiving redstone link facing up at (1200, 0, 1200), tuned to the iron ingot by a click on
    // its first slot.
    game.keys.clear();
    game.selected = 0;
    game.hand = horizon_client::first_person::Hand::Main;
    game.world.blocks.insert([1200, -1, 1200], 1);
    game.world.blocks.insert([1200, 0, 1200], 21);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1200, y: 0, z: 1200 }, BlockStateId(0), BlockStateId(21));
    game.native_level_tick_post("minecraft:overworld");
    game.inventory[0] = Some(ItemStack::new("minecraft:iron_ingot", 1));
    game.aim = Some([1200, 0, 1200]);
    game.aim_point = Some([1200.5, 3.01 / 16.0, 1200.0 + 10.5 / 16.0]);
    let mut canceled = false;
    game.native_right_click_block([1200, 0, 1200], [0, 1, 0], &mut canceled);
    assert!(canceled, "the link's slot took the iron");
    game.aim_point = None;
    // The controller used on the link: its client binds the next button pressed, W (consumed),
    // to the link's frequency, in the player's tick.
    let controller = ItemStack::new("create:linked_controller", 1);
    game.inventory[0] = Some(controller.clone());
    let from = game.sent.len();
    let mut claimed = false;
    game.native_use_item_on(&controller, [1200, 0, 1200], [0, 1, 0], &mut claimed);
    assert!(claimed);
    relay(&mut game, from, false);
    assert!(key(&mut game, "key.keyboard.w", Press), "bind mode takes W");
    let mark = game.said.len();
    controller_tick(&mut game);
    assert!(game.said[mark..].iter().any(|l| l == "create.linked_controller.key_bound"), "bound: {:?}", &game.said[mark..]);
    assert!(!key(&mut game, "key.keyboard.w", Release));
    // The screen's stand-in: space (button 4) on the gold ingot.
    let mut ran = false;
    game.native_perform_command(&player, "create_debug controller 4 gold_ingot", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("controller button 4: minecraft:gold_ingot + "));
    // Its use (in the air) toggles it on the client. W is consumed, E is not; W held powers the
    // receiver (the player within the links' 256 blocks), renewed every 5 ticks past the 30-tick
    // timeout; released, it is off a tick later.
    PLAYER_AT.set([1203.5, 0.0, 1203.5]);
    game.world.game_time += 20;
    let from = game.sent.len();
    let mut claimed = false;
    game.native_use_item(&controller, &mut claimed);
    relay(&mut game, from, false);
    assert!(key(&mut game, "key.keyboard.w", Press), "the active controller takes W");
    assert!(!key(&mut game, "key.keyboard.e", Press), "and leaves E");
    let mark = game.said.len();
    controller_tick(&mut game);
    assert!(game.said[mark..].iter().any(|l| l == "set [1200, 0, 1200] powered=true"), "held: {:?}", &game.said[mark..]);
    let mark = game.said.len();
    assert_eq!(rx(&level), 15);
    for _ in 0..40 { controller_tick(&mut game); }
    assert_eq!(rx(&level), 15, "held");
    assert!(!key(&mut game, "key.keyboard.w", Release));
    controller_tick(&mut game);
    assert_eq!(rx(&level), 0, "released");
    // Toggled off: W reaches the game again.
    let from = game.sent.len();
    game.native_use_item(&controller, &mut claimed);
    relay(&mut game, from, false);
    assert!(!key(&mut game, "key.keyboard.w", Press));
    key(&mut game, "key.keyboard.w", Release);
    // The controller on a lectern (1210, 0, 1200): a lectern controller, read by comparators as
    // 15. Used by a player at it, W powers the receiver from the lectern's controller; walking
    // away stops it; sneaking takes the lectern's controller off (dropped) and leaves a lectern.
    game.world.blocks.insert([1210, 0, 1200], 83);
    let mut claimed = false;
    game.native_use_item_on(&controller, [1210, 0, 1200], [0, 1, 0], &mut claimed);
    assert!(claimed);
    assert_eq!(game.world.blocks.get(&[1210, 0, 1200]), Some(&65));
    assert!(game.inventory[0].is_none(), "the controller went on the lectern");
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1210, y: 0, z: 1200 }, BlockStateId(83), BlockStateId(65));
    assert_eq!(comparator(&game, [1210, 0, 1200], 65), 15);
    PLAYER_AT.set([1210.5, 0.0, 1201.2]);
    let from = game.sent.len();
    let mut canceled = false;
    game.native_right_click_block([1210, 0, 1200], [0, 1, 0], &mut canceled);
    assert!(canceled, "the lectern is used");
    relay(&mut game, from, false);
    assert!(key(&mut game, "key.keyboard.w", Press));
    let mark = game.said.len();
    controller_tick(&mut game);
    assert!(game.said[mark..].iter().any(|l| l == "set [1200, 0, 1200] powered=true"), "lectern: {:?}", &game.said[mark..]);
    assert_eq!(rx(&level), 15);
    key(&mut game, "key.keyboard.w", Release);
    controller_tick(&mut game);
    assert_eq!(rx(&level), 0);
    PLAYER_AT.set([1230.5, 0.0, 1200.5]);
    let from = game.sent.len();
    game.native_level_tick_post("minecraft:overworld");
    relay(&mut game, from, false);
    assert!(game.sent[from..].iter().any(|m| m.0 == 7 && m.1 == "create:controller"), "the user left");
    assert!(!key(&mut game, "key.keyboard.w", Press), "the client is idle");
    key(&mut game, "key.keyboard.w", Release);
    PLAYER_AT.set([1210.5, 0.0, 1201.2]);
    game.keys.insert(winit::keyboard::KeyCode::ShiftLeft);
    let mut canceled = false;
    game.native_right_click_block([1210, 0, 1200], [0, 1, 0], &mut canceled);
    game.keys.clear();
    assert!(canceled);
    assert_eq!(game.world.blocks.get(&[1210, 0, 1200]), Some(&83), "a lectern again");
    assert!(create_contraption_body::DROPPED.with_borrow(|d| d.iter().any(|x| x.1 == "create:linked_controller")));
    PLAYER_AT.set([0.5, 2.0, 0.5]);
    seats_turntables_fans_and_rails(&mut game, &level);
    minecart_contraptions(&mut game, &level);
    train_controls(&mut game);
    // Logistics networks across restarts: the world's save writes network "Base" (its link at
    // (1000, 1, 1000), owned by the player who linked it) to `data/create/logistics_networks.dat`
    // once; unchanged, a later save writes nothing. Forgotten and read back as after a restart,
    // its link counts as unloaded; a second link joining it saves both, the owner kept.
    use horizon_world::nbt::Tag;
    let networks = |game: &Game<'_>| -> Vec<horizon_world::nbt::Compound> {
        match game.saved.get("create/logistics_networks").and_then(|d| d.get(&"LogisticsNetworks".into())) {
            Some(Tag::List(l)) => l.iter().filter_map(|t| match t { Tag::Compound(c) => Some(c.clone()), _ => None }).collect(),
            _ => Vec::new(),
        }
    };
    let links = |c: &horizon_world::nbt::Compound| match c.get(&"Links".into()) { Some(Tag::List(l)) => l.len(), _ => 0 };
    game.native_level_save("minecraft:overworld");
    assert_eq!(game.saved_dirty, vec!["create/logistics_networks".to_owned()]);
    let saved = networks(&game);
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(links(&saved[0]), 1);
    assert_eq!(saved[0].get(&"Owner".into()), Some(&Tag::IntArray(vec![0, 0, 0, 7])));
    game.native_level_save("minecraft:overworld");
    assert_eq!(game.saved_dirty.len(), 1, "unchanged: not written");
    create_stock::forget();
    game.native_level_load("minecraft:overworld");
    game.native_level_save("minecraft:overworld");
    assert_eq!(game.saved_dirty.len(), 1, "read back: unchanged");
    game.world.blocks.insert([1300, 1, 1300], 54);
    level.native_block_changed(LevelDimension::Overworld, BlockPos { x: 1300, y: 1, z: 1300 }, BlockStateId(0), BlockStateId(54));
    game.native_level_tick_post("minecraft:overworld");
    game.native_perform_command(&player, "create_debug network 1300 1 1300 Base", &mut ran);
    assert!(game.said.last().is_some_and(|l| l.starts_with("network ")), "{:?}", game.said.last());
    game.native_level_save("minecraft:overworld");
    assert_eq!(game.saved_dirty.len(), 2);
    let saved = networks(&game);
    assert_eq!(saved.len(), 1);
    assert_eq!(links(&saved[0]), 2, "the unloaded link and the new one");
    assert_eq!(saved[0].get(&"Owner".into()), Some(&Tag::IntArray(vec![0, 0, 0, 7])));
    // Copycats. A panel placed at (1320, 0, 1320) looking down faces up and shows the copycat
    // base cut to its 3 pixels; stone used on it becomes its material (one used), drawn on its
    // client cut to the same box; stone again turns nothing and passes; a depot (a block
    // entity) is refused. The wrench takes the stone back; stone again, then the column saves
    // it. A step placed at (1322, 0, 1320) facing south, clicked high on a side, takes the upper
    // half and shows the base in its upper south quarter. The panel broken drops its stone and
    // its client draws the block again.
    let copycat_tick = |game: &mut Game<'_>| {
        let from = game.sent.len();
        game.native_level_tick_post("minecraft:overworld");
        let messages: Vec<Vec<u8>> = game.sent[from..].iter()
            .filter(|m| m.0 == 7 && m.1 == "create:copycat").map(|m| m.2.clone()).collect();
        for m in &messages { game.native_net_from_server("create:copycat", m); }
        messages.len()
    };
    let panel = [1320, 0, 1320];
    let panel_box = vec![[[0.0f32, 0.0, 0.0], [1.0, 3.0 / 16.0, 1.0]]];
    (game.creative, game.hand, game.offhand, game.pitch, game.yaw) =
        (false, horizon_client::first_person::Hand::Main, None, 90.0, 0.0);
    game.keys.clear();
    game.world.blocks.insert(panel, 71);
    let panel_item = horizon_client::items::ItemStack::new("create:copycat_panel", 1);
    game.native_place_block(&panel_item, panel, BlockStateId(71));
    level.native_block_changed(LevelDimension::Overworld, at(panel), BlockStateId(0), BlockStateId(71));
    assert_eq!(copycat_tick(&mut game), 1);
    let drawn = OVERRIDES.with_borrow(|o| o.get(&panel).cloned()).expect("the base is drawn");
    assert_eq!((drawn.state, &drawn.boxes), (BlockStateId(75), &panel_box));
    assert_eq!(copycat_tick(&mut game), 0, "nothing more to send");
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("minecraft:stone", 5));
    game.aim = Some(panel);
    game.aim_point = Some([1320.5, 3.0 / 16.0, 1320.5]);
    let mut canceled = false;
    game.native_right_click_block(panel, [0, 1, 0], &mut canceled);
    assert!(canceled, "the panel takes stone");
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(4));
    copycat_tick(&mut game);
    let drawn = OVERRIDES.with_borrow(|o| o.get(&panel).cloned()).unwrap();
    assert_eq!((drawn.state, &drawn.boxes), (BlockStateId(1), &panel_box));
    game.native_perform_command(&player, "create_debug copycat 1320 0 1320", &mut ran);
    assert_eq!(game.said.last().map(String::as_str), Some("copycat at 1320 0 1320: minecraft:stone (minecraft:stone)"));
    let mut canceled = false;
    game.native_right_click_block(panel, [0, 1, 0], &mut canceled);
    assert!(!canceled, "stone again: stone has nothing to turn");
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(4));
    let step = [1322, 0, 1320];
    let step_item = horizon_client::items::ItemStack::new("create:copycat_step", 1);
    game.yaw = 0.0;
    game.aim = Some([1322, 0, 1319]);
    game.aim_point = Some([1322.5, 0.75, 1320.0]);
    let mut claimed = false;
    game.native_use_item_on(&step_item, [1322, 0, 1319], [0, 0, 1], &mut claimed);
    assert!(!claimed);
    game.world.blocks.insert(step, 73);
    game.native_place_block(&step_item, step, BlockStateId(73));
    assert_eq!(game.world.blocks.get(&step), Some(&74), "facing south, high on a side: the upper half");
    level.native_block_changed(LevelDimension::Overworld, at(step), BlockStateId(0), BlockStateId(74));
    copycat_tick(&mut game);
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("create:depot", 1));
    let mut canceled = false;
    game.native_right_click_block(step, [0, 1, 0], &mut canceled);
    assert!(!canceled, "a block entity is refused");
    let drawn = OVERRIDES.with_borrow(|o| o.get(&step).cloned()).unwrap();
    assert_eq!((drawn.state, drawn.boxes), (BlockStateId(75), vec![[[0.0, 0.5, 0.5], [1.0, 1.0, 1.0]]]));
    game.inventory[0] = Some(horizon_client::items::ItemStack::new("minecraft:stone", 4));
    let wrench = horizon_client::items::ItemStack::new("create:wrench", 1);
    let mut claimed = false;
    game.native_use_item_on(&wrench, panel, [0, 1, 0], &mut claimed);
    assert!(claimed);
    assert_eq!(game.inventory[0].as_ref().map(|s| s.count), Some(5), "the stone is back");
    assert_eq!(game.world.blocks.get(&panel), Some(&71), "a wrench never turns a copycat");
    copycat_tick(&mut game);
    assert_eq!(OVERRIDES.with_borrow(|o| o.get(&panel).map(|m| m.state)), Some(BlockStateId(75)));
    let mut canceled = false;
    game.native_right_click_block(panel, [0, 1, 0], &mut canceled);
    assert!(canceled);
    let mut data = horizon_world::nbt::Compound::new();
    game.native_chunk_data_save("minecraft:overworld", [1320 >> 4, 1320 >> 4], &mut data);
    let saved = match data.get(&"create_copycat".into()) { Some(horizon_world::nbt::Tag::Compound(c)) => c.clone(), other => panic!("{other:?}") };
    assert_eq!(saved.get(&"Pos".into()), Some(&horizon_world::nbt::Tag::IntArray(vec![1320, 0, 1320, 1322, 0, 1320])));
    assert_eq!(saved.get(&"Materials".into()), Some(&horizon_world::nbt::Tag::List(vec![
        horizon_world::nbt::Tag::String("minecraft:stone".into()), horizon_world::nbt::Tag::String("".into())])));
    game.world.blocks.remove(&panel);
    level.native_block_changed(LevelDimension::Overworld, at(panel), BlockStateId(71), BlockStateId(0));
    copycat_tick(&mut game);
    assert!(create_contraption_body::DROPPED.with_borrow(|d| d.iter().any(|x| x.1 == "minecraft:stone" && x.0 == [1320.5, 0.5, 1320.5])));
    assert!(OVERRIDES.with_borrow(|o| !o.contains_key(&panel)), "its own model again");
    (game.aim, game.aim_point, game.pitch, game.yaw) = (None, None, 0.0, 180.0);
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


# The argument kinds Horizon's native commands take (`native_server.rs`, `fn parser`): a usage
# with any other kind is left out at load, with only a warning in the log.
COMMAND_KINDS = {'word', 'text', 'integer', 'number', 'boolean', 'player', 'item', 'block_pos'}


def command_kinds(root):
    bad = []
    for path in sorted((root/'mixins/create').glob('*.rs')):
        for usage in re.findall(r'"create_debug[^"]*"|"create[a-z_]*\s[^"]*<[^"]*"', path.read_text()):
            for kind in re.findall(r'[<\[][a-z_]+:([a-z_]+)[>\]]', usage):
                if kind not in COMMAND_KINDS:
                    bad.append(f'{path.name}: {usage}: {kind}')
    assert not bad, bad


def main():
    root = horizon.ROOT
    command_kinds(root)
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
        # Modules that reach deep into Horizon's internals are compiled against Horizon itself
        # (`horizon.py check`); here they are swapped for stand-ins with the same functions.
        mods = scratch/'mixins'
        shutil.copytree(root/'mixins', mods, ignore=shutil.ignore_patterns('target'))
        for standin in (root/'tools/fixture').glob('*.rs'):
            shutil.copy(standin, mods/'create'/standin.name)
        base = ['--workspace', str(original), '--mods', str(mods)]
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
