#!/usr/bin/env python3
"""Smoke-test a staged Create package on the signed Sunrise Nightly.

Run under Xvfb with the matching sunrise-ctl, private vanilla resource tree, and
verified jar directory. --refusal-fixture additionally expects a disposable build
whose entity_join_level hook vetoes create:potato_projectile and logs the veto.
"""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile


def run(args, scratch):
    world = scratch / "game"
    world.mkdir()
    (world / "eula.txt").write_text("eula=true\n")
    shutil.copytree(args.jars, scratch / "home/jars")
    socket = scratch / "control.sock"
    env = dict(os.environ, SUNRISE_HOME=str(scratch / "home"), SUNRISE_CONTROL=str(socket))

    def ctl(*words):
        out = subprocess.run(
            [args.ctl, "--socket", socket, "--timeout", "45", *words],
            cwd=world, env=env, capture_output=True, text=True, timeout=51,
        )
        result = json.loads(out.stdout)
        assert out.returncode == 0 and result.get("ok") is not False, (words, result)
        return result

    def cmd(command):
        result = ctl("command", command)
        print(command, result["chat"], flush=True)
        return result

    def health(tag, expected):
        line = cmd(f"data get entity @e[tag={tag},limit=1] Health")["chat"][0]
        assert float(line.rsplit(": ", 1)[-1].removesuffix("f")) == expected, line

    def load(ammo):
        assert cmd(f"item replace entity @s hotbar.1 with {ammo} 10")["success"]

    pid = ctl("launch", "--game", args.game, "--game-dir", world,
              "--resources", args.resources, "--test-terrain",
              "--render-distance", "4", "--width", "800", "--height", "600")["pid"]
    with os.fdopen(os.pidfd_open(pid), "rb") as game:
        try:
            ctl("open", "PotatoSmoke", "0", "creative")
            ctl("wait-ready")
            cmd("tp @s 9.5 76 8.5 0 0")
            cmd("fill 4 76 6 16 84 27 minecraft:air")
            cmd("fill 4 75 6 16 75 27 minecraft:stone")
            cmd("gamerule spawn_mobs false")
            cmd("gamemode survival @s")
            cmd("item replace entity @s hotbar.0 with create:potato_cannon")
            load("minecraft:potato")
            ctl("select", "1")
            ctl("look", "0", "0")
            cmd('summon minecraft:pig 8.9 76 16 {NoAI:1b,NoGravity:1b,Tags:["shot_target"]}')
            health("shot_target", 10)
            ctl("wait", "12")
            ctl("use-item")
            ctl("wait", "35")
            if args.refusal_fixture:
                health("shot_target", 10)
                assert cmd("clear @s minecraft:potato 0")["result"] == 10
                selected = cmd("data get entity @s SelectedItem")
                assert selected["success"] and "minecraft:damage" not in selected["chat"][0]
            else:
                health("shot_target", 5)
                assert cmd("clear @s minecraft:potato 0")["result"] == 9
                assert cmd('data get entity @s SelectedItem.components."minecraft:damage"')["result"] == 1
                cmd("kill @e[tag=shot_target]")
                cmd("fill 7 75 9 12 75 24 minecraft:farmland[moisture=7]")
                ctl("look", "0", "12")
                ctl("use-item")
                ctl("wait", "35")
                assert cmd("fill 7 76 9 12 76 24 minecraft:air replace minecraft:potatoes")["result"] == 1
                assert cmd("clear @s minecraft:potato 0")["result"] == 8
                assert cmd("item replace entity @s weapon.offhand with minecraft:cake")["success"]
                ctl("look", "0", "0")
                cmd('summon minecraft:pig 8.9 76 14 {NoAI:1b,NoGravity:1b,Tags:["sticky_target"]}')
                ctl("use-item")
                ctl("wait", "40")
                health("sticky_target", 2)
                assert cmd("clear @s minecraft:cake 0")["result"] == 0
                assert cmd("clear @s minecraft:potato 0")["result"] == 8
        finally:
            ctl("quit")
            assert select.select([game], [], [], 20)[0]
    if args.refusal_fixture:
        assert "[potato check] join refused" in (scratch / "control.log").read_text()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ctl", "game", "resources", "jars", "scratch"):
        parser.add_argument(f"--{name}", type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument("--refusal-fixture", action="store_true")
    args = parser.parse_args()
    args.scratch.mkdir(parents=True, exist_ok=True)
    run(args, Path(tempfile.mkdtemp(prefix="potato-projectile-", dir=args.scratch)))


if __name__ == "__main__":
    main()
