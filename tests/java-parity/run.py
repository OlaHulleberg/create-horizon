#!/usr/bin/env python3
"""Compare Create's pinned Java method bodies with the public Rust engine.

No game, SDK, jar, account, player directory or network is needed. Java builds
only isolated fixture contexts, so this is not an in-game parity certificate.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

from specs import SPECS

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
REFERENCE = json.loads((HERE / "reference.json").read_text())


def execute(argv, *, stdin=None, cwd=None, timeout=300):
    result = subprocess.run(list(map(str, argv)), input=stdin, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            cwd=cwd, timeout=timeout, check=False)
    if result.returncode:
        raise RuntimeError(f"{argv[0]} exited {result.returncode}\n{result.stderr[-12000:]}")
    return result.stdout


def source_at(checkout, path):
    if not path.startswith("src/main/java/com/simibubi/create/") or ".." in Path(path).parts:
        raise ValueError(f"Refusing non-Create source path: {path}")
    return execute(["git", "-C", checkout, "show", f"{REFERENCE['revision']}:{path}"])


def compare_lines(spec, cases, expected, actual, example_limit=20):
    """A malformed/truncated stream is a harness error, never a partial pass."""
    if not cases or len(expected) != len(cases) or len(actual) != len(cases):
        raise ValueError(f"Row count: cases={len(cases)}, Java={len(expected)}, Rust={len(actual)}")
    counts = Counter(passed=0, different=0, unsupported=0, panicked=0)
    reasons = Counter()
    examples = []
    sampled = Counter()
    for i, (case, java, rust) in enumerate(zip(cases, expected, actual)):
        if not case.strip() or not java.strip() or not rust.strip():
            raise ValueError(f"Empty case or observation at row {i}")
        if rust.startswith("!harness "):
            raise ValueError(f"Rust adapter error at row {i}: {case[:500]}: {rust}")
        if rust.startswith("!unsupported "):
            outcome, reason = "unsupported", rust.removeprefix("!unsupported ")
        elif rust.startswith("!panic "):
            outcome, reason = "panicked", rust.removeprefix("!panic ")
        elif spec.observations_equal:
            outcome, reason = (("passed", "") if spec.observations_equal(java, rust)
                               else ("different", "structured observation differs"))
        elif spec.name == "network":
            outcome, reason = (("passed", "") if json.loads(java) == json.loads(rust)
                               else ("different", "network totals differ"))
        elif case.split()[0] in spec.numeric_tags:
            left, right = java.split(), rust.split()
            equal = bool(left) and len(left) == len(right)
            for a, b in zip(left, right):
                a, b = float(a), float(b)
                equal &= math.isfinite(a) and math.isfinite(b) and (a == b or abs(a-b) < spec.tolerance)
            outcome, reason = ("passed", "") if equal else ("different", "numeric observation differs")
        else:
            outcome, reason = (("passed", "") if java.split() == rust.split()
                               else ("different", "state observation differs"))
        counts[outcome] += 1
        if outcome != "passed":
            reasons[reason] += 1
            if sampled[outcome] < example_limit:
                examples.append({"index": i, "input": case, "java": java[:3000],
                                 "rust": rust[:3000], "outcome": outcome})
                sampled[outcome] += 1
    return {"cases": len(cases), **counts, "reasons": dict(reasons), "examples": examples}


def java_reference(spec, checkout, work, java, javac):
    folder = work / spec.name
    folder.mkdir(parents=True, exist_ok=True)
    if spec.name == "network":
        path="src/main/java/com/simibubi/create/content/kinetics/KineticNetwork.java"
        text=source_at(checkout,path)
        source=folder / "KineticNetwork.java"
        source.write_text(text)
        execute([javac,"-d",folder,source,HERE / "java/KineticBlockEntity.java",
                 HERE / "java/NetworkOracle.java",HERE / "java/NetworkBatch.java"])
        return folder,[{"path":path,"source_sha256":hashlib.sha256(text.encode()).hexdigest(),"scope":"complete class"}]
    template = (HERE / "java" / (spec.template + ".java.in")).read_text()
    methods = []
    for marker, path, method, selector in spec.methods:
        text = source_at(checkout, path)
        source = folder / Path(path).name
        source.write_text(text)
        command = [java, "-cp", work, "ExportMethod", source, method]
        if selector:
            command.append(selector)
        body = execute(command)
        token = "@@" + marker + "@@"
        if template.count(token) != 1:
            raise ValueError(f"Expected exactly one template token {token}")
        template = template.replace(token, body)
        methods.append({"path": path, "method": method, "selector": selector,
                        "source_sha256": hashlib.sha256(text.encode()).hexdigest(),
                        "method_sha256": hashlib.sha256(body.encode()).hexdigest()})
    if re.search(r"@@[A-Z_]+@@", template):
        raise ValueError("Unresolved Java template token")
    source = folder / (spec.template + ".java")
    source.write_text(template)
    execute([javac, "-d", folder, source])
    return folder, methods


def write_report(report, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2) + "\n")
    lines = ["# Create Java behaviour comparison", "",
             f"Java reference: Create {REFERENCE['version']} at `{REFERENCE['revision']}`.", "",
             "This report covers isolated method contexts, not a Minecraft world or rendered pixels.", "",
             "| Suite | Cases | Match | Difference | Unavailable | Panic | Harness error |",
             "|---|---:|---:|---:|---:|---:|---|"]
    for s in report["suites"]:
        lines.append(f"| {s['name']} | {s.get('cases', 0)} | {s.get('passed', 0)} | "
                     f"{s.get('different', 0)} | {s.get('unsupported', 0)} | "
                     f"{s.get('panicked', 0)} | {'yes' if s.get('error') else '—'} |")
    if report.get("error"):
        lines += ["", "## Harness error", "", "```text", report["error"][:12000], "```"]
    for s in report["suites"]:
        if s.get("error") or s.get("reasons"):
            lines += ["", "## " + s["name"], "", s["scope"]]
            if s.get("error"):
                lines += ["", "```text", s["error"][:12000], "```"]
            for reason, count in s.get("reasons", {}).items():
                lines += ["", f"- {count}: {reason}"]
            shown = Counter()
            for example in s.get("examples", []):
                if shown[example["outcome"]] >= 2:
                    continue
                shown[example["outcome"]] += 1
                lines += ["", "```json", json.dumps(example, ensure_ascii=False), "```"]
    path.with_suffix(".md").write_text("\n".join(lines) + "\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--create-source", type=Path, required=True,
                        help="Git checkout containing the pinned Create commit; read only")
    parser.add_argument("--report", type=Path, default=ROOT / "target/java-parity/report.json")
    parser.add_argument("--suite", action="append", choices=[s.name for s in SPECS])
    parser.add_argument("--probe", type=Path, help="An already built test probe (CI/repeated runs)")
    parser.add_argument("--report-only", action="store_true",
                        help="Explicit observational mode: write all differences, exit 0; not a passing parity gate")
    args = parser.parse_args(argv)
    java, javac, cargo = (shutil.which(name) for name in ["java", "javac", "cargo"])
    if not java or not javac or (not args.probe and not cargo):
        parser.error("JDK and Cargo must be on PATH (or pass --probe for an existing probe)")
    work = args.report.resolve().parent / "work"
    work.mkdir(parents=True, exist_ok=True)
    report = {"schema": 1, "reference": REFERENCE, "mode": "report-only" if args.report_only else "gate",
              "scope": "isolated Java methods versus public engine adapters", "suites": []}
    start = time.monotonic()
    try:
        report["subject_revision"] = execute(["git", "-C", ROOT, "rev-parse", "HEAD"]).strip()
        report["subject_engine_tree"] = execute(["git", "-C", ROOT, "rev-parse", "HEAD:create/src/engine"]).strip()
        report["subject_engine_diff"] = execute(["git", "-C", ROOT, "diff", "HEAD", "--", "create/src/engine"])
        report["java_runtime"] = subprocess.run([java, "-version"], text=True, stdout=subprocess.PIPE,
                                                stderr=subprocess.STDOUT, check=True).stdout.strip()
        resolved = execute(["git", "-C", args.create_source, "rev-parse", REFERENCE["revision"] + "^{commit}"]).strip()
        if resolved != REFERENCE["revision"]:
            raise ValueError("The Java reference commit differs")
        version = execute(["git", "-C", args.create_source, "show", resolved + ":gradle.properties"])
        if not re.search(r"^mod_version\s*=\s*6\.0\.11\s*$", version, re.MULTILINE):
            raise ValueError("The reference is not Create 6.0.11")
        execute([javac, "-d", work, HERE / "java/ExportMethod.java"])
        if args.probe:
            probe = args.probe.resolve()
        else:
            execute([cargo, "build", "--locked", "--manifest-path", HERE / "probe/Cargo.toml",
                     "--target-dir", ROOT / "target/java-parity/rust"], timeout=900)
            probe = ROOT / "target/java-parity/rust/debug/create-java-parity-probe"
        for spec in SPECS:
            if args.suite and spec.name not in args.suite:
                continue
            row = {"name": spec.name, "scope": spec.scope}
            try:
                folder, methods = java_reference(spec, args.create_source, work, java, javac)
                cases = spec.cases()
                stdin = "\n".join(cases) + "\n"
                expected = execute([java, "-cp", folder, spec.template], stdin=stdin).splitlines()
                if spec.projection:
                    if len(expected) != len(cases):
                        raise ValueError("Cannot project a truncated Java observation stream")
                    expected = [spec.projection(c, e) for c, e in zip(cases, expected)]
                actual = execute([probe, spec.name], stdin=stdin).splitlines()
                row.update(compare_lines(spec, cases, expected, actual))
                row["methods"] = methods
                row["inputs_sha256"] = hashlib.sha256(stdin.encode()).hexdigest()
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
                row["error"] = str(error)
            report["suites"].append(row)
            write_report(report, args.report)
            print(spec.name, {k: v for k, v in row.items() if k in ["cases", "passed", "different", "unsupported", "panicked", "error"]}, flush=True)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        report["error"] = str(error)
    report["elapsed_seconds"] = round(time.monotonic() - start, 3)
    report["complete_match"] = bool(report["suites"]) and not report.get("error") and all(
        not s.get("error") and s.get("passed") == s.get("cases") for s in report["suites"])
    write_report(report, args.report)
    if report.get("error") or any(s.get("error") for s in report["suites"]):
        if report.get("error"):
            print(report["error"], file=sys.stderr)
        return 2
    return 0 if args.report_only or report["complete_match"] else 1


if __name__ == "__main__":
    sys.exit(main())
