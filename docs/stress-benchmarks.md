# Stress benchmarks

Head-to-head stress run, including the embedded tsz type checker. Raw output:
[`stress-benchmark-results.json`](stress-benchmark-results.json).

Method: fresh process per invocation, median of 40 runs for execution, 15 runs for
type checking, output verified on every run. Parallel test: 8 concurrent processes ×
20 batches (160 jobs), failures counted. Workloads: `benches/cases/stress.js`
(8 × fib(24), expected output `370944`) and `benches/cases/stress_types.ts`
(~16 KB, 120 interfaces + 120 typed functions).

## Execution latency

| Tool | Median (ms) | Best (ms) | Failures |
|---|---:|---:|---:|
| orb AOT | 6.01 | 5.92 | 0 |
| orb JIT (`--native`) | 8.54 | 8.41 | 0 |
| bun | 12.44 | 11.96 | 0 |
| deno (`--no-check`) | 26.28 | 25.57 | 0 |
| node | 46.90 | 42.77 | 0 |
| orb compat (Boa) | 320.00 | 313.30 | 0 |

## Parallel throughput (8 concurrent × 20 batches)

| Tool | Jobs | Wall (s) | Jobs/s | Failures |
|---|---:|---:|---:|---:|
| orb AOT | 160 | 0.49 | 324.9 | 0 |
| orb JIT | 160 | 0.65 | 246.3 | 0 |
| bun | 160 | 1.06 | 150.5 | 0 |
| deno | 160 | 2.18 | 73.4 | 0 |
| node | 160 | 3.28 | 48.8 | 0 |

## Type checking (same strict config, libs ES5 + ESNext + DOM)

| Tool | Median (ms) | Best (ms) | Failures |
|---|---:|---:|---:|
| tsz (external binary) | 61.72 | 60.83 | 0 |
| orb typecheck (embedded tsz) | 129.75 | 116.00 | 0 |
| tsgo | 446.94 | 438.11 | 0 |

The embedded checker uses the same tsz source as the external binary and this orb was
rebuilt with the normal release profile (`opt-level=3`, thin LTO, serial jobs). That
cut `orb typecheck` from 141 ms to 115 ms, but it is still about 1.9&times; slower than
the standalone tsz binary. The remaining gap comes from carrying Boa/SWC/Cranelift in
the same 73 MB executable, from the `WasmProgram` JSON round-trip, and from loading the
embedded lib files on every invocation. It is integration overhead, not a different
checker.

## Reproduce

```bash
CARGO_BUILD_JOBS=1 cargo build --release -p cli
./target/release/orb build --native benches/cases/stress.js -o benches/cases/stress_native
node benches/stress_bench.mjs
```

The harness skips missing tools, prints all three tables and writes the JSON results.
