#!/usr/bin/env node
// Wall-clock comparison of orb (JIT, compat, AOT) with bun, deno and node.
//
// Every runner executes the same program, which prints 75025. Each measurement
// is a fresh process, so interpreter/JIT startup is included. tsgo and tsz are
// type checkers, not runners, so they are measured in a separate section.
//
// Usage:
//   cargo build --release
//   ./target/release/orb build --native benches/cases/fib.js -o benches/cases/fib_native
//   node benches/runtime_bench.mjs [--runs 50]
//
// Prints Markdown tables and writes docs/benchmark-results.json.

import { existsSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const JS = path.join(ROOT, 'benches/cases/fib.js');
const TSCONFIG = path.join(ROOT, 'benches/cases/tsconfig.json');
const EXPECTED = '75025';
const WARMUPS = 5;
const TIMEOUT_MS = 60_000;

function parseRuns() {
  const index = process.argv.indexOf('--runs');
  if (index === -1) return 50;
  const value = Number(process.argv[index + 1]);
  if (!Number.isInteger(value) || value < 1) {
    console.error('error: --runs requires a positive integer');
    process.exit(2);
  }
  return value;
}

function available(command) {
  return spawnSync(command, ['--version'], {
    cwd: ROOT,
    stdio: 'ignore',
    timeout: 10_000,
  }).status === 0;
}

function execute(argv) {
  const start = performance.now();
  const result = spawnSync(argv[0], argv.slice(1), {
    cwd: ROOT,
    encoding: 'utf8',
    timeout: TIMEOUT_MS,
    maxBuffer: 8 * 1024 * 1024,
  });
  return {
    ms: performance.now() - start,
    ok: result.status === 0 && !result.error,
    out: (result.stdout ?? '').trim(),
    err: result.error?.message ?? (result.stderr ?? '').trim(),
  };
}

function median(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function measure(name, argv, runs, expectOutput = true) {
  for (let i = 0; i < WARMUPS; i++) execute(argv);
  const samples = [];
  let failures = 0;
  for (let i = 0; i < runs; i++) {
    const result = execute(argv);
    samples.push(result.ms);
    if (!result.ok || (expectOutput && result.out !== EXPECTED)) {
      failures++;
      if (failures === 1) {
        console.error(`warning: ${name}: ${result.err || `output ${JSON.stringify(result.out)}`}`);
      }
    }
  }
  return {
    runner: name,
    median_ms: Number(median(samples).toFixed(3)),
    min_ms: Number(Math.min(...samples).toFixed(3)),
    failures,
  };
}

function markdown(rows) {
  const lines = ['| Runner | Median (ms) | Best (ms) | Failures |', '|---|---:|---:|---:|'];
  for (const row of rows) {
    lines.push(`| ${row.runner} | ${row.median_ms.toFixed(3)} | ${row.min_ms.toFixed(3)} | ${row.failures} |`);
  }
  return lines.join('\n');
}

function main() {
  const runs = parseRuns();
  const orb = path.join(ROOT, 'target/release/orb');
  const native = path.join(ROOT, 'benches/cases/fib_native');

  const runners = [];
  if (existsSync(native)) runners.push(['orb AOT binary', [native]]);
  if (existsSync(orb)) {
    runners.push(['orb JIT (--native)', [orb, 'run', '--native', JS]]);
    runners.push(['orb compat (--compat)', [orb, 'run', '--compat', JS]]);
  }
  if (available('bun')) runners.push(['bun', ['bun', JS]]);
  if (available('deno')) runners.push(['deno (--no-check)', ['deno', 'run', '--no-check', JS]]);
  if (available('node')) runners.push(['node', ['node', JS]]);

  const results = runners.map(([name, argv]) => measure(name, argv, runs));

  const checkers = [];
  for (const [name, program] of [['tsgo', 'tsgo'], ['tsz', 'tsz']]) {
    if (available(program)) {
      checkers.push(
        measure(`${name} --noEmit (type check)`, [program, '--noEmit', '-p', TSCONFIG], runs, false),
      );
    }
  }

  console.log(markdown(results));
  if (checkers.length) {
    console.log('\nSeparate job (not comparable with execution):');
    for (const row of checkers) console.log(`  ${row.runner}: ${row.median_ms.toFixed(3)} ms median`);
  }

  const out = path.join(ROOT, 'docs/benchmark-results.json');
  writeFileSync(
    out,
    JSON.stringify(
      {
        workload: 'fib(25) + console.log, fresh process per run',
        machine: 'local development machine, Linux x86_64',
        runs,
        harness: 'node ESM (benches/runtime_bench.mjs)',
        results,
        typechecks: checkers,
      },
      null,
      2,
    ) + '\n',
  );
  console.log(`\nwrote ${out}`);
}

main();
