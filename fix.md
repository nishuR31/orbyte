update the version to 3.0.0,apply the below read and work,do as its said:I've received and inspected `engine-review.zip`.

My initial assessment: **you have a functioning compiler prototype, but it is currently a restricted JavaScript-to-native-code compiler, not yet a general-purpose JavaScript/TypeScript engine.**

There is a meaningful foundation:

- Separate Rust crates for lexer, parser, AST, semantic analysis, IR, optimizer and runtime.
- Cranelift native-code backend.
- IR verification.
- Reference interpreter for differential testing.
- Compiler diagnostics and a documented limitations report.

However, I found some architectural issues that are more important than simply adding BigInt and private fields.

**I would not start adding more syntax until these issues are addressed.**

# 1. Critical findings

These are the problems I'd address before expanding the language.

## CRITICAL 1 — Your TypeScript type annotations are discarded

**Files:**

- `crates/parser/src/lib.rs`
- `crates/semantic/src/lib.rs`

**Parser lines 126–159:** `skip_type_annotation()` consumes TypeScript annotations without constructing any type representation.

**Parser lines 161–191:** `skip_type_decl()` discards entire `type` and `interface` declarations.

For example:

```typescript
const age: number = "twenty";
```

A proper TypeScript compiler must reject this.

Your parser removes `: number`, and semantic analysis infers the type from `"twenty"` instead.

This means you currently have TypeScript syntax erasure, not TypeScript type checking.

**Required fix:**

1. Introduce a `TypeAnnotation` AST representation.
2. Preserve annotations on variables, parameters and return types.
3. Parse interfaces and type aliases into actual declarations.
4. Implement structural type checking.
5. Validate assignments, function arguments and return values against declared types.

Priority: **CRITICAL**

## CRITICAL 2 — Null and undefined are treated as the same value

**Files:**

- `crates/semantic/src/hir.rs`, lines 13–21
- `crates/runtime/src/interp.rs`, lines 16–24
- `crates/backend-cranelift/src/lib.rs`, lines 576–580
- `crates/ir-builder/src/lib.rs`, lines 289–292

Your runtime has a single `Null` representation.

The following should produce different results:

```javascript
console.log(null);
console.log(undefined);

console.log(typeof null);
console.log(typeof undefined);
```

Expected:

```text
null
undefined
object
undefined
```

Your native backend currently prints `"undefined"` for `Type::Null`.

**Required fix:**

Introduce distinct runtime representations:

```rust
enum Value {
    Number(f64),
    Boolean(bool),
    String(String),
    Null,
    Undefined,
    // Future variants...
}
```

The IR, interpreter and native backend must all agree on these representations.

Priority: **CRITICAL**

## CRITICAL 3 — Object properties have an incorrect global type system

**File:** `crates/semantic/src/lib.rs`

Relevant lines:

- 138: `str_props`
- 744–747: property access
- 783–793: object construction
- 795–807: property assignment

You maintain a global set of string property names.

This is problematic because property types belong to individual objects and can change dynamically.

Consider:

```javascript
const first = {
  value: 100,
};

const second = {
  value: "hello",
};

console.log(first.value);
console.log(second.value);
```

Your global `str_props` mechanism cannot correctly distinguish these two properties.

The native runtime makes this worse.

**File:** `crates/runtime/src/lib.rs`, lines 407–410.

You maintain separate maps:

```rust
num_props
str_props
obj_props
```

This is not an adequate representation of JavaScript objects.

**Required fix:**

Move towards a unified tagged-value representation:

```rust
enum JsValue {
    Number(f64),
    Boolean(bool),
    String(JsString),
    Object(GcRef),
    Array(GcRef),
    Null,
    Undefined,
}
```

Then use a single property storage mechanism.

Eventually introduce object shapes/hidden classes for optimization.

Priority: **CRITICAL**

## CRITICAL 4 — Runtime allocations are never released

**File:** `crates/runtime/src/lib.rs`

Inspect these locations:

| Lines   | Allocation          |
| ------- | ------------------- |
| 158–159 | String handle       |
| 181–182 | Concatenated string |
| 220–221 | Number-to-string    |
| 268–269 | Uppercase string    |
| 304–305 | String slice        |
| 330–331 | String replacement  |
| 358–361 | Array               |
| 413–419 | Object              |

You repeatedly use:

```rust
Box::into_raw(...)
```

But I couldn't find corresponding ownership cleanup.

You even have:

```rust
pub owned: bool
```

inside `JsString`, but it isn't used to implement reclamation.

This creates memory leaks.

For a long-running execution engine, this is unacceptable.

**Required fix:**

Implement a managed heap.

Suggested progression:

1. Centralized allocation tracking.
2. Root tracking for live references.
3. Mark-and-sweep garbage collector.
4. Write barriers.
5. Generational GC later.

Don't attempt a concurrent garbage collector yet.

Priority: **CRITICAL**

## CRITICAL 5 — JavaScript strings use UTF-8 byte positions instead of UTF-16 semantics

**Files:**

- `crates/runtime/src/lib.rs`
- `crates/runtime/src/interp.rs`

Native runtime:

- Line 255: `rt_str_len`
- Lines 295–301: `rt_str_slice`

Interpreter:

- Lines 142–146: string length
- Lines 160–170: string slicing

Rust's `String::len()` returns UTF-8 bytes.

JavaScript string `.length` returns UTF-16 code units.

Example:

```javascript
const value = "🚀";

console.log(value.length);
console.log("hello".length);
```

Expected:

```text
2
5
```

Your implementation returns 4 for the first string.

There is another problem: slicing a UTF-8 string at an arbitrary byte offset can panic.

For example:

```javascript
console.log("🚀abc".slice(1, 2));
```

Your implementation can attempt to slice inside a multibyte UTF-8 character.

**Required fix:**

Implement JavaScript-compatible UTF-16 indexing and slicing.

This is both a correctness issue and a runtime stability issue.

Priority: **CRITICAL**

# 2. Compiler architecture problems

## HIGH 1 — Your parser is too specialized

**File:** `crates/parser/src/lib.rs`

Your AST currently supports:

```rust
pub enum ExprKind {
    Number(f64),
    Str(String),
    Bool(bool),
    Ident(String),
    // ...
}
```

But it doesn't have general representations for:

- Function expressions.
- Arrow functions.
- Class expressions.
- Await expressions.
- Yield expressions.
- Tagged templates.
- JSX elements.

There is a workaround around lines 325–409 that recognizes arrow functions specifically inside variable declarations.

There is also special handling for `.filter()` callbacks.

This is not a general JavaScript expression parser.

For example:

```javascript
const square = (x) => x * x;
```

may be recognized through your special case.

But:

```javascript
const result = numbers.map((x) => x * 2);
```

requires an arrow function as a normal expression.

Your AST cannot represent that properly.

**Required fix:**

Make functions first-class AST expressions.

For example:

```rust
ExprKind::ArrowFunction {
    params: Vec<Pattern>,
    body: ArrowBody,
}
```

And:

```rust
ExprKind::FunctionExpression {
    name: Option<Ident>,
    params: Vec<Pattern>,
    body: Vec<Stmt>,
}
```

Do not keep adding special cases to variable declaration parsing.

## HIGH 2 — Module imports and exports are discarded

**File:** `crates/parser/src/lib.rs`, lines 194–222.

Your `skip_module_decl()` discards general import/export declarations.

This means:

```typescript
import { add } from "./math";

console.log(add(10, 20));
```

cannot work as a proper module.

You also parse some exported declarations by stripping the `export` keyword and parsing the declaration itself.

That is not a module implementation.

**Required fix:**

Implement:

- Module AST.
- Import declarations.
- Named exports.
- Default exports.
- Module resolution.
- Dependency graph.
- Circular dependency handling.
- Module caching.

Keep module linking separate from parsing.

## HIGH 3 — JSX and TSX aren't supported by the actual frontend

Your CLI:

`cli/src/main.rs`, lines 94–99.

Currently accepts:

```text
.js
.mjs
.cjs
.ts
.mts
.cts
```

It rejects:

```text
.jsx
.tsx
```

Additionally, the AST has no JSX node representation.

**Required fix:**

Add JSX parsing and transformation.

Initially, transform JSX into ordinary runtime calls or a dedicated IR representation.

Don't mix JSX parsing into the ordinary binary-expression parser.

## HIGH 4 — Your runtime isn't dynamically typed

**Files:**

- `crates/semantic/src/hir.rs`
- `crates/ir/src/lib.rs`
- `crates/runtime/src/lib.rs`

Your current type system contains:

```rust
pub enum Type {
    Number,
    Bool,
    String,
    Arr,
    Obj,
    Null,
}
```

This is insufficient for JavaScript.

Consider:

```javascript
function calculate(value) {
  return value + 10;
}

console.log(calculate(20));
console.log(calculate("hello"));
```

JavaScript permits both calls.

Your semantic analyzer statically classifies parameters, including using `param_is_obj()` at `semantic/src/lib.rs`, lines 58–86.

Function parameters default to `Number` unless object access is detected.

That's a heuristic, not JavaScript type inference.

**Required fix:**

Introduce a dynamic value representation and preserve runtime operator semantics.

For example:

```rust
enum JsValue {
    Number(f64),
    String(GcRef),
    Boolean(bool),
    Object(GcRef),
    Array(GcRef),
    Function(GcRef),
    Null,
    Undefined,
    BigInt(GcRef),
    Symbol(GcRef),
}
```

Then implement operations such as:

```rust
fn js_add(lhs: JsValue, rhs: JsValue) -> JsValue;
```

This should implement ECMAScript addition semantics rather than relying on compile-time operand types.

## HIGH 5 — Arrays are restricted to numbers

**Files:**

- `crates/runtime/src/lib.rs`, lines 354–405
- `crates/runtime/src/interp.rs`, lines 197–212
- `crates/semantic/src/lib.rs`, lines 752–781

Your array storage is:

```rust
pub struct JsArray {
    pub elements: Vec<f64>,
}
```

This cannot represent:

```javascript
const values = [10, "hello", true, null, { name: "Nishu" }];
```

**Required fix:**

Use a unified tagged-value array.

Eventually implement:

- Sparse arrays.
- Holes.
- Array length semantics.
- Property descriptors.
- Array prototypes.
- Iterators.

---

# 3. Runtime correctness issues

These are additional issues I found during source inspection.

| Priority | File and lines                         | Problem                                                          |
| -------- | -------------------------------------- | ---------------------------------------------------------------- |
| High     | `runtime/src/lib.rs:372–382`           | Out-of-bounds array access returns NaN instead of undefined      |
| High     | `runtime/src/lib.rs:433–445`           | Missing numeric object properties return NaN                     |
| High     | `runtime/src/lib.rs:460–472`           | Missing string properties return null                            |
| High     | `runtime/src/lib.rs:421–499`           | Separate property maps break ordinary dynamic property semantics |
| Medium   | `runtime/src/interp.rs:142–170`        | Interpreter has the same UTF-8/UTF-16 string problem             |
| Medium   | `backend-cranelift/src/lib.rs:568–574` | Arrays print as `[object Object]`                                |
| Medium   | `runtime/src/lib.rs:334–348`           | String split implementation needs ECMAScript edge-case handling  |

One particularly important example:

```javascript
const values = [10, 20];

console.log(values[100]);
```

Expected:

```text
undefined
```

Your current runtime returns NaN.

These differences matter because your objective is a JavaScript execution engine, not merely a language with JavaScript-like syntax.

---

# 4. Native backend assessment

**File:** `crates/backend-cranelift/src/lib.rs`

Your backend uses Cranelift JIT.

This is a good initial choice.

What is already present:

- Native code generation.
- Function declarations.
- Runtime helper calls.
- Native function invocation.
- Execution context.
- Call-depth tracking.
- Runtime error propagation.

However, there are important missing components.

| Component                 | Current situation             |
| ------------------------- | ----------------------------- |
| Native code generation    | Implemented for restricted IR |
| JIT execution             | Implemented                   |
| Baseline compilation      | Limited implementation        |
| Runtime profiling         | Not implemented               |
| Type feedback             | Not implemented               |
| Optimizing JIT tier       | Not implemented               |
| Speculative optimization  | Not implemented               |
| Deoptimization            | Not implemented               |
| AOT executable generation | Not implemented               |
| Register allocation       | Delegated to Cranelift        |
| Garbage collection        | Not implemented               |

### Important distinction

Your current engine does compile supported code into native machine code.

However, it does not yet implement the V8-style execution pipeline.

V8's major performance advantage isn't simply that it generates native instructions.

It also uses:

- Runtime feedback.
- Inline caches.
- Speculative optimization.
- Multiple compilation tiers.
- Deoptimization.

Those are major future milestones.

Also, your `engine run` command compiles into executable memory and executes it. It doesn't currently produce a standalone native executable.

---

# 5. CLI issues

**File:** `cli/src/main.rs`

### Issue 1: Threads argument does nothing

Lines 128:

```rust
let _ = threads;
```

You accept `--threads`, but don't actually configure a thread pool or parallel runtime.

Remove the flag until implemented, or implement it properly.

### Issue 2: No AOT build command

Your CLI currently provides:

```bash
engine run app.js
engine inspect app.js
```

Eventually you need:

```bash
engine build app.js --target native
engine build app.ts --target native
```

The output should be a standalone executable, not just an in-memory JIT module.

### Issue 3: No JSX/TSX extensions

As mentioned earlier, add `.jsx` and `.tsx` after implementing their parsers.

---

# 6. What I would fix first

Don't try to implement every missing JavaScript feature immediately.

I'd follow this order:

| Priority | Task                                        | Reason                              |
| -------- | ------------------------------------------- | ----------------------------------- |
| P0       | Unified dynamic `JsValue`                   | Foundation for JavaScript semantics |
| P0       | Separate null and undefined                 | Correctness                         |
| P0       | Fix UTF-16 string handling                  | Correctness and stability           |
| P0       | Object property model                       | Correctness                         |
| P0       | Memory management / GC                      | Long-running execution              |
| P1       | Preserve TypeScript annotations             | Actual TypeScript support           |
| P1       | General expression AST                      | Proper JavaScript parser            |
| P1       | First-class functions and closures          | Core JavaScript functionality       |
| P1       | Arrays and objects with dynamic values      | Runtime completeness                |
| P1       | Module system                               | Real application execution          |
| P2       | JSX/TSX                                     | Frontend ecosystem                  |
| P2       | BigInt and private fields                   | Language compatibility              |
| P2       | Exceptions and async execution              | Runtime completeness                |
| P3       | Runtime profiling                           | JIT optimization                    |
| P3       | Speculative optimization and deoptimization | Performance                         |
| P3       | AOT executable generation                   | Deployment                          |

## 7. Specific improvements to your existing architecture

I would retain most of your crate separation.

However, I'd evolve it towards:

```text
crates/
│
├── lexer/
├── parser/
├── ast/
│
├── semantic/
├── typechecker/
├── module-resolver/
│
├── hir/
├── ir/
├── ir-builder/
│
├── optimizer/
│   ├── constant-folding
│   ├── dce
│   ├── inlining
│   └── specialization
│
├── codegen/
│
├── backend-cranelift/
├── backend-llvm/
│
├── runtime/
│   ├── value/
│   ├── objects/
│   ├── functions/
│   ├── strings/
│   ├── arrays/
│   ├── gc/
│   ├── exceptions/
│   ├── promises/
│   └── modules/
│
├── jit/
├── profiler/
└── diagnostics/
```

These are proposed future boundaries, not instructions to create all these crates immediately.

One architectural principle: keep your IR independent of Cranelift. Your existing IR already moves in this direction, which is good.

---

# 8. What is already good?

I don't recommend throwing away the existing implementation.

There are several decisions worth retaining:

1. **Separate frontend and backend:** Good architectural separation.
2. **Reference interpreter:** Extremely useful for differential testing.
3. **IR verifier:** Keep this and expand its invariants.
4. **Cranelift:** Reasonable native backend for the initial implementation.
5. **Diagnostics:** Source spans and located errors are important.
6. **Differential testing:** Comparing against Node.js is the right approach.
7. **Limitations documentation:** Keep documenting unsupported features explicitly.

Your existing `cli/tests/random_diff.rs` is particularly useful. It generates programs and compares results against Node.js across JIT/interpreter and optimized/unoptimized configurations.

Expand this approach to strings, objects, arrays, coercion and control flow as those features become supported.

---

# 9. My overall verdict

| Area                   | Assessment                                      |
| ---------------------- | ----------------------------------------------- |
| Rust project structure | Good foundation                                 |
| Lexer                  | Restricted                                      |
| Parser                 | Restricted; several syntax-specific workarounds |
| AST                    | Insufficient for full JS/TS                     |
| Semantic analysis      | Restricted static inference                     |
| TypeScript support     | Mostly annotation erasure                       |
| IR                     | Good initial foundation                         |
| Optimizer              | Initial static optimization                     |
| Cranelift backend      | Working restricted JIT architecture             |
| Runtime                | Needs substantial redesign                      |
| Memory management      | Critical missing component                      |
| Testing                | Good starting approach                          |
| Production readiness   | Not ready                                       |

**My recommendation: Continue with this repository. Don't rewrite it. But redesign the runtime value model and TypeScript frontend before expanding the feature set.**

One limitation of this review: I performed source inspection, not a live build. The review environment does not have Rust/Cargo installed, and the ZIP doesn't contain the build directory. Therefore, I haven't independently verified compilation or execution.

## 10. What I want you to do next

Before changing anything, send your coding agent this task:

Perform a focused architectural redesign proposal for the existing Rust JavaScript/TypeScript engine.

Do not modify source code yet.

Priorities:

1. Design a unified dynamic JavaScript value representation.
2. Design a proper object/property storage model.
3. Separate null and undefined throughout the IR and runtime.
4. Design memory ownership and garbage collection.
5. Replace UTF-8 byte-based string operations with ECMAScript-compatible UTF-16 semantics.
6. Design a proper TypeScript annotation AST and type-checking pipeline.
7. Identify the minimum changes required to support first-class functions and closures.
8. Preserve the existing Cranelift backend and reference interpreter where practical.

For each proposed change, document:

- Existing implementation and file locations.
- Architectural limitations.
- Proposed replacement.
- Affected crates and dependencies.
- Migration sequence.
- Compatibility risks.
- Required tests.

Do not introduce speculative abstractions without a concrete requirement.

Do not rewrite working components unnecessarily.

The objective is to evolve the existing codebase into an independent JavaScript/TypeScript execution engine, not replace it with another compiler project.

**My strongest advice:** Your next milestone should be a correct dynamic runtime foundation, not more syntax support. A compiler that accepts 100% of syntax but executes it incorrectly is less valuable than one that supports 20% correctly and has an architecture capable of reaching 100%.

remove the lines from here this file once the fix it applied
