#!/usr/bin/env python3
"""Wall-clock comparison of orb (JIT, compat, AOT) with bun, deno and node.

tsgo and tsz are TypeScript checkers, not runners, so they are measured in a
separate type-check section rather than mixed into the execution table.

Every runner executes the same program that prints 610. Each measurement is a
fresh process, so interpreter/JIT startup is included. tsgo is a type checker,
not a runner, so it is measured separately and never mixed into the table.

Usage:
    cargo build --release
    ./target/release/orb build --native benches/cases/fib.js -o benches/cases/fib_native
    python3 benches/runtime_bench.py [--runs 50]

Prints a Markdown table and writes docs/benchmark-results.json.
"""

import argparse
import json
import shutil
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
JS = ROOT / "benches/cases/fib.js"
NATIVE = ROOT / "benches/cases/fib_native"
EXPECTED = "75025"  # fib(25)


def available(program):
    return shutil.which(program) is not None


def runners():
    orb = ROOT / "target/release/orb"
    runners = []
    if NATIVE.exists():
        runners.append(("orb AOT binary", [str(NATIVE)], JS))
    if orb.exists():
        runners.append(("orb JIT (--native)", [str(orb), "run", "--native", str(JS)], JS))
        runners.append(("orb compat (--compat)", [str(orb), "run", "--compat", str(JS)], JS))
    if available("bun"):
        runners.append(("bun", ["bun", str(JS)], JS))
    if available("deno"):
        runners.append(("deno (--no-check)", ["deno", "run", "--no-check", str(JS)], JS))
    if available("node"):
        runners.append(("node", ["node", str(JS)], JS))
    return runners


def measure(name, argv, runs, check_output=True):
    for _ in range(5):
        subprocess.run(argv, capture_output=True, text=True, cwd=ROOT)
    samples = []
    for _ in range(runs):
        import time

        start = time.perf_counter()
        result = subprocess.run(argv, capture_output=True, text=True, cwd=ROOT)
        samples.append((time.perf_counter() - start) * 1000.0)
        bad_output = check_output and result.stdout.strip() != EXPECTED
        if result.returncode != 0 or bad_output:
            print(
                f"warning: {name} returned {result.returncode} / {result.stdout.strip()!r}",
                file=sys.stderr,
            )
            return None
    return {
        "runner": name,
        "median_ms": round(statistics.median(samples), 3),
        "min_ms": round(min(samples), 3),
        "runs": runs,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=50)
    args = parser.parse_args()

    results = []
    for name, argv, _ in runners():
        result = measure(name, argv, args.runs)
        if result:
            results.append(result)

    typechecks = []
    for name, program in [("tsgo", "tsgo"), ("tsz", "tsz")]:
        if available(program):
            result = measure(
                f"{name} --noEmit (type check)",
                [program, "--noEmit", "-p", str(ROOT / "benches/cases/tsconfig.json")],
                args.runs,
                check_output=False,
            )
            if result:
                typechecks.append(result)

    table = ["| Runner | Median (ms) | Best (ms) |", "|---|---:|---:|"]
    for row in results:
        table.append(f"| {row['runner']} | {row['median_ms']:.3f} | {row['min_ms']:.3f} |")
    print("\n".join(table))
    if typechecks:
        print("\nSeparate job (not comparable with execution):")
        for row in typechecks:
            print(f"  {row['runner']}: {row['median_ms']:.3f} ms median")

    out = ROOT / "docs/benchmark-results.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(
        json.dumps(
            {
                "workload": "fib(25) + console.log, fresh process per run",
                "machine": "local development machine, Linux x86_64",
                "runs": args.runs,
                "results": results,
                "typechecks": typechecks,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
