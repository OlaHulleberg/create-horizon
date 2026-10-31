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
    """A two-crate stand-in for Horizon with the hook points, types and paths the bundle uses."""
    files = {
        'Cargo.toml': '[workspace]\nmembers = ["crates/horizon-world", "crates/horizon-client"]\nresolver = "2"\n',
        'crates/horizon-world/Cargo.toml': '[package]\nname = "horizon-world"\nversion = "0.1.0"\nedition = "2024"\n',
        'crates/horizon-world/src/lib.rs': '''
pub mod registry {
    #[derive(Clone, Copy)] pub struct BlockStateId(pub u32);
    pub struct BlockState { pub name: String }
    pub struct BlockRegistry { pub states: Vec<BlockState> }
    impl BlockRegistry {
        pub fn len(&self) -> usize { self.states.len() }
        pub fn is_empty(&self) -> bool { self.states.is_empty() }
        pub fn state(&self, id: BlockStateId) -> Option<&BlockState> { self.states.get(id.0 as usize) }
    }
}
''',
        'crates/horizon-client/Cargo.toml': ('[package]\nname = "horizon-client"\nversion = "0.1.0"\nedition = "2024"\n'
                                             '[dependencies]\nhorizon-world = { path = "../horizon-world" }\n'),
        'crates/horizon-client/src/lib.rs': 'pub mod game;\n',
        'crates/horizon-client/src/game.rs': '''
use horizon_world::registry::BlockRegistry;
pub mod native_lifecycle {
    pub enum WorldPhase { WorldOpenReady, WorldOpenFailed, WorldCloseFinal, WorldCloseFailed }
}
pub mod native_hooks {
    use super::{Game, native_lifecycle::WorldPhase};
    impl Game<'_> {
        pub fn native_world_lifecycle(&mut self, phase: WorldPhase, error_text: &str) {
            let _ = (phase, error_text);
            *self.calls += 1;
        }
    }
}
pub struct Assets { pub registry: &'static BlockRegistry }
pub struct Pack { pub assets: Assets }
pub struct Game<'a> { pub pack: Pack, pub calls: &'a mut u32 }
#[test] fn world_open_finds_create_states_and_close_forgets_them() {
    use horizon_world::registry::BlockState;
    let names = ["minecraft:air", "minecraft:stone", "create:shaft", "create:shaft"];
    let registry = Box::leak(Box::new(BlockRegistry {
        states: names.iter().map(|n| BlockState { name: n.to_string() }).collect(),
    }));
    let mut calls = 0;
    let mut game = Game { pack: Pack { assets: Assets { registry } }, calls: &mut calls };
    game.native_world_lifecycle(native_lifecycle::WorldPhase::WorldOpenReady, "");
    assert_eq!(create_engine::states::index(2), Some(0));
    assert_eq!(create_engine::states::index(1), None);
    game.native_world_lifecycle(native_lifecycle::WorldPhase::WorldCloseFinal, "");
    assert_eq!(create_engine::states::index(2), None);
    assert_eq!(calls, 2, "the original body still runs");
}
''',
        'native-hooks.json': json.dumps({'hooks': [{'name': 'world_lifecycle', 'version': 1,
            'target': "horizon_client::game::native_hooks::Game<'_>::native_world_lifecycle"}]}),
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
