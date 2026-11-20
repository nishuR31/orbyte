# orbyte

A Rust-powered runtime that compiles JavaScript to native machine code, with an embedded
compatibility runtime for the rest of JavaScript/TypeScript and a Node/Bun-shaped command
surface. This repository is at **version 3.0.0**.

- The native Cranelift pipeline handles a restricted, statically typed subset and follows
  **“reject, never miscompile.”** The subset is documented in
  [`engine.limitations.md`](engine.limitations.md).
- Programs outside that subset run through the compatibility runtime (SWC for
  TypeScript/JSX erasure, Boa for ECMAScript execution). Minimal CommonJS, relative ESM
  imports, timers, and the test harness are implemented there.
- The 3.0 dynamic-runtime architecture and migration plan are in
  [`docs/architecture/redesign-v3.md`](docs/architecture/redesign-v3.md).

The native pipeline is:
source -> tokens -> AST -> typed HIR -> IR -> Cranelift -> machine code -> CPU.

## Installation

Prebuilt releases are published per OS and architecture. Each archive contains the
`orb` binary plus the runtime static library needed by `orb build --native`.

> **Maintainer note:** publish release assets with the exact names in the table below.
> The [release workflow](.github/workflows/release.yml) does this on a `v*` tag. Until
> the first release is published, use [From source](#from-source).

### Install script (recommended)

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/nishuR31/orbyte/main/scripts/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/nishuR31/orbyte/main/scripts/install.ps1 | iex
```

Both scripts install into `$HOME/.orb/bin` (Unix) or `%LOCALAPPDATA%\Orb\bin`
(Windows) and print the PATH line if needed. Environment overrides:

| Variable | Meaning |
|---|---|
| `ORB_REPO` | GitHub slug `owner/repo` (default: `nishuR31/orbyte`) |
| `ORB_VERSION` | release tag, or `latest` (default) |
| `ORB_INSTALL_DIR` | destination directory |

Example from a fork or for a pinned build:

```bash
curl -fsSL https://raw.githubusercontent.com/octocat/orb/main/scripts/install.sh | \
  ORB_REPO=octocat/orb ORB_VERSION=v3.0.0 bash
```

### Manual download

Open the repository's **Releases** page and download the archive for your platform:

| OS | Architecture | Asset |
|---|---|---|
| Linux | x86_64 | `orb-linux-x86_64.tar.gz` |
| Linux | aarch64 | `orb-linux-aarch64.tar.gz` |
| macOS | x86_64 | `orb-macos-x86_64.tar.gz` |
| macOS | aarch64 (Apple Silicon) | `orb-macos-aarch64.tar.gz` |
| Windows | x86_64 | `orb-windows-x86_64.zip` |
| Windows | aarch64 | `orb-windows-aarch64.zip` |

Extract it, move `orb` (`orb.exe` on Windows) and the adjacent runtime library
(`libruntime.a` / `runtime.lib`) into a directory on your PATH, then make the binary
executable on Unix:

```bash
chmod +x orb
```

Keeping the runtime library next to the binary lets `orb build --native` find it
automatically; otherwise set `ORB_RUNTIME_LIB=/path/to/libruntime.a`.

### From source

Requires Rust 1.77 or newer:

```bash
git clone https://github.com/nishuR31/orbyte.git
cd orbyte
cargo build --release
./target/release/orb --version
```

### Verify

```bash
orb --version   # 3.0.0
orb run examples/javascript/calculate.js   # 610  (from a source checkout)
```

A release archive contains only the binary, so use any `.js` file you have for the
second check. Running `orb build --native` additionally needs a C compiler on Unix
(`cc`) or the MSVC linker on Windows; see [Ahead-of-time native executables](#ahead-of-time-native-executables).

### Updating

The install script is the update mechanism — re-running it replaces the installed
binary in place and keeps the previous one as `orb.old` (`orb.old.exe` on Windows).
From an installed copy:

```bash
orb self-update --check   # is a newer release available?
orb self-update           # download and install the latest release
```

`orb self-update` queries the GitHub Releases API, compares numeric version
components, then re-runs the matching installer. It never updates in the background
and sends no telemetry. `ORB_REPO`, `ORB_VERSION` and `ORB_INSTALL_DIR` are honoured.
When offline, just re-run the install script or build from source.

Why keep the binary current: the native backend is still gaining syntax coverage under
the "reject, never miscompile" rule, AOT linking and platform support get fixes, and
benchmarks/packaging change between releases. A stale binary keeps old diagnostics and
old accepted-subset behaviour, so a program that works on `main` may be rejected by an
old install. Updates are explicit so they never surprise you mid-run.

### Benchmarks

[`docs/benchmarks.md`](docs/benchmarks.md) has measured startup-to-output numbers for
orb (AOT, JIT, compat), bun, deno and node, plus a separate type-check comparison
between tsz and tsgo. [`docs/stress-benchmarks.md`](docs/stress-benchmarks.md) adds
8-way parallel throughput and an embedded-checker comparison. Reproduce with
`python3 benches/runtime_bench.py --runs 50` and `python3 benches/stress_bench.py`.

## Quick start

Requires Rust 1.77 or newer (the runtime uses `std::mem::offset_of!`).

```bash
cargo test                                   # unit + end-to-end tests (reference interpreter)
cargo run -p cli -- run examples/javascript/calculate.js
# 610
cargo run -p cli -- run testCaseJs.js        # full ECMAScript via the compatibility runtime
cargo run -p cli -- run testCaseTs.ts        # TypeScript + JSX transpilation and execution
cargo run -p cli -- run testCaseJs.js --native   # force the native subset and show its diagnostic
cargo run -p cli -- run testCaseTs.ts --compat   # skip the native pipeline
cargo run -p cli -- inspect examples/javascript/calculate.js   # dump the IR
cargo run -p cli -- run --interp examples/javascript/fib.js    # reference interpreter instead of the JIT
cargo run -p cli -- run --no-opt examples/javascript/loops.js  # skip the IR optimiser
```

The binary is called `orb` (`cargo build --release` puts it in `target/release/orb`).
For daily use, `orbx` runs that binary straight from this checkout, so a stale
`orb` on PATH cannot shadow the code you are editing:

```bash
orbx --version                  # 3.0.0
orbx test
ORBX_REBUILD=1 orbx ...    # rebuild before running
```

It lives at `scripts/orbx` (Bash) with a `scripts/orbx.ps1` equivalent for Windows;
symlink or copy it into a PATH directory to install. Override the checkout with
`ORB_ROOT`, or the binary with `ORB_BIN`. CI runs `cargo check`, `cargo test` and
an AOT smoke test on Linux, macOS and Windows.

Some auxiliary development scripts in `tests/` and `examples/limitations/` still require
Bash; they are not part of CI. `package.json` scripts that use POSIX shell syntax need
a shell such as Git Bash on Windows.

## Command surface

Bare `orb <file>` is equivalent to `orb run <file>`. `orb node ...` and
`orb bun ...` are aliases for the same routing; they are not emulations of those
binaries.

```
orb [file] [args...]                 run a file
orb run <file|script> [args...]      run a file or a package.json script
orb run -e <code>                    evaluate JavaScript/TypeScript
orb run -p <code>                    evaluate and print the result
orb test [path|filter...]            run *.test.* / *.spec.* files
orb repl                             interactive session
orb check <file> | -c                parse without executing
orb typecheck <file.ts>              type-check TypeScript with embedded tsz
orb inspect <file> [--no-opt]        print the native IR
orb build --target js <file> [-o out.js]  transpile TS/TSX/JSX to JS (local)
orb build --native <file> [-o out]   compile a standalone native executable
orb build [args...]                  bundle (delegated to Bun)
orb init [-y] [--force] [dir]        create package.json / index.ts
orb install bun [--force] [--version <v>] [--npm] [--dry-run]
orb install|add|remove|update ...    package management (delegated)
orb x|bunx <package> [args...]       execute a package binary (delegated)
orb exec|create|link|unlink|pm ...   Bun/npm commands (delegated)
orb upgrade                          install or upgrade Bun
orb --version | --revision | --help
```

`orb install bun` is implemented by this binary: it runs the official installer with
`curl` or `wget` (PowerShell on Windows) and also supports `--npm`, `--force`,
`--version <v>`, and `--dry-run`. Package-management and bundling commands delegate to
Bun when it is installed, with npm/npx fallbacks. `ORB_NO_DELEGATE=1` disables
delegation.

### Ahead-of-time native executables

`orb build --native` runs the same IR → Cranelift pipeline as `orb run`, but emits a
relocatable object with a C `main` and links it with the runtime static library into a
standalone executable. No JS orb, Bun, or Node is needed to run the result.

```bash
cargo build --release                         # builds orb + libruntime.a
orbx build --native examples/javascript/calculate.js -o calculate
./calculate                                   # 610
```

The emitted code is position-independent, so the linker produces a normal PIE with no
text relocations. String literals live in the object's data section and are referenced
by relocation rather than by host pointers. A runtime failure prints to stderr and exits
with status 1, matching `orb run --native`.

Requirements: `target/release/libruntime.a` (Unix) or `runtime.lib` (Windows MSVC) built
from the same checkout. Unix links with `cc`; Windows uses `rustc` as the linker driver so
COFF objects and the right MSVC system libraries are handled automatically (the MSVC
linker must be on PATH, e.g. via `ilammy/msvc-dev-cmd` in CI). `ORB_AOT_LINKER=rustc`
forces that path on Unix; `ORB_RUNTIME_LIB` overrides library discovery. `orbx`
rebuilds the workspace first when `ORBX_REBUILD=1` is set. Default output names get
`.exe` on Windows.

Scope: AOT covers the **native subset only** (the same programs `orb run --native`
accepts). TypeScript, JSX, ES modules, async, and other compatibility-runtime features
keep using `orb run` or Bun bundling — they have no native IR to emit yet.

### TypeScript type checking

> **Third-party component:** the type checker is **not written by the orbyte project**.
> It is the excellent [`tsz`](https://github.com/tsz-org/tsz) checker by Mohsen Azimi
> and contributors, vendored under [`third_party/tsz`](third_party/tsz/README.orbyte.md).
> Thank you to the tsz developers — your work made this possible. If you maintain tsz or
> want to help, please test `orb typecheck` and send fixes, bug reports or PRs here or
> upstream.

`orb typecheck <file.ts>` runs that third-party tsz checker **in-process** — the checker
source is vendored, so there is no subprocess, no Node, no network call and no extra
install. It uses bundled ES5 + ESNext + DOM libs and prints diagnostics with file, line,
column and TS code; the exit status is 1 when errors are found.

```bash
orb typecheck app.ts
# app.ts(3,9): Error TS2322: Type 'string' is not assignable to type 'number'.
```

This is independent of `orb check`, which only parses the native subset. Type checking
is aimed at TypeScript programs that otherwise run through the compatibility runtime;
it does not change AOT output, which remains native-subset only.

### Run flags

| Flag | Meaning |
|---|---|
| `--native` | Disable the compatibility fallback; fail outside the native subset with a located diagnostic |
| `--compat` | Skip the native pipeline and use the compatibility runtime |
| `--interp` | Use the reference IR interpreter instead of Cranelift (native only) |
| `--no-opt` | Skip the IR optimiser (native only) |
| `--threads <N>` | Worker-thread count for native runtime jobs |
| `--watch`, `--hot` | Re-run when relevant files change |
| `--env-file <f>` | Load environment variables from `<f>` (repeatable) |
| `--no-env-file` | Disable automatic `.env` loading |
| `--input-type <t>` | `auto`, `module`, or `commonjs` |
| `--cwd <dir>` | Resolve files relative to `<dir>` |
| `--preload`, `-r`, `--import` | Evaluate a file before the entry (repeatable) |
| `--` | Everything after this belongs to the program |

`-p`/`--print` consumes the next expression and prints its completion value. Unknown flags
before the entry file are usage errors; unknown flags after it are passed to the program.

## Execution paths

`orb run <file>` first tries the native Cranelift pipeline. When
`compat::should_skip_native` can tell in advance that the source is outside the native
subset (`.ts`, `.tsx`, `.jsx`, module syntax, `import`, `require`, BigInt, private fields,
async), that attempt is skipped for speed. This is only a hint: `--native` always runs the
native pipeline so the real diagnostic is reported.

If the native frontend rejects the program, `crates/compat`:

1. erases TypeScript and JSX with SWC (`.ts`, `.mts`, `.cts`, `.tsx`, `.jsx`), retrying a
   `.ts` file as TSX when it contains JSX;
2. parses, links and evaluates the resulting ECMAScript module with Boa, resolving
   relative and absolute file imports (including imported TypeScript/JSX and JSON modules
   with `type: "json"`);
3. drains promise jobs and timers, and fails the run on unhandled promise rejections; and
4. provides minimal CommonJS for `.cjs` files: `require` of relative/absolute
   `.js`/`.cjs`/`.json`/TS/JSX files, directory resolution via `package.json#main` and
   `index.*`, a shared module cache, `module.exports`, `exports`, `__filename`,
   `__dirname`, and `require.resolve`.

Plain `.js`/`.mjs`/`.cjs` sources are passed to Boa unchanged. SWC helper functions are
inlined, so the transpiled program has no `@swc/helpers` dependency. TypeScript/JSX
erasure results are cached in-process and on disk (see below), so repeated runs and test
suites do not pay the transform twice.

Bare package specifiers resolve through `node_modules` (`package.json` `main` and
`exports`, scoped packages), for both `import` and `require`. Common Node built-ins
(`path`, `fs`, `os`, `util`, `assert`, `events`, `url`, `querystring`, `buffer`,
`process`, `stream`, `crypto`) are provided as shims, with `node:` prefixes accepted.
Named imports from CommonJS modules work. `fetch` is available (backed by `curl`) with
`Response.text()/json()/arrayBuffer()` and `Headers`. `document` and `React` remain small
JSX stubs; TypeScript is not type-checked; there is no import map, and timers are a small
macrotask queue rather than a full Node event loop. `orb build --target js` writes the
transpiled JavaScript locally without invoking Bun.

### Transpile cache

Erasure is keyed by content and parser mode. The cache root is `$ORB_CACHE_DIR`, else
`$XDG_CACHE_HOME/orb/transpile`, else `$HOME/.cache/orb/transpile`. Writes are
atomic. Set `ORB_NO_CACHE=1` to disable the disk cache. Plain JavaScript bypasses SWC
and the cache entirely.

## Test runner

```bash
orb test                       # discover tests under the current directory
orb test tests/math.test.ts    # run one file
orb test math                  # case-insensitive path filter
orb test --test-name-pattern adds
orb test --bail --concurrency 8 --watch
```

The runner discovers `*.test.*`, `*.spec.*`, `*_test.*`, `*_spec.*`, and source files
inside `test`/`tests`/`__tests__` directories. Existing positional paths are roots;
nonexistent tokens are path filters. Files run in parallel and their captured output is
printed in input order. `bun:test` and `node:test` both resolve to the harness inside test
runs.

## What worked at milestone 2.1 (historical snapshot)

| Area | Supported |
|---|---|
| Values | numbers (`f64`), booleans |
| Declarations | `let`, `const`, `var` (initializer required) |
| Expressions | `+ - * / %`, unary `-` and `!`, `< <= > >=`, `== != === !==`, `&&` and `\|\|` (booleans, short-circuiting), assignment, `+= -= *= /= %=`, prefix `++x`/`--x`, calls, parentheses |
| Statements | blocks, `if`/`else`, `while`, `for (init; cond; update)`, `break`, `continue`, `return`, expression statements (including postfix `x++`/`x--`) |
| Functions | top-level declarations (hoisted), recursion (including mutual recursion), numeric parameters, number/boolean/void results |
| Output | `console.log(x)` with exactly one argument |
| Syntax | `//` and `/* */` comments, semicolon insertion at line terminators (LF, CR, CRLF, U+2028, U+2029; never inside `for (...)` headers), `.js` and `.mjs` files |
| Safety | call-depth limit of 10,000 in both the JIT and the interpreter; overflow is a recoverable `RuntimeError::StackOverflow` (CLI prints `RangeError: Maximum call stack size exceeded`, exit code 1); the host process is never terminated by the runtime |
| Optimiser | constant folding, local store-to-load forwarding, folding of constant branches, dead-code and unreachable-block removal, value renumbering; the IR is re-verified **after every pass** |

## Deliberate limits at milestone 2.1 (historical snapshot)

The semantic pass follows one rule: **reject, never miscompile.** Anything whose
JavaScript behaviour is not reproduced exactly yet is a compile error:

* No implicit conversions: `1 + true`, `if (1)`, `!0`, `1 && true` and `1 == true` are errors.
* No `undefined`: uninitialised declarations, missing arguments and functions that can fall off the end with a value are errors.
* Variables are statically typed (a number variable cannot later hold a boolean).
* `let`/`const` follow the temporal dead zone: any access before the declaration has been evaluated is an error (`let x = x + 1`, or reading an outer `x` that an inner `let x` shadows later in the same block), because JavaScript throws a `ReferenceError` there.
* `var` is function-scoped but not hoisted as `undefined`: it cannot be used before its declaration, and a `var` declared in a nested block (an `if` or loop body) can only be read inside that block, since outside it the declaration might not have run and JavaScript would yield `undefined`.
* Postfix `x++` / `x--` is only accepted where its value is discarded (a statement or a `for` update); `let y = x++` is an error.
* Top-level variables are not visible inside functions yet (globals need the runtime).
* No nested functions or closures, no strings, objects, arrays, ternaries or `switch`.
* A function whose every `return` is reached only through an unresolvable recursion pattern is still rejected, but the common cases (boolean helpers, even/odd mutual recursion) now infer correctly.
* Number printing uses Rust's shortest round-trip format; extreme magnitudes differ from JavaScript's exponent notation.

## Testing

```bash
cargo test
```

* Unit tests in every crate (lexer, parser, IR verifier including malformed-IR cases, optimiser, backend).
* `crates/backend-cranelift`: runs real native code: stack overflow returns an error, a failed run does not affect the next run of the same module, independent modules run concurrently, malformed IR is rejected without panicking.
* `crates/compat`: SWC/TSX erasure, enums/namespaces, legacy decorators, JSX execution, modules and promise jobs, relative/TypeScript imports, clean missing/bare-import errors, unhandled rejections, BigInt, private class fields, timers/process, CommonJS `require` and cache, built-in rejection, and the native-skip heuristic.
* `crates/optimizer`: verifies DCE keeps object/key/value operands used by object-property instructions, then re-verifies the IR.
* `crates/compiler`: end-to-end tests. Each program runs through the interpreter on the **unoptimised and optimised IR** and the outputs must match, so every test is also an optimiser regression test.
* `cli/tests/commands.rs`: 12 integration tests for eval/print, check, CommonJS, ESM + TypeScript, the test runner, `install bun`, delegation refusal, stdin, unknown flags, unsupported extensions, package scripts, and `--native` rejection.
* `cli/tests/differential.rs`: runs every `examples/javascript/*.js` through the real `orb` binary as **JIT, JIT `--no-opt`, interpreter, interpreter `--no-opt`** and requires identical output. If `name.expected` exists next to an example, the output must also match it. The `.expected` files were generated with real Node.js (`node name.js`), so they are an independent oracle. The compile-error test also checks that the default fallback runs valid out-of-subset JavaScript while `--native` still rejects it.
* `cli/tests/random_diff.rs`: 40 seeded random programs, each run by Node.js and by all four orb configurations; outputs must be identical. It skips itself (with a message) when `node` is not installed.

To add a case, drop `foo.js` into `examples/javascript/`, run `node foo.js > foo.expected`, and `cargo test`.

`bash tests/compare.sh` runs the 29-program differential corpus (node/bun as
oracles). `bash tests/realworld/run.sh` covers realistic programs using bare npm
packages, Node built-ins, `fetch`, JSX/TSX and mixed module systems against
node/bun; it starts and stops a local HTTP server for the `fetch` case.

## Layout

```
crates/
  diagnostics/       spans and error rendering
  ast/               syntax tree
  lexer/             tokenizer (tracks newlines for semicolon insertion)
  parser/            recursive descent + precedence climbing
  semantic/          scopes, local slots, static checks -> typed HIR
  ir/                IR data model, verifier, printer
  ir-builder/        HIR -> IR lowering
  optimizer/         IR passes (fold, DCE, unreachable blocks, value compaction), verified after each pass
  runtime/           execution context, runtime errors, helpers, number formatting, frame-based reference interpreter
  backend-cranelift/ IR -> Cranelift -> executable memory
  compat/            compatibility runtime: SWC transpilation, Boa execution, CommonJS
  compiler/          frontend pipeline + end-to-end tests
cli/                 the `orb` binary
  main.rs            command routing / usage
  execute.rs         run, eval, print, check, repl, stdin, watch, inspect
  testrunner.rs      test discovery, parallel execution, reporting
  package.rs         delegation, install bun, init, upgrade
  util.rs            watcher, lookup, classification helpers
  envfile.rs         .env loading
```

## Design decisions worth knowing

* **IR values are block-local; cross-block state lives in numbered locals.**
  Cranelift's `Variable` turns locals into SSA. When the optimiser needs
  explicit phi nodes, add a mem2reg-style pass inside `ir` and the rest of the
  pipeline does not change.
* **The IR is typed (`f64`, `bool`) and has no dynamic values yet.** Introducing
  tagged values (NaN-boxing or pointer tagging) is the first big runtime
  decision and will extend `Type` and the instruction set.
* **A reference interpreter ships with the runtime.** Every end-to-end test runs
  through it, and `--interp` lets you diff its output against the JIT.
* **Short-circuit operators and block-local values.** `&&`/`||` split a block, so
  their result travels through a fresh IR local, and any operand evaluated
  *before* a branching sibling is spilled to a local and reloaded afterwards
  (`ir-builder`'s `operands`).
* **`for` loops lower to `while` with an update block.** `continue` jumps to the
  update block, `break` to the exit block.
* **Every generated function takes a hidden `ExecContext` pointer.** The context
  holds the call-depth counter and an error flag and is created fresh by
  `CompiledModule::run`, so there is no mutable global state in generated code.
  When the depth limit is hit, the function records the error and returns zero;
  after every call, generated code checks the flag and returns immediately, so
  the native stack unwinds by ordinary returns. `run` returns
  `Result<(), RuntimeError>` and nothing calls `process::exit`.
* **The interpreter uses an explicit frame stack**, not Rust recursion, so the
  depth limit is a counter and no oversized thread stack is needed.
* **The backend verifies its own input** (`ir::verify`) and reports invalid IR as
  an error instead of panicking. The strict verifier requires every declared
  value to be defined exactly once, so the optimiser renumbers values after each
  pass that removes definitions.
* **The Cranelift version is pinned to `0.112`.** Its API changes between
  releases (notably `Variable` handling and `jump`/`brif` argument types), so
  upgrade deliberately.
* **The compatibility runtime is a stopgap, not the 3.0 foundation.** It exists so
  complete JavaScript/TypeScript programs can run and be tested today; the native
  dynamic runtime in the proposal remains the goal.
* **Performance choices are deliberately conservative.** The native-skip heuristic only
  skips work that cannot succeed, and the transpile cache never changes semantics: the
  key includes source content, filename, and parser mode.

## Suggested next steps

The next implementation phase is the dynamic value foundation described in
[`docs/architecture/redesign-v3.md`](docs/architecture/redesign-v3.md). Do not add more syntax
before that foundation lands.

## Verification and performance

A 29-program differential corpus (`tests/compare.sh`, `tests/corpus/`) compares the
orb against Node (JavaScript) and Bun (TypeScript/JSX): **29/29 byte-identical**.
`orb -` reads stdin. `console.log` uses Node-shaped inspection. The native
optimiser now verifies only the function it changes and removes dead functions
before code generation; total JIT compile time on the 5,007-line fixture fell
from 1327 ms to under 9 ms with identical output. The in-process benchmark
(`cargo bench --offline --bench components_bench`) now covers all 20 orb
aspects — native stages, integrated frontends, diagnostics and the compatibility
runtime — verifies every stage before timing it, uses a true median, excludes
setup work from timed regions, and keeps benchmark output out of the report.
See [`docs/comparison.md`](docs/comparison.md) for the full comparison with node,
bun, tsc, tsgo and tsz, and `python3 benches/compare.py` for reproducible timings.

For performance work use `target/release/orb`: it is roughly 4–5× faster than
the debug binary on the compatibility paths (9.8 ms vs 44 ms for `testCaseJs.js`).

## Build status

The workspace compiles and `cargo test --offline --workspace` passes with
**0 failures** with Rust 1.99.0 and Node v24.16.0. The build has no warnings. The two
acceptance programs also pass through the compatibility runtime:

```bash
target/debug/orb run testCaseJs.js   # exit 0, 165 lines
target/debug/orb run testCaseTs.ts   # exit 0, 23 lines
```

The exact, deliberately rejected features are listed in
[`engine.limitations.md`](engine.limitations.md); `--native` makes those diagnoses
visible.
