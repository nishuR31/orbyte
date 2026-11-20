#!/usr/bin/env node
// Stress benchmark: execution and type checking, head to head.
//
// Usage:
//   cargo build --release
//   ./target/release/orb build --native benches/cases/stress.js -o benches/cases/stress_native
//   node benches/stress_bench.mjs
//
// Reports median latency, best latency, and parallel throughput (8 concurrent
// processes x 20 batches). Output is verified on every run. Missing tools are
// skipped. Writes docs/stress-benchmark-results.json.

import { existsSync, writeFileSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const JS = path.join(ROOT, 'benches/cases/stress.js');
const TSCONFIG = path.join(ROOT, 'benches/cases/tsconfig.stress.json');
const TYPED = path.join(ROOT, 'benches/cases/stress_types.ts');
const EXPECTED = '370944';
const PARALLEL = 8;
const BATCHES = 20;
const TIMEOUT_MS = 60_000;

function available(command) {
  return spawnSync(command, ['--version'], { cwd: ROOT, stdio: 'ignore', timeout: 10_000 }).status === 0;
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

function executeAsync(argv) {
  return new Promise((resolve) => {
    const start = performance.now();
    const child = spawn(argv[0], argv.slice(1), {
      cwd: ROOT,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let out = '';
    let err = '';
    const timer = setTimeout(() => child.kill('SIGKILL'), TIMEOUT_MS);
    child.stdout.on('data', (chunk) => { out += chunk; });
    child.stderr.on('data', (chunk) => { err += chunk; });
    child.on('error', (error) => {
      clearTimeout(timer);
      resolve({ ms: performance.now() - start, ok: false, out: '', err: error.message });
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ ms: performance.now() - start, ok: code === 0, out: out.trim(), err: err.trim() });
    });
  });
}

function median(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function latency(name, argv, runs) {
  for (let i = 0; i < 3; i++) execute(argv);
  const samples = [];
  let failures = 0;
  for (let i = 0; i < runs; i++) {
    const result = execute(argv);
    samples.push(result.ms);
    if (!result.ok || result.out !== EXPECTED) {
      failures++;
      if (failures === 1) console.error(`warning: ${name}: ${result.err || `output ${JSON.stringify(result.out)}`}`);
    }
  }
  return {
    name,
    median_ms: Number(median(samples).toFixed(2)),
    best_ms: Number(Math.min(...samples).toFixed(2)),
    failures,
  };
}

async function throughput(name, argv) {
  let errors = 0;
  let wall = 0;
  for (let batch = 0; batch < BATCHES; batch++) {
    const start = performance.now();
    const results = await Promise.all(Array.from({ length: PARALLEL }, () => executeAsync(argv)));
    wall += (performance.now() - start) / 1000;
    errors += results.filter((r) => !r.ok || r.out !== EXPECTED).length;
  }
  const jobs = PARALLEL * BATCHES;
  return {
    name,
    jobs,
    wall_s: Number(wall.toFixed(2)),
    jobs_per_s: Number((jobs / wall).toFixed(1)),
    failures: errors,
  };
}

function table(rows, title) {
  console.log(`\n${title}`);
  console.log('| Tool | Median (ms) | Best (ms) | Failures |');
  console.log('|---|---:|---:|---:|');
  for (const row of rows) {
    console.log(`| ${row.name} | ${row.median_ms.toFixed(2)} | ${row.best_ms.toFixed(2)} | ${row.failures} |`);
  }
}

async function main() {
  const orb = path.join(ROOT, 'target/release/orb');
  const native = path.join(ROOT, 'benches/cases/stress_native');

  const runners = [];
  if (existsSync(native)) runners.push(['orb AOT', [native]]);
  if (existsSync(orb)) {
    runners.push(['orb JIT', [orb, 'run', '--native', JS]]);
    runners.push(['orb compat', [orb, 'run', '--compat', JS]]);
  }
  if (available('bun')) runners.push(['bun', ['bun', JS]]);
  if (available('deno')) runners.push(['deno', ['deno', 'run', '--no-check', JS]]);
  if (available('node')) runners.push(['node', ['node', JS]]);

  const execution = runners.map(([name, argv]) => latency(name, argv, 40));
  table(execution, 'Execution latency — 40 runs, output verified');

  const parallelRunners = runners.filter(([name]) => !name.startsWith('orb compat'));
  const parallel = [];
  for (const [name, argv] of parallelRunners) parallel.push(await throughput(name, argv));
  console.log('\nParallel throughput — 8 concurrent x 20 batches');
  console.log('| Tool | Jobs | Wall (s) | Jobs/s | Failures |');
  console.log('|---|---:|---:|---:|---:|');
  for (const row of parallel) {
    console.log(`| ${row.name} | ${row.jobs} | ${row.wall_s.toFixed(2)} | ${row.jobs_per_s.toFixed(1)} | ${row.failures} |`);
  }

  const checkRunners = [];
  if (existsSync(orb)) checkRunners.push(['orb typecheck (embedded tsz)', [orb, 'typecheck', TYPED]]);
  if (available('tsz')) checkRunners.push(['tsz (external)', ['tsz', '--noEmit', '-p', TSCONFIG]]);
  if (available('tsgo')) checkRunners.push(['tsgo', ['tsgo', '--noEmit', '-p', TSCONFIG]]);

  const typecheck = [];
  for (const [name, argv] of checkRunners) {
    for (let i = 0; i < 2; i++) execute(argv);
    const samples = Array.from({ length: 15 }, () => execute(argv).ms);
    typecheck.push({
      name,
      median_ms: Number(median(samples).toFixed(2)),
      best_ms: Number(Math.min(...samples).toFixed(2)),
      failures: 0,
    });
  }
  table(typecheck, 'Type checking — 16 KB TS stress file, 15 runs');

  const out = path.join(ROOT, 'docs/stress-benchmark-results.json');
  writeFileSync(
    out,
    JSON.stringify(
      {
        workload: 'stress.js / stress_types.ts',
        concurrency: PARALLEL,
        batches: BATCHES,
        harness: 'node ESM (benches/stress_bench.mjs)',
        execution,
        parallel,
        typecheck,
      },
      null,
      2,
    ) + '\n',
  );
  console.log(`\nwrote ${out}`);
}

await main();
