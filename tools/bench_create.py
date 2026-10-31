#!/usr/bin/env python3
"""Tick and frame times of the built game for one Create scenario, through /workspace/headless.sh.

Each run starts the game invisibly (Xvfb, --offscreen, scratch homes) from a scratch folder, checks that
the renderer is the discrete GPU (not llvmpipe), builds the scenario with commands, holds the camera
over it and then flies a fixed path with `horizon-ctl fly ... raw`, which reports every frame time and
the server thread's smoothed tick time. Scenarios:

  off      no content mod (Horizon as it is)
  idle     the Create content mod on, no Create block in the world
  static   the content mod on, a 32x32 floor of Create decoration blocks in view (phase 1)
  kinetic  the content mod on, 64 creative motors each turning a line of 63 shafts: 4,096 turning
           parts in view, steady (phase 2)
  chains   the control for kinetic: the same layout in vanilla iron blocks and iron chains
  processing  16 rows of a creative motor turning 16 mechanical presses, each over a depot of 64
           iron ingots: 256 presses pressing at 16 rpm (phase 3)
  logistics  32 belts 48 long at 16 rpm, each carrying 21 stacks dropped on it: 672 stacks moving
           (phase 4)
  redstone 32 observer clocks, each driving a chain of 16 toggle latches (a binary counter): 512
           latches switching (phase 7)
  repeaters  the control for redstone: the same chains of vanilla repeaters

    python3 tools/bench_create.py --game target/horizon-cargo/fast/horizon-client \\
        --content target/create-content --scenario idle --repeats 3 --out benchmarks/p0-idle.json
"""
import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADLESS = Path("/workspace/headless.sh")
# The scene is built this far above the spawn point, in loaded chunks.
LIFT = 30
STATIC_BLOCKS = ["andesite_casing", "brass_casing", "copper_casing", "cut_andesite_bricks",
                 "polished_cut_crimsite", "layered_veridium", "framed_glass", "copper_shingles"]


def ctl(ctl_bin, sock, *args, timeout=600):
    out = subprocess.run([str(ctl_bin), "--socket", sock, "--timeout", str(timeout), *args],
                         capture_output=True, text=True, timeout=timeout + 30)
    if out.returncode:
        raise RuntimeError(f"horizon-ctl {' '.join(args)}: {out.stderr.strip()}")
    return json.loads(out.stdout) if out.stdout.strip().startswith("{") else out.stdout


def command(ctl_bin, sock, line, failures):
    chat = ctl(ctl_bin, sock, "command", line).get("chat", [])
    bad = [c for c in chat if not c.startswith(("Changed the block", "Successfully filled", "No blocks were"))]
    if bad:
        failures.append((line, bad))


def build(ctl_bin, sock, scenario, center):
    """Builds the scene; returns the commands that failed (an unloaded position, a limit)."""
    x, y, z = center
    failures = []
    # Clear layer by layer: one fill may change at most 32,768 blocks.
    for yy in range(y - 7, y + 4):
        command(ctl_bin, sock, f"fill {x-32} {yy} {z-32} {x+32} {yy} {z+32} air", failures)
    if scenario == "static":
        # 32x32 floor, one block kind per 4-wide stripe: 1024 Create blocks in view.
        for i, name in enumerate(STATIC_BLOCKS):
            x0 = x - 16 + i * 4
            command(ctl_bin, sock, f"fill {x0} {y-5} {z-16} {x0+3} {y-5} {z+15} create:{name}", failures)
    if scenario in ("kinetic", "chains"):
        # `chains`, the control: the same layout in vanilla blocks (the shaft's template, an iron chain).
        motor, shaft = (("create:creative_motor[facing=east]", "create:shaft[axis=x]") if scenario == "kinetic"
                        else ("minecraft:iron_block", "minecraft:iron_chain[axis=x]"))
        for row in range(64):
            zz = z - 32 + row
            command(ctl_bin, sock, f"setblock {x-32} {y-6} {zz} {motor}", failures)
            command(ctl_bin, sock, f"fill {x-31} {y-6} {zz} {x+31} {y-6} {zz} {shaft}", failures)
    if scenario == "processing":
        for row in range(16):
            zz = z - 16 + 2 * row
            command(ctl_bin, sock, f"setblock {x-17} {y-4} {zz} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"fill {x-16} {y-4} {zz} {x-1} {y-4} {zz} create:mechanical_press[facing=east]",
                    failures)
            # Placed holding their stack, as a saved depot loads (one by one: Horizon's `fill`
            # drops block entity data for mod blocks).
            for i in range(16):
                command(ctl_bin, sock, f"setblock {x-16+i} {y-6} {zz} "
                        'create:depot{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:64}]}', failures)
    if scenario == "logistics":
        belt = "facing=south,slope=horizontal,casing=false,waterlogged=false"
        for row in range(32):
            xx = x - 31 + 2 * row
            command(ctl_bin, sock, f"setblock {xx-1} {y-6} {z-24} create:creative_motor[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z-24} create:belt[{belt},part=start]", failures)
            command(ctl_bin, sock, f"fill {xx} {y-6} {z-23} {xx} {y-6} {z+22} create:belt[{belt},part=middle]",
                    failures)
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+23} create:belt[{belt},part=end]", failures)
            # A stone past the end: the stacks queue up instead of falling off.
            command(ctl_bin, sock, f"setblock {xx} {y-6} {z+24} minecraft:stone", failures)
        # Wait for the chains to form, then drop a stack on every other segment of the first 42.
        time.sleep(1)
        for row in range(32):
            xx = x - 31 + 2 * row
            for s in range(0, 42, 2):
                ctl(ctl_bin, sock, "command",
                    f"summon minecraft:item {xx + 0.5} {y - 5} {z - 24 + s + 0.5} "
                    "{Item:{id:'minecraft:iron_ingot',count:8}}")
    if scenario in ("redstone", "repeaters"):
        diode = ("create:powered_toggle_latch[facing=west]" if scenario == "redstone"
                 else "minecraft:repeater[facing=west]")
        for row in range(32):
            zz = z - 31 + 2 * row
            command(ctl_bin, sock, f"fill {x-17} {y-7} {zz} {x+2} {y-7} {zz} minecraft:stone", failures)
            command(ctl_bin, sock, f"fill {x-14} {y-6} {zz} {x+1} {y-6} {zz} {diode}", failures)
            # Two observers watching each other: a clock whose pulses leave the east one's back.
            command(ctl_bin, sock, f"setblock {x-16} {y-6} {zz} minecraft:observer[facing=east]", failures)
            command(ctl_bin, sock, f"setblock {x-15} {y-6} {zz} minecraft:observer[facing=west]", failures)
    return failures


def count_create(ctl_bin, sock, center):
    """Create blocks the scene really holds, counted in 16-row bands: one `blocks` reply lists at
    most 4,096 blocks."""
    x, y, z = center
    total = 0
    for band in range(z - 32, z + 33, 16):
        found = ctl(ctl_bin, sock, "blocks", str(x - 32), str(y - 7), str(band), str(x + 32), str(y - 4),
                    str(min(band + 15, z + 32)))
        total += sum(n for k, n in found.get("counts", {}).items() if k.startswith("create:"))
    return total


def run_once(args, scenario):
    work = Path(tempfile.mkdtemp(prefix="create-bench-"))
    # As perf_bench.py: no frame cap and no VSync, so frame times show the work, not the cap; and
    # no idle cap (vanilla's `inactivityFpsLimit` "afk" drops to 30 fps a minute after the last
    # input, which a long scene build reaches).
    (work / "options.txt").write_text("maxFps:260\nenableVsync:false\nrenderDistance:%d\n"
                                      "simulationDistance:12\npauseOnLostFocus:false\n"
                                      "inactivityFpsLimit:minimized\n" % args.distance)
    game = Path(args.game).resolve()
    launch = ["--world", "B", "--creative", "--seed", "1234", "--no-pause", "--weather", "clear",
              "--time", "6000", "--render-distance", str(args.distance)]
    if scenario != "off":
        launch += ["--dev-mod", str(Path(args.content).resolve())]
    out = subprocess.run([str(HEADLESS), str(game), *launch], cwd=work, capture_output=True, text=True)
    sock = re.search(r"SOCK=(\S+)", out.stdout).group(1)
    scratch = Path(sock).parent
    ctl_bin = game.parent / "horizon-ctl"
    try:
        launch_state = json.loads((scratch / "launch.log").read_text().strip().splitlines()[-1])
        gpu = launch_state["renderer"]["gpu"]
        if "llvmpipe" in gpu.lower() or "BMG" not in gpu:
            raise SystemExit(f"wrong renderer: {gpu}")
        ctl(ctl_bin, sock, "wait-ready")
        spawn = ctl(ctl_bin, sock, "state")["player"]["position"]
        center = (int(spawn[0]), int(spawn[1]) + LIFT, int(spawn[2]))
        expected = {"off": 0, "idle": 0, "static": 1024, "kinetic": 4096, "chains": 0,
                    "processing": 528, "logistics": 32 * 48 + 32, "redstone": 512,
                    "repeaters": 0}[scenario]
        for attempt in range(5):
            failures = build(ctl_bin, sock, scenario, center)
            have = count_create(ctl_bin, sock, center)
            if have == expected:
                break
            print(f"attempt {attempt}: {have} of {expected} Create blocks; failed: {failures[:3]}")
            time.sleep(2)
        else:
            raise SystemExit(f"scene holds {have} Create blocks, not {expected}")
        x, y, z = center
        ctl(ctl_bin, sock, "tp", str(x), str(y), str(z))
        ctl(ctl_bin, sock, "fly", str(x), str(y), str(z), str(x), str(y), str(z), "0.1", "40")
        ctl(ctl_bin, sock, "wait-ready")
        time.sleep(args.settle)
        # A slow circle-ish line over the scenario, looking down at it.
        summary = ctl(ctl_bin, sock, "fly", str(x - 12), str(y), str(z - 12), str(x + 12), str(y), str(z + 12),
                      str(args.seconds), "40", "raw", timeout=int(args.seconds) + 120)
        if args.screenshot:
            ctl(ctl_bin, sock, "screenshot", str(Path(args.screenshot).resolve()))
        pressed = None
        if scenario == "logistics":
            # The stacks are on the belts, not lying around as entities.
            ents = ctl(ctl_bin, sock, "entities")
            pressed = sum(1 for e in ents.get("entities", []) if "item" in str(e.get("type", "")))
        if scenario in ("redstone", "repeaters"):
            # The counters really count: the states along one chain.
            chain = ctl(ctl_bin, sock, "blocks", str(x - 14), str(y - 6), str(z - 31), str(x + 1), str(y - 6),
                        str(z - 31)).get("blocks", [])
            # A latch's output is `powering`; a repeater's, `powered`. Two reads a second apart:
            # a counter changes between them.
            key = "powering=true" if scenario == "redstone" else "powered=true"
            first = "".join("1" if key in b[3] else "0" for b in sorted(chain))
            time.sleep(1)
            chain = ctl(ctl_bin, sock, "blocks", str(x - 14), str(y - 6), str(z - 31), str(x + 1), str(y - 6),
                        str(z - 31)).get("blocks", [])
            pressed = [first, "".join("1" if key in b[3] else "0" for b in sorted(chain))]
        if scenario == "processing":
            # The work really ran: sheets in one depot's outputs.
            pressed = ctl(ctl_bin, sock, "command", f"data get block {x-8} {y-6} {z}").get("chat")
        frames = summary.get("frame_ms") or []
        return {"gpu": gpu, "create_blocks": have, "frames": len(frames),
                "frame_ms_p50": statistics.median(frames) if frames else None,
                "frame_ms_p99": sorted(frames)[int(len(frames) * 0.99) - 1] if frames else None,
                "fps": len(frames) / args.seconds,
                "tick_ms": summary.get("tick_ms"), "tick_ms_max": summary.get("tick_ms_max"),
                "check": pressed}
    finally:
        try:
            ctl(ctl_bin, sock, "quit", timeout=60)
        except Exception:
            pass
        pid = scratch / "xvfb.pid"
        if pid.exists():
            subprocess.run(["kill", pid.read_text().strip()], capture_output=True)
        if args.keep_logs:
            keep = Path(args.keep_logs) / f"{scenario}-{int(time.time())}"
            keep.mkdir(parents=True, exist_ok=True)
            for log in scratch.glob("*.log"):
                shutil.copy(log, keep / log.name)
        shutil.rmtree(scratch, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--game", default=str(ROOT / "target/horizon-cargo/fast/horizon-client"))
    p.add_argument("--content", default=str(ROOT / "target/create-content"))
    p.add_argument("--scenario", choices=["off", "idle", "static", "kinetic", "chains", "processing",
                                          "logistics", "redstone", "repeaters"], required=True)
    p.add_argument("--repeats", type=int, default=3)
    p.add_argument("--seconds", type=float, default=12.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--distance", type=int, default=12)
    p.add_argument("--out")
    p.add_argument("--screenshot", help="a screenshot of the scene after the last flight")
    p.add_argument("--keep-logs", help="copy each run's game logs into this folder")
    args = p.parse_args()
    runs = [run_once(args, args.scenario) for _ in range(args.repeats)]
    keys = ["fps", "frame_ms_p50", "frame_ms_p99", "tick_ms", "tick_ms_max"]
    result = {"scenario": args.scenario, "distance": args.distance, "seconds": args.seconds,
              "loadavg_before": os.getloadavg(), "runs": runs,
              "mean": {k: statistics.mean(r[k] for r in runs) for k in keys},
              "stdev": {k: statistics.stdev([r[k] for r in runs]) if len(runs) > 1 else 0.0 for k in keys}}
    text = json.dumps(result, indent=2)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
