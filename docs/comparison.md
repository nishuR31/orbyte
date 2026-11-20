# Engine 3.0.0 — verification and comparison

Date: 2026-11-19. Machine: Linux x86-64, Rust 1.99.0, Node v24.16.0,
Bun 1.4.3, `tsc` 7.0.2, `tsgo` 7.0.0-dev.20260707.2, `tsz` 7.0.2.

## 1. Runtime differential corpus

`tests/compare.sh` runs every program in `tests/corpus/` through the engine and
through an oracle (`node` for `.js`/`.mjs`/`.cjs`, `bun` for `.ts`/`.tsx`), then
compares exit codes and stdout.

| Run | Files | Matches | Mismatches |
|---|---:|---:|---:|
| Before console-format fix | 29 | 28 | 1 |
| After fix | 29 | 29 | 0 |

The one mismatch was `19_console_format.js`: Boa's inspector printed multi-line
objects, double-quoted strings and `Array(2)` for nested arrays. The compatibility
runtime now uses a Node-shaped inspector (`crates/compat/src/inspect.rs`), so
`console.log({ a: 1, b: 'two' }, [1, [2, 3]])` prints
`{ a: 1, b: 'two' } [ 1, [ 2, 3 ] ]`, byte-identical to Node.

The corpus covers loops, recursion, strings, arrays, objects, closures/arrows,
classes + private fields, async/await, errors, destructuring/spread, `Map`/`Set`,
regex, BigInt, generators, relative ESM/CJS, JSON, JSX, advanced TypeScript
syntax, async iteration, and modern builtins. All programs exit 0 on both sides.

The 24-case CLI probe suite (`engine run`, `--native`, `--compat`, eval/print,
check, ESM, CJS, dynamic import, bare-import rejection, unhandled rejections,
timers/prelude, shebang, top-level await, stdin, package scripts, install dry-run)
also passes 24/24. `cargo test --offline --workspace` passes **179 tests, 0 failures**.

## 2. TypeScript checking comparison

| Input | tsc | tsgo | tsz | engine `check` |
|---|---|---|---|---|
| `const x: number = 1;` | exit 0 | exit 0 | exit 0 | exit 0 |
| `const x: number = "oops";` | `TS2322` at 1:7, exit 1 | `TS2322` at 1:7, exit 1 | `TS2322` at 1:7, exit 1 | exit 0 |

`tsc`, `tsgo` and `tsz` produce the identical diagnostic on the type error.
`engine check` parses and erases types but does not type-check; this is a known
and documented limitation of the native/compatibility pipeline. Runtime
execution of TypeScript by the engine (SWC erasure + Boa) matches Bun on all TS
corpus files.

## 3. Component benchmark (all aspects)

`cargo bench --offline --bench components_bench` exercises **every engine
aspect** in-process: a 5,007-line, 1,002-function synthetic program drives the
native pipeline, while separate TS/JS fixtures drive the compiler frontends,
the diagnostics renderer and the compatibility runtime. Process-spawn overhead
is not counted.

The benchmark first verifies each stage and refuses to report timings if a
stage is broken: lexer/parser/semantic counts are checked, both IR forms pass
`ir::verify`, the interpreter and the native JIT must print the same
`1001411\n`, and the compatibility path must print `49995000` (JS) and `15`
(TS). Timing fixes in this round:

- The median is now the *true* median (the two middle samples are averaged for
  an even sample count); the old code reported the upper median.
- The IR clone needed by the optimiser loop is made **before** the timer, so it
  is no longer charged to the optimiser.
- Program output is captured or sent to `/dev/null`, so benchmarked
  `console.log` calls no longer pollute the report.

Representative medians (ms, 10 iterations, release bench profile):

| # | Component | Median ms | Detail |
|---:|---|---:|---|
| 1 | Lexer | 1.15 | 34,045 tokens, 100 MB/s |
| 2 | Parser | 3.64 | 1,002 statements |
| 3 | AST clone | 1.11 | deep copy |
| 4 | Semantic analysis | 1.91 | 1,002 functions |
| 5 | IR lowering | 0.99 | 6,003 blocks |
| 6 | IR verifier | 0.17 | whole module |
| 7 | IR printer | 2.55 | module to string |
| 8 | IR optimiser | 0.23 | clone excluded |
| 9 | Cranelift codegen | 0.46 | JIT emit |
| 10 | IR interpreter | ~0.00 | output captured |
| 11 | Native execution | ~0.00 | output silenced |
| 12 | `compile_to_ir` | 7.18 | integrated frontend |
| 13 | `compile_optimized` | 7.10 | integrated pipeline |
| 14 | Diagnostics render | 0.11 | 100 diagnostics |
| 15 | `should_skip_native` | 0.31 | source scan |
| 16 | Transpile JS | 0.00 | passthrough, no SWC |
| 17 | Transpile TS (cache hit) | ~0.00 | memory/disk cache |
| 18 | Transpile TS (cold) | 0.14 | SWC + disk write |
| 19 | Boa execution JS | 8.41 | output captured |
| 20 | Boa execution TS | 7.96 | output captured |

Full-pipeline JIT compile (lexer + parser + semantic + lowering + verify +
optimiser + codegen) is roughly **8.5 ms**. Historically this total fell from
1327 ms to 6.9 ms before the verifier was added as a separate timed stage; the
same two fixes are responsible:

1. The optimiser verified the *whole module* after each pass of each function —
   quadratic in the number of functions. It now verifies only the function it
   changed (`ir::verify_function`) and does one whole-module check at the end.
2. Module-level dead-function elimination removes functions unreachable from the
   entry before local passes and code generation, and remaps call references.
   The frontend emits one IR function per source function, so a program that
   defines 1,002 functions but calls only two now compiles only those two.

Both changes are covered by tests (an unreachable-function/remap unit test, plus
the full differential corpus and workspace suite).

## 4. Wall-clock, debug vs release

Median of 15 interleaved runs on an otherwise idle machine (process creation is
included and costs ~3 ms here, so these are conservative):

| Case | Debug | Release |
|---|---:|---:|
| `run testCaseJs.js` | 44.01 ms | 9.82 ms |
| `run testCaseTs.ts` | 32.27 ms | 7.24 ms |
| `-e console.log(1)` | 13.99 ms | 2.87 ms |
| native `calculate.js` | 15.00 ms | 3.10 ms |

Use `target/release/engine` for performance work; the debug binary is roughly
4–5× slower on the compatibility paths.

`python3 benches/compare.py [--runs N]` reproduces the cross-tool comparison.
Note that process spawning on this host is unusually slow (~30–60 ms per
`subprocess.run`), which dominates and quantifies wall-clock numbers comparing
separate binaries; use the in-process component benchmark for engine changes.

## 5. Fixes in this round

- `engine -` now reads stdin (conventional dash entry) instead of rejecting it
  as an unknown flag; regression test added.
- Node-shaped `console.log` inspection for objects, arrays and nested values.
- Optimiser verification and dead-function elimination (see §3).
- New `duck/` Goose CLI dispatcher (separate project) for node/bun/tsc/tsgo/tsz.

## 6. Real-world compatibility round

`tests/realworld/run.sh` runs 13 realistic programs through the engine and through
node/bun. All 13 match byte-for-byte, including:

- bare npm packages via `node_modules` with `main`/`exports` and scoped names,
  in both ESM (`import { greet } from "demo-pkg"`) and CJS (`require(...)`);
- Node built-ins `path`, `os`, `util`, `fs`, `assert`, `events`, `url`, `buffer`
  (with and without the `node:` prefix, in both module systems);
- `fetch` against a local HTTP server, with `await res.text()`;
- JSX/TSX with a local factory and with a bare `react` import, mixed `.jsx`
  importing a typed `.ts` helper;
- TypeScript decorators/enums/namespaces/interfaces and ESM→CJS interop.

Implementation: `crates/compat/src/resolve.rs` (Node-style resolution),
`crates/compat/src/builtins.rs` (built-in shims plus `fetch`), named-export
detection for CommonJS in the ESM loader, and `engine build --target js`
(local SWC transpilation independent of Bun). Integration tests cover bare
resolution, built-ins and the build command. Remaining gaps: JSX `React`/`document`
are stubs, no import map, TypeScript is not type-checked, timers are a small queue.
