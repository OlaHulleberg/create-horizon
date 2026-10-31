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
''',
        'crates/horizon-world/Cargo.toml': '[package]\nname = "horizon-world"\nversion = "0.1.0"\nedition = "2024"\n',
        'crates/horizon-world/src/lib.rs': '''
pub mod nbt {
    #[derive(Clone, PartialEq, Debug)] pub enum Tag { IntArray(Vec<i32>), Compound(Compound) }
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct JavaString(String);
    impl From<&str> for JavaString { fn from(s: &str) -> Self { JavaString(s.to_owned()) } }
    pub type Compound = std::collections::BTreeMap<JavaString, Tag>;
}
pub mod level {
    #[derive(Clone, Copy)] pub enum Dimension { Overworld, Nether, End }
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
                                             'winit = { path = "../winit" }\n'),
        'crates/horizon-client/src/lib.rs': '''
// The game's module reaches the lib as `horizon_client`, as Horizon's binary does.
extern crate self as horizon_client;
pub mod game;
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
    pub mod flags { pub const UPDATE_CLIENTS: i32 = 2; }
    use native_lifecycle::WorldPhase;
    use native_server::{NativeCommand, NativeServerPlayer};
    impl GameLevel<'_> {
        pub fn native_block_changed(&self, dimension: LevelDimension, pos: BlockPos, old: BlockStateId, new: BlockStateId) {
            let _ = (dimension, pos, old, new);
        }
        pub fn native_neighbor_changed(&self, dimension: LevelDimension, pos: BlockPos, state: BlockStateId) {
            let _ = (dimension, pos, state);
        }
    }
    impl Game<'_> {
        pub fn native_right_click_block(&mut self, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
            let _ = (pos, face, canceled);
        }
        pub fn spectator(&self) -> bool { false }
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

        pub fn native_feedback(&mut self, line: horizon_client::text::Text) { self.said.push(line.0); }
        pub fn native_run_command(&mut self, line: &str) -> bool { self.said.push(line.to_owned()); true }
    }
}
pub struct Assets { pub registry: &'static BlockRegistry }
pub struct Pack { pub assets: Assets }
pub struct World { pub blocks: std::collections::HashMap<[i32; 3], u32>, pub game_time: i64 }
impl World {
    pub fn block(&self, x: i32, y: i32, z: i32) -> BlockStateId { BlockStateId(*self.blocks.get(&[x, y, z]).unwrap_or(&0)) }
}
pub struct Hit { pub pos: [i32; 3] }
pub struct Player { pub exhaustion: f32 }
impl Player { pub fn cause_food_exhaustion(&mut self, amount: f32) { self.exhaustion += amount; } }
pub struct Game<'a> {
    pub pack: Pack, pub calls: &'a mut u32, pub said: Vec<String>, pub world: World,
    pub sent: Vec<(u128, String, Vec<u8>)>, pub parts: Vec<(u32, [i32; 3], u8, f32)>,
    pub aim: Option<[i32; 3]>, pub unsaved: Vec<[i32; 2]>,
    pub creative: bool, pub player: Option<Player>, pub keys: std::collections::HashSet<winit::keyboard::KeyCode>,
}
pub struct GameLevel<'a> { pub registry: &'a BlockRegistry }
pub struct Behaviours;
pub struct Ticks;
impl Behaviours {
    pub fn has_neighbor_signal(&self, level: &mut GameLevel<'_>, ticks: &mut Ticks, pos: BlockPos) -> bool {
        let _ = (level, ticks, pos);
        true
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
    // 2: shaft along x; 3: creative motor facing east.
    states.push(BlockState { name: "create:shaft".into(), properties: props("axis", "x") });
    states.push(BlockState { name: "create:creative_motor".into(), properties: props("facing", "east") });
    let registry = Box::leak(Box::new(BlockRegistry { states }));
    let mut calls = 0;
    let mut game = Game { pack: Pack { assets: Assets { registry } }, calls: &mut calls, said: vec![],
        world: World { blocks: Default::default(), game_time: 0 }, sent: vec![], parts: vec![], aim: None, unsaved: vec![],
        creative: false, player: Some(Player { exhaustion: 0.0 }), keys: Default::default() };
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
    assert_eq!(commands.len(), 1);
    let player = game.native_acting_player().unwrap();
    let mut ran = false;
    game.native_perform_command(&player, "create_debug speed 2 0 0", &mut ran);
    assert!(ran);
    assert_eq!(game.said.last().map(String::as_str), Some("create speed at 2 0 0: 17 rpm"), "after the scroll");
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
