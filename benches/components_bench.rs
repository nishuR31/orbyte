//! In-process benchmark for every component of the orb.
//!
//! Run with `cargo bench --offline --bench components_bench`. The synthetic
//! source is built once and reused, so process startup and file I/O do not
//! distort component timings. Every stage is verified against a known-good
//! output before it is timed, so a regression cannot be benchmarked silently.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use diagnostics::{Diagnostic, Span};

static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);

fn capture_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "orb-bench-{}-{}.out",
        std::process::id(),
        CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Runs `f` with stdout redirected to a temporary file and returns its output.
fn capture_stdout<F: FnOnce()>(f: F) -> String {
    let path = capture_path();
    let mut guard = stdout_capture::redirect_to(&path);
    f();
    guard.restore();
    let text = std::fs::read_to_string(&path).expect("read capture file");
    let _ = std::fs::remove_file(&path);
    text
}

fn null_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/dev/null")
    }
    #[cfg(windows)]
    {
        PathBuf::from("NUL")
    }
}

// The native runtime writes through the process stdout. Rust has no stable
// output-capture hook, so redirect the OS-level stdout handle. POSIX uses file
// descriptors; Windows uses the console std handle that `std::io::stdout`
// resolves on each write.
#[cfg(unix)]
mod stdout_capture {
    use super::*;
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
        fn close(fd: i32) -> i32;
    }

    pub struct Guard {
        saved: i32,
        _file: File,
        restored: bool,
    }

    pub fn redirect_to(path: &Path) -> Guard {
        Guard::redirect_to(path)
    }

    impl Guard {
        pub fn redirect_to(path: &Path) -> Self {
            io::stdout().flush().expect("flush stdout before redirect");
            let file = File::create(path).expect("create capture file");
            let saved = unsafe { dup(1) };
            assert!(saved >= 0, "dup(1) failed");
            let rc = unsafe { dup2(file.as_raw_fd(), 1) };
            assert!(rc >= 0, "dup2 failed");
            Self { saved, _file: file, restored: false }
        }

        pub fn restore(&mut self) {
            if self.restored {
                return;
            }
            io::stdout().flush().expect("flush stdout before restore");
            assert!(unsafe { dup2(self.saved, 1) } >= 0, "restore dup2 failed");
            assert!(unsafe { close(self.saved) } == 0, "close failed");
            self.restored = true;
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.restore();
        }
    }
}

#[cfg(windows)]
mod stdout_capture {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_OUTPUT_HANDLE};

    pub struct Guard {
        saved: HANDLE,
        handle: HANDLE,
        restored: bool,
    }

    pub fn redirect_to(path: &Path) -> Guard {
        Guard::redirect_to(path)
    }

    impl Guard {
        pub fn redirect_to(path: &Path) -> Self {
            io::stdout().flush().expect("flush stdout before redirect");
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    CREATE_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            assert!(handle != INVALID_HANDLE_VALUE && !handle.is_null(), "CreateFileW failed");
            let saved = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            assert!(unsafe { SetStdHandle(STD_OUTPUT_HANDLE, handle) } != 0, "SetStdHandle failed");
            Self { saved, handle, restored: false }
        }

        pub fn restore(&mut self) {
            if self.restored {
                return;
            }
            io::stdout().flush().expect("flush stdout before restore");
            assert!(unsafe { SetStdHandle(STD_OUTPUT_HANDLE, self.saved) } != 0, "restore SetStdHandle failed");
            assert!(unsafe { CloseHandle(self.handle) } != 0, "CloseHandle failed");
            self.restored = true;
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.restore();
        }
    }
}

/// True median: for an even sample count the two middle values are averaged.
/// The old benchmark used `samples[len / 2]`, which reported the upper median.
fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
    let n = samples.len();
    debug_assert!(n > 0);
    if n % 2 == 1 {
        samples[n / 2]
    } else {
        (samples[n / 2 - 1] + samples[n / 2]) / 2.0
    }
}

/// Times `f` for `iterations` runs and returns (median ms, last return value).
fn bench_ms<F, T>(iterations: usize, mut f: F) -> (f64, Option<T>)
where
    F: FnMut() -> T,
{
    let mut samples = Vec::with_capacity(iterations);
    let mut last = None;
    for _ in 0..iterations {
        let t0 = Instant::now();
        last = Some(f());
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    (median(&mut samples), last)
}

fn row(index: usize, name: &str, ms: f64, detail: &str) {
    println!("{:>2}. {:<42} {:>9.3} ms   {}", index, name, ms, detail);
}

fn main() {
    println!("=== ENGINE COMPONENT BENCHMARK SUITE (all aspects) ===");

    let iterations = 10;

    // ------------------------------------------------------------------
    // Synthetic inputs
    // ------------------------------------------------------------------
    let mut large_src = String::new();
    large_src.push_str("function compute(a, b) {\n");
    large_src.push_str("    let x = a * 2 + b;\n");
    large_src.push_str("    let y = 0xFF + 0b1010 + 0o77 + 1_000_000;\n");
    large_src.push_str("    return x + y;\n");
    large_src.push_str("}\n\n");
    for i in 0..1000 {
        large_src.push_str(&format!(
            "function fn_{i}(val) {{\n    let res = compute(val, {i});\n    if (res > 100) {{ return res; }} else {{ return {i}; }}\n}}\n\n"
        ));
    }
    large_src.push_str("console.log(fn_999(42));\n");

    let ts_src = "\
interface Point { x: number; y: number; }
type Label = string | number;
function distance(a: Point, b: Point): number {
  const dx: number = a.x - b.x;
  const dy: number = a.y - b.y;
  return Math.sqrt(dx * dx + dy * dy);
}
export const origin: Point = { x: 0, y: 0 };
export function label(p: Point): Label { return `${p.x},${p.y}`; }
";

    let js_exec_src = "\
let s = 0;
for (let i = 0; i < 10000; i++) { s += i; }
console.log(s);
";

    let ts_exec_src = "\
const xs: number[] = [1, 2, 3, 4, 5];
let total: number = 0;
for (const x of xs) { total += x; }
console.log(total);
";

    println!(
        "Native fixture: {} bytes, {} lines, 1002 functions",
        large_src.len(),
        large_src.lines().count()
    );
    println!("Verification: every stage is checked before it is timed.\n");

    // ------------------------------------------------------------------
    // Correctness verification (single pass, not timed)
    // ------------------------------------------------------------------
    let tokens = lexer::tokenize(&large_src).expect("lexing failed");
    let ast = parser::parse(&large_src).expect("parsing failed");
    let hir = semantic::analyze(&ast).expect("semantic analysis failed");
    let ir = ir_builder::lower(&hir);
    ir::verify(&ir).expect("unoptimised IR failed verification");

    let mut unopt_ir = ir.clone();
    optimizer::optimize(&mut unopt_ir).expect("optimizer failed");
    ir::verify(&unopt_ir).expect("optimised IR failed verification");

    let mut interp_out = Vec::new();
    runtime::interp::run(&unopt_ir, &mut interp_out).expect("interpreter execution failed");
    let interp_text = String::from_utf8(interp_out).expect("interpreter output is not UTF-8");
    assert_eq!(interp_text, "1001411\n", "interpreter output changed");

    let compiled = backend_cranelift::compile(&unopt_ir).expect("cranelift compilation failed");
    let native_text = capture_stdout(|| compiled.run().expect("native execution failed"));
    assert_eq!(native_text, interp_text, "native and interpreter output differ");

    let js_out = compat::run_collect(js_exec_src, "bench.js").expect("compat JS execution failed");
    assert_eq!(js_out, "49995000\n", "compat JS output changed");
    let ts_out = compat::run_collect(ts_exec_src, "bench.ts").expect("compat TS execution failed");
    assert_eq!(ts_out, "15\n", "compat TS output changed");

    let ts_code = compat::transpile_source(ts_src, "bench.ts").expect("TS transpile failed");
    assert!(ts_code.contains("function distance"), "TS erasure lost the function");
    assert!(!ts_code.contains(": number"), "TS erasure left type annotations");

    let diag = Diagnostic::new("benchmark diagnostic", Span::new(9, 20));
    assert!(diag.render("bench.js", &large_src).contains("bench.js:1:"));

    println!("  lexer tokens ............ {}", tokens.len());
    println!("  parser statements ....... {}", ast.body.len());
    println!("  semantic functions ...... {}", hir.functions.len());
    println!(
        "  IR basic blocks ......... {}",
        ir.functions.iter().map(|f| f.blocks.len()).sum::<usize>()
    );
    println!("  interpreter/native ...... agree ({interp_text:?})");
    println!("  compat JS / TS ......... agree ({js_out:?} / {ts_out:?})");
    println!("  diagnostics render ...... ok\n");

    // ------------------------------------------------------------------
    // Benchmarks
    // ------------------------------------------------------------------
    let (lex_ms, tokens_count) = bench_ms(iterations, || lexer::tokenize(&large_src).unwrap());
    let (parse_ms, ast_count) = bench_ms(iterations, || parser::parse(&large_src).unwrap());

    let (ast_clone_ms, _) = bench_ms(iterations, || ast.clone());

    let (sem_ms, funcs_count) = bench_ms(iterations, || semantic::analyze(&ast).unwrap());

    let (ir_ms, blocks_count) = bench_ms(iterations, || ir_builder::lower(&hir));

    let (verify_ms, _) = bench_ms(iterations, || ir::verify(&ir).expect("verify failed"));

    let (print_ms, _) = bench_ms(iterations, || ir.to_string());

    // Clone happens *before* the timer: cloning is not part of optimising.
    let mut opt_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let mut ir_clone = ir.clone();
        let t0 = Instant::now();
        optimizer::optimize(&mut ir_clone).expect("optimizer failed");
        opt_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let opt_ms = median(&mut opt_times);

    let (jit_ms, _) = bench_ms(iterations, || {
        backend_cranelift::compile(&unopt_ir).expect("cranelift compilation failed")
    });

    let (interp_ms, interp_last) = bench_ms(iterations, || {
        let mut out = Vec::new();
        runtime::interp::run(&unopt_ir, &mut out).expect("interpreter execution failed");
        out
    });
    debug_assert_eq!(interp_last.expect("interp output"), interp_text.as_bytes());

    // Native code writes through fd 1, so silence it for the timed loop.
    let null = null_path();
    let mut exec_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let guard = stdout_capture::redirect_to(&null);
        let t0 = Instant::now();
        compiled.run().expect("native execution failed");
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        drop(guard);
        exec_times.push(ms);
    }
    let exec_ms = median(&mut exec_times);

    let (frontend_ms, _) = bench_ms(iterations, || {
        compiler::compile_to_ir(&large_src).expect("frontend failed")
    });
    let (pipeline_ms, _) = bench_ms(iterations, || {
        compiler::compile_optimized(&large_src).expect("optimised pipeline failed")
    });

    let (diag_ms, _) = bench_ms(iterations, || {
        let mut total = 0;
        for i in 0..100 {
            let offset = (i * 37) % (large_src.len() - 1);
            let d = Diagnostic::new("benchmark diagnostic", Span::new(offset, offset + 1));
            total += d.render("bench.js", &large_src).len();
        }
        total
    });

    let (skip_ms, _) = bench_ms(iterations, || {
        compat::should_skip_native(&large_src, "bench.js")
    });

    let (js_passthrough_ms, _) = bench_ms(iterations, || {
        compat::transpile_source(&large_src, "bench.js").expect("passthrough failed")
    });

    // Warm up, then time cache hits on the identical TS source.
    compat::transpile_source(ts_src, "bench.ts").expect("TS transpile failed");
    let (ts_warm_ms, _) = bench_ms(iterations, || {
        compat::transpile_source(ts_src, "bench.ts").expect("TS transpile failed")
    });

    // A unique comment per iteration guarantees a cache miss: this is the
    // real SWC erasure cost, including the disk-cache write.
    let mut cold_sources = (0..iterations)
        .map(|i| format!("// iteration {i}\n{ts_src}"))
        .collect::<Vec<_>>()
        .into_iter();
    let (ts_cold_ms, _) = bench_ms(iterations, || {
        let src = cold_sources.next().expect("cold source");
        compat::transpile_source(&src, "bench.ts").expect("TS transpile failed")
    });

    let (compat_js_ms, _) = bench_ms(iterations, || {
        compat::run_collect(js_exec_src, "bench.js").expect("compat JS execution failed")
    });
    let (compat_ts_ms, _) = bench_ms(iterations, || {
        compat::run_collect(ts_exec_src, "bench.ts").expect("compat TS execution failed")
    });

    // ------------------------------------------------------------------
    // Report
    // ------------------------------------------------------------------
    println!("--- Native pipeline (median of {iterations}) ---");
    row(1, "Lexer", lex_ms, &format!("{} tokens, {:.2} MB/s", tokens_count.expect("tokens").len(), (large_src.len() as f64 / 1_000_000.0) / (lex_ms / 1000.0)));
    row(2, "Parser", parse_ms, &format!("{} AST statements", ast_count.expect("ast").body.len()));
    row(3, "AST clone", ast_clone_ms, "deep structural copy");
    row(4, "Semantic analysis / type check", sem_ms, &format!("{} functions", funcs_count.expect("hir").functions.len()));
    row(5, "IR lowering / builder", ir_ms, &format!("{} basic blocks", blocks_count.expect("ir").functions.iter().map(|f| f.blocks.len()).sum::<usize>()));
    row(6, "IR verifier", verify_ms, "whole module");
    row(7, "IR printer", print_ms, "module to string");
    row(8, "IR optimiser (clone excluded)", opt_ms, "fixed-point passes");
    row(9, "Cranelift machine codegen", jit_ms, "JIT native emit");
    row(10, "Reference IR interpreter", interp_ms, "output captured");
    row(11, "Native CPU execution", exec_ms, "output silenced");

    let total_compile = lex_ms + parse_ms + sem_ms + ir_ms + verify_ms + opt_ms + jit_ms;
    println!("    Full-pipeline JIT compile (stages 1,2,4,5,6,8,9): {total_compile:.2} ms");

    println!("\n--- Integrated compiler frontends ---");
    row(12, "compiler::compile_to_ir", frontend_ms, "source -> verified IR");
    row(13, "compiler::compile_optimized", pipeline_ms, "source -> optimised IR");

    println!("\n--- Cross-cutting components ---");
    row(14, "Diagnostics rendering", diag_ms, "100 diagnostics");

    println!("\n--- Compatibility runtime (fallback path) ---");
    row(15, "should_skip_native heuristic", skip_ms, "source scan");
    row(16, "Transpile: JS passthrough", js_passthrough_ms, "no SWC");
    row(17, "Transpile: TS cached (hit)", ts_warm_ms, "memory/disk cache");
    row(18, "Transpile: TS erasure (cold)", ts_cold_ms, "SWC + disk write");
    row(19, "Boa execution: JS", compat_js_ms, "output captured");
    row(20, "Boa execution: TS", compat_ts_ms, "output captured");

    println!("\nAll stages verified; no console output leaked into this report.");
}
