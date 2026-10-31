#!/usr/bin/env python3
"""Compare perf_bench reports (benchmarks/perf/<label>-rd<N>.json) against a baseline, as Markdown.

    python3 tools/perf_table.py benchmarks/perf/create-p0-modsoff-rd12.json benchmarks/perf/create-p0-idle-rd12.json

The first report is the baseline; each other column shows its mean and the change from it. A change
smaller than the two runs' combined 95% confidence intervals is marked "~" (noise).
"""
import json
import sys
from pathlib import Path

KEYS = ["startup.title_ms", "startup.world_ms"]
SCENES = ["village", "forest", "ocean", "mountain"]
for s in SCENES:
    KEYS += [f"fps.{s}.avg_fps", f"fps.{s}.low_1pct_fps", f"chunks.{s}.load_ms"]
KEYS += ["memory.steady_rss_mb", "memory.peak_rss_mb"]


def main(paths):
    reports = [json.loads(Path(p).read_text()) for p in paths]
    names = [r.get("label") or Path(p).stem for r, p in zip(reports, paths)]
    print("| metric | " + " | ".join(names) + " |")
    print("|---" * (len(names) + 1) + "|")
    for key in KEYS:
        base = reports[0]["metrics"].get(key)
        if not base:
            continue
        cells = [f"{base['mean']:.1f} ± {base['ci95']:.1f}"]
        for r in reports[1:]:
            m = r["metrics"].get(key)
            if not m:
                cells.append("-")
                continue
            delta = m["mean"] - base["mean"]
            pct = 100 * delta / base["mean"] if base["mean"] else 0.0
            noise = abs(delta) <= base["ci95"] + m["ci95"]
            cells.append(f"{m['mean']:.1f} ({pct:+.1f}%{' ~' if noise else ''})")
        print(f"| {key} | " + " | ".join(cells) + " |")
    print()
    print("load average before: " + ", ".join(
        f"{n} {r['conditions'].get('load_avg_1m')}" for n, r in zip(names, reports)))


if __name__ == "__main__":
    main(sys.argv[1:])
