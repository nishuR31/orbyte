# Runtime benchmarks

Measured on a local Linux x86_64 development machine, release builds, median of 50
fresh-process runs (5 warmups discarded). The workload is `benches/cases/fib.js`:

```js
function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
console.log(fib(25));
```

Every runner must print `75025`. Timing includes process startup and, for the JIT and
interpreter paths, compilation/transpilation. These numbers describe
**startup-to-output**, not steady-state throughput.

## Execution results

| Runner | Median (ms) | Best (ms) | Relative to orb AOT |
|---|---:|---:|---:|
| orb AOT binary | 2.902 | 2.805 | 1.0&times; |
| orb JIT (`--native`) | 5.303 | 5.189 | 1.8&times; |
| bun (JS) | 8.755 | 8.507 | 3.0&times; |
| deno (`--no-check`, JS) | 21.873 | 21.437 | 7.5&times; |
| node (JS) | 42.410 | 38.117 | 14.6&times; |
| orb compat (`--compat`, Boa) | 79.406 | 78.649 | 27.4&times; |

Type checkers are a **different job** and are measured separately with an isolated
config (`benches/cases/tsconfig.json`, `types: []`, `skipLibCheck`):

| Checker | Median (ms) | Best (ms) |
|---|---:|---:|
| tsz (`tsz-org/tsz`, `--noEmit -p`) | 39.724 | — |
| tsgo (`@typescript/native-preview`, `--noEmit -p`) | 118.884 | — |

orb now embeds the same tsz checker in `orb typecheck`; these are the external CLI
binaries on the same small file and serve as a reference point, not a speedup claim.
On this file tsz is about 3.1&times; faster than tsgo, consistent with the tsz project's
own small-sample claim. See [`stress-benchmarks.md`](stress-benchmarks.md) for the
head-to-head with the embedded checker on a larger file.

## Reading the table

- orb AOT wins because it is a plain native executable: no engine startup, no parse,
  no JIT. That is the purpose of `orb build --native`.
- orb JIT still beats bun, deno and node on this small program because the native subset
  lowers straight to Cranelift without a full JS engine.
- orb compat uses Boa and is slower than node here. It buys language coverage, not
  speed, and is the honest fallback for programs outside the native subset.
- Variants matter: `deno run --no-check` skips type checking; `bun` and `node` run the
  plain `.js` file, not TypeScript.

## Reproduce

```bash
npm install -g @typescript/native-preview deno    # bun was already installed
curl -fsSL https://tsz.dev/install | sh            # tsz from tsz-org/tsz
cargo build --release
./target/release/orb build --native benches/cases/fib.js -o benches/cases/fib_native
node benches/runtime_bench.mjs --runs 50
```

The script verifies every runner's output, prints a Markdown table, and writes the raw
numbers to [`benchmark-results.json`](benchmark-results.json). Missing tools are skipped,
so the table reflects only what was actually measured. Run it on your own machine —
these numbers are illustrative, not a guarantee.
