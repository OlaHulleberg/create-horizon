#!/usr/bin/env python3
"""Tick and frame times of the built game for one Create scenario, through /workspace/headless.sh.

Each run starts the game invisibly (Xvfb, --offscreen, scratch homes) from a scratch folder, checks that
the renderer is the discrete GPU (not llvmpipe), builds the scenario with commands, holds the camera
over it and then flies a fixed path with `horizon-ctl fly ... raw`, which reports every frame time and
the server thread's smoothed tick time. Scenarios:

  off      no content mod (Horizon as it is)
  idle     the Create content mod on, no Create block in the world
  static   the content mod on, a 32x32 floor of Create decoration blocks in view (phase 1)

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
CENTER = (8, 100, 8)
STATIC_BLOCKS = ["andesite_casing", "brass_casing", "copper_casing", "cut_andesite_bricks",
                 "polished_cut_crimsite", "layered_veridium", "framed_glass", "copper_shingles"]


def ctl(ctl_bin, sock, *args, timeout=600):
    out = subprocess.run([str(ctl_bin), "--socket", sock, "--timeout", str(timeout), *args],
                         capture_output=True, text=True, timeout=timeout + 30)
    if out.returncode:
        raise RuntimeError(f"horizon-ctl {' '.join(args)}: {out.stderr.strip()}")
    return json.loads(out.stdout) if out.stdout.strip().startswith("{") else out.stdout


def build(ctl_bin, sock, scenario):
    x, y, z = CENTER
    ctl(ctl_bin, sock, "command", f"fill {x-20} {y-6} {z-20} {x+20} {y+20} {z+20} air")
    if scenario == "static":
        # 32x32 floor, one block kind per 4-wide stripe: 1024 Create blocks in view.
        for i, name in enumerate(STATIC_BLOCKS):
            x0 = x - 16 + i * 4
            ctl(ctl_bin, sock, "command", f"fill {x0} {y-5} {z-16} {x0+3} {y-5} {z+15} create:{name}")


def run_once(args, scenario):
    work = Path(tempfile.mkdtemp(prefix="create-bench-"))
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
        build(ctl_bin, sock, scenario)
        x, y, z = CENTER
        ctl(ctl_bin, sock, "tp", str(x), str(y), str(z))
        ctl(ctl_bin, sock, "fly", str(x), str(y), str(z), str(x), str(y), str(z), "0.1", "40")
        ctl(ctl_bin, sock, "wait-ready")
        time.sleep(args.settle)
        # A slow circle-ish line over the scenario, looking down at it.
        summary = ctl(ctl_bin, sock, "fly", str(x - 12), str(y), str(z - 12), str(x + 12), str(y), str(z + 12),
                      str(args.seconds), "40", "raw", timeout=int(args.seconds) + 120)
        frames = summary.get("frame_ms") or []
        return {"gpu": gpu, "frames": len(frames),
                "frame_ms_p50": statistics.median(frames) if frames else None,
                "frame_ms_p99": sorted(frames)[int(len(frames) * 0.99) - 1] if frames else None,
                "fps": len(frames) / args.seconds,
                "tick_ms": summary.get("tick_ms"), "tick_ms_max": summary.get("tick_ms_max")}
    finally:
        try:
            ctl(ctl_bin, sock, "quit", timeout=60)
        except Exception:
            pass
        pid = scratch / "xvfb.pid"
        if pid.exists():
            subprocess.run(["kill", pid.read_text().strip()], capture_output=True)
        shutil.rmtree(scratch, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--game", default=str(ROOT / "target/horizon-cargo/fast/horizon-client"))
    p.add_argument("--content", default=str(ROOT / "target/create-content"))
    p.add_argument("--scenario", choices=["off", "idle", "static"], required=True)
    p.add_argument("--repeats", type=int, default=3)
    p.add_argument("--seconds", type=float, default=12.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--distance", type=int, default=12)
    p.add_argument("--out")
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
