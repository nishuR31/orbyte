#!/usr/bin/env python3
"""Wall-clock comparison of orb (debug/release) with node and bun.

Interleaves runs so machine noise affects all tools equally. Results are
printed as a Markdown table. Usage:

    python3 benches/compare.py [--runs 15] [--filter NAME]

Set ORB_DEBUG / ORB_RELEASE to override binary paths.
"""
from __future__ import annotations

import argparse
import os
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE_SUFFIX = ".exe" if os.name == "nt" else ""
ORB_DEBUG = os.environ.get("ORB_DEBUG", str(ROOT / f"target/debug/orb{EXE_SUFFIX}"))
ORB_RELEASE = os.environ.get("ORB_RELEASE", str(ROOT / f"target/release/orb{EXE_SUFFIX}"))

# Each case maps a tool key to argv. Tools that cannot run a case are skipped.
CASES: dict[str, dict[str, list[str]]] = {
    "startup": {
        "orb": ["{orb}", "--version"],
        "node": ["{node}", "--version"],
        "bun": ["{bun}", "--version"],
    },
    "native-calc": {
        "orb": ["{orb}", "run", "examples/javascript/calculate.js"],
        "node": ["{node}", "examples/javascript/calculate.js"],
        "bun": ["{bun}", "examples/javascript/calculate.js"],
    },
    "native-fib32": {
        "orb": ["{orb}", "run", "examples/javascript/fib32.js"],
        "node": ["{node}", "examples/javascript/fib32.js"],
        "bun": ["{bun}", "examples/javascript/fib32.js"],
    },
    "native-primes100k": {
        "orb": ["{orb}", "run", "examples/javascript/primes100k.js"],
        "node": ["{node}", "examples/javascript/primes100k.js"],
        "bun": ["{bun}", "examples/javascript/primes100k.js"],
    },
    "fallback-js": {
        "orb": ["{orb}", "run", "testCaseJs.js"],
        "node": ["{node}", "testCaseJs.js"],
        "bun": ["{bun}", "testCaseJs.js"],
    },
    "fallback-ts": {
        "orb": ["{orb}", "run", "testCaseTs.ts"],
        "bun": ["{bun}", "testCaseTs.ts"],
    },
}

TOOLS = [
    ("orb debug", "orb", ORB_DEBUG),
    ("orb release", "orb", ORB_RELEASE),
    ("node", "node", shutil.which("node")),
    ("bun", "bun", shutil.which("bun")),
]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--filter", default="")
    args = parser.parse_args()

    available = [(label, key, path) for label, key, path in TOOLS if path and Path(path).exists()]
    cases = {name: argv for name, argv in CASES.items() if args.filter in name}

    samples: dict[tuple[str, str], list[float]] = {}
    with open(os.devnull, "wb") as devnull:
        for _ in range(args.runs):
            for label, key, path in available:
                for case_name, per_tool in cases.items():
                    argv = per_tool.get(key)
                    if argv is None:
                        continue
                    cmd = [path if part.startswith("{") else part for part in argv]
                    started = time.perf_counter()
                    result = subprocess.run(cmd, cwd=ROOT, stdout=devnull, stderr=devnull, timeout=60)
                    elapsed = (time.perf_counter() - started) * 1000
                    if result.returncode == 0:
                        samples.setdefault((label, case_name), []).append(elapsed)

    print(f"# orb wall-clock comparison ({args.runs} runs, median ms)\n")
    print("| case | " + " | ".join(label for label, _, _ in available) + " |")
    print("|---|" + "|".join("---:" for _ in available) + "|")
    for case_name in cases:
        cells = []
        for label, _, _ in available:
            xs = samples.get((label, case_name), [])
            cells.append(f"{statistics.median(xs):.2f}" if xs else "n/a")
        print(f"| {case_name} | " + " | ".join(cells) + " |")
    print()

    for label, _, _ in available:
        xs = [x for (l, _), values in samples.items() if l == label for x in values]
        if xs:
            print(f"- {label}: {len(xs)} successful runs, overall mean {statistics.mean(xs):.2f} ms")


if __name__ == "__main__":
    main()
