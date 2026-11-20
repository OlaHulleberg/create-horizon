#!/usr/bin/env python3
"""Check toolbox notifications and loot in a disposable game (Linux, Python 3.9+)."""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import tempfile


def check_toolbox(ctl):
    def command(line, *, succeeds=True):
        reply = ctl("command", line)
        if reply["success"] != succeeds or reply["result"] is None:
            raise SystemExit(f"{line}: {reply}")
        return reply

    def block_data(pos, path):
        return command(f"data get block {pos} {path}")["chat"][0].split(": ", 1)[1]

    def kind_at(pos, index, item):
        path = f"Compartments[{index}]"
        saved = (block_data(pos, f"{path}.id"), block_data(pos, f"{path}.count"))
        if saved != (json.dumps(item), "1"):
            raise SystemExit(f"{pos} {path}: expected one {item}, got {saved}")

    toolbox = "create:white_toolbox"
    source = "9 76 9"
    target = "10 76 8"
    diamond = "minecraft:diamond"
    copper = "minecraft:copper_ingot"
    copper_stack = '{id:"minecraft:copper_ingot",count:7,Slot:4b}'

    ctl("open", "ToolboxSmoke", "0", "creative")
    ctl("wait-ready")
    # A fill may already match the terrain; the subsequent block and item checks must succeed.
    ctl("command", "fill 8 76 7 11 79 10 air")
    ctl("command", "fill 8 75 7 11 75 10 minecraft:stone")
    command(f"setblock {source} {toolbox}")

    command(f"item replace block {source} container.0 with {diamond} 3")
    kind_at(source, 0, diamond)
    command(f"item replace block {source} container.0 with minecraft:air")
    command(f"execute if data block {source} Items[0]", succeeds=False)
    kind_at(source, 0, diamond)
    command(f"item replace block {source} container.4 with {copper} 7")
    kind_at(source, 0, diamond)
    kind_at(source, 1, copper)
    command(f"execute if data block {source} Items[{copper_stack}]")
    original_uuid = block_data(source, "UniqueId")
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
    kind_at(target, 0, diamond)
    kind_at(target, 1, copper)
    command(f"execute if data block {target} Items[{copper_stack}]")
    command(f"execute if data block {target} {{UniqueId:{original_uuid}}}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ctl", type=Path, help="the pinned Nightly's sunrise-ctl")
    parser.add_argument("game", type=Path, help="the pinned sunrise-client staged with the Create native mod")
    parser.add_argument("resources", type=Path, help="a private copy of the Nightly's prepared resources")
    parser.add_argument("jars", type=Path, help="folder with the verified Create jar named by SHA-512")
    args = parser.parse_args()
    if not hasattr(os, "pidfd_open"):
        parser.error("this smoke test requires Linux pidfd support")

    with tempfile.TemporaryDirectory(prefix="toolbox-smoke-") as tmp:
        scratch = Path(tmp).resolve()
        game_dir = scratch / "game"
        game_dir.mkdir()
        (game_dir / "eula.txt").write_text("eula=true\n")
        home = scratch / "home"
        shutil.copytree(args.jars, home / "jars")
        ctl_bin = args.ctl.resolve()
        socket = scratch / "control.sock"
        env = dict(os.environ, SUNRISE_HOME=str(home), SUNRISE_CONTROL=str(socket))

        def ctl(*words, timeout=90):
            call = subprocess.run(
                [ctl_bin, "--socket", socket, "--timeout", str(timeout), *words],
                text=True, stdout=subprocess.PIPE, timeout=timeout + 5, cwd=game_dir, env=env,
            )
            reply = json.loads(call.stdout)
            if call.returncode or reply.get("ok") is False:
                raise SystemExit(f"{words}: {reply}")
            return reply

        launch = ctl(
            "launch", "--game", args.game.resolve(), "--game-dir", game_dir,
            "--resources", args.resources.resolve(), "--test-terrain", "--render-distance", "2",
            "--width", "640", "--height", "480",
        )
        pidfd = os.pidfd_open(launch["pid"])
        try:
            check_toolbox(ctl)
        finally:
            # Quit replies before the world finishes saving. Wait before deleting its directory.
            try:
                ctl("quit", timeout=10)
            finally:
                if not select.select([pidfd], [], [], 15)[0]:
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                    select.select([pidfd], [], [], 5)
                os.close(pidfd)
    print("Toolbox kept diamond in compartment 0, copper in compartment 1, seven ingots and its UUID.")


if __name__ == "__main__":
    main()
