#!/usr/bin/env python3
"""Smoke-test a staged Create package on the signed Sunrise Nightly.

Run under Xvfb with the matching sunrise-ctl, private vanilla resource tree, and
verified jar directory. --fixture runs the extra cases on a disposable build with
tools/fixtures/potato-projectile.patch applied.
"""

import argparse
import ast
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import struct
import tempfile


def run(args, scratch, scenario):
    world = scratch / "game"
    world.mkdir()
    (world / "eula.txt").write_text("eula=true\n")
    shutil.copytree(args.jars, scratch / "home/jars")
    socket = scratch / "control.sock"
    env = dict(os.environ, SUNRISE_HOME=str(scratch / "home"), SUNRISE_CONTROL=str(socket))

    if scenario == "veto":
        env["POTATO_TEST_REFUSAL"] = "1"
    if scenario in {"swap", "ammo_swap"}:
        env["POTATO_TEST_SWAP"] = "1"

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

    def open_world():
        pid = ctl("launch", "--game", args.game, "--game-dir", world,
                  "--resources", args.resources, "--test-terrain",
                  "--render-distance", "4", "--width", "800", "--height", "600")["pid"]
        game = os.pidfd_open(pid)
        ctl("open", "PotatoSmoke", "0", "creative")
        ctl("wait-ready")
        return game

    def stop(game):
        ctl("quit")
        assert select.select([game], [], [], 20)[0]
        os.close(game)

    def log():
        return (scratch / "control.log").read_text()

    def samples(trace, uuid=None):
        rows = []
        for shot, at, velocity, owner, age, stuck in re.findall(
            r"\[flight\] server \d+ Some\((\d+)\) from=(\[[^\n]+?\]) velocity=(\[[^\n]+?\]) "
            r"owner=Some\((\d+)\) age (\d+) stuck (None|Some)", trace
        ):
            if uuid is None or shot == uuid:
                rows.append((shot, ast.literal_eval(at), ast.literal_eval(velocity), owner, int(age), stuck == "Some"))
        return rows

    game = open_world()
    try:
        cmd("tp @s 9.5 76 8.5 0 0")
        cmd("fill 4 76 6 16 88 40 minecraft:air")
        cmd("fill 4 75 6 16 75 40 minecraft:stone")
        cmd("gamerule spawn_mobs false")
        cmd("gamemode survival @s")
        cmd("item replace entity @s hotbar.0 with create:potato_cannon")
        load("minecraft:potato")
        ctl("select", "1")
        ctl("look", "0", "0")
        if scenario == "flight":
            cmd("tick rate 4")
            ctl("look", "0", "-14")
            ctl("use-item")
            ctl("wait", "12")
            cmd("tick freeze")
            ctl("save")
        else:
            if scenario == "swap":
                cmd("item replace entity @s hotbar.2 with minecraft:iron_pickaxe")
            if scenario == "ammo_swap":
                cmd("item replace entity @s weapon.offhand with create:potato_cannon[minecraft:damage=7]")
                assert cmd("execute if items entity @s weapon.offhand create:potato_cannon[minecraft:damage=7]")["result"] == 1
                cmd("item replace entity @s hotbar.0 with minecraft:potato 10")
                cmd("item replace entity @s hotbar.1 with minecraft:air")
                cmd("item replace entity @s hotbar.2 with minecraft:potato 10")
            target_x = 10.1 if scenario == "ammo_swap" else 8.9
            cmd(f'summon minecraft:pig {target_x} 76 16 {{NoAI:1b,NoGravity:1b,Tags:["shot_target"]}}')
            health("shot_target", 10)
            ctl("wait", "12")
            ctl("use-item")
            ctl("wait", "35")
            if scenario in {"veto", "swap", "ammo_swap"}:
                health("shot_target", 10)
                assert cmd("clear @s minecraft:potato 0")["result"] == (20 if scenario == "ammo_swap" else 10)
                if scenario == "ammo_swap":
                    for slot in (0, 2):
                        assert cmd(f"execute if items entity @s hotbar.{slot} minecraft:potato")["result"] == 10
                    assert cmd("execute if items entity @s weapon.offhand create:potato_cannon[minecraft:damage=7]")["result"] == 1
                else:
                    items = ["create:potato_cannon"]
                    if scenario == "swap":
                        items.append("minecraft:iron_pickaxe")
                    for item in items:
                        stack = cmd(f'data get entity @s Inventory[{{id:"{item}"}}]')
                        assert stack["success"] and "minecraft:damage" not in stack["chat"][0], stack
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
        stop(game)

    if scenario == "veto":
        assert "[potato check] join refused" in log()
    if scenario in {"swap", "ammo_swap"}:
        assert "[potato check] selected slot changed" in log()
        assert not samples(log()), "an unpaid projectile flew after the selected slot changed"
    if scenario != "flight":
        return

    before = log()
    (scratch / "before-reopen.log").write_text(before)
    flying = samples(before)
    assert len(flying) >= 2, flying
    uuid, at, velocity, owner, _, _ = flying[-1]
    assert re.search(r"\[flight\] renderer id \d+ ammo potato", before)
    assert re.search(r"\[flight\] client \d+ \[", before)
    game = open_world()
    try:
        cmd("tick rate 20")
        cmd("tick unfreeze")
        ctl("wait", "35")
        after = samples(log(), uuid)
        assert after, "saved projectile UUID was absent after reopen"
        _, reopened_at, reopened_velocity, reopened_owner, _, _ = after[0]
        drag = struct.unpack("f", struct.pack("f", 0.99))[0]
        step = [v * drag for v in [velocity[0], velocity[1] - 0.05, velocity[2]]]
        assert reopened_owner == owner, (reopened_owner, owner)
        assert reopened_velocity == step, (reopened_velocity, step)
        assert reopened_at == [x + v for x, v in zip(at, step)], (at, reopened_at, step)

        cmd("tp @s 9.5 76 8.5 0 0")
        cmd("item replace entity @s weapon.offhand with minecraft:cake")
        cmd("fill 7 76 13 10 76 15 minecraft:stone")
        ctl("look", "0", "0")
        cmd('summon minecraft:pig 8.9 77 14 {NoAI:1b,NoGravity:1b,Tags:["sticky_target"]}')
        cmd("tick rate 4")
        ctl("use-item")
        ctl("wait", "6")
        health("sticky_target", 2)
        stuck = [row for row in samples(log()) if row[-1]]
        assert stuck and re.search(r"\[flight\] renderer id \d+ ammo cake", log())
        target = "@e[tag=sticky_target,limit=1]"
        cmd(f"tp {target} 7.9 77 14")
        ctl("wait", "3")
        after = [row for row in samples(log(), stuck[-1][0]) if row[-1]]
        assert after[-1][1][0] == stuck[-1][1][0] - 1, (stuck[-1], after[-1])
        cmd(f"kill {target}")
        cmd("tick rate 20")
        ctl("wait", "20")

        # All split shots originate together. They must all hit the wall in their first
        # sweep rather than masking one another, and spend just one ammo and durability.
        cmd("fill 7 76 11 11 81 11 minecraft:stone")
        load("minecraft:sweet_berries")
        previous = log().count("[flight] hit block")
        ctl("use-item")
        ctl("wait", "15")
        impacts = re.findall(r"\[flight\] hit block (\d+) age (\d+) at", log())[previous:]
        assert len(impacts) == 3 and len({shot for shot, _ in impacts}) == 3, impacts
        assert all(age == "1" for _, age in impacts), impacts
        assert cmd("clear @s minecraft:sweet_berries 0")["result"] == 9
        assert cmd('data get entity @s SelectedItem.components."minecraft:damage"')["result"] == 3
    finally:
        stop(game)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ctl", "game", "resources", "jars", "scratch"):
        parser.add_argument(f"--{name}", type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument("--fixture", action="store_true")
    args = parser.parse_args()
    args.scratch.mkdir(parents=True, exist_ok=True)
    for scenario in ("veto", "swap", "ammo_swap", "flight") if args.fixture else ("smoke",):
        scratch = Path(tempfile.mkdtemp(prefix=f"potato-{scenario}-", dir=args.scratch))
        print(f"{scenario}: {scratch}", flush=True)
        run(args, scratch, scenario)


if __name__ == "__main__":
    main()
