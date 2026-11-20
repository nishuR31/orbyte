# Vendored tsz type checker

This directory contains the Rust type-checker source from
[`tsz-org/tsz`](https://github.com/tsz-org/tsz), tag `v0.1.75`, vendored so that
`orb` builds entirely from this repository — no network access, no external
git dependency, no `tsz` binary or Node installation at build or run time.

## What is here

Only the crates required to build `tsz-core` are kept:

```
crates/tsz-common, tsz-scanner, tsz-parser, tsz-binder, tsz-solver,
tsz-lowering, tsz-checker, tsz-emitter, tsz-lsp, tsz-core
```

Tests, benches, docs, website assets and CLI sources were removed. The root
`Cargo.toml` is trimmed to that crate list. Nothing here is executed as a
separate process; `crates/typecheck` calls the Rust library directly.

## Integration

`crates/typecheck` in the main workspace exposes our own small API
(`typecheck::check_file`) and owns all CLI-facing formatting. The only thing
orb uses from here is `tsz_core::WasmProgram` plus the bundled ES5, ESNext and DOM
libs that `crates/typecheck` loads. Our workspace `Cargo.toml` has
`exclude = ["third_party/tsz"]` so this nested workspace stays independent.

## Limitations

- **Pinned version.** This is tsz v0.1.75, not a rolling dependency. Upstream fixes
  require re-vendoring and re-testing here.
- **Pre-release upstream.** tsz itself warns it is not yet a drop-in `tsc`:
  diagnostics, inference and emit can differ from TypeScript. Its conformance
  numbers belong to upstream; we removed the conformance suite from this copy and
  do not re-verify them.
- **High-level API only.** We call `WasmProgram::check_all()`, which serializes
  diagnostics to JSON before our wrapper parses them. That costs time and means
  no incremental or LSP-style checking — every `orb typecheck` is a cold, single
  invocation.
- **Single file.** `orb typecheck` checks one file with bundled standard libs. It
  does not read `tsconfig.json`, resolve imports or traverse `node_modules`.
- **Heavy build.** The checker is a large Rust dependency tree. On memory-limited
  machines use `CARGO_BUILD_JOBS=1`; the normal release profile is recommended for
  performance, but a low-memory profile (`opt-level=1`, `LTO=false`) was needed to
  build on the machine where this was integrated.
- **Pruned tree.** Tests, benches, docs, website and CLI sources were removed from
  the vendored copy; only the crates needed by `tsz-core` remain.

## Acknowledgement — third-party code

This is **third-party code**. The type checker inside `orb typecheck` is the work of
the [`tsz-org/tsz`](https://github.com/tsz-org/tsz) developers, not the orbyte project.
We took their Rust checker and wrapped it with a thin local API; all of the hard type
system work lives here and belongs to them.

**Thank you to Mohsen Azimi and every tsz contributor.** Building a fast TypeScript
checker in Rust is a large undertaking, and orbyte's type-checking feature exists only
because of it.

We warmly invite the tsz developers and the community to:
- test `orb typecheck <file.ts>` against real projects,
- report differences or bugs here or upstream in tsz,
- contribute fixes, tests and improvements to either project.

## License

tsz is Copyright (c) Mohsen Azimi and contributors, licensed under the
Apache License 2.0 — see [`LICENSE.txt`](LICENSE.txt). It is included unmodified
apart from the file pruning and workspace member list described above. Any
fixes or local adaptations should be recorded here.
