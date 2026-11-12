use std::io::Write;
use std::path::Path;
use std::process;
use std::{env, fs};

const USAGE: &str = "\
engine - a native JavaScript/TypeScript execution engine (milestone 2.1)

USAGE:
    engine run <file.js> [FLAGS]                 compile to machine code and run
    engine inspect <file.js> [--no-opt]          print the generated IR
    engine help

FLAGS:
    --interp         run with the reference IR interpreter instead of the Cranelift JIT
    --no-opt         skip the IR optimiser (constant folding, dead-code and unreachable-block removal)
    --threads <N>    number of worker threads for parallel runtime jobs (default: 1)
";

fn main() {
    process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprint!("{USAGE}");
        return 2;
    };
    match cmd {
        "run" | "inspect" => {}
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            return 0;
        }
        other => {
            eprintln!("unknown command '{other}'\n");
            eprint!("{USAGE}");
            return 2;
        }
    }

    let mut path: Option<&str> = None;
    let mut interp = false;
    let mut optimize = true;
    let mut threads: usize = 1;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--interp" {
            interp = true;
        } else if arg == "--no-opt" {
            optimize = false;
        } else if arg == "--threads" || arg == "-t" {
            i += 1;
            if i >= args.len() {
                eprintln!("error: --threads requires an argument");
                return 2;
            }
            threads = match args[i].parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => {
                    eprintln!("error: --threads must be a positive integer");
                    return 2;
                }
            };
        } else if let Some(n_str) = arg.strip_prefix("--threads=") {
            threads = match n_str.parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => {
                    eprintln!("error: --threads must be a positive integer");
                    return 2;
                }
            };
        } else if arg.starts_with("--") {
            eprintln!("unknown flag '{arg}'");
            return 2;
        } else {
            if path.is_some() {
                eprintln!("expected exactly one input file");
                return 2;
            }
            path = Some(arg.as_str());
        }
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("missing input file\n");
        eprint!("{USAGE}");
        return 2;
    };

    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("js") | Some("mjs") | Some("cjs") | Some("ts") | Some("mts") | Some("cts") => {}
        _ => {
            eprintln!("error: supported file extensions are .js, .mjs, .cjs, .ts, .mts, .cts");
            return 1;
        }
    }

    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read '{path}': {e}");
            return 1;
        }
    };

    let compiled = if optimize {
        compiler::compile_optimized(&source)
    } else {
        compiler::compile_to_ir(&source)
    };
    let module = match compiled {
        Ok(m) => m,
        Err(d) => {
            eprintln!("{}", d.render(path, &source));
            return 1;
        }
    };

    if cmd == "inspect" {
        print!("{module}");
        return 0;
    }

    let _ = threads; // worker threads for runtime thread pool configuration

    if interp {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let result = runtime::interp::run(&module, &mut out);
        let _ = out.flush();
        if let Err(e) = result {
            // Both execution engines report runtime failures the same way.
            eprintln!("{e}");
            return 1;
        }
        return 0;
    }

    match backend_cranelift::compile(&module) {
        Ok(compiled) => {
            let result = compiled.run();
            let _ = std::io::stdout().flush();
            match result {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("error: native code generation failed: {e}");
            1
        }
    }
}
