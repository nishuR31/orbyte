# Engine Limitations Report
> Generated: 2026-11-13 | Binary: `/home/nishu/.local/bin/engine` (milestone 2.3)  
> Full test suite: **106 unit & integration tests, 0 failures** ✅  
> Working limitation probes: **7/18 pass** ✅ | **11/18 rejected cleanly ("reject, never miscompile")** 🛡️  

---

## ✅ What The Engine Supports Today

| Feature | Example | Status |
|---|---|---|
| Number arithmetic | `let x = 1 + 2 * 3;` | ✅ |
| Boolean logic & short-circuit | `a && b \|\| c` | ✅ |
| String literals & concatenation | `"hello" + " " + "world"` | ✅ |
| **Mixed string coercion (`+`)** | `"count: " + 42`, `true + "!"` | ✅ **NEW in M2.2** |
| **`typeof` operator** | `typeof x === "number"` | ✅ **NEW in M2.2** |
| **Template literals** | `` `Hello, ${name}!` `` | ✅ **NEW in M2.2** |
| **String `.length` property** | `s.length` | ✅ **NEW in M2.2** |
| **String methods** | `.toUpperCase()`, `.includes()`, `.slice()`, `.replace()`, `.split().length` | ✅ **NEW in M2.2** |
| Variables: `let`, `const`, `var` | `let x = 42;` (multi-decl: `let a = 1, b = 2;`) | ✅ |
| Block scoping & shadowing | `{ let x = 1; }` | ✅ |
| Temporal dead zone (TDZ) | Rejects `x; let x = 1;` | ✅ |
| `if / else` | `if (x > 0) { ... }` | ✅ |
| `while` loop | `while (n > 0) { n-- }` | ✅ |
| `for` loop w/ break/continue | `for (let i=0; i<n; i++) { ... }` | ✅ |
| Named functions + hoisting | `function f(a, b) { ... }` | ✅ |
| Recursion | Fibonacci, GCD | ✅ |
| `console.log` | Strings, numbers, booleans | ✅ |
| Comparison operators | `==`, `===`, `!=`, `!==`, `<`, `>` | ✅ |
| Compound assignment | `x += 1`, `x -= 2`, etc. | ✅ |
| Pre/post increment/decrement | `++i`, `i++`, `--i` | ✅ |
| NaN / Infinity semantics | IEEE 754 | ✅ |
| Hex / binary / octal literals | `0xFF`, `0b1010`, `0o17` | ✅ |
| Numeric separators | `1_000_000` | ✅ |
| Unicode identifiers | `let café = 1;` | ✅ |
| Automatic semicolon insertion (ASI) | Per spec | ✅ |
| CRLF and all line terminators | `\r\n`, `\u2028`, `\u2029` | ✅ |
| **TypeScript type annotations** | `: number`, `: string`, `?: T` | ✅ erased |
| **TypeScript `type` declarations** | `type Foo = ...;` | ✅ skipped |
| **TypeScript `interface` declarations** | `interface Foo { ... }` | ✅ skipped |
| **TypeScript typed object parameters** | `function distance(a: Point, b: Point)` | ✅ **NEW in M2.3** |
| **Array literals & indexing** | `[1, 2, 3]`, `arr[i]`, `arr.length`, `arr.push(v)` | ✅ **NEW in M2.3** |
| **Object literals & member access** | `{ name: "Alice", age: 30 }`, `obj.prop`, `obj.prop = v` | ✅ **NEW in M2.3** |
| **`Math.sqrt` & `Math.abs`** | Native JIT SIMD `Math.sqrt(x)`, `Math.abs(x)` | ✅ **NEW in M2.3** |
| **Destructuring assignment** | `let [a, b, c] = [1, 2, 3];`, `let { x, y } = point;` | ✅ **NEW in M2.3** |
| **Parameter destructuring** | `function coords({ x, y }) { ... }` | ✅ **NEW in M2.3** |
| **`export` / `import` headers** | `export const x = 1;` | ✅ |
| JIT compilation (Cranelift) | Native x86-64 machine code | ✅ |
| IR interpreter fallback | `--interp` flag | ✅ |
| Constant folding / DCE optimizer | `--no-opt` to disable | ✅ |
| Multi-thread runner | `--threads N` | ✅ |

---

## 🚫 Current Limitations (Rejects Safely — Never Miscompiles)

> [!IMPORTANT]
> The engine follows the **"reject, never miscompile"** principle. Every unsupported feature produces a precise compile-time error with file/line/column pointing to the exact problem. No silent wrong output.

### 1. Closures / First-Class Functions
```js
return function() { ... };   // error: expected an expression, found 'function'
let f = function() { ... };  // error: same
apply(fn, x);                // can't pass a named function as an argument value
```
**Root cause:** Functions aren't values in the current IR. No `FunctionRef` value type or capture frames.  
**Planned:** Milestone 3 (function pointer + closure capture).

---

### 2. Arrow Functions
```js
const add = (a, b) => a + b;  // error: expected ')', found ','
const sq = x => x * x;        // error: expected ';', found '=>'
```
**Root cause:** Arrow function syntax and `=>` token not in the parser.  
**Planned:** Alongside closures in Milestone 3.

---

### 3. `async` / `await` / Promises / Event Loop
```js
async function f() { ... }    // error: expected ';', found 'function'
await fetchData();             // await not a keyword
setTimeout(() => {}, 100);    // no event queue
```
**Root cause:** No asynchronous runtime — all execution is synchronous. `async`/`await` not tokenized.  
**Planned:** Milestone 4+ (async runtime + event loop).

---

### 4. `instanceof` / `in` Operators
```js
x instanceof Foo   // not a keyword
"k" in obj         // in not an expression operator
```
**Note:** `typeof` is now fully supported! `instanceof` and `in` require object/prototype hierarchy.  
**Planned:** Milestone 3 / 4.

---

### 5. `try` / `catch` / `throw` / Exceptions
```js
throw new Error("oops");     // error: expected ';', found 'new'
try { } catch (e) { }        // try/catch not in parser
```
**Root cause:** `try`, `catch`, `throw`, `new` not implemented.  
**Planned:** Milestone 3 (requires heap for Error objects and unwinding support).

---

### 6. Classes (`class` / `extends` / `this`)
```js
class Animal { }         // error: expected ';', found identifier 'Animal'
this.name = "Rex";       // this not a keyword
new Dog("Rex");          // new not implemented
```
**Root cause:** OOP syntax not in the parser or IR.  
**Planned:** Milestone 4+.

---

### 7. Nullish Coalescing (`??`) / Optional Chaining (`?.`)
```js
x ?? "default"    // rejects cleanly: types must match or fallback
obj?.prop         // requires tagged union runtime
```
**Root cause:** `??` and `?.` require `null`/`undefined` as runtime heap/tagged values.  
**Planned:** Milestone 3.

---

### 8. Generators / `yield` / `for...of` Iterables
```js
function* gen() { yield 1; }   // error: expected an identifier, found '*'
for (let v of iterable) { }    // for...of not in parser
```
**Root cause:** Generator state machines and iterator protocol not implemented.  
**Planned:** Milestone 4+.

---

### 9. Regular Expressions
```js
let re = /hello/i;           // error: unexpected character (regex literal not tokenized)
```
**Root cause:** Regex literal tokenization and engine runtime not in place.  
**Planned:** Milestone 3.

---

### 10. Mixed Non-String Arithmetic Coercion
```js
"5" - 2       // error: operand of '-' must be number, found string
true + 1      // error: operand of '+' must be number, found boolean
```
**Note:** String concatenation coercion (`"Count: " + 42`, `true + "!"`) is **now supported**. Non-string arithmetic subtraction/multiplication/addition coercion (`"5" - 2`) is intentionally rejected at compile-time to maintain strict type safety.  
**Planned:** Milestone 4+ (optional dynamic coercion mode).

---

### 11. Spread / Rest Operators (`...`)
```js
function sum(...args) { }     // error: expected an identifier, found '.'
[...arr1, ...arr2]            // depends on arrays
```
**Root cause:** `...` syntax not in the parser.  
**Planned:** Milestone 3.

---

## Summary Table

| # | Category | Error Produced | Milestone |
|---|---|---|---|
| 1 | Closures / function values | `expected an expression, found 'function'` | M3 |
| 2 | Arrow functions | `expected ')', found ','` | M3 |
| 3 | `async`/`await`/Promises | `expected ';', found 'function'` | M4+ |
| 4 | `instanceof`/`in` | `expected ')', found identifier` | M3 |
| 5 | `try`/`catch`/`throw` | `expected ';', found 'new'` | M3 |
| 6 | Classes / `this` / `new` | `expected ';', found identifier 'Animal'` | M4+ |
| 7 | `??` / `?.` | `cannot coalesce null with string` | M3 |
| 8 | Generators / `for...of` | `expected an identifier, found '*'` | M4+ |
| 9 | Regular expressions | `unexpected character` | M3 |
| 10 | Mixed arithmetic coercion | `operand must be number, found string` | M4+ |
| 11 | Spread / rest `...` | `expected an identifier, found '.'` | M3 |

---

## Milestone 2.3 Features Implemented ✅

1. **`typeof` operator** — compile-time resolved static string evaluation (`"number"`, `"string"`, `"boolean"`, `"undefined"`, `"object"`). Zero runtime overhead.
2. **Template literals** — lexer token desugaring with `${expr}` interpolation, backtick escapes, multiline support, and nested expressions.
3. **String methods & properties**:
   - `.length`
   - `.toUpperCase()`
   - `.includes(needle)`
   - `.slice(start, end)`
   - `.replace(from, to)`
   - `.split(sep).length`
4. **String concatenation coercion**:
   - `number + string` -> `NumToStr` + `ConcatStr`
   - `string + number` -> `NumToStr` + `ConcatStr`
   - `boolean + string` -> `BoolToStr` + `ConcatStr`
   - `string + boolean` -> `BoolToStr` + `ConcatStr`
5. **Multi-variable declarations** — `let a = 1, b = 2;` parsed and lowered cleanly across all declarations.
6. **Array literals & indexing** — `[1, 2, 3]`, `arr[i]`, `arr.length`, `arr.push(v)`, `arr[i] = v`.
7. **Object literals & property access** — `{ name: "Alice", age: 30 }`, `obj.prop`, `obj.prop = v`.
8. **Native Math SIMD instructions** — `Math.sqrt(x)`, `Math.abs(x)` lowered directly to native x86-64 SIMD instructions without runtime call overhead.
9. **TypeScript typed parameters & return inference** — `function distance(a: Point, b: Point): number` where parameters are dynamically typed to objects based on member access usage.
10. **Destructuring assignment & parameter destructuring** — `let [a, b, c] = [1, 2, 3];`, `let { x, y } = { x: 10, y: 20 };`, and `function coords({ x, y }) { ... }` desugared into direct element/property accesses.
