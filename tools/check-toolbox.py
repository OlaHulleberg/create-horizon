#!/usr/bin/env python3
"""Check toolbox notifications and loot in a fresh world on the pinned Nightly."""

import argparse
import json
import subprocess
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ctl", help="path to the pinned Nightly's sunrise-ctl")
    parser.add_argument("socket", help="control socket of that Nightly, at its title screen")
    args = parser.parse_args()

    def ctl(*words):
        output = subprocess.check_output(
            [args.ctl, "--socket", args.socket, *words], text=True, timeout=90
        )
        reply = json.loads(output)
        if reply.get("ok") is False:
            raise SystemExit(f"{words}: {reply}")
        return reply

    def command(line, *, succeeds=True):
        reply = ctl("command", line)
        if reply["success"] != succeeds or reply["result"] is None:
            raise SystemExit(f"{line}: {reply}")
        return reply

    toolbox = "create:white_toolbox"
    source = "9 76 9"
    target = "10 76 8"
    diamond_kind = '{id:"minecraft:diamond",count:1}'
    copper_kind = '{id:"minecraft:copper_ingot",count:1}'
    copper_stack = '{id:"minecraft:copper_ingot",count:7,Slot:4b}'

    ctl("open", f"ToolboxSmoke-{uuid.uuid4().hex[:8]}", "0", "creative")
    ctl("wait-ready")
    # A fill may already match the terrain; the subsequent block and item checks must succeed.
    ctl("command", "fill 8 76 7 11 79 10 air")
    ctl("command", "fill 8 75 7 11 75 10 minecraft:stone")
    command(f"setblock {source} {toolbox}")

    command(f"item replace block {source} container.0 with minecraft:diamond 3")
    command(f"execute if data block {source} Compartments[{diamond_kind}]")
    command(f"item replace block {source} container.0 with minecraft:air")
    command(f"execute if data block {source} Items[0]", succeeds=False)
    command(f"execute if data block {source} Compartments[{diamond_kind}]")
    command(f"item replace block {source} container.4 with minecraft:copper_ingot 7")
    command(f"execute if data block {source} Compartments[{copper_kind}]")
    command(f"execute if data block {source} Items[{copper_stack}]")
    original_uuid = command(f"data get block {source} UniqueId")["chat"][0].split(": ", 1)[1]
    if not original_uuid.startswith("[I;"):
        raise SystemExit(f"expected toolbox UUID, got {original_uuid}")

    command(f"setblock {source} air destroy")
    items = "@e[type=minecraft:item,distance=..2]"
    drops = command(f"execute positioned {source} if entity {items}")
    if drops["result"] != 1:
        raise SystemExit(f"expected a single toolbox drop with no spilled stacks: {drops}")
    drop = "@e[type=minecraft:item,distance=..2,limit=1]"
    command(f'execute positioned {source} if data entity {drop} {{Item:{{id:"{toolbox}",count:1}}}}')

    command("tp @s 9.5 76 9.5")
    ctl("wait", "24")
    ctl("select", toolbox)
    command("tp @s 9.5 76 8.5")
    ctl("wait", "5")
    ctl("place", *target.split())
    command(f"execute if block {target} {toolbox}")
    command(f"execute if data block {target} Compartments[{diamond_kind}]")
    command(f"execute if data block {target} Compartments[{copper_kind}]")
    command(f"execute if data block {target} Items[{copper_stack}]")
    command(f"execute if data block {target} {{UniqueId:{original_uuid}}}")
    ctl("quit")
    print("Toolbox kept its two kinds, seven copper ingots and UUID through one drop and re-placement.")


if __name__ == "__main__":
    main()
