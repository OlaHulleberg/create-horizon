#!/usr/bin/env python3
"""Pinned Nightly smoke test for the actual cart glue, run under Xvfb on Linux.

Pass the staged create app, the matching sunrise-ctl, prepared vanilla resources,
and the pinned Create jar directory. The test creates a disposable world and game home.

--refusal-fixture expects a disposable build that vetoes boarding at x=14, z=10
using tools/fixtures/cart-rider.patch. Its fixture-swap command dismounts the exact
Create rider and mounts a pig in the same server callback. The fixture-refuse-drops command vetoes
item joins at the teardown location until restart; fixture-present queries the exact entity
UUID through the Host; fixture-tank totals fluid in distinct tanks in three vertical cells.
The test checks the veto log markers.
"""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
from uuid import UUID


def run(args, scratch):
    world = scratch / "game"
    world.mkdir()
    (world / "eula.txt").write_text("eula=true\n")
    shutil.copytree(args.jars, scratch / "home/jars")
    socket = scratch / "control.sock"
    env = dict(os.environ, SUNRISE_HOME=str(scratch / "home"), SUNRISE_CONTROL=str(socket))

    def ctl(*command):
        output = subprocess.run(
            [args.ctl, "--socket", socket, "--timeout", "45", *command],
            cwd=world, env=env, capture_output=True, text=True, timeout=51,
        )
        if output.returncode:
            raise RuntimeError(command, output.stdout, output.stderr)
        result = json.loads(output.stdout)
        assert result.get("ok") is not False, (command, result)
        return result

    def cmd(command, expected=True):
        result = ctl("command", command)
        print(command, result, flush=True)
        if expected is not None:
            assert result["success"] is expected, (command, result)
        return result

    def launch():
        result = ctl(
            "launch", "--game", args.game, "--game-dir", world, "--resources", args.resources,
            "--test-terrain", "--render-distance", "3", "--width", "640", "--height", "480",
        )
        fd = os.pidfd_open(result["pid"])
        ctl("open", "CartRider", "0", "creative")
        ctl("wait-ready")
        return fd

    def stop(fd):
        ctl("quit")
        assert select.select([fd], [], [], 20)[0], "Sunrise did not exit"
        os.close(fd)

    cart = "@e[type=minecraft:minecart,sort=nearest,limit=1]"

    def rider_uuid(words):
        values = words.removeprefix("[I;").removesuffix("]").split(",")
        return UUID(bytes=b"".join((int(v) & 0xFFFFFFFF).to_bytes(4, "big") for v in values))

    def passenger(selector=cart):
        result = cmd(f"data get entity {selector} Passengers[0].id")
        assert result["chat"][0].endswith('"create:contraption"'), result
        cmd(f"data get entity {selector} Passengers[1].id", False)
        words = cmd(f"data get entity {selector} Passengers[0].UUID")["chat"][0].split(": ")[-1]
        if args.refusal_fixture:
            result = cmd(f"create_debug carts fixture-present {rider_uuid(words).int}")["chat"][0]
            assert result == "rider present", result
        return words

    def assert_gone(words):
        uuid = rider_uuid(words)
        if args.refusal_fixture:
            result = cmd(f"create_debug carts fixture-present {uuid.int}")["chat"][0]
            assert result == "rider absent", result
        else:
            cmd(f"data get entity {uuid} UUID", False)

    fd = launch()
    try:
        cmd("tp @s 10.5 77 7.5")
        cmd("fill 6 76 7 14 82 13 minecraft:air", None)
        cmd("fill 6 75 7 14 75 13 minecraft:stone")
        cmd("setblock 9 76 9 create:cart_assembler[shape=east_west,rail_type=regular]")
        cmd("setblock 9 77 9 minecraft:oak_planks")
        cmd("setblock 9 75 9 minecraft:redstone_block")
        cmd("summon minecraft:minecart 9.5 76.1 9.5")
        ctl("wait", "35")
        first_uuid = passenger()
        cmd("execute if block 9 77 9 minecraft:air")
        cmd("tp @s 512.5 100 512.5")
        ctl("wait", "105")
        cmd("execute if loaded 9 76 9", False)
        cmd("tp @s 10.5 77 7.5")
        ctl("wait-ready")
        ctl("wait", "25")
        assert passenger() == first_uuid, "chunk reload did not retain the same rider"
        cmd("execute if block 9 77 9 minecraft:air")
    finally:
        stop(fd)

    fd = launch()
    try:
        assert passenger() == first_uuid, "world reopen did not retain the same rider"
        cmd("setblock 9 75 9 minecraft:stone")
        ctl("wait", "35")
        cmd(f"data get entity {cart} Passengers[0].id", False)
        cmd("execute if block 9 77 9 minecraft:oak_planks")
        assert_gone(first_uuid)
        cmd(f"kill {cart}")

        def pack(block, storage=""):
            return (
                'create:minecart_contraption[minecraft:custom_data={'
                '"create:minecart_contraption_data":{Blocks:[{Pos:[I;0,1,0],'
                f'State:"{block}"}}],InitialOrientation:"WEST"{storage}'
                '}}]'
            )

        def place(source, stack):
            cmd("setblock 10 76 10 minecraft:rail[shape=east_west]", None)
            cmd("tp @s 10.5 76 10.5 0 90")
            cmd("clear @s", None)
            cmd(f"item replace entity @s {source} with {stack} 2")
            ctl("select", "1")
            ctl("look", "0", "90")
            ctl("mouse", "right", "click")
            ctl("wait", "10")
            return passenger()

        def cart_pos(selector=cart):
            values = cmd(f"data get entity {selector} Pos")["chat"][0].split(": ")[-1]
            return tuple(float(v.strip().removesuffix("d")) for v in values[1:-1].split(","))

        def wrench_cart(selector, handled=True):
            feedback = cmd(f"create_debug carts fixture-wrench {uuid_arg(selector)}")["chat"][0]
            assert feedback == ("wrench handled" if handled else "wrench unhandled"), feedback
            ctl("wait", "15")

        cmd("gamemode survival @s")
        stack = pack("minecraft:oak_planks")
        for hand, other, other_item in [
            ("hotbar.0", "weapon.offhand", "minecraft:emerald"),
            ("weapon.offhand", "weapon.mainhand", "minecraft:stick"),
        ]:
            # Give the opposite hand its own stack before clicking.
            cmd("setblock 10 76 10 minecraft:rail[shape=east_west]", None)
            cmd("tp @s 10.5 76 10.5 0 90")
            cmd("clear @s", None)
            cmd(f"item replace entity @s {hand} with {stack} 2")
            cmd(f"item replace entity @s {other} with {other_item} 7")
            ctl("select", "1")
            ctl("look", "0", "90")
            ctl("mouse", "right", "click")
            ctl("wait", "12")
            uuid = passenger()
            count = cmd(f"clear @s {stack} 0")["result"]
            assert count == 1, (hand, count)
            count = cmd(f"clear @s {other_item} 0")["result"]
            assert count == 7, (other, count)
            cmd(f"kill {cart}")
            ctl("wait", "15")
            cmd("execute if block 10 77 10 minecraft:oak_planks")
            assert_gone(uuid)
            cmd("setblock 10 77 10 minecraft:air")

        cmd("gamemode creative @s")
        damage_uuid = place("hotbar.0", stack)
        cmd(f"damage {cart} 100 minecraft:generic")
        ctl("wait", "20")
        cmd(f"execute if entity {cart}", False)
        cmd("execute if block 10 77 10 minecraft:oak_planks")
        assert_gone(damage_uuid)
        cmd("setblock 10 77 10 minecraft:air")
        stored = ',Storage:[{Pos:[I;0,1,0],Size:20,Items:[{Slot:0,id:"minecraft:gold_ingot",count:7}]}]'
        vault_uuid = place("hotbar.0", pack("create:item_vault", stored))
        cmd("tp @s 10.5 77 7.5")
        cmd("setblock 10 77 10 create:item_vault")
        cmd("item replace block 10 77 10 container.0 with minecraft:diamond 3")
        cmd("setblock 10 76 10 minecraft:activator_rail[shape=east_west,powered=true]")
        cmd("setblock 10 75 10 minecraft:redstone_block")
        ctl("wait", "30")
        cmd(f"data get entity {cart} Passengers[0].id", False)
        assert_gone(vault_uuid)
        count = cmd("execute if items block 10 77 10 container.0 minecraft:diamond")["result"]
        assert count == 3, count
        ctl("wait", "25")
        result = ctl("entities")
        drops = [e["item"] for e in result["entities"] if e.get("item")]
        for item, expected in [("minecraft:gold_ingot", 7), ("create:item_vault", 1)]:
            dropped = sum(i["count"] for i in drops if i["item"] == item)
            collected = cmd(f"clear @s {item} 0", None)["result"]
            assert dropped + collected == expected, (item, dropped, collected)
        cmd(f"kill {cart}")
        cmd("setblock 10 77 10 minecraft:air")
        cmd("setblock 10 75 10 minecraft:stone")

        cmd("tp @s 10.5 76 7.5")
        cmd("setblock 9 77 9 minecraft:air", None)
        cmd("setblock 9 76 9 minecraft:rail[shape=east_west]")
        cmd("setblock 8 76 9 create:cart_assembler[shape=east_west,rail_type=regular]")
        cmd("setblock 12 76 9 create:cart_assembler[shape=east_west,rail_type=regular]")
        cmd("fill 8 77 9 12 77 9 minecraft:oak_planks")
        cmd("create_debug glue 8 77 9 12 77 9")
        cmd('summon minecraft:minecart 12.5 76.1 9.5 {Tags:["secondary"]}')
        cmd("setblock 8 75 9 minecraft:redstone_block")
        cmd('summon minecraft:minecart 8.5 76.1 9.5 {Tags:["primary"]}')
        main = "@e[tag=primary,limit=1]"
        mate = "@e[tag=secondary,limit=1]"
        ctl("wait", "45")
        coupled_uuid = passenger(main)
        cmd(f"data get entity {mate} Passengers[0].id", False)
        result = cmd("create_debug carts list")["chat"][0]
        assert "coupled true" in result, result
        for x in range(8, 13):
            cmd(f"execute if block {x} 77 9 minecraft:air")
        cmd("tp @s 512.5 100 512.5")
        ctl("wait", "100")
        cmd("execute if loaded 8 76 9", False)
        cmd("tp @s 10.5 76 7.5")
        ctl("wait", "50")
        assert passenger(main) == coupled_uuid
        cmd(f"data get entity {mate} Passengers[0].id", False)
        result = cmd("create_debug carts list")["chat"][0]
        assert "coupled true" in result, result
        # Both carts remain on their assemblers; disturb the separation without asserting
        # occupied drag or any pinned-engine trajectory.
        x0, _, _ = cart_pos(main)
        _, y, z = cart_pos(mate)
        cmd(f"tp {mate} 12.85 {y} {z}")
        ctl("wait", "15")
        assert cart_pos(mate)[0] - cart_pos(main)[0] < 12.85 - x0
        if args.refusal_fixture:
            def uuid_arg(selector):
                words = cmd(f"data get entity {selector} UUID")["chat"][0].split(": ")[-1]
                return rider_uuid(words).int

            # Remove both assemblers so the restored structure is not assembled again.
            cmd("setblock 8 76 9 minecraft:rail[shape=east_west]")
            cmd("setblock 12 76 9 minecraft:rail[shape=east_west]")
            cmd('summon minecraft:minecart 12.5 76.1 11.5 {Tags:["third"]}')
            third = "@e[tag=third,limit=1]"
            feedback = cmd(f"create_debug carts fixture-couple {uuid_arg(mate)} {uuid_arg(third)}")["chat"][0]
            assert feedback == "ordinary link added", feedback
            cmd("tp @s 12.5 76 7.5")
            cmd("gamemode survival @s")
            cmd("clear @s", None)
            cmd("item replace entity @s hotbar.0 with create:wrench")
            ctl("select", "1")
            wrench_cart(mate)
            assert cmd("clear @s create:minecart_coupling 0")["result"] == 1
            assert_gone(coupled_uuid)
            blocks = ctl("blocks", "4", "76", "4", "16", "81", "15")["blocks"]
            assert sum(block[-1] == "minecraft:oak_planks" for block in blocks) == 5, blocks
            cmd("setblock 8 75 9 minecraft:stone")
        else:
            cmd("setblock 8 75 9 minecraft:stone")
            ctl("wait", "40")
            for x in range(8, 13):
                cmd(f"execute if block {x} 77 9 minecraft:oak_planks")
            assert_gone(coupled_uuid)

        if args.refusal_fixture:
            cmd("kill @e[type=minecraft:minecart]")
            cmd("gamemode survival @s")
            cmd("setblock 14 76 10 minecraft:rail[shape=east_west]")
            cmd("tp @s 14.5 76 10.5 0 90")
            cmd("clear @s", None)
            cmd(f"item replace entity @s hotbar.0 with {stack} 2")
            ctl("select", "1")
            ctl("look", "0", "90")
            ctl("mouse", "right", "click")
            ctl("wait", "12")
            cmd(f"execute if entity {cart}", False)
            assert cmd(f"clear @s {stack} 0")["result"] == 2

            cmd("gamemode creative @s")
            uuid = place("hotbar.0", stack)
            assert "swapped rider" in cmd("create_debug carts fixture-swap")["chat"][0]
            ctl("wait", "20")
            result = cmd(f"data get entity {cart} Passengers[0].id")["chat"][0]
            assert result.endswith('"minecraft:pig"'), result
            cmd(f"data get entity {cart} Passengers[1].id", False)
            cmd("execute if block 10 77 10 minecraft:oak_planks")
            assert_gone(uuid)
            cmd("clear @s minecraft:oak_planks 0", False)
            drops = [e["item"]["item"] for e in ctl("entities")["entities"] if e.get("item")]
            assert "minecraft:oak_planks" not in drops, drops
            cmd("setblock 10 77 10 minecraft:air")
            ctl("wait", "20")
            cmd("execute if block 10 77 10 minecraft:air")
            cmd("kill @e[type=minecraft:pig]")
            cmd(f"kill {cart}")

            tank_blocks = ",".join(f'{{Pos:[I;0,{y},0],State:"create:fluid_tank"}}' for y in range(1, 4))
            tank_item = (
                'create:minecart_contraption[minecraft:custom_data={"create:minecart_contraption_data":'
                f'{{Blocks:[{tank_blocks}],InitialOrientation:"WEST",'
                'Held:[{pos:[I;0,1,0],items:[],held:{block:"fluid_tank",tank:[I;1,3,1,0,22000]}}]}}]'
            )
            for obstructed, amount in [(79, 16000), (77, 0)]:
                tank_uuid = place("hotbar.0", tank_item)
                cmd("tp @s 10.5 77 7.5")
                cmd(f"setblock 10 {obstructed} 10 minecraft:obsidian")
                cmd("setblock 10 76 10 minecraft:activator_rail[shape=east_west,powered=true]")
                cmd("setblock 10 75 10 minecraft:redstone_block")
                ctl("wait", "35")
                cmd(f"execute if block 10 {obstructed} 10 minecraft:obsidian")
                result = cmd("create_debug carts fixture-tank 10 77 10")["chat"][0]
                assert result == str(amount), (obstructed, result)
                assert_gone(tank_uuid)
                cmd(f"kill {cart}")
                cmd("fill 10 77 10 10 79 10 minecraft:air")
                cmd("setblock 10 75 10 minecraft:stone")

            pending_uuid = place("hotbar.0", pack("create:item_vault", stored))
            cmd("tp @s 10.5 77 7.5")
            cmd("setblock 10 77 10 minecraft:obsidian")
            assert "refusing teardown drops" in cmd("create_debug carts fixture-refuse-drops")["chat"][0]
            cmd("setblock 10 76 10 minecraft:activator_rail[shape=east_west,powered=true]")
            cmd("setblock 10 75 10 minecraft:redstone_block")
            ctl("wait", "30")
            cmd(f"data get entity {cart} Passengers[0].id", False)
            cmd(f"kill {cart}")
            ctl("wait", "25")
            result = cmd(f"create_debug carts fixture-present {rider_uuid(pending_uuid).int}")["chat"][0]
            assert result == "rider present", result
            cmd("clear @s minecraft:gold_ingot 0", False)
            cmd("clear @s create:item_vault 0", False)
            drops = [e["item"]["item"] for e in ctl("entities")["entities"] if e.get("item")]
            assert "minecraft:gold_ingot" not in drops and "create:item_vault" not in drops, drops
    finally:
        stop(fd)
    if args.refusal_fixture:
        log = (scratch / "control.log").read_text()
        assert "[cart check] mount deliberately refused" in log
        assert "[cart check] teardown item deliberately refused" in log
        (scratch / "control-before-retry.log").write_text(log)
        fd = launch()
        try:
            ctl("wait", "45")
            cmd(f"execute if entity {cart}", False)
            assert_gone(pending_uuid)
            cmd("execute if block 10 77 10 minecraft:obsidian")
            drops = [e["item"] for e in ctl("entities")["entities"] if e.get("item")]
            for item, expected in [("minecraft:gold_ingot", 7), ("create:item_vault", 1)]:
                count = sum(i["count"] for i in drops if i["item"] == item)
                count += cmd(f"clear @s {item} 0", None)["result"]
                assert count == expected, (item, count)
            cmd("gamemode creative @s")
            cmd("tp @s 8.5 76 7.5")
            cmd("fill 6 77 9 14 79 12 minecraft:air", None)
            cmd("setblock 8 76 9 minecraft:rail[shape=east_west]", None)
            cmd("setblock 12 76 9 minecraft:rail[shape=east_west]", None)
            cmd("setblock 8 76 11 minecraft:rail[shape=east_west]", None)
            cmd("setblock 12 76 11 minecraft:rail[shape=east_west]", None)
            for name, x, z in [("old_a", 8, 9), ("old_b", 12, 9), ("old_c", 8, 11), ("old_d", 12, 11)]:
                cmd(f'summon minecraft:minecart {x + .5} 76.1 {z + .5} {{Tags:["{name}"]}}')
            names = [f"@e[tag=old_{letter},limit=1]" for letter in "abcd"]
            ids = [uuid_arg(selector) for selector in names]
            feedback = cmd("create_debug carts fixture-legacy " + " ".join(map(str, ids)))["chat"][0]
            assert feedback == "old save written", feedback
        finally:
            stop(fd)
        fd = launch()
        try:
            ctl("wait", "15")
            feedback = cmd("create_debug carts fixture-old-state " + " ".join(map(str, ids)))["chat"][0]
            assert feedback == "ordinary=true obsolete=false mode=2 clean=true", feedback
            cmd("gamemode survival @s")
            cmd("clear @s", None)
            cmd("item replace entity @s hotbar.0 with create:wrench")
            ctl("select", "1")
            cmd("tp @s 8.5 76 12.8")
            wrench_cart(names[2], handled=False)
            cmd("clear @s create:minecart_coupling 0", False)
            cmd(f"data get entity {names[2]} Passengers[0].id", False)
            cmd("tp @s 8.5 76 7.5")
            wrench_cart(names[0])
            assert cmd("clear @s create:minecart_coupling 0")["result"] == 1
        finally:
            stop(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ctl", "game", "jars", "resources", "scratch"):
        parser.add_argument(f"--{name}", type=lambda s: Path(s).resolve(), required=True)
    parser.add_argument("--refusal-fixture", action="store_true",
                        help="also run the tests that require tools/fixtures/cart-rider.patch")
    args = parser.parse_args()
    args.scratch.mkdir(parents=True, exist_ok=True)
    print(f"Cart proof: {args.scratch}", flush=True)
    run(args, Path(tempfile.mkdtemp(prefix="cart-rider-", dir=args.scratch)))


if __name__ == "__main__":
    main()
