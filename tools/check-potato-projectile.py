#!/usr/bin/env python3
"""Smoke-test a staged Create package on its signed Sunrise Nightly.

Run under Xvfb with the matching sunrise-ctl, private vanilla resources and
verified jars. --fixture uses a disposable build with the fixture patch applied.
"""

import argparse
import ast
import json
import os
from pathlib import Path
import re
import select
import shutil
import struct
import subprocess
import tempfile

CAKE_DROP = '@e[type=minecraft:item,nbt={Item:{id:"minecraft:cake"}}]'
RECOVERY_CANNON = 'create:potato_cannon[minecraft:enchantments={"create:potato_recovery":7}]'
BACKTANK = 'create:copper_backtank[minecraft:custom_data={"create:backtank_air":30}]'
MARK = 'create:potato_cannon[minecraft:custom_data={"create:shaper_swap":1b}]'
FIXTURES = ("veto", "swap", "ammo_swap", "pay_switch", "flight", "sky", "owner", "sticky", "split", "partial")


def samples(trace, uuid=None):
    rows = []
    for entity, shot, age, at, v, owner, stuck, recovery in re.findall(
        r"\[flight\] tick (\d+) (\d+) age=(\d+) from=(\[[^\n]+?\]) velocity=(\[[^\n]+?\]) "
        r"owner=(\d+) stuck=(true|false) recovery=([0-9.]+)", trace
    ):
        if uuid is None or shot == uuid:
            rows.append((entity, shot, int(age), ast.literal_eval(at), ast.literal_eval(v), owner, stuck == "true", float(recovery)))
    return rows


def potato_step(velocity):
    drag = struct.unpack("f", struct.pack("f", 0.99))[0]
    return [v * drag for v in [velocity[0], velocity[1] - 0.05, velocity[2]]]


def run(args, scratch, scenario):
    world = scratch / "game"
    world.mkdir()
    (world / "eula.txt").write_text("eula=true\n")
    shutil.copytree(args.jars, scratch / "home/jars")
    socket = scratch / "control.sock"
    env = dict(os.environ, SUNRISE_HOME=str(scratch / "home"), SUNRISE_CONTROL=str(socket), POTATO_TEST=scenario)

    def ctl(*words):
        out = subprocess.run(
            [args.ctl, "--socket", socket, "--timeout", "45", *words], cwd=world,
            env=env, capture_output=True, text=True, timeout=51,
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

    def count_drops(item):
        return sum(row["item"]["count"] for row in ctl("entities", "12", "item")["entities"]
                   if row["item"]["item"] == item)

    def replace(slot, item):
        assert cmd(f"item replace entity @s {slot} with {item}")["success"]

    def load(item):
        replace("hotbar.1", f"{item} 10")

    def pig(tag, x=8.9, y=76, z=16):
        assert cmd(f'summon minecraft:pig {x} {y} {z} {{NoAI:1b,NoGravity:1b,Tags:["{tag}"]}}')["success"]

    def open_world():
        pid = ctl("launch", "--game", args.game, "--game-dir", world, "--resources", args.resources,
                  "--test-terrain", "--render-distance", "4", "--width", "800", "--height", "600")["pid"]
        handle = os.pidfd_open(pid)
        ctl("open", "PotatoSmoke", "0", "creative")
        ctl("wait-ready")
        return handle

    def stop(game):
        ctl("quit")
        assert select.select([game], [], [], 20)[0]
        os.close(game)

    def log():
        return (scratch / "control.log").read_text()

    def assert_joins(kind, count, trace=None):
        joined = re.findall(r"\[flight\] join (\d+) saved=false ammo=(\S+)", trace if trace is not None else log())
        assert len(joined) == count and all(ammo == f'Some("{kind}")' for _, ammo in joined), joined
        return [entity for entity, _ in joined]

    def damage_sources(trace=None):
        return [(int(direct), int(causing)) for direct, causing in re.findall(
            r"\[flight\] damage source direct=(\d+) causing=(\d+)", trace if trace is not None else log()
        )]

    game = open_world()
    try:
        cmd("tp @s 9.5 76 8.5 0 0")
        cmd("fill 4 76 6 16 88 40 minecraft:air")
        cmd("fill 4 75 6 16 75 40 minecraft:stone")
        cmd("gamerule spawn_mobs false")
        cmd("gamemode survival @s")
        replace("hotbar.0", "create:potato_cannon[minecraft:damage=7]" if scenario == "smoke" else "create:potato_cannon")
        load("minecraft:potato")
        ctl("select", "1")
        ctl("look", "0", "0")

        if scenario == "flight":
            cmd("tick freeze")
            ctl("look", "0", "-14")
            ctl("use-item")
            cmd("tick step 3")
            ctl("wait", "12")
            ctl("save")
            # Look at the actual frozen shot from two oblique angles, not at a spawn crumb
            # or at a dropped item. Inspect these screenshots privately, never publish them.
            drawn = re.findall(r"\[flight\] client position (\[[^\n]+\])", log())
            assert drawn, "the client did not see the frozen projectile"
            at = ast.literal_eval(drawn[-1])
            cmd("gamemode creative @s")
            for name, offset in (("east", 3.0), ("west", -3.0)):
                cmd(f"tp @s {at[0] + offset} {max(76.3, at[1] - 1.3)} {at[2] - 0.6}")
                ctl("look-at", *map(str, at))
                ctl("wait", "3")
                ctl("screenshot", scratch / f"billboard-{name}.png")
            angles = [float(yaw) for yaw in re.findall(r"\[flight\] drawing \d+ as potato yaw=([-\d.]+)", log())]
            assert any(yaw > 60 for yaw in angles) and any(yaw < -60 for yaw in angles), angles
        elif scenario == "sky":
            cmd("gamemode creative @s")
            cmd("tp @s 9.5 317 8.5 0 -70")
            ctl("look", "0", "-70")
            ctl("use-item")
            ctl("wait", "15")
            ids = assert_joins("potato", 1)
            trace = log()
            assert f"[flight] unanswered clip {ids[0]} " in trace, trace[-4500:]
            assert any(at[1] > 320.0 for _, _, _, at, *_ in samples(trace)), samples(trace)
            assert "[flight] hit block" not in trace and not damage_sources(trace)
        elif scenario == "owner":
            # Keep the original column loaded while the shooter is briefly in another dimension.
            assert cmd("forceload add 8 8")['success']
            cmd("tick freeze")
            pig("absent")
            ctl("use-item")
            assert cmd("execute in minecraft:the_nether run tp @s 0.5 90 0.5")['success']
            cmd("tick unfreeze")
            ctl("wait", "20")
            cmd("execute in minecraft:overworld run tp @s 9.5 76 8.5 0 0")
            health("absent", 5)
            cmd("kill @e[tag=absent]")
            ctl("wait-ready")
            ctl("wait", "30")
            ctl("select", "1")
            ctl("look", "0", "0")
            cmd("tick freeze")
            pig("returned")
            ctl("use-item")
            cmd("execute in minecraft:the_nether run tp @s 0.5 90 0.5")
            cmd("execute in minecraft:overworld run tp @s 9.5 76 8.5 0 0")
            cmd("tick unfreeze")
            ctl("wait", "20")
            health("returned", 5)
            ids = [int(entity) for entity in assert_joins("potato", 2)]
            events = damage_sources()
            assert events[0] == (ids[0], 0), events
            assert events[1][0] == ids[1] and events[1][1] > 0, events
        elif scenario == "sticky":
            # A full survival inventory stays full after the debit: the cakes are stacked in
            # the offhand, and the shot must recover into the world, not ask for inventory room.
            replace("hotbar.0", RECOVERY_CANNON)
            for slot in [*(f"hotbar.{i}" for i in range(1, 9)), *(f"inventory.{i}" for i in range(27))]:
                replace(slot, "minecraft:cobblestone 64")
            replace("weapon.offhand", "minecraft:cake[minecraft:max_stack_size=16] 2")
            pig("sticky_target", y=76.25, z=14)
            cmd("tick freeze")
            ctl("use-item")
            cmd("tick step 4")
            ctl("wait", "6")
            health("sticky_target", 2)
            assert_joins("cake", 1)
            assert count_drops("minecraft:cake") == 1
            assert cmd("execute if items entity @s weapon.offhand minecraft:cake")["result"] == 1
            # Remove the recovered item so it can't be mistaken for the billboard or picked
            # up after reopen. A second recovery would leave a new item in the world.
            assert cmd(f"kill {CAKE_DROP}")["result"] == 1
            stuck = [row for row in samples(log()) if row[-2]]
            assert stuck and stuck[-1][-1] == 0.0, stuck
            shot = stuck[-1]
            target = "@e[tag=sticky_target,limit=1]"
            cmd(f"tp {target} 7.9 76 14")
            cmd("tick step 2")
            ctl("wait", "6")
            moved = [row for row in samples(log(), shot[1]) if row[-2]][-1]
            assert moved[3][0] == shot[3][0] - 1, (shot, moved)
            at = samples(log(), shot[1])[-1][3]
            cmd(f"kill {target}")
            pig("reopen_target", at[0], at[1], at[2])
            ctl("save")
        elif scenario in {"split", "partial"}:
            replace("hotbar.0", RECOVERY_CANNON)
            cmd("fill 7 76 11 11 81 11 minecraft:stone")
            load("minecraft:sweet_berries")
            cmd("tick freeze")
            ctl("use-item")
            cmd("tick step 1")
            ctl("wait", "12")
            ids = assert_joins("sweet_berry", 3)
            trace = log()
            admitted = ids if scenario == "split" else ids[1:]
            if scenario == "partial":
                assert f"[potato check] join refused {ids[0]}" in trace
            impacts = re.findall(r"\[flight\] hit block (\d+) age=(\d+)", trace)
            assert impacts == [(entity, "1") for entity in admitted], impacts
            assert [row[0] for row in samples(trace) if row[-1] > 0] == admitted[:1]
            assert count_drops("minecraft:sweet_berries") == 1
            assert cmd("clear @s minecraft:sweet_berries 0")["result"] == 9
        else:
            if scenario in {"swap", "pay_switch"}:
                replace("hotbar.2", "minecraft:iron_pickaxe[minecraft:damage=3]")
            if scenario == "pay_switch":
                replace("hotbar.0", "create:potato_cannon[minecraft:damage=7]")
                replace("armor.chest", BACKTANK)
                replace("weapon.offhand", "create:potato_cannon")
            if scenario == "veto":
                replace("weapon.offhand", "create:potato_cannon")
            if scenario == "ammo_swap":
                replace("weapon.offhand", "create:potato_cannon[minecraft:damage=7]")
                replace("hotbar.0", "minecraft:potato 10")
                replace("hotbar.1", "minecraft:air")
                replace("hotbar.2", "minecraft:potato 10")
            pig("shot_target", x=10.1 if scenario == "ammo_swap" else 8.9)
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
                elif scenario == "veto":
                    assert cmd(f"execute if items entity @s hotbar.0 {MARK}")["result"] == 0
                    ctl("use-item")
                    ctl("wait", "35")
                    health("shot_target", 5)
                    assert cmd(f"execute if items entity @s hotbar.0 {MARK}")["result"] == 1
                    assert cmd("clear @s minecraft:potato 0")["result"] == 9
                else:
                    assert cmd("execute if items entity @s hotbar.2 minecraft:iron_pickaxe[minecraft:damage=3]")["result"] == 1
                assert_joins("potato", 2 if scenario == "veto" else 1)
                if scenario != "veto":
                    assert "[potato check] selected slot changed at join" in log()
                    assert not samples(log()), "an unpaid projectile ticked"
            elif scenario == "pay_switch":
                health("shot_target", 5)
                assert_joins("potato", 1)
                assert "[potato check] selected slot changed on ammo debit" in log()
                assert cmd("data get entity @s SelectedItemSlot")["result"] == 2
                assert cmd("execute if items entity @s hotbar.2 minecraft:iron_pickaxe[minecraft:damage=3]")["result"] == 1
                assert cmd(f"execute if items entity @s hotbar.0 {MARK[:-1]},minecraft:damage=7]")["result"] == 1
                assert cmd(f"execute if items entity @s armor.chest {BACKTANK.replace('30', '26')}")["result"] == 1
                assert cmd("clear @s minecraft:potato 0")["result"] == 9
            else:
                health("shot_target", 5)
                assert cmd("clear @s minecraft:potato 0")["result"] == 9
                assert cmd("execute if items entity @s hotbar.0 create:potato_cannon[minecraft:damage=7]")["result"] == 1
                cmd("kill @e[tag=shot_target]")
                cmd("fill 7 75 9 12 75 24 minecraft:farmland[moisture=7]")
                ctl("look", "0", "12")
                ctl("use-item")
                ctl("wait", "35")
                assert cmd("fill 7 76 9 12 76 24 minecraft:air replace minecraft:potatoes")["result"] == 1
                assert cmd("clear @s minecraft:potato 0")["result"] == 8
                replace("weapon.offhand", "minecraft:cake")
                ctl("look", "0", "0")
                pig("sticky_target", z=14)
                ctl("use-item")
                ctl("wait", "40")
                health("sticky_target", 2)
                assert cmd("clear @s minecraft:cake 0")["result"] == 0
                assert cmd("clear @s minecraft:potato 0")["result"] == 8
    finally:
        stop(game)

    if scenario not in {"flight", "sticky"}:
        return
    before = log()
    (scratch / "before-reopen.log").write_text(before)
    game = open_world()
    try:
        cmd("tick rate 20")
        cmd("tick unfreeze")
        ctl("wait", "24")
        if scenario == "flight":
            assert_joins("potato", 1, before)
            flying = samples(before)
            assert len(flying) >= 2, flying
            _, uuid, _, at, velocity, owner, _, _ = flying[-1]
            restarted = samples(log(), uuid)
            assert restarted, "saved projectile UUID was absent after reopen"
            _, _, _, reopened_at, reopened_velocity, reopened_owner, _, _ = restarted[0]
            step = potato_step(velocity)
            assert reopened_owner == owner and owner != '0', (reopened_owner, owner)
            assert reopened_velocity == step, (reopened_velocity, step)
            assert reopened_at == [x + v for x, v in zip(at, step)], (at, reopened_at, step)
        else:
            shot = [row for row in samples(before) if row[-2]][-1]
            assert samples(log(), shot[1])[0][-2:] == (False, 0.0)
            health("reopen_target", 2)
            assert count_drops("minecraft:cake") == 0
    finally:
        stop(game)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ctl", "game", "resources", "jars", "scratch"):
        parser.add_argument(f"--{name}", type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument("--fixture", action="store_true")
    parser.add_argument("--case", action="append", choices=FIXTURES)
    args = parser.parse_args()
    if args.case and not args.fixture:
        parser.error("--case needs a disposable --fixture build")
    args.scratch.mkdir(parents=True, exist_ok=True)
    for scenario in (args.case or FIXTURES) if args.fixture else ("smoke",):
        scratch = Path(tempfile.mkdtemp(prefix=f"{scenario}-", dir=args.scratch))
        print(f"{scenario}: {scratch}", flush=True)
        run(args, scratch, scenario)


if __name__ == "__main__":
    main()
