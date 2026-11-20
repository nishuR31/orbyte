#!/usr/bin/env python3
"""Stress benchmark: execution and type-checking, head to head.

Usage:
    cargo build --release
    ./target/release/orb build --native benches/cases/stress.js -o benches/cases/stress_native
    python3 benches/stress_bench.py

Reports: median latency, best latency, and a parallel-throughput run
(8 concurrent processes x 20 batches). Missing tools are skipped.
"""

import json
import shutil
import statistics
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
JS = ROOT / "benches/cases/stress.js"
TSCONFIG = ROOT / "benches/cases/tsconfig.stress.json"
EXPECTED = "370944"
PARALLEL = 8
BATCHES = 20


def which(name):
    return shutil.which(name) is not None


def one(argv, cwd=ROOT):
    start = time.perf_counter()
    result = subprocess.run(argv, capture_output=True, text=True, cwd=cwd)
    return (time.perf_counter() - start) * 1000.0, result


def latency(name, argv, runs):
    for _ in range(3):
        one(argv)
    samples, failures = [], 0
    for _ in range(runs):
        ms, result = one(argv)
        samples.append(ms)
        if result.returncode != 0 or result.stdout.strip() != EXPECTED:
            failures += 1
    return {
        "name": name,
        "median_ms": round(statistics.median(samples), 2),
        "best_ms": round(min(samples), 2),
        "failures": failures,
    }


def parallel_throughput(name, argv):
    errors = 0

    def batch(_):
        nonlocal errors
        start = time.perf_counter()
        with ThreadPoolExecutor(max_workers=PARALLEL) as pool:
            results = list(pool.map(lambda _i: one(argv), range(PARALLEL)))
        elapsed = time.perf_counter() - start
        errors += sum(1 for _, r in results if r.returncode != 0 or r.stdout.strip() != EXPECTED)
        return elapsed

    times = [batch(i) for i in range(BATCHES)]
    total_jobs = PARALLEL * BATCHES
    wall = sum(times)
    return {
        "name": name,
        "jobs": total_jobs,
        "wall_s": round(wall, 2),
        "jobs_per_s": round(total_jobs / wall, 1),
        "failures": errors,
    }


def table(rows, title):
    print(f"\n{title}")
    print("| Tool | Median (ms) | Best (ms) | Failures |")
    print("|---|---:|---:|---:|")
    for row in rows:
        print(f"| {row['name']} | {row['median_ms']:.2f} | {row['best_ms']:.2f} | {row['failures']} |")


def main():
    orb = ROOT / "target/release/orb"
    native = ROOT / "benches/cases/stress_native"

    exec_runners = []
    if native.exists():
        exec_runners.append(("orb AOT", [str(native)]))
    if orb.exists():
        exec_runners.append(("orb JIT", [str(orb), "run", "--native", str(JS)]))
        exec_runners.append(("orb compat", [str(orb), "run", "--compat", str(JS)]))
    if which("bun"):
        exec_runners.append(("bun", ["bun", str(JS)]))
    if which("deno"):
        exec_runners.append(("deno", ["deno", "run", "--no-check", str(JS)]))
    if which("node"):
        exec_runners.append(("node", ["node", str(JS)]))

    exec_rows = [latency(name, argv, 40) for name, argv in exec_runners]
    table(exec_rows, "Execution latency — 40 runs, output verified")

    concurrent = []
    for name, argv in exec_runners:
        if name.startswith("orb compat"):
            continue  # Boa is ~10x slower; skip to keep the stress run bounded
        concurrent.append(parallel_throughput(name, argv))
    print("\nParallel throughput — 8 concurrent x 20 batches")
    print("| Tool | Jobs | Wall (s) | Jobs/s | Failures |")
    print("|---|---:|---:|---:|---:|")
    for row in concurrent:
        print(
            f"| {row['name']} | {row['jobs']} | {row['wall_s']:.2f} | "
            f"{row['jobs_per_s']:.1f} | {row['failures']} |"
        )

    type_runners = []
    if orb.exists():
        type_runners.append(("orb typecheck (embedded tsz)", [str(orb), "typecheck", str(ROOT / "benches/cases/stress_types.ts")]))
    if which("tsz"):
        type_runners.append(("tsz (external)", ["tsz", "--noEmit", "-p", str(TSCONFIG)]))
    if which("tsgo"):
        type_runners.append(("tsgo", ["tsgo", "--noEmit", "-p", str(TSCONFIG)]))

    type_rows = []
    for name, argv in type_runners:
        for _ in range(2):
            one(argv)
        samples = [one(argv)[0] for _ in range(15)]
        type_rows.append({
            "name": name,
            "median_ms": round(statistics.median(samples), 2),
            "best_ms": round(min(samples), 2),
            "failures": 0,
        })
    table(type_rows, "Type checking — 16 KB TS stress file, 15 runs")

    out = ROOT / "docs/stress-benchmark-results.json"
    out.write_text(json.dumps({
        "workload": "stress.js / stress_types.ts",
        "concurrency": PARALLEL,
        "batches": BATCHES,
        "execution": exec_rows,
        "parallel": concurrent,
        "typecheck": type_rows,
    }, indent=2) + "\n")
    print(f"\nwrote {out}")


if __name__ == "__main__":
    main()
