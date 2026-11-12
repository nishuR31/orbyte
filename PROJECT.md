# Native JavaScript/TypeScript Engine

> A Rust-powered JavaScript and TypeScript execution engine designed to compile source code into optimized native machine code and execute it directly on the host CPU.

**Project Status:** Initial Development  
**Implementation Language:** Rust  
**Initial Target:** Linux x86-64  
**Future Targets:** ARM64, Windows, macOS

---

# 1. Vision

Build an independent, high-performance JavaScript/TypeScript execution engine in Rust.

The objective is to develop an alternative execution platform to existing JavaScript engines such as V8 and JavaScriptCore.

Unlike conventional TypeScript compilers that transform TypeScript into JavaScript, this project aims to compile JavaScript and TypeScript into optimized native machine code.

The engine must eventually support:

- JavaScript
- TypeScript
- JSX
- TSX
- ECMAScript modules
- CommonJS compatibility
- Native machine-code generation
- JIT compilation
- Ahead-of-time compilation
- Runtime optimization
- Garbage collection
- Asynchronous execution

The long-term objective is to execute JavaScript and TypeScript applications without depending on an external JavaScript engine.

# 2. Core Objectives

1. Build a complete compiler frontend in Rust.
2. Implement JavaScript parsing and semantic analysis.
3. Implement TypeScript type analysis and inference.
4. Design a custom Intermediate Representation (IR).
5. Implement optimization passes.
6. Generate native machine code.
7. Build an independent JavaScript runtime.
8. Implement a JIT compilation pipeline.
9. Support dynamic JavaScript semantics.
10. Provide a production-quality CLI and developer experience.

# 3. Supported Languages

The frontend must progressively support:

| Extension | Language              |
| --------- | --------------------- |
| `.js`     | JavaScript            |
| `.mjs`    | JavaScript ES Modules |
| `.cjs`    | CommonJS JavaScript   |
| `.ts`     | TypeScript            |
| `.mts`    | TypeScript ES Modules |
| `.cts`    | CommonJS TypeScript   |
| `.jsx`    | JavaScript JSX        |
| `.tsx`    | TypeScript JSX        |

The engine must preserve JavaScript semantics regardless of whether source files are written in JavaScript or TypeScript.

TypeScript annotations may provide optimization information but must not silently change JavaScript runtime behavior.

# 4. System Architecture

The engine should follow a modular compiler and runtime architecture.

```text
                 SOURCE CODE
             JS / TS / JSX / TSX
                      |
                      v
                LEXER / PARSER
                      |
                      v
                 AST GENERATION
                      |
                      v
               SEMANTIC ANALYSIS
                      |
                      v
               TYPE ANALYSIS
                      |
                      v
              INTERMEDIATE REPRESENTATION
                      |
                      v
               OPTIMIZATION ENGINE
                      |
                      v
                EXECUTION ENGINE
                      |
              +-------+-------+
              |               |
              v               v
          BASELINE JIT    OPTIMIZING JIT
              |               |
              +-------+-------+
                      |
                      v
               NATIVE CODEGEN
                      |
                      v
               MACHINE CODE
                      |
                      v
                     CPU
```

The runtime operates alongside the execution engine and provides the necessary JavaScript execution semantics.

# 5. Major Components

## 5.1 Compiler Frontend

Responsibilities:

- Lexical analysis.
- Token generation.
- Parsing.
- AST construction.
- Syntax validation.
- Error reporting.
- Source location tracking.
- JSX and TSX parsing.

The frontend must preserve sufficient source information for diagnostics and source maps.

## 5.2 Semantic Analysis

Responsibilities:

- Scope resolution.
- Variable binding.
- Symbol tables.
- Declaration analysis.
- Function analysis.
- Module resolution.
- Closure analysis.
- Control-flow analysis.

## 5.3 Type System

Implement:

- Primitive types.
- Union types.
- Intersection types.
- Interfaces.
- Type aliases.
- Generics.
- Function signatures.
- Structural typing.
- Type inference.
- Control-flow-based narrowing.

TypeScript's type system must be treated as compile-time information, not as a replacement for JavaScript's runtime semantics.

## 5.4 Intermediate Representation

Develop a custom SSA-based Intermediate Representation.

Requirements:

- Explicit control-flow graphs.
- Basic blocks.
- SSA values.
- Phi nodes.
- Type information.
- Function representations.
- Memory operations.
- Runtime calls.
- Exception handling.
- Deoptimization metadata.

The IR must remain independent of individual machine architectures.

## 5.5 Optimization Engine

Implement optimization passes incrementally.

Initial optimizations:

- Constant folding.
- Constant propagation.
- Dead-code elimination.
- Common subexpression elimination.
- Function inlining.
- Strength reduction.
- Loop optimization.
- Escape analysis.
- Redundant type-check elimination.

Advanced optimizations:

- Profile-guided optimization.
- Speculative optimization.
- Type specialization.
- Inline caching.
- Hidden classes or equivalent object-shape tracking.
- Deoptimization.
- Register allocation.

Never apply optimizations that violate ECMAScript observable behavior.

## 5.6 Native Code Generator

Initial backend: Cranelift.

Responsibilities:

- Lower IR into backend IR.
- Generate machine code.
- Handle calling conventions.
- Manage register allocation through the backend.
- Generate executable code.
- Manage code memory safely.
- Support runtime function calls.

Future backends:

- LLVM.
- Custom native backend.
- x86-64.
- ARM64.

## 5.7 Runtime

The runtime must eventually provide:

### JavaScript Object Model

- Objects.
- Properties.
- Prototypes.
- Property descriptors.
- Getters and setters.
- Symbols.
- Arrays.
- Functions.
- Closures.

### Memory Management

- Heap allocation.
- Garbage collection.
- Object lifetime management.
- Write barriers.
- Stack management.
- Root tracking.

Start with a simple correct tracing garbage collector before implementing generational or concurrent collection.

### Execution Semantics

- Dynamic typing.
- Type conversions.
- Equality operations.
- Exceptions.
- Function invocation.
- Closures.
- `this` binding.
- Strict mode.
- Async/await.
- Promises.
- Iterators.
- Generators.

### Module System

- ECMAScript modules.
- Module resolution.
- Import/export.
- Module caching.
- Circular dependencies.

### Standard APIs

Progressively implement:

- Console.
- Timers.
- Text encoding.
- JSON.
- Collections.
- File system.
- Networking.
- Process management.

Browser APIs are outside the initial scope.

## 5.8 JIT Compiler

The JIT must eventually support multiple compilation tiers.

**Tier 1: Baseline compilation**

Quickly compile functions into executable machine code.

**Tier 2: Optimizing compilation**

Use runtime profiling to identify frequently executed functions and optimize them.

**Tier 3: Deoptimization**

When speculative assumptions become invalid, safely return execution to a less optimized representation.

The engine must support runtime profiling and compilation decisions.

# 6. Proposed Repository Structure

```text
engine/
│
├── Cargo.toml
├── Cargo.lock
├── README.md
├── PROJECT.md
├── LICENSE
│
├── crates/
│   │
│   ├── lexer/
│   ├── parser/
│   ├── ast/
│   ├── diagnostics/
│   │
│   ├── semantic/
│   ├── typechecker/
│   ├── module-resolver/
│   │
│   ├── ir/
│   ├── ir-builder/
│   ├── optimizer/
│   │
│   ├── codegen/
│   ├── backend-cranelift/
│   ├── backend-llvm/
│   │
│   ├── runtime/
│   ├── gc/
│   ├── memory/
│   ├── object-model/
│   │
│   ├── jit/
│   ├── profiler/
│   ├── deoptimizer/
│   │
│   └── compiler/
│
├── cli/
│   └── src/
│
├── tests/
│   ├── lexer/
│   ├── parser/
│   ├── semantics/
│   ├── typescript/
│   ├── runtime/
│   ├── jit/
│   ├── conformance/
│   └── integration/
│
├── benchmarks/
│   ├── startup/
│   ├── arithmetic/
│   ├── objects/
│   ├── functions/
│   ├── arrays/
│   ├── strings/
│   ├── memory/
│   └── real-world/
│
├── docs/
│   ├── architecture/
│   ├── compiler/
│   ├── runtime/
│   ├── ir/
│   ├── optimization/
│   └── decisions/
│
└── examples/
    ├── javascript/
    ├── typescript/
    ├── jsx/
    └── tsx/
```

This is the intended logical organization. Create crates incrementally rather than creating empty crates for every component immediately.

# 7. Development Roadmap

## Phase 0: Research and Architecture

- Study V8's execution pipeline.
- Understand JavaScriptCore's architecture.
- Study speculative optimization.
- Study inline caching.
- Study garbage collection.
- Understand Cranelift.
- Define JavaScript semantic requirements.
- Design the initial IR.

Deliverable: Architecture documentation and technical decisions.

## Phase 1: Compiler Frontend

- Implement lexer.
- Implement parser.
- Generate AST.
- Add syntax diagnostics.
- Support JavaScript fundamentals.
- Add TypeScript syntax.
- Add JSX and TSX syntax.
- Implement module syntax.

Deliverable: A functional source frontend.

## Phase 2: Semantic Analysis

- Implement lexical scopes.
- Implement symbol resolution.
- Implement declarations.
- Implement control-flow graphs.
- Implement TypeScript type analysis.
- Implement type inference.

Deliverable: Semantically analyzed programs.

## Phase 3: Intermediate Representation

- Define IR data structures.
- Implement AST-to-IR lowering.
- Implement basic blocks.
- Implement SSA construction.
- Implement control-flow representation.
- Add IR verification.

Deliverable: Executable-program representations independent of source syntax.

## Phase 4: First Native Compilation

- Integrate Cranelift.
- Lower IR into Cranelift IR.
- Generate native code.
- Implement executable memory management.
- Implement function invocation.
- Implement basic runtime helpers.

Deliverable: Compile and execute a restricted subset of JavaScript/TypeScript natively.

## Phase 5: Runtime Development

- Implement JavaScript values.
- Implement objects.
- Implement functions.
- Implement closures.
- Implement arrays.
- Implement prototype behavior.
- Implement exceptions.
- Implement garbage collection.

Deliverable: An increasingly complete JavaScript execution environment.

## Phase 6: JIT Compilation

- Implement baseline JIT.
- Add execution counters.
- Add runtime profiling.
- Add type feedback.
- Implement optimizing JIT.
- Implement speculative optimization.
- Implement deoptimization.

Deliverable: Multi-tier execution engine.

## Phase 7: Advanced Runtime

- Promises.
- Async/await.
- Event loop.
- Modules.
- CommonJS interoperability.
- Native APIs.
- Networking.
- File system.

Deliverable: A usable standalone runtime.

## Phase 8: Performance and Production Readiness

- Benchmark against V8.
- Benchmark against Bun.
- Benchmark against Deno.
- Measure cold-start performance.
- Measure steady-state throughput.
- Measure memory consumption.
- Optimize garbage collection.
- Improve compilation latency.
- Add debugging and profiling support.

Deliverable: A benchmarked, independently executable platform.

# 8. CLI Design

The intended CLI interface:

```bash
# Execute a JavaScript file
engine run app.js

# Execute TypeScript
engine run app.ts

# Execute JSX
engine run app.jsx

# Execute TSX
engine run app.tsx

# Compile to native executable
engine build app.ts --target native

# Compile to WebAssembly
engine build app.ts --target wasm

# Generate JavaScript
engine build app.ts --target js

# Show compilation information
engine inspect app.ts

# Run benchmarks
engine bench
```

These are target interfaces, not promises that all commands will be available in the initial release.

# 9. Performance Goals

Performance must be measured rather than assumed.

Track:

| Metric                   | Purpose                               |
| ------------------------ | ------------------------------------- |
| Cold-start latency       | Time required to start execution      |
| Compilation latency      | Frontend and backend compilation time |
| Runtime throughput       | Operations completed per second       |
| Memory consumption       | Heap and process memory               |
| GC pause time            | Garbage collection latency            |
| Code generation time     | Native code generation overhead       |
| Deoptimization frequency | Stability of speculative assumptions  |

Benchmark against established engines using equivalent workloads and identical test conditions.

Do not optimize only synthetic arithmetic benchmarks.

# 10. Engineering Principles

1. Correctness before performance.
2. ECMAScript semantics must be preserved.
3. TypeScript types are not runtime guarantees.
4. Keep frontend, IR, optimizer and backend independent.
5. Avoid premature architecture complexity.
6. Prefer measurable optimization.
7. Every optimization must have correctness tests.
8. Runtime behavior must be deterministic where ECMAScript requires it.
9. Memory safety must be maintained across native execution boundaries.
10. Support incremental development without sacrificing architectural consistency.

# 11. Non-Goals for Initial Development

Do not initially attempt:

- Browser implementation.
- DOM implementation.
- CSS rendering.
- Browser networking APIs.
- Complete Node.js API compatibility.
- Complete TypeScript specification.
- Multiple machine architectures simultaneously.
- Custom assembly generation before validating the IR.
- A custom garbage collector before establishing a working runtime.
- Production-grade concurrency before basic execution works.

# 12. Testing and Compatibility

Use the following as external correctness references:

- Test262: ECMAScript conformance suite.
- TypeScript compiler tests: TypeScript language behavior.
- JSX transformation tests.
- Runtime integration tests.
- Differential tests against established JavaScript engines.

Test262:
https://github.com/tc39/test262

TypeScript:
https://github.com/microsoft/TypeScript

ECMAScript specification:
https://tc39.es/ecma262/

Run supported tests continuously through CI.

# 13. Initial Implementation Milestone

The first milestone is intentionally small.

Build a working prototype that:

1. Reads a JavaScript source file.
2. Tokenizes the source.
3. Parses it into an AST.
4. Performs basic semantic analysis.
5. Lowers a restricted subset into IR.
6. Generates native machine code using Cranelift.
7. Executes the generated code.
8. Returns the correct result.

Initial supported language features:

- Numbers.
- Booleans.
- Variables.
- Arithmetic.
- Comparisons.
- Functions.
- Function calls.
- Conditional statements.
- Return statements.

Example:

```javascript
function calculate(a, b) {
  return a * b + 10;
}

console.log(calculate(20, 30));
```

Expected output:

```text
610
```

The initial prototype does not need complete JavaScript compatibility.

It must, however, have a clean architecture that allows additional language and runtime features without rewriting the compiler.

---

# 14. Definition of Success

The project succeeds when it can:

- Execute JavaScript and TypeScript independently.
- Generate native machine code.
- Preserve supported ECMAScript semantics.
- Execute dynamic JavaScript behavior.
- Perform runtime profiling and optimization.
- Support asynchronous execution.
- Manage memory independently.
- Run applications without requiring V8 or JavaScriptCore.
- Demonstrate measurable performance characteristics against established engines.

**Final vision: A standalone, Rust-powered native JavaScript and TypeScript execution platform.**

### Immediate next step

Do not start implementing the GC, JIT, parser and runtime simultaneously.

Start with **Phase 0 and Phase 1**, then get a tiny JavaScript program executing through Cranelift.

One architectural recommendation: keep the IR independent of Cranelift. Otherwise, your compiler will become tightly coupled to its first backend, making future LLVM or custom backend support unnecessarily difficult.
