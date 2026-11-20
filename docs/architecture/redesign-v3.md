# Engine 3.0 — Architectural Redesign Proposal

**Status:** proposal; preserved as the implementation plan, written when the binary was
named `engine` (now `orb`). An interim compatibility runtime (SWC + Boa) was added in
`crates/compat` to execute programs outside the native subset; it does not implement
this proposal. The `orb` binary additionally exposes a Node/Bun-shaped command surface
(`run`, `test`, `repl`, `check`, `typecheck`, `inspect`, `build`, `init`, package
management, `install bun` and `self-update`) on top of the two execution paths. Type
checking is third-party tsz, vendored under `third_party/tsz`.
**Target version:** `3.0.0`
**Date:** 2026-11-13
**Scope:** the existing `lexer → parser → AST → semantic/HIR → ir-builder → ir → optimizer → backend-cranelift` pipeline and the `runtime::interp` reference interpreter.

---

## 0. Purpose and constraints

The engine currently supports a deliberately restricted, statically typed JavaScript subset. The next milestone must be a **correct dynamic runtime foundation**, not more syntax. This document proposes that foundation and the minimum sequence required to reach it without replacing the working compiler pipeline.

Design constraints taken from the review:

1. Build a unified dynamic value representation.
2. Build a real object/property storage model.
3. Keep `null` and `undefined` distinct everywhere.
4. Give every allocation an owner and add garbage collection.
5. Implement ECMAScript-compatible UTF-16 string semantics.
6. Parse and check TypeScript annotations instead of silently discarding them.
7. Add the minimum needed for first-class functions and closures.
8. Preserve Cranelift and the reference interpreter where practical.

Additional constraints:

- Do not introduce abstractions without a concrete current requirement.
- Do not rewrite working components unnecessarily.
- Keep the existing crate pipeline, IR verifier, optimizer passes and differential-testing approach.
- No source changes in this step. This is the proposal that precedes implementation.

### 0.1 Baseline facts verified in the repository

| Area | Current state |
|---|---|
| Workspace version | was `0.1.0` in `Cargo.toml:6` at review time; now `3.0.0`; all crates inherit it |
| Pipeline | `compiler/src/lib.rs:17-30` |
| IR types | Six static types in `ir/src/lib.rs:19-27` and `semantic/src/hir.rs:13-21` |
| Runtime values | `runtime::interp::Val` (`runtime/src/interp.rs:16-24`) and C-ABI handles in `runtime/src/lib.rs` |
| Calls | Direct `FuncRef` only: `ir/src/lib.rs:101`; `semantic/src/lib.rs` rejects functions as values |
| TypeScript | Annotations/declarations/modules are skipped: `parser/src/lib.rs:126-223` |
| Strings | UTF-8 bytes behind `JsString`: `runtime/src/lib.rs:150-155` |
| Arrays | `Vec<f64>`: `runtime/src/lib.rs:354-405` |
| Objects | Three separate `HashMap`s: `runtime/src/lib.rs:407-500` |
| Allocation | `Box::into_raw` with no matching free: `runtime/src/lib.rs:158-182`, `216-221`, `258-331`, `358-361`, `413-419` |
| `null` / `undefined` | Both lower to `ConstNull` and print as `undefined`: `semantic/src/lib.rs:536-537`, `ir-builder/src/lib.rs:289-293`, `backend-cranelift/src/lib.rs:577-584` |
| `console.log` of objects | Prints `[object Object]`: `backend-cranelift/src/lib.rs:570-576` |
| Threads | `--threads` is parsed and ignored: `cli/src/main.rs:128` |

Local baseline toolchain: `cargo 1.99.0`, `rustc 1.99.0`, `node v24.16.0`.

### 0.2 Interim compatibility runtime (not the 3.0 foundation)

To run complete JavaScript/TypeScript programs before the dynamic runtime below exists,
the CLI now has an embedded fallback in `crates/compat`:

- The native Cranelift pipeline is still tried first and is unchanged for the supported
  subset.
- When the native frontend rejects a program, SWC 80.0.1 erases TypeScript/JSX and
  Boa 0.22.0 evaluates the result. `--native` disables this fallback; `--compat` forces it.
- The fallback binds no dynamic `JsValue`, has no GC, object model, UTF-16 string
  semantics, closure capture, or TypeScript type checking. It resolves relative/absolute
  ESM file imports, but not bare package specifiers or Node.js built-ins. It also provides
  minimal CommonJS (relative/absolute `require`, module cache, `module.exports`,
  `__dirname`/`__filename`), promise jobs, a small timer queue, and the `process`/
  `performance` prelude needed by ordinary programs. It is an execution compatibility
  layer only.
- `compat::should_skip_native` avoids doomed native attempts for TypeScript, JSX, module
  syntax, `import`, `require`, BigInt, private fields, and async sources. It is a hint,
  never a rejection: `--native` always runs the native pipeline so its real located
  diagnostic is reported.
- TypeScript/JSX erasure is cached in-process and on disk by content and parser mode, so
  repeated runs and test suites do not repeat the SWC transform.
- The native diagnostics for BigInt literals and private class names were made
  actionable, and the native parser now rejects dynamic `import()`/`import.meta` instead
  of silently skipping them, so `--native` still demonstrates the “reject, never
  miscompile” contract.

This section is an implementation note, not a revision of the proposal. The phases in
sections 1–10 remain the required work for the 3.0 native runtime. In particular, the
compatibility runtime is not the unified `JsValue`, object model, GC, or UTF-16 string
implementation described below, and its CommonJS/timer support does not change the native
subset or its diagnostics.

---

## 1. Unified dynamic JavaScript value representation

### 1.1 Existing implementation and file locations

- `semantic/src/hir.rs:13-21` defines `Type::{Number, Bool, String, Arr, Obj, Null}`.
- `ir/src/lib.rs:19-27` defines the matching `Type::{F64, Bool, Str, Arr, Obj, Null}`.
- `ir-builder/src/lib.rs:17-26` maps HIR types to IR types.
- `backend-cranelift/src/lib.rs:58-64` maps `F64 → f64`, `Bool → i8`, and `Str/Arr/Obj/Null → I64`.
- Values are block-local SSA values with a static type (`ir/src/lib.rs:5-12`); locals carry cross-block state.
- The interpreter uses `Val` (`runtime/src/interp.rs:16-24`), a second, independent value model.

### 1.2 Architectural limitations

- A local slot, parameter, return value or container element can only hold one static type. There is no way to store `1` in a variable on one path and `"one"` on another.
- Every new dynamic feature requires either a new static type or a new instruction family. Arrays and objects are already split into `ArrayGet/Set`, `ObjGetNum`, `ObjGetStr`, `ObjGetObj`, etc. (`ir/src/lib.rs:83-94`).
- The runtime and interpreter implement the same operations twice with different representations (`Val` vs `*mut JsArray`/`*mut JsObject`/`*mut JsString`), so semantics can drift.
- `Print { ty, value }` (`ir/src/lib.rs:103`) bakes static output selection into IR; the backend prints arrays/objects as `[object Object]` and `null`/`undefined` as `undefined`.
- JavaScript `+`, `==`, `!`, `&&`, `||`, `if`, property access and function calls all depend on runtime type tags. The static type system is therefore not only incomplete, it blocks correct ECMAScript semantics.

### 1.3 Proposed replacement

Introduce a **16-byte tagged value** shared by every stage:

```rust
// new leaf crate: crates/js-value/src/lib.rs
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsValue {
    pub tag: u64,
    pub payload: u64,
}

#[repr(u64)]
pub enum ValueTag {
    Undefined = 0,
    Null = 1,
    Bool = 2,
    Number = 3,
    String = 4,
    Object = 5,
    Function = 6,
}
```

- `Number`: IEEE-754 bits in `payload`; `NaN` is stored as a canonical quiet NaN (`0x7FF8_0000_0000_0000`) by `JsValue::number`.
- `Bool`: `payload` is `0` or `1`.
- `Undefined` / `Null`: `payload` is `0`.
- `String`, `Object`, `Function`: `payload` is a `Gc<T>` pointer encoded as `u64`; decoding is centralized in `runtime::value`.
- Arrays are `Object` values whose object kind is `Array`; this avoids a separate tag that would need 64-bit `length` semantics in every match.

**Why 16 bytes instead of NaN boxing:** a 16-byte tag/payload pair needs no NaN canonicalization on every arithmetic result and makes no 48-bit pointer assumptions. The engine's current priority is correctness. NaN boxing is a possible later optimization only if profiling shows value width is a bottleneck; it is not required now and is explicitly not part of this proposal.

**Why a new `js-value` crate:** `ir`, `ast`, `runtime` and `backend-cranelift` all need the value and UTF-16 string types, but `runtime` already depends on `ir` (`runtime/Cargo.toml`). Putting the shared types in `runtime` would create a dependency cycle. `js-value` depends only on `std`; it contains `JsValue`, `ValueTag`, and `JsStringData` (Section 5).

**IR change:** replace the six static types with one dynamic value type:

```rust
pub enum Type {
    Value,
}
```

`Option<Type>` on a function return remains the void/non-void distinction. Every parameter, local, block value and instruction operand is `Type::Value`. `Print` becomes `Print { value: Value }`; the runtime dispatches on the tag. `ConstNull` is joined by `ConstUndefined`, and the remaining `Const*` instructions produce `Type::Value`.

Static type information is still useful, but it moves **out of the soundness-critical IR**. The HIR keeps a `ty` field for diagnostics and optional optimization hints; the IR verifier no longer trusts it for representation. This deliberately avoids a hybrid system in which some values are static and some dynamic.

**Runtime operations layer:** add `runtime::ops` (or `runtime::value_ops`) with one implementation of every dynamic operation:

- `to_boolean`, `to_number`, `to_string`, `to_primitive`
- `add`, `sub`, `mul`, `div`, `rem`, `neg`
- `strict_eq`, `abstract_eq`, `lt`, `le`
- `truthy` for branches
- `typeof_value`
- `print_value` (Node-compatible inspection for the supported subset)

The Cranelift backend initially calls these helpers for every dynamic operation. Numeric fast paths (`if both tags are Number`) are a later optimization; correctness comes first.

**Cranelift representation:** an IR `Value` becomes a pair of Cranelift values:

```rust
struct ValueRepr {
    tag: clif::Value,     // I64
    payload: clif::Value, // I64
}
```

Runtime helpers use `#[repr(C)] JsValue` parameters and returns. On the current x86-64 System V target a 16-byte two-integer struct is passed and returned in two registers, matching a Cranelift signature with two `I64` results. This ABI must be proven by a dedicated test before mass migration (Section 9.2). If a platform does not match, the fallback is an explicit out-parameter form; no semantic change is required.

**Interpreter:** `Val` is deleted. Frames store `JsValue` and delegate every operation to the same `runtime::ops` functions. This guarantees JIT/interpreter agreement by construction instead of by duplicated logic.

### 1.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `js-value` (new) | `JsValue`, `ValueTag`, `JsStringData`, encoding helpers, unit tests |
| `ast`, `parser`, `lexer` | String literal type becomes `JsStringData` (Section 5) |
| `semantic` | HIR types become dynamic; static hints retained only for diagnostics/optimization |
| `ir` | One value type; const/print/instruction signatures updated; verifier updated |
| `ir-builder` | Emits dynamic values and `ToBool`; lowers every operation to runtime-capable IR |
| `runtime` | `ops` module, generic print, interpreter uses `JsValue` |
| `backend-cranelift` | `ValueRepr` pair, helper signatures, dynamic calls to `runtime::ops` |
| `optimizer` | Constant folding restricted to values it can prove; no static-type assumptions |
| `compiler` | Unchanged pipeline order |
| `cli` | No new dependency; version/threads metadata cleanup |

No new external dependency is required for this step.

### 1.5 Migration sequence

1. Add the `js-value` crate with encoding/decoding and property tests. No pipeline changes.
2. Add `runtime::ops` for primitives (`undefined`, `null`, bool, number, string handles) and unit-test it directly.
3. Change `ir::Type` to `Type::Value` and update `ir::verify` to check def-use, block locality and terminator shape rather than static types.
4. Migrate `ir-builder` constants, locals and arithmetic to dynamic values; add `ToBool` and lower conditions through it.
5. Migrate `runtime::interp` to `JsValue` and `runtime::ops`; delete `Val`.
6. Migrate `backend-cranelift` to `ValueRepr` and helper calls; remove the static print switch.
7. Update `Print` to `rt_print_value`; implement `undefined` and `null` as distinct tags in the same change.
8. Keep the optimizer's numeric/bool folding; disable any fold that depends on a static type it cannot prove.
9. Run the full test suite plus the Node differential matrix.

This is the largest single phase. It should be landed as a sequence of small commits that each keep `cargo test --workspace` green, not as one un-reviewable change.

### 1.6 Compatibility risks

- **ABI risk:** two-word struct passing/returning must match Cranelift's signatures on x86-64 and ARM64. Mitigation: a standalone ABI test that compiles a tiny Cranelift function calling a Rust helper with `JsValue`, plus a runtime assertion on `size_of::<JsValue>() == 16`.
- **Performance regression:** every operation becomes a helper call. Accepted for this milestone; add inline fast paths later behind guards.
- **Optimizer unsoundness:** the optimizer currently assumes static types. Every pass must be audited; constant folding must only fold values whose tags it knows.
- **NaN normalization:** canonicalizing NaN at construction preserves `NaN !== NaN`; tests must still distinguish `-0` and `+0` correctly.
- **Debug output:** `engine inspect` changes from typed IR to dynamic IR. Existing snapshot tests must be updated deliberately.
- **HIR diagnostics:** error messages that currently say "cannot compare number with boolean" become runtime errors or coercion rules. This is a user-visible semantic change and must be documented.

### 1.7 Required tests

- Unit: encode/decode every tag; number round-trip including `NaN`, `±0`, `±Infinity`; bool; null/undefined distinction.
- Unit: `to_boolean`, `to_number`, `to_string`, `to_primitive` tables for all primitive combinations.
- Unit: `add` covers number+number, string+anything, anything+string, bool/number coercion.
- Unit: `strict_eq` vs `abstract_eq` for `null`/`undefined`, `NaN`, `±0`, strings, object identity.
- IR verifier: rejects cross-block values, use-before-def, duplicate definitions, wrong terminator shape; accepts a dynamic function.
- Interpreter/JIT differential: the existing corpus plus new dynamic-value programs.
- Node differential: `console.log` of every supported value, `typeof`, `==`/`===`, `+`, `if (non-boolean)`, `&&`/`||` returning operands.
- ABI: Cranelift calls a Rust `extern "C" fn(JsValue, JsValue) -> JsValue` and gets identical results (Section 9.2).

---

## 2. Object and property storage model

### 2.1 Existing implementation and file locations

- `JsObject` at `runtime/src/lib.rs:407-411` stores three independent maps:
  - `num_props: HashMap<String, f64>`
  - `str_props: HashMap<String, *mut JsString>`
  - `obj_props: HashMap<String, *mut JsObject>`
- `rt_alloc_obj` at `runtime/src/lib.rs:413-419`.
- Get/set helpers at `runtime/src/lib.rs:421-500`.
- `JsArray` at `runtime/src/lib.rs:354-355` stores `Vec<f64>`.
- IR object instructions are type-split: `ObjSetNum/GetNum`, `ObjSetStr/GetStr`, `ObjSetObj/GetObj` (`ir/src/lib.rs:89-94`).
- Semantic keys come from literal member names and a `str_props` set, with static shape inference (`semantic/src/lib.rs:742-807`).

### 2.2 Architectural limitations

- A property cannot change type. `obj.x = 1` and later `obj.x = "one"` require different maps and different instructions; the static semantic layer rejects or misroutes mixed shapes.
- Property key identity is a lossy `String::from_utf8_lossy` conversion of a `JsString` (`runtime/src/lib.rs:424-425`), which is wrong for UTF-16 strings and inconsistent with string equality.
- `HashMap` does not preserve insertion order. ECMAScript defines property enumeration order (integer indices ascending, then strings in insertion order, then symbols), so `Object.keys`/`console.log` cannot be correct.
- Missing numeric properties return `NaN` (`runtime/src/lib.rs:446-448`), missing string/object properties return null (`runtime/src/lib.rs:475`, `502`). JavaScript requires `undefined` for both, and must distinguish "missing" from "present with value `undefined`".
- There is no prototype chain, so `obj.toString`, `arr.length`, `arr.push`, and string methods cannot be implemented uniformly.
- Arrays are `Vec<f64>`: no strings, objects, booleans, holes, or `length` semantics. `rt_array_set` silently ignores growth beyond `len + 1` (`runtime/src/lib.rs:390-401`).
- There is no property descriptor model, so enumerability/writability/configurability cannot be represented.

### 2.3 Proposed replacement

```rust
// runtime::object
pub struct JsObject {
    pub header: GcHeader,
    pub prototype: Option<Gc<JsObject>>,
    pub properties: PropertyMap,
    pub kind: ObjectKind,
}

pub enum ObjectKind {
    Ordinary,
    Array(JsArrayData),
    Function(Gc<JsFunction>),
}

pub struct PropertyMap {
    /// Insertion-ordered storage. Deletion leaves a tombstone or compacts.
    entries: Vec<Property>,
    /// Key -> index for O(1) lookup.
    index: HashMap<PropertyKey, usize>,
}

pub struct Property {
    pub key: PropertyKey,
    pub value: JsValue,
    pub enumerable: bool,
    pub writable: bool,
    pub configurable: bool,
}

pub enum PropertyKey {
    /// Canonical array index, printed as a decimal string.
    Index(u32),
    /// Interned UTF-16 string. ASCII keys use a fast path.
    String(InternedString),
}
```

**Concrete requirements that justify each part:**

- `JsValue` value: mixed-type properties are required by ordinary JS.
- Ordered `PropertyMap`: required for deterministic `console.log`, `Object.keys`, `for...in` and Node differential tests.
- `PropertyKey::Index`: required to canonicalize `"0"` and `0` to the same element and to implement ECMAScript enumeration order.
- Descriptor booleans: required for array `length` (non-enumerable) and builtin methods (non-enumerable), even before `Object.defineProperty` exists.
- `prototype`: required as soon as method lookup is dynamic. Initially install only `Object.prototype`, `Array.prototype`, `Function.prototype` with the small builtin surface the engine already exposes (`toString`, string methods, array methods).
- `ObjectKind::Array`: arrays need `length` and indexed element access without adding a second tag to `JsValue`.

**Arrays:**

```rust
pub struct JsArrayData {
    /// Dense prefix; `None` is a hole (reads as `undefined`).
    elements: Vec<Option<JsValue>>,
    /// Sparse overflow for large indices.
    sparse: HashMap<u32, JsValue>,
    /// ECMAScript `length`, which can exceed the highest present index.
    length: u32,
}
```

- `array[i]` reads `elements[i]` or `sparse[i]`, else `undefined`.
- Writing beyond the dense limit (for example index `> 1_000_000`) goes to `sparse`; this avoids `Vec` blow-up while preserving semantics.
- `length` is maintained on writes; shrinking `length` removes elements above the new length.
- `hasOwnProperty`/`in` must distinguish holes from present-`undefined`.

**Strings and string methods:** with the prototype chain, `s.toUpperCase()` becomes a normal property lookup on the string's prototype. The existing static method recognition in `semantic` (`StrUpper`, `StrIncludes`, etc.) is initially retained as a lowering fast path, but the runtime implementation must be the single source of truth.

**Object property ordering:** iteration yields all `Index` keys in ascending numeric order, then all `String` keys in insertion order, skipping tombstones. This matches ECMAScript for the supported subset.

**IR:** replace the six object instructions with one pair:

```rust
GetProp { dst, obj: Value, key: Value },
SetProp { obj: Value, key: Value, val: Value },
```

and keep `ArrayGet/ArraySet/ArrayLen` only as optimized forms if profiling justifies them; otherwise `GetProp`/`SetProp` handles arrays through `ObjectKind`. The initial implementation should use only `GetProp`/`SetProp` to avoid a second semantic path.

### 2.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `js-value` | `JsValue`, `PropertyKey`-adjacent string data |
| `ir` | `GetProp`/`SetProp` replace six object instructions |
| `ir-builder` | One lowering path for member access and subscript |
| `runtime` | `object.rs`, `PropertyMap`, `JsArrayData`, prototype lookup, Node-style inspection |
| `semantic` | Removes `str_props` and `param_is_obj` heuristics; member access becomes dynamic |
| `backend-cranelift` | New `rt_get_prop`/`rt_set_prop` imports; removes split object imports |
| `optimizer` | Must treat property access as side-effecting/non-pure |

Dependencies: `indexmap` is **not required** if `PropertyMap` is implemented as `Vec + HashMap`. Prefer the custom map to avoid a dependency until the implementation proves it needs one. If `indexmap` is used, it is a concrete ordering requirement, not speculation.

### 2.5 Migration sequence

1. Introduce `PropertyKey`, `Property`, `PropertyMap` with unit tests for insertion order, integer-key ordering, deletion and holes. No pipeline changes.
2. Introduce `JsObject` with `ObjectKind::Ordinary` and `GetProp`/`SetProp`; keep the old helpers as wrappers until all callers migrate.
3. Add `ObjectKind::Array` with dense + sparse elements and `length`.
4. Add minimal prototypes and move string/array methods onto them.
5. Change `semantic` member access to dynamic lookup; delete `param_is_obj` and `str_props` shape inference.
6. Change `ir-builder` to emit `GetProp`/`SetProp`.
7. Implement `print_value` inspection using the property map (`[ 1, 2 ]`, `{ a: 1 }`, holes as `<1 empty item>`, nested depth 2, circular reference markers).
8. Delete the old split maps and helpers.

### 2.6 Compatibility risks

- **Property order:** Node uses insertion order plus integer-index rules; a naive `HashMap` implementation will fail differential tests.
- **Array `length`:** writes, deletes, holes and `length` truncation are easy to get subtly wrong. Test them explicitly.
- **Prototype lookup performance:** every property access walks a chain. Acceptable until inline caches exist; do not add caches in this phase.
- **String keys:** property keys are UTF-16, but internal maps may use a different representation. Equality and hashing must be defined on the same units (Section 5).
- **Builtin scope:** only methods already advertised by the engine should be installed. Do not add a large standard library as part of this migration.
- **`console.log` formatting:** matching Node exactly for nested/circular objects is a separate formatter concern; keep it in one module and test it independently.

### 2.7 Required tests

- `PropertyMap`: insert/update/delete, insertion order, integer-index ordering, key canonicalization (`0` vs `"0"`), hole vs present `undefined`.
- Object: get/set mixed types; missing → `undefined`; prototype lookup; shadowing; `hasOwnProperty`; deletion.
- Array: push/pop/`length`, sparse index, hole reads, truncation, mixed element types, `Array.isArray` if exposed.
- IR: `GetProp`/`SetProp` verifier and optimizer behavior.
- Node differential: `Object.keys`, `for...in` (if supported), arrays with holes, nested object printing, `arr.length` after writes.
- Formatter: golden tests for arrays, objects, cycles, nested depth.

---

## 3. Separate `null` and `undefined`

### 3.1 Existing implementation and file locations

- `ast` already distinguishes `ExprKind::Null` and `ExprKind::Undefined` (`ast/src/lib.rs:119-120`), and the lexer has separate tokens (`lexer/src/lib.rs:30-31`, `210-211`).
- `semantic/src/lib.rs:536-537` lowers both to `Type::Null`; `ExprKind::Undefined` is typed as `Type::Null`.
- `ir-builder/src/lib.rs:289-293` lowers both to `Inst::ConstNull`.
- `ir/src/lib.rs:69` has only `ConstNull`.
- `runtime/src/interp.rs:21` has only `Val::Null`.
- `backend-cranelift/src/lib.rs:577-584` prints `null` as `"undefined"`.
- `typeof` handling in `semantic/src/lib.rs:548-583` hard-codes `null → "object"` and `undefined → "undefined"` from static syntax/type, not runtime values.
- Missing properties return `NaN` or null (Section 2).

### 3.2 Architectural limitations

- `null` and `undefined` are indistinguishable after parsing. `null === undefined` currently compares equal, `null ?? x` and `undefined ?? x` cannot differ, and printing is wrong for both.
- `typeof null` is only correct because of a static special case; once values are dynamic, `typeof` must inspect the runtime tag.
- Missing property reads cannot produce `undefined` because there is no representation for it.
- `OptionalChain` (`ir/src/lib.rs:107`) currently tests only the single `Null` type.
- Default parameters, `void 0`, destructuring defaults, and `hasOwnProperty` semantics all depend on the distinction.

### 3.3 Proposed replacement

- `JsValue` tags: `Undefined = 0`, `Null = 1` (Section 1.3).
- HIR: add `Type::Undefined`; `ExprKind::Undefined` gets `Type::Undefined`; `null` keeps `Type::Null`.
- IR: add `ConstUndefined { dst }`; keep `ConstNull`.
- Runtime: `JsValue::undefined()` and `JsValue::null()`.
- `typeof`: runtime `rt_typeof(value)` returns `"undefined"` for `Undefined`, `"object"` for `Null` and objects, `"function"` for functions, and the primitive names otherwise.
- Missing property and out-of-bounds array reads return `undefined`.
- Strict equality: `Undefined !== Null`; same-tag comparison otherwise.
- Abstract equality (when enabled): `null == undefined` is `true`, and neither equals `0`, `""`, or `false`.
- `??` and optional chaining test both `Null` and `Undefined`.
- `console.log`: `null` prints `null`, `undefined` prints `undefined`.
- `void` expression: not required in this phase; if the parser does not support it, leave it unsupported rather than conflating it with `undefined`.

### 3.4 Affected crates and dependencies

`js-value`, `ast` (no structural change), `semantic`, `ir`, `ir-builder`, `runtime`, `backend-cranelift`, `optimizer` (constant folding for the new tag), `compiler` tests.

No new dependencies.

### 3.5 Migration sequence

1. Add `Undefined` to `JsValue` and `ConstUndefined` to IR.
2. Change HIR `Undefined` to its own type; update static diagnostics that match on `Type::Null`.
3. Lower both constants distinctly; update `NullishCoalesce`/`OptionalChain` tests.
4. Update `rt_print_value` and `typeof`.
5. Change property/array reads to return `undefined` when missing.
6. Update equality helpers and tests.
7. Add `null`/`undefined` cases to the Node differential corpus.

### 3.6 Compatibility risks

- Existing tests expect `null` to print as `undefined`; update them deliberately.
- Existing static comparison rules reject `null == undefined`; dynamic equality must define it rather than reject.
- `typeof` must no longer be folded from static syntax. The optimizer can fold `typeof` only for constants it can prove.
- `OptionalChain` previously only checked `Type::Null`; ensure it checks both tags.
- Property reads that previously returned `NaN`/null may change program output; this is a correctness fix, but it will surface in differential tests.

### 3.7 Required tests

- `null` vs `undefined` printing, `typeof`, `===`, `!==`, `==` (if enabled), `??`, optional chaining.
- Missing object property and out-of-bounds array element both produce `undefined`.
- `null` is not `undefined` in a map/array value; identity and strict equality.
- Constant folding of `null`, `undefined`, and `??` preserves behavior in JIT and interpreter.
- Node differential: a dedicated null/undefined corpus.

---

## 4. Memory ownership and garbage collection

### 4.1 Existing implementation and file locations

- `JsString` owns a `ptr`/`len` pair and an unused `owned` flag: `runtime/src/lib.rs:150-155`.
- `rt_alloc_str` boxes a handle and never frees it: `runtime/src/lib.rs:158-160`.
- `rt_concat_str`, `rt_num_to_str`, `rt_str_upper`, `rt_str_slice`, `rt_str_replace` allocate new byte buffers with `Box::into_raw`: `runtime/src/lib.rs:179-182`, `216-221`, `266-269`, `302-305`, `328-331`.
- `JsArray` and `JsObject` are boxed and never freed: `runtime/src/lib.rs:358-361`, `413-419`.
- String literals are leaked for the JIT process lifetime: `backend-cranelift/src/lib.rs:586-594`.
- `ExecContext` already exists per run and controls call depth/errors (`backend-cranelift/src/lib.rs:8-15`); it is the natural owner for a heap.
- The interpreter uses `Rc<RefCell<...>>` (`runtime/src/interp.rs:21-22`), which cannot collect cycles and retains values for the process.

### 4.2 Architectural limitations

- Every `Box::into_raw` is a leak. A loop that concatenates strings or creates objects grows memory without bound.
- The `owned` flag is never read; there is no ownership model at all.
- `Rc` in the interpreter leaks cycles (two objects referencing each other) and is a different memory model from native code.
- There is no root set. Even a tracing collector cannot know which values are live in JIT-generated code.
- `Box::leak` for string literals is process-lifetime memory, not run-lifetime memory; repeated `CompiledModule::run` calls accumulate.
- The runtime is single-threaded (`CompiledModule` is intentionally not `Send`/`Sync`; `cli` ignores `--threads`), so a stop-the-world collector is acceptable initially.

### 4.3 Proposed replacement

**Collector:** precise, non-moving, stop-the-world mark-sweep.

- Non-moving: existing `Gc<T>` pointers remain valid, so Cranelift code does not need to update pointers after collection.
- Mark-sweep: handles cycles; no reference-count cycle collector.
- Stop-the-world: collection happens only at explicit safepoints; no concurrent marking in this phase.
- Generational/compacting/concurrent collection is explicitly deferred until measurements justify it.

**Heap:**

```rust
// runtime::gc
pub struct Heap {
    head: *mut GcHeader,
    bytes_allocated: usize,
    threshold: usize,
    gc_requested: bool,
}

#[repr(C)]
pub struct GcHeader {
    pub mark: Cell<bool>,
    pub kind: GcKind,
    pub next: *mut GcHeader,
    pub size: usize,
}

pub struct Gc<T> {
    ptr: NonNull<GcBox<T>>,
}

#[repr(C)]
pub struct GcBox<T> {
    pub header: GcHeader,
    pub value: T,
}
```

- `Heap::alloc<T: Trace>(value: T) -> Gc<T>` links a `GcBox` into the intrusive list.
- `Trace` visits every `JsValue` held by the object: strings have no children; arrays/objects visit property values and prototypes; functions visit captures and prototype; `BindingCell` visits its value.
- `Heap::collect(&mut RootSet)` marks from roots, sweeps unmarked boxes, and resets marks.
- `ExecContext` owns the `Heap`; dropping the context frees every remaining object. This fixes process-lifetime leaks even before collection runs.

**Roots — native code:** a shadow stack.

- `ExecContext` owns `roots: Vec<JsValue>`.
- Every generated function reserves `locals.len()` root slots on entry (`rt_enter_frame(ctx, nslots)`), writes every local assignment to both the Cranelift variable and its root slot, and releases the slots on return (`rt_leave_frame`).
- Parameters are stored into root slots **before** the function-entry safepoint.
- Values held only in registers between safepoints are not roots, but this is safe because collection happens only at safepoints placed where no block-local temporaries are live:
  - function entry, after parameters are rooted;
  - loop headers/backedges, where cross-block state is in locals.
- Allocation helpers never collect directly. They set `gc_requested` when the threshold is exceeded. The next safepoint performs collection. This avoids needing to root every temporary inside a helper.
- A `Safepoint` IR instruction is emitted at those points. It is side-effecting and must not be removed or reordered by the optimizer.
- If a future pass needs a safepoint in the middle of an expression, it must root the live temporaries first. Document this invariant next to `Safepoint`.

**Roots — interpreter:** the explicit `Vec<Frame>` is already a complete root set. Frames hold `JsValue` locals and value slots; collection scans all frames. The interpreter can collect at function entry and loop headers, matching native behavior.

**Literal strings:** remove `Box::leak`. A `ConstStr` lowers to a runtime allocation (`rt_const_str(ctx, ptr, len)`) that copies the UTF-16 literal into the GC heap on execution. This is correct and allows collection. A per-run literal table is a later optimization if allocation shows up in profiles.

**GC trigger and stress mode:**

- Default threshold: 1 MiB, doubling after a collection that frees less than 25% of the heap.
- `ENGINE_GC_STRESS=1` (or `--gc-stress`) collects at every safepoint.
- A collection during an active helper is forbidden; helpers must be allocation-only or safepoint-free.

**Threads:** keep the engine single-threaded. Remove or explicitly reject `--threads` (`cli/src/main.rs:17`, `:128`) instead of silently ignoring it. A GC that is not thread-safe must not be exposed behind a thread flag.

### 4.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `runtime` | `gc.rs`, `Trace`, `Heap`, `RootSet`, `Gc<T>`, `ExecContext` ownership; all allocation helpers migrated |
| `js-value` | `JsValue` pointer encoding remains opaque; no GC dependency |
| `ir` | `Safepoint` instruction |
| `ir-builder` | Emits safepoints at entry/loop headers |
| `backend-cranelift` | Frame root slots, `rt_enter_frame`/`rt_leave_frame`/`rt_safepoint`, remove literal leak |
| `optimizer` | Treats `Safepoint` as a barrier |
| `cli` | GC stress flag; honest `--threads` behavior |
| `runtime::interp` | Roots all frames; collection at matching safepoints |

No external GC crate. `gc-arena` and similar lifetime-based collectors are a poor fit for JIT raw pointers and would force a rewrite of the value model; a small custom mark-sweep is more appropriate for this codebase.

### 4.5 Migration sequence

1. Add `gc.rs` with `GcHeader`, `Gc<T>`, `Heap::alloc`, intrusive list, and a test-only `collect` that marks nothing; unit-test allocation/drop.
2. Move all runtime allocations (`JsString`, `JsArray`, `JsObject`, future `BindingCell`) behind `Heap::alloc`; keep `Box` ownership inside `GcBox`.
3. Add `Trace` implementations and a mark phase; test with synthetic object graphs, including cycles.
4. Add `Safepoint` to IR and emit it at function entry and loop headers.
5. Add shadow-stack frame calls to the Cranelift backend; root every local.
6. Teach the interpreter to root frames and collect at safepoints.
7. Remove `Box::into_raw`/`Box::leak` from `runtime` and `backend-cranelift`; add a CI grep guard.
8. Add GC stress mode and long-loop tests.
9. Drop the heap with `ExecContext`; verify repeated runs do not grow RSS.

### 4.6 Compatibility risks

- **Missing roots → use-after-free.** This is the highest-risk part of the migration. Mitigation: GC stress mode, ASan/Valgrind runs, and a strict rule that every local has a root slot.
- **Safepoint placement:** collecting while a block-local temporary is live and unrooted would be a correctness bug. Keep safepoints at block boundaries only and document the invariant.
- **Optimizer barriers:** if `Safepoint` is treated as pure, dead-code elimination can remove it. Verifier and optimizer tests must cover this.
- **Performance:** shadow-stack stores on every local assignment and stop-the-world pauses. Acceptable initially; optimize later with stack maps or fewer safepoints.
- **String literals:** removing `Box::leak` means literals are collected; any raw pointer cached outside the heap becomes invalid. Ensure literals are only referenced through `JsValue`.
- **GC and JIT ABI:** helpers must not allocate after taking raw pointers unless those pointers are rooted. Keep helper scopes small.
- **Threads:** a single-threaded collector is not compatible with the advertised `--threads`; fix the CLI rather than pretending.

### 4.7 Required tests

- `Heap`: allocate/drop, threshold growth, mark/sweep, cycles, large object graphs.
- GC stress: every existing test under `ENGINE_GC_STRESS=1`.
- Long loop: allocate one million strings/objects; assert RSS stays bounded and no crash.
- Roots: a local holding the only reference to an object survives a collection; an unreachable object is collected.
- Safepoints: verifier/optimizer tests that `Safepoint` is preserved and acts as a barrier.
- Repeated `run()`: output identical and memory does not grow across 1,000 runs.
- Miri for `runtime` and `js-value` unit tests where JIT is not involved; ASan/Valgrind for the JIT integration where supported.
- Leak guard: `rg "Box::into_raw|Box::leak" crates/runtime crates/backend-cranelift` returns no production hits at the end of the phase.

---

## 5. UTF-16 string semantics

### 5.1 Existing implementation and file locations

- `JsString { ptr: *const u8, len: usize, owned: bool }` at `runtime/src/lib.rs:150-155`.
- String literals are decoded to Rust `String` in the lexer (`lexer/src/lib.rs:431-500`); `TokenKind::Str(String)` (`lexer/src/lib.rs:13-14`).
- `String` appears in AST/HIR/IR constants: `ast` `ExprKind::Str(String)`, `hir::ExprKind::Str(String)`, `ir::Inst::ConstStr { value: String }`.
- `rt_str_len` returns byte length (`runtime/src/lib.rs:250-256`).
- `rt_str_slice` indexes UTF-8 bytes and can panic on a non-char boundary (`runtime/src/lib.rs:288-306`).
- `rt_str_upper` uses Rust `to_uppercase` on UTF-8 (`runtime/src/lib.rs:258-269`).
- `rt_str_includes`, `rt_str_replace`, `rt_str_split_count` use UTF-8 Rust strings.
- Property keys are built with `String::from_utf8_lossy` (`runtime/src/lib.rs:424-425`).

### 5.2 Architectural limitations

- JavaScript strings are sequences of UTF-16 code units, not UTF-8 bytes and not Unicode scalar values.
- `"😀".length` is `2` in JavaScript, but `rt_str_len` returns UTF-8 byte length `4` (and a `chars().count()` implementation would return `1`).
- `slice`, `charAt`, `charCodeAt` and indexing operate on code units; byte or scalar indexing produces different results and can split a UTF-8 sequence.
- Lone surrogates are valid JavaScript string values (`"\uD800"`) but cannot be represented in Rust `String`.
- Property keys with non-BMP characters are compared by code units; lossy UTF-8 conversion can merge distinct strings.
- `toUpperCase` on a lone surrogate must not panic or silently drop data.

### 5.3 Proposed replacement

```rust
// js-value
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JsStringData {
    /// UTF-16 code units. May contain lone surrogates.
    units: Box<[u16]>,
}
```

- `units.len()` is the JavaScript `length`.
- Indexing, `slice`, `charAt`, `charCodeAt` and comparisons use code units.
- Concatenation appends code units; surrogate pairs and lone surrogates are preserved.
- Equality is exact code-unit equality.
- `toUpperCase` maps well-formed scalar values with Rust's Unicode case conversion; lone surrogates are passed through unchanged. Full ECMAScript case-conversion tables are a later conformance task.
- Source code and identifiers remain Rust `String` (source is valid UTF-8); only runtime string values and property keys are UTF-16.
- Conversion at the boundary:
  - Source escape decoding builds `u16` units, including surrogate pairs and lone surrogates.
  - Output uses `String::from_utf16_lossy` (lone surrogates become U+FFFD, matching typical Node stdout encoding).
  - `JSON`/source emission, if added later, must not use lossy conversion blindly.

**`JsString` object:**

```rust
pub struct JsString {
    pub header: GcHeader,
    pub data: JsStringData,
}
```

`JsValue::String(Gc<JsString>)`. Property keys use an interned form of `JsStringData`; the interner lives in `ExecContext`/`Heap` and guarantees stable equality/hash.

**Lexer/parser/AST:** `TokenKind::Str(JsStringData)`; `ExprKind::Str(JsStringData)`; `hir::ExprKind::Str(JsStringData)`; `ir::Inst::ConstStr { value: JsStringData }`. The `Display` implementation for `JsStringData` escapes non-ASCII units for readable IR dumps.

### 5.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `js-value` | `JsStringData`, escape/format helpers |
| `lexer` | String token type; decode escapes to `u16` |
| `ast`, `parser` | String literal type; template literal pieces |
| `semantic` | HIR string literal type; string method lowering |
| `ir` | `ConstStr` value type; `Display` escaping |
| `ir-builder` | Pass-through |
| `runtime` | All string helpers operate on `[u16]`; `rt_str_len/slice/...`; property key conversion |
| `backend-cranelift` | Static literal bytes are UTF-16; length is unit count |
| `optimizer` | String constants, if folded, use `JsStringData` |

No external dependency. `unicode-segmentation` is not required: JavaScript string length/indexing is code-unit based, not grapheme based.

### 5.5 Migration sequence

1. Add `JsStringData` with unit tests for ASCII, BMP, astral pairs, lone surrogates, equality and hashing.
2. Add escape parsing/formatting helpers; test `\u`, `\u{...}`, surrogate pairs and lone surrogates.
3. Change the lexer token and AST literal types.
4. Change HIR/IR constants and `Display`.
5. Change runtime helpers one at a time: length, equality, concat, slice, includes, replace, split, upper.
6. Change property key creation to use UTF-16 units.
7. Change the backend's static literal emission.
8. Add Node differential tests for all of the above.
9. Delete `from_utf8_unchecked`/byte-indexing string code.

### 5.6 Compatibility risks

- **Lossy conversion:** using `String` anywhere in the runtime path will silently convert lone surrogates. Keep `JsStringData` through the entire path.
- **Slice boundaries:** indices are code-unit offsets; a slice may split a surrogate pair. JavaScript permits this; do not "fix" it to scalar boundaries.
- **Case conversion:** Rust's Unicode tables differ from ECMAScript's versioned Unicode data. Document the supported subset and pin the Unicode version or add a conformance note.
- **Property keys:** hashing `[u16]` is more expensive than `str`. Add an ASCII fast path only if measured.
- **IR display:** `engine inspect` output changes; update golden tests.
- **Template literals:** they also produce strings; do not leave them on the old `String` path.

### 5.7 Required tests

- `"abc".length === 3`, `"😀".length === 2`, `"😀".slice(0,1).length === 1`, `charCodeAt` high/low surrogate values.
- Lone surrogate literal `"\uD800"` round-trips through concat, slice, equality and property keys.
- `"\uD83D\uDE00"` and `"\u{1F600}"` produce the same units.
- `slice` with negative offsets, out-of-range offsets and reversed ranges.
- `includes`/`replace`/`split` with astral characters and lone surrogates.
- Property keys `"😀"` and `"\uD800"` are distinct from similar-looking strings.
- Node differential: string corpus includes astral characters, lone surrogates, combining marks and RTL text.
- Formatter/IR display: escaped output is stable.

---

## 6. TypeScript annotation AST and type-checking pipeline

### 6.1 Existing implementation and file locations

- `skip_type_annotation()`: `parser/src/lib.rs:126-159`; called for parameters, variables, return types and declarations at lines 243, 288, 323, 676, 709, 739, 748, 771.
- `skip_type_decl()`: `parser/src/lib.rs:161-192`; `skip_module_decl()`: `parser/src/lib.rs:194-223`.
- `parse_stmt` dispatches `Type`/`Interface` to `skip_type_decl` and `Export`/`Import` to `skip_module_decl` (`parser/src/lib.rs:548-549`).
- There are no type-annotation nodes in `ast/src/lib.rs`; no type checker exists.
- `compiler::compile_to_ir` runs parser → semantic → IR (`compiler/src/lib.rs:17-21`), with no TS pass.
- The CLI accepts `.ts`, `.mts`, `.cts` but not `.tsx`/`.jsx` (`cli/src/main.rs:94-100`).

### 6.2 Architectural limitations

- Type annotations are discarded before the AST exists, so a checker cannot run and diagnostics cannot point at types.
- `interface`, `type`, `import` and `export` declarations are silently dropped. This is worse than rejecting them: a program can compile with missing runtime behavior (`export`/`import`) or missing type information without any warning.
- There is no distinction between "type-only" syntax that is safely erased (`: number`, `interface`, `type alias`) and syntax with runtime semantics (`enum`, decorators, namespaces, `import` values).
- TypeScript grammar is not JavaScript grammar; parsing types requires a real type AST, including unions, generics, object types, tuples, function types, literal types and type operators.
- The parser has no dialect mode, so it cannot distinguish `.ts` from `.tsx` or JSX.
- There is no conformance target, so "TypeScript support" cannot be measured.

### 6.3 Proposed replacement

**Dialect-aware parsing:**

```rust
pub enum Dialect { Js, Ts, Tsx }

pub struct ParseOptions {
    pub dialect: Dialect,
}

pub fn parse_with_options(source: &str, options: &ParseOptions) -> Result<Program>;
```

`parse(source)` remains as a JS-compatible entry point for existing tests. The CLI chooses the dialect from the extension. `.tsx`/`.jsx` are rejected with a clear "not implemented yet" diagnostic until JSX parsing exists; do not accept them and parse `<` as comparison.

**Type AST:**

```rust
pub enum TypeAnn {
    Primitive(PrimitiveType),          // number, string, boolean, any, unknown, never, void, ...
    Literal(LiteralType),              // 42, "x", true
    Array(Box<TypeAnn>),
    Tuple(Vec<TypeAnn>),
    Object(Vec<TypeMember>),
    Union(Vec<TypeAnn>),
    Intersection(Vec<TypeAnn>),
    Function { params: Vec<ParameterAnn>, ret: Box<TypeAnn> },
    TypeRef { name: Ident, args: Vec<TypeAnn> },
    Typeof(Expr),
    Keyof(Box<TypeAnn>),
    Parenthesized(Box<TypeAnn>),
    Optional(Box<TypeAnn>),
}
```

Attach annotations to declarations instead of discarding them:

- `Param { name, ty: Option<TypeAnn>, optional: bool }`
- `FunctionDecl { name, params, ret: Option<TypeAnn>, body, ... }`
- `VarDecl { name, ty: Option<TypeAnn>, init, ... }`
- New `StmtKind::{TypeAlias, Interface, Enum, Import, Export}`.

**Checker (new `typecheck` crate, or a `semantic::ts` module):**

- Runs after parsing and before HIR lowering for `.ts` inputs.
- Produces `Diagnostic` values with spans; no code generation.
- Execution still erases type-only syntax after checking; the HIR does not need a second type system for codegen.
- Initial conformance target (v1):
  - primitive annotations, arrays, tuples, object types, unions, literal types;
  - function parameter/return annotations, optional parameters;
  - variable annotations and inference from initializers;
  - `type` aliases and `interface` declarations with structural assignability;
  - basic generics (identity/constraints) and `as` assertions;
  - excess property checks for object literals;
  - `null`/`undefined` strictness (configurable; default strict).
- Explicitly out of v1: conditional/mapped types, template literal types, declaration merging, decorators, namespaces, `enum` runtime semantics, `import`/`export` linking, JSX. These must produce a clear "unsupported TypeScript feature" diagnostic, not be skipped.
- Type-only imports/exports (`import type`, `export type`) can be erased safely; value imports/exports must be diagnosed as "modules not implemented yet."
- `enum` is not erasable (it creates runtime objects). Either lower it in a later phase or reject it in v1.
- `as`/`!`/`satisfies` are erased after checking. `satisfies` can be accepted once the checker supports the expression type.

**Compiler pipeline:**

```
source
  -> lexer
  -> parser (dialect)
  -> AST (annotations retained)
  -> [typecheck for TS]
  -> semantic/HIR (type-only syntax erased)
  -> IR -> optimizer -> backend
```

**Conformance testing:** run a fixture corpus through `tsc --noEmit --strict` and through the new checker; compare accept/reject. Do not compare exact error text. Skip the test loudly if `tsc` is not installed.

### 6.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `lexer` | Already emits `Type`/`Interface`/`Import`/`Export` tokens; add any missing TS punctuation if needed |
| `ast` | `TypeAnn`, declaration fields, new statement kinds, dialect-independent |
| `parser` | Real type parser; annotation attachment; no silent skipping; dialect mode |
| `typecheck` (new) or `semantic` | Checker, symbol tables, assignability, diagnostics |
| `semantic` | Erases type-only syntax; rejects runtime TS constructs with clear errors |
| `compiler` | Runs the checker for TS dialects |
| `cli` | Extension→dialect mapping; `.tsx`/`.jsx` diagnostics; `--check-types` if useful during migration |
| `diagnostics` | Possibly add diagnostic codes for TS errors |

No new external dependency is required. A full TypeScript parser/checker is a large project; the proposal deliberately scopes v1 to a checked subset and rejects the rest.

### 6.5 Migration sequence

1. Add `TypeAnn` and declaration fields to `ast` with parser unit tests; do not change execution.
2. Implement `parse_type_annotation` and attach annotations; replace every `skip_type_annotation` call.
3. Parse `type` aliases and `interface` declarations into AST nodes; stop skipping `import`/`export`, and emit explicit diagnostics.
4. Add a `--check-types` mode that runs a minimal checker but still executes; use it to build the corpus.
5. Implement assignability/inference incrementally: primitives → arrays/objects → unions/literals → functions → aliases/interfaces → generics.
6. Make the checker default-on for `.ts`/`.mts`/`.cts`; keep `--no-check-types` for migration if needed.
7. Add `tsc` differential fixtures and CI wiring.
8. Add `.tsx`/JSX only in a separate milestone; until then, reject the extensions.

### 6.6 Compatibility risks

- **Grammar ambiguity:** `<T>(x: T) => x` versus comparison; `as` precedence; arrow return types. Use speculative parsing with token-position rewind and test ambiguities explicitly.
- **Silent erasure:** the current behavior hides missing modules. Rejecting is a breaking change; it is the correct one.
- **Scope of checking:** claiming "TypeScript support" with a subset is misleading. Publish a conformance table and make unsupported constructs hard errors.
- **Type/runtime divergence:** TypeScript's `enum`, `namespace`, parameter properties and decorators have runtime semantics. Never erase them.
- **Performance:** checking large files is additional compile time; keep it optional until the corpus is stable.
- **Diagnostics:** users expect `tsc`-quality messages; v1 should prioritize correct accept/reject over message parity.
- **`tsc` availability:** CI may not have TypeScript installed; skip the differential test loudly, do not silently pass.

### 6.7 Required tests

- Parser: every `TypeAnn` form, nested generics, unions/intersections, function types, tuples, object types, optional params, return types.
- Parser: annotations on `let`/`const`/`var`, function params/returns, arrow functions, class-free code.
- Parser: `type` alias and `interface` bodies are retained, not skipped.
- Checker: valid/invalid assignments for primitives, arrays, objects, unions, literals, functions; excess property checks; `null`/`undefined` strictness.
- Checker: aliases/interfaces and structural compatibility; basic generic identity.
- Diagnostics: unsupported `enum`, `namespace`, decorator, value `import`/`export`, JSX produce clear errors.
- `tsc` differential: fixtures accepted/rejected by both; no silent divergence.
- Regression: all existing `.js` tests still parse and run; `.ts` tests produce the same output after checking.

---

## 7. First-class functions and closures

### 7.1 Existing implementation and file locations

- `ir::Inst::Call { dst, callee: FuncRef, args }` is direct-only (`ir/src/lib.rs:101`).
- `semantic` resolves function names to `FuncId` and rejects using a function as a value: "functions can't be used as values yet" (`semantic/src/lib.rs:544-546`).
- Function declarations are collected before body checking (`semantic/src/lib.rs:100-128`); there is no function-expression value.
- Parser supports `function` declarations and a few arrow forms, including a special `filter` case (`parser/src/lib.rs:325-423`, `659-755`).
- IR has no closure, capture, function object or indirect-call instruction.
- The backend calls `FuncRef` directly; there is no function value in the runtime.

### 7.2 Architectural limitations

- Functions cannot be passed, returned, stored in variables/objects/arrays, or called dynamically.
- There are no closures or captured variables. The parser's arrow handling desugars only narrow syntactic patterns into named declarations; it is not general.
- There is no `this`, `arguments`, `call`/`apply`/`bind`, or method dispatch on function values.
- Function declarations are hoisted as direct symbols; there is no runtime function object.
- A dynamic call cannot know the callee's arity or code pointer at compile time.

### 7.3 Proposed replacement

**Runtime function object:**

```rust
pub struct JsFunction {
    pub header: GcHeader,
    pub code: *const u8,                 // uniform dynamic entry point
    pub captures: Box<[Gc<BindingCell>]>, // empty for non-closures
    pub prototype: Option<Gc<JsObject>>,
    pub name: JsStringData,
    pub length: u32,
}

pub struct BindingCell {
    pub header: GcHeader,
    /// `None` models the temporal dead zone for captured `let`/`const`.
    pub value: Option<JsValue>,
}
```

**Capture model:** box each captured binding individually in a `BindingCell`.

- A closure stores `Gc<BindingCell>` for each free variable it captures.
- Reads/writes inside the closure go through `GetCapture`/`SetCapture`.
- A `let` declaration executed multiple times (for example inside a loop) allocates a fresh cell each execution, which gives per-iteration capture semantics.
- Cells use `None` for the TDZ; reading before initialization raises `ReferenceError`, matching JavaScript.
- This is simpler than environment chains and sufficient for the minimum closure support. Environment records can replace it later without changing the AST/checker.

**HIR:**

```rust
// Function metadata
pub struct Function {
    pub captures: Vec<Capture>, // outer slot, inner slot, by-reference
    ...
}

pub enum ExprKind {
    Closure { func: FuncId, captures: Vec<Capture> },
    GetCapture(u32),
    SetCapture(u32, Box<Expr>),
    CallValue(Box<Expr>, Vec<Expr>),
    ...
}
```

- Free-variable analysis runs in `semantic` before lowering. It marks captured locals and decides which function declarations must become closure objects.
- Non-escaping function declarations with no captures keep direct calls.
- Function declarations that are used as values (or capture variables) produce a closure object at the declaration's scope entry, preserving hoisting.
- `Call` remains for statically known, non-reassigned callees; `CallValue` handles everything else.

**IR:**

```rust
MakeClosure { dst, func: FuncRef, captures: Vec<Value> },
GetCapture { dst, index: u32 },
SetCapture { index: u32, src: Value },
CallValue { dst, callee: Value, args: Vec<Value> },
```

- `MakeClosure` calls `rt_make_closure(ctx, code_ptr, captures_ptr, len)`.
- `CallValue` calls `rt_call_value(ctx, callee, argc, argv_ptr)`; the runtime checks callability and call depth.
- `Call` stays direct.

**Uniform dynamic entry point:** every function that can be called dynamically gets a wrapper with a stable ABI:

```rust
extern "C" fn dynamic_entry(
    ctx: *mut ExecContext,
    callee: *const JsFunction,
    this: JsValue,
    argc: usize,
    argv: *const JsValue,
) -> JsValue;
```

- Direct calls keep the existing specialized internal signature; the wrapper unpacks `argv` into the internal parameters.
- Missing arguments become `undefined`; extra arguments are ignored (or exposed later through `arguments`).
- `this` is included in the wrapper from the start because function values are primarily useful for method calls; plain calls pass `undefined`. This is not a speculative abstraction: once functions are values, `obj.f()` must not require a second ABI revision.
- `CallValue` uses Cranelift `call_indirect` with this signature. The callee's code pointer comes from `JsFunction.code`.
- The wrapper is also the place to enforce `MAX_CALL_DEPTH`; generated functions still keep the existing depth counter in `ExecContext`.

**Interpreter:** frames gain `callee: Option<Gc<JsFunction>>` and `captures`; `CallValue` pushes a frame exactly like a direct call. `this` is stored in the frame even if v1 only exposes it to future method builtins.

**Minimum feature set for this phase:**

- function declarations as values;
- function expressions and arrow functions as closures;
- capture by reference, including mutation;
- returning/passing/storing functions;
- recursion through a function value;
- per-iteration `let` capture;
- TDZ for captured bindings;
- `typeof f === "function"`;
- direct calls unchanged.

**Deferred:** `arguments`, `call`/`apply`/`bind`, `new`, constructors, getters/setters, `Function.prototype`, async/generator functions, `eval`.

### 7.4 Affected crates and dependencies

| Crate | Change |
|---|---|
| `ast` | Function expressions/arrows as first-class AST nodes; parameter/capture metadata if needed |
| `parser` | General arrow/function-expression parsing; remove narrow desugaring |
| `semantic` | Free-variable/capture analysis; function values; TDZ cells; HIR closure nodes |
| `ir` | `MakeClosure`, `GetCapture`, `SetCapture`, `CallValue` |
| `ir-builder` | Lower closures and dynamic calls; direct-call fast path |
| `runtime` | `JsFunction`, `BindingCell`, `rt_make_closure`, `rt_call_value`, `this` in frames |
| `backend-cranelift` | Dynamic entry wrappers, `call_indirect`, capture loads/stores, function object ABI |
| `runtime::interp` | Closure frames, capture cells, dynamic calls |
| `optimizer` | Treats `MakeClosure` as allocating; may not inline/erase it |
| `js-value` | `JsValue::Function` tag; no dependency on runtime types |

No new external dependency.

### 7.5 Migration sequence

1. Add `JsFunction` and `BindingCell` to the runtime with unit tests; allocate through the GC heap.
2. Add HIR capture analysis and closure nodes; add parser support for function expressions/arrows as expressions.
3. Add `MakeClosure`, `GetCapture`, `SetCapture` and `CallValue` to IR, verifier and interpreter.
4. Implement `rt_make_closure` and `rt_call_value`; add the uniform dynamic entry wrapper to the backend.
5. Lower function declarations used as values to `MakeClosure`; keep direct calls for non-escaping functions.
6. Implement capture cells in the backend and interpreter; add TDZ checks.
7. Remove the parser's `filter`/arrow special cases and the semantic "functions can't be used as values" rejection.
8. Add closure tests under GC stress and Node differential.

### 7.6 Compatibility risks

- **TDZ and hoisting:** captured `let`/`const` must throw on early access; `var` has different rules. Do not model all bindings as initialized cells.
- **Per-iteration capture:** a single cell for a loop variable is the classic closure bug. Allocate a cell per `let` declaration execution and test `for (let i...)` closures.
- **Call ABI:** adding `this` changes wrappers, but only dynamic entries. Direct calls remain stable.
- **GC:** a closure captures cells; cells capture values; cycles are expected (`function f(){ return f; }`). Mark-sweep must handle them.
- **Escape analysis:** marking the wrong locals as captured is safe but slow; failing to mark one is a correctness bug. Test nested closures and mutation.
- **Parser special cases:** the existing `filter` desugaring is not general. Removing it may break examples/tests that relied on it; replace them with real closure tests.
- **Function identity:** each evaluation of a function expression creates a new object; `f === f` is true, `make() === make()` is false. Test it.
- **`this`:** binding rules are subtle. In this phase, only pass `this` through; do not claim full method semantics until property calls are implemented.

### 7.7 Required tests

- Counter closure: `function make(){ let n=0; return ()=>++n; }` retains state across calls.
- Shared capture: two closures over the same binding observe each other's writes.
- Per-iteration capture in `for (let i...)` produces `0,1,2`, not `3,3,3`.
- Nested closures capture through multiple levels.
- Return/pass/store functions in variables, arrays and objects.
- Recursion through a function value; mutual recursion.
- TDZ: calling a closure that reads a captured `let` before initialization throws.
- `typeof f === "function"`; function identity; arity/missing/extra arguments.
- GC stress: closures and cells are collected; cycles are reclaimed.
- Direct-call vs `CallValue` differential; interpreter vs JIT differential.
- Node differential for all closure cases.

---

## 8. Preserving Cranelift and the reference interpreter

### 8.1 Existing implementation and file locations

- Cranelift backend: `backend-cranelift/src/lib.rs` (type mapping `:58-64`, runtime imports `:77-192`, print `:555-584`, static string leak `:586-594`).
- Reference interpreter: `runtime/src/interp.rs`, with `Val` and duplicated semantics.
- Differential harnesses: `cli/tests/differential.rs`, `cli/tests/random_diff.rs`.
- Compiler tests run interpreter on optimized and unoptimized IR (`compiler/src/lib.rs:33-60`).
- IR verifier: `ir/src/verify.rs`, run after lowering and every optimizer pass (`compiler/src/lib.rs:6-14`, `optimizer/src/lib.rs`).

### 8.2 Architectural limitations

- The backend and interpreter implement separate value models and operations; each new feature must be implemented twice.
- Static IR types force the backend to choose machine representations per type; dynamic values cannot be represented without a uniform repr.
- `Print` and string/array/object instructions are backend-specialized, so runtime semantics leak into codegen.
- The optimizer assumes static types and purity in places where dynamic operations are side-effecting (property access, allocation, calls).
- The verifier checks static types, which will no longer exist.

### 8.3 Proposed replacement

Keep both execution paths, but make them share one semantics layer:

```
                    +----------------------+
source -> ... -> IR |  runtime::ops        |
                    |  value/object/string |
                    +----------+-----------+
                               |
             +-----------------+-----------------+
             |                                   |
   backend-cranelift (JIT)              runtime::interp
   calls rt_* helpers                   calls the same rt_* helpers
```

**Backend:**

- All IR values are `ValueRepr { tag, payload }`.
- Arithmetic, comparison, truthiness, property access, calls and printing call `runtime::ops` helpers.
- Cranelift retains control flow, locals/SSA, direct calls, stack frames, and (later) guarded numeric fast paths.
- `Safepoint` calls are emitted at function entry and loop headers.
- Function parameters/locals use root slots as described in Section 4.
- `ConstStr` copies UTF-16 literals into the GC heap; no `Box::leak`.
- `Print` becomes `rt_print_value`.
- `Call` remains direct; `CallValue` uses `call_indirect` through the uniform wrapper.
- The JIT's `ExecContext` owns the heap, roots and error/depth state.

**Interpreter:**

- Frames store `JsValue`, not `Val`.
- Every operation delegates to `runtime::ops`; the interpreter only implements control flow, frame management and calls.
- Frames are the root set; collection happens at the same safepoints as native code.
- The interpreter remains the reference for differential testing and for environments where JIT is unavailable.

**Optimizer:**

- Constant folding only for values with known tags; `Const` covers numbers, booleans, null and undefined initially, strings when the string type is stable.
- `Safepoint`, property access, allocation and dynamic calls are barriers.
- Dead-code elimination must not remove side-effecting runtime calls.
- The verifier is rewritten for dynamic values: def-use, dominance/block locality, terminator shape, call target validity, and `Safepoint` preservation. It no longer enforces static types.

**Static hints:** HIR may keep `ty: Option<Type>` for diagnostics and optional optimization, but the IR and backend must not depend on it for correctness. A hint can only enable an optimization after a runtime tag check or when the value is a provable constant.

### 8.4 Affected crates and dependencies

All crates in the pipeline. No new external dependencies beyond those already listed. Cranelift stays pinned at `0.112` (`Cargo.toml:23-27`); do not upgrade it during this redesign. The existing runtime symbol/import mechanism (`backend-cranelift/src/lib.rs:141-279`) is extended, not replaced.

### 8.5 Migration sequence

1. Rewrite the verifier for dynamic values and add tests.
2. Migrate the interpreter to `runtime::ops`; this becomes the executable specification.
3. Migrate the backend value representation and helper imports; run differential tests after each instruction family.
4. Update the optimizer to the new purity/constant rules.
5. Add `Safepoint` and root-slot handling.
6. Add dynamic calls/closures.
7. Keep `--interp`, `--no-opt` and `inspect` working throughout.

### 8.6 Compatibility risks

- **Divergence:** if the interpreter and JIT call different helpers, differential tests will catch it only if the corpus covers the feature. Add a table of operations and test each in both paths.
- **Verifier weakening:** a dynamic verifier that only checks def-use can miss real bugs. Keep structural checks strict and add targeted tests for every instruction.
- **Optimizer unsoundness:** side-effecting helpers must be modeled. When in doubt, mark an instruction impure.
- **Cranelift version:** pinned; API changes are out of scope.
- **Performance:** helper calls are slower than inline code. Acceptable for correctness; add fast paths later with guards.
- **Platform ABI:** two-word values must be validated on x86-64 first; ARM64 is a later target but the design should not assume little-endian layout beyond the explicit tag/payload encoding.

### 8.7 Required tests

- Every IR instruction: verifier accept/reject, interpreter result, JIT result, optimized result.
- Differential matrix: JIT vs interpreter, optimized vs unoptimized, both vs Node, for every supported feature.
- `Safepoint` preservation through every optimizer pass.
- Direct vs dynamic call equivalence.
- Print formatting in both paths.
- `inspect` golden tests.
- ABI test (Section 9.2).
- No regressions in existing 100+ tests.

---

## 9. Cross-cutting implementation plan

### 9.1 Phase order and gates

The order is chosen so each phase builds on a correct foundation and does not require redoing earlier work.

| Phase | Content | Exit gate |
|---|---|---|
| **0. Baseline (this change)** | Version `3.0.0`, proposal, docs; no behavior changes | `cargo test --workspace` green; proposal reviewed |
| **1. Dynamic values** | `js-value`, IR `Type::Value`, `runtime::ops`, interpreter and JIT migration, `null`/`undefined`, generic print | Existing tests + dynamic-value differential corpus green in all four configurations |
| **2. Heap and GC** | `Gc<T>`, `Heap`, `Trace`, shadow stack, safepoints, stress mode, remove leaks | GC stress and long-loop memory tests pass; no production `Box::into_raw`/`Box::leak` |
| **3. Object model** | Ordered `PropertyMap`, prototypes, array holes/length, dynamic `GetProp`/`SetProp`, Node-style inspection | Object/array differential corpus green |
| **4. UTF-16 strings** | `JsStringData` through lexer/AST/HIR/IR/runtime/backend | String corpus with astral/lone surrogates green |
| **5. Closures** | `BindingCell`, capture analysis, closure IR, dynamic entry wrapper, `call_indirect` | Closure corpus + GC stress green |
| **6. TypeScript** | `TypeAnn`, parser, checker, dialect mode, `tsc` fixtures | TS corpus accept/reject matches `tsc` for the supported subset |
| **7. Cleanup** | Docs, README/limitations, remove obsolete code, benchmarks | Full suite, docs build, no stale milestone claims |

**Why not TypeScript first?** A checker on top of a statically typed, incorrect runtime would validate programs the engine cannot execute correctly. The runtime foundation comes first, matching the review's strongest advice.

**Why GC before objects/strings?** Objects and strings are the main allocation sites. Establishing ownership first avoids rewriting allocation twice.

**Why null/undefined in Phase 1?** They are tags in the value representation; adding them later would touch every value match and every test again.

### 9.2 ABI validation spike (do this first in Phase 1)

Before migrating the pipeline, prove the calling convention:

1. Define `#[repr(C)] struct JsValue { tag: u64, payload: u64 }`.
2. Write a Rust `extern "C" fn add_values(a: JsValue, b: JsValue) -> JsValue`.
3. Build a tiny Cranelift JIT function with a signature taking two `I64` pairs and returning two `I64`s; call the Rust symbol.
4. Assert the result for numbers, booleans, null and undefined.
5. Repeat with the crate's actual `CompiledModule` path.
6. Document the result in this file or a follow-up ADR.

If the ABI does not match, switch runtime helpers to explicit out-parameters:

```rust
extern "C" fn add_values(a_tag: u64, a_payload: u64, b_tag: u64, b_payload: u64,
                         out_tag: *mut u64, out_payload: *mut u64);
```

This is mechanical and does not change the value design.

### 9.3 Dependency policy

- Add no dependency for values, GC, UTF-16 or the checker unless a concrete requirement cannot be met with `std`.
- Do not upgrade Cranelift in this redesign.
- If `indexmap` is added for ordered properties, justify it with the enumeration-order requirement; a small `Vec + HashMap` is preferred initially.
- No `unsafe` outside `runtime`/`backend-cranelift`/`js-value` without a documented safety comment.

### 9.4 Test strategy

**Unit tests:** every crate keeps its existing tests; add focused tests for the new modules (`js-value`, `runtime::ops`, `runtime::gc`, `runtime::object`, `typecheck`).

**IR tests:** verifier accept/reject; optimizer pass behavior; `Safepoint` barriers; direct/indirect calls.

**Differential tests:**

- JIT vs interpreter.
- Optimized vs unoptimized.
- Engine vs Node for JS.
- Engine checker vs `tsc --noEmit` for TS.
- GC stress vs normal for every runtime test.

**Memory tests:** Miri for pure Rust units, ASan/Valgrind for FFI/JIT where available, RSS bounds for loops, repeated `run()`.

**Fuzzing:** parser and lexer fuzz targets are valuable after the grammar stabilizes; not a gate for Phase 1.

**Benchmarks:** keep `benches/components_bench.rs`; add a value/GC microbenchmark only after correctness gates pass.

### 9.5 Non-goals for 3.0 foundation

- Full ECMAScript standard library, modules, async/await, generators, classes, `Proxy`, `Symbol`, `BigInt`, `Reflect`.
- Full TypeScript type system (conditional/mapped/template literal types, decorators, namespaces, declaration merging).
- JSX/TSX parsing.
- Multi-threaded or concurrent GC; moving/compacting GC.
- NaN boxing, pointer compression, inline caches, JIT tiering.
- `new`, `this`-dependent method semantics beyond passing `this` through the ABI.
- Exact `tsc` error-message parity.
- Source maps and debugger integration.

These are deferred deliberately, not forgotten.

### 9.6 Risks register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| ABI mismatch for two-word values | Medium | High | Spike in 9.2 before migration; fallback out-params |
| Missing GC roots in JIT code | Medium | High (UAF) | Shadow stack for every local; GC stress; ASan/Valgrind |
| Optimizer unsoundness after dynamic migration | High | High | Audit every pass; purity barriers; verify after each pass |
| Performance regression from helper calls | High | Medium | Accept for correctness; add guarded fast paths later |
| UTF-16 migration touching every string path | High | Medium | `JsStringData` in `js-value`; migrate stage by stage |
| TS scope creep | High | Medium | Publish v1 subset; reject unsupported constructs explicitly |
| Closure capture bugs (per-iteration/TDZ) | High | High | Targeted tests; cell-per-binding model |
| Property order/array holes wrong | Medium | Medium | Golden tests vs Node |
| GC pauses in loops | Medium | Low | Threshold tuning; later generational GC |
| Docs drift | High | Low | Phase 7 gate; single source of truth in this document |

### 9.7 Acceptance criteria for the 3.0 foundation

The foundation is complete when:

1. A variable can hold different primitive types on different paths and the engine executes correctly in JIT and interpreter.
2. `null` and `undefined` are distinct in values, equality, `typeof`, printing, missing properties and `??`.
3. Objects store mixed-type properties in ECMAScript order; arrays support mixed values, holes and `length`.
4. Strings use UTF-16 code units; astral characters and lone surrogates behave like Node.
5. All allocations are owned by a heap; cycles are collected; repeated runs do not grow memory.
6. Functions are values; closures capture by reference; per-iteration `let` capture is correct.
7. TypeScript annotations are parsed into AST and checked for the documented subset; unsupported constructs are rejected, not erased.
8. Cranelift and the interpreter both pass the differential matrix; the optimizer preserves behavior and `Safepoint`s.
9. The workspace version is `3.0.0`; README/limitations describe the actual state.

---

## 10. Appendix: target IR sketch

```text
Type = Value | Void

Function {
    name: String,
    params: Vec<Value>,          // all dynamic
    ret: Option<Value>,
    locals: Vec<Value>,
    blocks: Vec<Block>,
}

Inst =
    ConstNumber { dst, bits: u64 }
  | ConstBool { dst, value: bool }
  | ConstNull { dst }
  | ConstUndefined { dst }
  | ConstStr { dst, value: JsStringData }
  | GetLocal { dst, local }
  | SetLocal { local, src }
  | ToBool { dst, src }
  | Add { dst, lhs, rhs }        // calls rt_add
  | Sub | Mul | Div | Rem
  | Neg | Not
  | StrictEq { dst, lhs, rhs }
  | AbstractEq { dst, lhs, rhs }
  | Lt | Le | Gt | Ge
  | GetProp { dst, obj, key }
  | SetProp { obj, key, val }
  | ArrayLen { dst, arr }
  | MakeClosure { dst, func, captures: Vec<Value> }
  | GetCapture { dst, index }
  | SetCapture { index, src }
  | Call { dst, callee: FuncRef, args: Vec<Value> }
  | CallValue { dst, callee: Value, args: Vec<Value> }
  | Print { value }
  | Safepoint
  | ...

Terminator =
    Jump(Block)
  | Branch { cond: Value, then_block, else_block }   // cond must be Bool
  | Return(Option<Value>)
```

The exact instruction set will be refined during Phase 1; this sketch records the invariants:

- all runtime values are `JsValue`;
- conditions are explicit `ToBool` results;
- property access is dynamic;
- calls are direct or dynamic;
- safepoints are explicit barriers;
- constants are heap-free until the runtime materializes them.
