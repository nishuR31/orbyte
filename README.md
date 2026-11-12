# Native JavaScript/TypeScript Engine

A Rust-powered engine that compiles JavaScript to native machine code.
This repository is at **milestone 2.1** (correctness and a recoverable native runtime): a restricted JavaScript subset goes
source -> tokens -> AST -> typed HIR -> IR -> Cranelift -> machine code -> CPU.

## Quick start

Requires Rust 1.77 or newer (the runtime uses `std::mem::offset_of!`).

```bash
cargo test                                   # unit + end-to-end tests (reference interpreter)
cargo run -p cli -- run examples/javascript/calculate.js
# 610
cargo run -p cli -- inspect examples/javascript/calculate.js   # dump the IR
cargo run -p cli -- run --interp examples/javascript/fib.js    # reference interpreter instead of the JIT
cargo run -p cli -- run --no-opt examples/javascript/loops.js  # skip the IR optimiser
```

The binary is called `engine` (`cargo build --release` puts it in `target/release/engine`).

## What works

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

## Deliberate limits

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
* `crates/compiler`: end-to-end tests. Each program runs through the interpreter on the **unoptimised and optimised IR** and the outputs must match, so every test is also an optimiser regression test.
* `cli/tests/differential.rs`: runs every `examples/javascript/*.js` through the real `engine` binary as **JIT, JIT `--no-opt`, interpreter, interpreter `--no-opt`** and requires identical output. If `name.expected` exists next to an example, the output must also match it. The `.expected` files were generated with real Node.js (`node name.js`), so they are an independent oracle.

* `cli/tests/random_diff.rs`: 40 seeded random programs, each run by Node.js and by all four engine configurations; outputs must be identical. It skips itself (with a message) when `node` is not installed.

To add a case, drop `foo.js` into `examples/javascript/`, run `node foo.js > foo.expected`, and `cargo test`.

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
  compiler/          frontend pipeline + end-to-end tests
cli/                 the `engine` binary
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

## Suggested next steps

1. Run `cargo test` (see "Build status") and fix whatever the compiler reports.
2. Run `cargo fmt --all` once, then make the `lint` CI job blocking.
3. CFG-wide dataflow in the optimiser (constant propagation across blocks), then SSA/phi.
4. Decide the value representation (tagged values) and add strings and objects.
5. Globals and closures.
6. TypeScript annotations in the parser, treated as erased types.

## Build status

This workspace was written without access to a Rust toolchain, **twice**: neither
milestone 2 nor milestone 2.1 has been compiled or run. Everything was checked by
careful reading, delimiter-balance scripts, and by validating the `.expected`
files and the random-program generator against real Node.js. Expect a handful of
small compile errors. The most likely places are `backend-cranelift` (it targets
the Cranelift `0.112` API: `declare_var(var, ty)`, `Variable::from_u32`, `brif`,
`icmp_imm`, `iadd_imm`, `load`/`store` with `MemFlags`) and borrow-checker details
in `runtime/src/interp.rs`. Paste the first errors back and they are quick to fix.

`cargo fmt --all` has also never been run over this code.
