#!/usr/bin/env node
// Wall-clock comparison of orb (debug/release) with node and bun.
// Interleaves runs so machine noise affects all tools equally.
//
// Usage:
//   node benches/compare.mjs [--runs 15] [--filter NAME]
//
// Set ORB_DEBUG / ORB_RELEASE to override binary paths.

import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EXE = process.platform === 'win32' ? '.exe' : '';

function fromEnvOr(name, fallback) {
  return process.env[name] || path.join(ROOT, fallback);
}

const ORB_DEBUG = fromEnvOr('ORB_DEBUG', `target/debug/orb${EXE}`);
const ORB_RELEASE = fromEnvOr('ORB_RELEASE', `target/release/orb${EXE}`);

const CASES = {
  startup: {
    orb: ['{orb}', '--version'],
    node: ['{node}', '--version'],
    bun: ['{bun}', '--version'],
  },
  'native-calc': {
    orb: ['{orb}', 'run', 'examples/javascript/calculate.js'],
    node: ['{node}', 'examples/javascript/calculate.js'],
    bun: ['{bun}', 'examples/javascript/calculate.js'],
  },
  'native-fib32': {
    orb: ['{orb}', 'run', 'examples/javascript/fib32.js'],
    node: ['{node}', 'examples/javascript/fib32.js'],
    bun: ['{bun}', 'examples/javascript/fib32.js'],
  },
  'native-primes100k': {
    orb: ['{orb}', 'run', 'examples/javascript/primes100k.js'],
    node: ['{node}', 'examples/javascript/primes100k.js'],
    bun: ['{bun}', 'examples/javascript/primes100k.js'],
  },
  'fallback-ts': {
    orb: ['{orb}', 'run', 'benches/cases/fib.ts'],
    bun: ['{bun}', 'benches/cases/fib.ts'],
  },
};

function find(name) {
  const result = spawnSync(name, ['--version'], { stdio: 'ignore', timeout: 10_000 });
  return result.status === 0 ? name : null;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function mean(values) {
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function main() {
  const runsIndex = process.argv.indexOf('--runs');
  const runs = runsIndex === -1 ? 15 : Number(process.argv[runsIndex + 1]);
  if (!Number.isInteger(runs) || runs < 1) {
    console.error('error: --runs requires a positive integer');
    process.exit(2);
  }
  const filterIndex = process.argv.indexOf('--filter');
  const filter = filterIndex === -1 ? '' : process.argv[filterIndex + 1] ?? '';

  const tools = [
    ['orb debug', 'orb', ORB_DEBUG],
    ['orb release', 'orb', ORB_RELEASE],
    ['node', 'node', find('node')],
    ['bun', 'bun', find('bun')],
  ].filter(([, , binary]) => binary && existsSync(binary));

  const cases = Object.fromEntries(Object.entries(CASES).filter(([name]) => name.includes(filter)));
  const samples = new Map();

  for (let i = 0; i < runs; i++) {
    for (const [label, key, binary] of tools) {
      for (const [caseName, perTool] of Object.entries(cases)) {
        const argv = perTool[key];
        if (!argv) continue;
        const command = argv.map((part) => (part.startsWith('{') ? binary : part));
        const start = performance.now();
        const result = spawnSync(command[0], command.slice(1), {
          cwd: ROOT,
          stdio: 'ignore',
          timeout: 60_000,
        });
        const ms = performance.now() - start;
        if (result.status === 0) {
          const id = `${label}\u0000${caseName}`;
          if (!samples.has(id)) samples.set(id, []);
          samples.get(id).push(ms);
        }
      }
    }
  }

  console.log(`# orb wall-clock comparison (${runs} runs, median ms)\n`);
  console.log(`| case | ${tools.map(([label]) => label).join(' | ')} |`);
  console.log(`|---|${tools.map(() => '---:').join('|')}|`);
  for (const caseName of Object.keys(cases)) {
    const cells = tools.map(([label]) => {
      const values = samples.get(`${label}\u0000${caseName}`) ?? [];
      return values.length ? median(values).toFixed(2) : 'n/a';
    });
    console.log(`| ${caseName} | ${cells.join(' | ')} |`);
  }
  console.log();
  for (const [label] of tools) {
    const values = [...samples.entries()]
      .filter(([id]) => id.startsWith(`${label}\u0000`))
      .flatMap(([, xs]) => xs);
    if (values.length) {
      console.log(`- ${label}: ${values.length} successful runs, overall mean ${mean(values).toFixed(2)} ms`);
    }
  }
}

main();
