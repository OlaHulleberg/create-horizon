#!/usr/bin/env python3
"""Pinned Nightly smoke test for the actual cart glue, run under Xvfb on Linux.

Pass the staged create app, the matching sunrise-ctl, prepared vanilla resources,
and the pinned Create jar directory. The test creates a disposable world and game home.

--refusal-fixture expects a disposable build with two additional listeners. Cancel
mounting create:contraption when the vehicle is in column x=14, z=10 (block coordinates),
and cancel item_toss for create:minecart_contraption. Log the two '[cart check]' messages
asserted below on stderr to prove that the tested interaction reached each listener.
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

    def passenger(selector=cart):
        result = cmd(f"data get entity {selector} Passengers[0].id")
        assert result["chat"][0].endswith('"create:contraption"'), result
        cmd(f"data get entity {selector} Passengers[1].id", False)
        return cmd(f"data get entity {selector} Passengers[0].UUID")["chat"][0].split(": ")[-1]

    def assert_gone(words):
        values = words.removeprefix("[I;").removesuffix("]").split(",")
        uuid = UUID(bytes=b"".join((int(v) & 0xFFFFFFFF).to_bytes(4, "big") for v in values))
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

        def click_cart(selector=cart):
            values = cmd(f"data get entity {selector} Pos")["chat"][0].split(": ")[-1]
            x, y, z = (float(v.strip().removesuffix("d")) for v in values[1:-1].split(","))
            ctl("look-at", str(x), str(y + 0.35), str(z))
            ctl("wait", "3")
            ctl("mouse", "right", "click")
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
            cmd("execute if block 10 77 10 minecraft:air")
            assert_gone(uuid)

        cmd("gamemode creative @s")
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

        pickup_uuid = place("hotbar.0", stack)
        cmd("tp @s 10.5 76 8.2")
        cmd("clear @s", None)
        cmd("item replace entity @s hotbar.0 with create:wrench")
        ctl("select", "1")
        click_cart()
        cmd(f"execute if entity {cart}", False)
        assert_gone(pickup_uuid)
        assert cmd("clear @s create:minecart_contraption 0")["result"] == 1
        result = cmd('data get entity @s Inventory[{id:"create:minecart_contraption"}].components.'
                     '"minecraft:custom_data"."create:minecart_contraption_data".Blocks[0].State')
        assert result["chat"][0].endswith('"minecraft:oak_planks"'), result

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

            uuid = place("hotbar.0", stack)
            cmd("tp @s 10.5 76 9.1")
            cmd("clear @s", None)
            cmd("item replace entity @s hotbar.0 with create:wrench")
            for slot in [*(f"hotbar.{i}" for i in range(1, 9)), *(f"inventory.{i}" for i in range(27))]:
                cmd(f"item replace entity @s {slot} with minecraft:cobblestone 64")
            ctl("select", "1")
            click_cart()
            assert passenger() == uuid
            cmd("clear @s create:minecart_contraption 0", False)
            assert cmd("clear @s minecraft:cobblestone 0")["result"] == 35 * 64
            cmd("item replace entity @s hotbar.1 with minecraft:air")
            ctl("wait", "10")
            click_cart()
            cmd(f"execute if entity {cart}", False)
            assert_gone(uuid)
            assert cmd("clear @s create:minecart_contraption 0")["result"] == 1
    finally:
        stop(fd)
    if args.refusal_fixture:
        log = (scratch / "control.log").read_text()
        assert "[cart check] mount deliberately refused" in log
        assert "[cart check] toss deliberately refused" in log


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ctl", "game", "jars", "resources", "scratch"):
        parser.add_argument(f"--{name}", type=lambda s: Path(s).resolve(), required=True)
    parser.add_argument("--refusal-fixture", action="store_true",
                        help="also test with a fixture mod vetoing Create riders at x=14 and packed cart tosses")
    args = parser.parse_args()
    args.scratch.mkdir(parents=True, exist_ok=True)
    print(f"Cart proof: {args.scratch}", flush=True)
    run(args, Path(tempfile.mkdtemp(prefix="cart-rider-", dir=args.scratch)))


if __name__ == "__main__":
    main()
