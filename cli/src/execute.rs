//! Execution commands: `run`, `inspect`, `check`, `repl`, stdin, and eval.
//!
//! Routing is native-first. The compatibility runtime (Boa + SWC) is engaged
//! when the program uses syntax the native frontend deliberately rejects, when
//! the extension is TypeScript/JSX, when the file is an ES module, or when
//! `--compat` is requested. `--native` disables the fallback and therefore
//! reports the native diagnostic instead.

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use compat::{CompatError, InputType, RunOptions};

use crate::{envfile, package, util, USAGE};

/// Parsed command line for `run` and its aliases.
#[derive(Debug, Clone)]
struct RunConfig {
    file: Option<String>,
    program_args: Vec<String>,
    interp: bool,
    optimize: bool,
    threads: usize,
    native: bool,
    compat: bool,
    watch: bool,
    env_files: Vec<String>,
    auto_env: bool,
    input_type: InputType,
    cwd: Option<String>,
    preloads: Vec<String>,
    eval: Option<String>,
    print: bool,
    check: bool,
    repl: bool,
    help: bool,
    version: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            file: None,
            program_args: Vec::new(),
            interp: false,
            optimize: true,
            threads: 1,
            native: false,
            compat: false,
            watch: false,
            env_files: Vec::new(),
            auto_env: true,
            input_type: InputType::Auto,
            cwd: None,
            preloads: Vec::new(),
            eval: None,
            print: false,
            check: false,
            repl: false,
            help: false,
            version: false,
        }
    }
}

fn parse_config(args: &[String]) -> Result<RunConfig, String> {
    let mut config = RunConfig::default();
    let mut index = 0;
    let mut passthrough = false;

    while index < args.len() {
        let arg = &args[index];

        // Everything after `--`, and everything after `-e` / `-p`, belongs to
        // the program. This mirrors Node and keeps `orb run -e ... --flag`
        // from consuming the flag.
        if passthrough || config.eval.is_some() {
            config.program_args.push(arg.clone());
            index += 1;
            continue;
        }

        match arg.as_str() {
            "--" => passthrough = true,
            // `-` is the conventional stdin entry, not an unknown flag.
            "-" => {
                if config.file.is_some() {
                    config.program_args.push(arg.clone());
                } else {
                    config.file = Some(arg.clone());
                }
            }
            "--help" | "-h" => config.help = true,
            "--interp" => config.interp = true,
            "--no-opt" => config.optimize = false,
            "--native" => config.native = true,
            "--compat" => config.compat = true,
            "--watch" | "--hot" => config.watch = true,
            "--no-env-file" => config.auto_env = false,
            "--check" | "-c" => config.check = true,
            "--print" | "-p" => {
                if config.file.is_some() {
                    config.program_args.push(arg.clone());
                } else {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "--print requires an argument".to_owned())?;
                    config.eval = Some(value.clone());
                    config.print = true;
                }
            }
            "-i" | "--interactive" => config.repl = true,
            "--version" => config.version = true,
            "--threads" | "-t" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--threads requires an argument".to_owned())?;
                config.threads = parse_threads(value)?;
            }
            "--env-file" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--env-file requires an argument".to_owned())?;
                config.env_files.push(value.clone());
            }
            "--input-type" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--input-type requires an argument".to_owned())?;
                config.input_type = InputType::parse(value).ok_or_else(|| {
                    format!("invalid --input-type '{value}' (expected auto, module, or commonjs)")
                })?;
            }
            "--cwd" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cwd requires an argument".to_owned())?;
                config.cwd = Some(value.clone());
            }
            "--preload" | "--require" | "-r" | "--import" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{arg} requires an argument"))?;
                config.preloads.push(value.clone());
            }
            "-e" | "--eval" => {
                if config.file.is_some() {
                    config.program_args.push(arg.clone());
                } else {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "--eval requires an argument".to_owned())?;
                    config.eval = Some(value.clone());
                }
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--threads=") {
                    config.threads = parse_threads(value)?;
                } else if let Some(value) = arg.strip_prefix("--env-file=") {
                    config.env_files.push(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--input-type=") {
                    config.input_type = InputType::parse(value).ok_or_else(|| {
                        format!(
                            "invalid --input-type '{value}' (expected auto, module, or commonjs)"
                        )
                    })?;
                } else if let Some(value) = arg.strip_prefix("--cwd=") {
                    config.cwd = Some(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--preload=") {
                    config.preloads.push(value.to_owned());
                } else if arg.starts_with('-') {
                    if is_ignored_flag(arg) {
                        // Accepted for source compatibility with Node/Bun.
                    } else if config.file.is_some() {
                        // After the entry file unknown flags belong to the program.
                        config.program_args.push(arg.clone());
                    } else {
                        return Err(format!("unknown flag '{arg}'"));
                    }
                } else if config.file.is_none() {
                    config.file = Some(arg.clone());
                } else {
                    config.program_args.push(arg.clone());
                }
            }
        }
        index += 1;
    }

    Ok(config)
}

fn parse_threads(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(threads) if threads > 0 => Ok(threads),
        _ => Err("--threads must be a positive integer".to_owned()),
    }
}

/// Flags this orb accepts but does not act on. They are common in Node/Bun
/// invocations and rejecting them would make the command surface needlessly
/// brittle.
fn is_ignored_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--smol"
            | "--bun"
            | "--no-install"
            | "--silent"
            | "--no-warnings"
            | "--no-deprecation"
            | "--trace-warnings"
            | "--enable-source-maps"
            | "--inspect"
            | "--inspect-brk"
            | "--preserve-symlinks"
            | "--no-addons"
            | "--experimental-vm-modules"
    ) || flag.starts_with("--inspect=")
        || flag.starts_with("--conditions=")
        || flag.starts_with("--define=")
        || flag.starts_with("--loader=")
}

/// Applies `--cwd` and `.env` loading once per process invocation.
fn prepare(config: &RunConfig) -> Result<(), i32> {
    if let Some(directory) = &config.cwd {
        util::change_dir(directory).map_err(|error| {
            eprintln!("error: {error}");
            1
        })?;
    }
    envfile::load(&config.env_files, config.auto_env).map_err(|error| {
        eprintln!("error: {error}");
        1
    })?;
    Ok(())
}

/// Entry point for `orb run ...` and for bare `orb <file>`.
pub(crate) fn command_run(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}\n");
            eprint!("{USAGE}");
            return 2;
        }
    };

    if config.help {
        print!("{USAGE}");
        return 0;
    }
    if config.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if let Err(code) = prepare(&config) {
        return code;
    }

    if config.repl {
        return command_repl(&[]);
    }

    if config.check {
        return check(&config);
    }

    if let Some(source) = &config.eval {
        return execute_source(source, "<eval>", &config, None);
    }

    let Some(file) = &config.file else {
        return stdin_or_repl(&config);
    };
    if file == "-" {
        return stdin_or_repl(&config);
    }

    let path = util::absolute(file);
    if !path.exists() {
        if let Some(code) = run_package_script(file, &config) {
            return code;
        }
        eprintln!("error: cannot find file or package script '{file}'");
        return 1;
    }

    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        if !util::is_supported_extension(&path) {
            eprintln!(
                "error: unsupported file extension '.{extension}' (expected .js .mjs .cjs .jsx .ts .mts .cts .tsx)"
            );
            return 1;
        }
    }

    run_file(&path, &config)
}

/// Entry point for a bare `orb` with no command or file.
pub(crate) fn run_stdin_or_repl(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}\n");
            eprint!("{USAGE}");
            return 2;
        }
    };
    if let Err(code) = prepare(&config) {
        return code;
    }
    stdin_or_repl(&config)
}

fn stdin_or_repl(config: &RunConfig) -> i32 {
    if io::stdin().is_terminal() {
        command_repl(&[])
    } else {
        let mut source = String::new();
        if let Err(error) = io::stdin().read_to_string(&mut source) {
            eprintln!("error: cannot read stdin: {error}");
            return 1;
        }
        execute_source(&source, "<stdin>", config, None)
    }
}

fn run_file(path: &Path, config: &RunConfig) -> i32 {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{}': {error}", path.display());
            return 1;
        }
    };

    let mut watcher = config
        .watch
        .then(|| util::Watcher::new(path.parent().unwrap_or(Path::new(".")).to_path_buf()));

    loop {
        let code = execute_source(&source, &path.to_string_lossy(), config, Some(path));
        let Some(watcher) = watcher.as_mut() else {
            return code;
        };
        watcher.wait();
        eprintln!("orb: change detected, re-running {}", path.display());
    }
}

/// Runs a source string through native-first routing.
fn execute_source(source: &str, filename: &str, config: &RunConfig, entry: Option<&Path>) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let options = options_for(filename, config, entry, &cwd);

    // `--print` needs the completion value, which only the compatibility
    // evaluator exposes.
    if config.print {
        return match compat::run_value(source, &options) {
            Ok(Some(value)) => {
                println!("{value}");
                0
            }
            Ok(None) => 0,
            Err(error) => report_compat_error(error),
        };
    }

    // `--native` must still attempt the native pipeline even when the heuristic
    // predicts rejection, so the user sees the real located diagnostic.
    let attempt_native =
        !config.compat && (config.native || !compat::should_skip_native(source, filename));

    if attempt_native {
        match compile_native(source, config.optimize) {
            Ok(module) => return run_native(&module, config.interp),
            Err(diagnostic) => {
                if config.native {
                    eprintln!("{}", diagnostic.render(filename, source));
                    return 1;
                }
                // Fall through to the compatibility runtime. The native
                // diagnostic is intentionally suppressed: the fallback is the
                // supported path for this program.
            }
        }
    }

    let _ = config.threads; // worker-thread count for the native runtime

    match compat::run_with_options(source, &options) {
        Ok(()) => 0,
        Err(error) => report_compat_error(error),
    }
}

fn options_for(
    filename: &str,
    config: &RunConfig,
    entry: Option<&Path>,
    cwd: &Path,
) -> RunOptions {
    let mut argv = vec!["orb".to_owned()];
    if let Some(entry) = entry {
        let absolute = if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            cwd.join(entry)
        };
        argv.push(absolute.to_string_lossy().into_owned());
    }
    argv.extend(config.program_args.iter().cloned());

    RunOptions {
        filename: filename.to_owned(),
        cwd: Some(cwd.to_path_buf()),
        argv,
        input_type: config.input_type,
        test_mode: false,
        preloads: config.preloads.clone(),
    }
}

fn compile_native(source: &str, optimize: bool) -> Result<ir::Module, diagnostics::Diagnostic> {
    if optimize {
        compiler::compile_optimized(source)
    } else {
        compiler::compile_to_ir(source)
    }
}

fn run_native(module: &ir::Module, interp: bool) -> i32 {
    if interp {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let result = runtime::interp::run(module, &mut out);
        let _ = out.flush();
        return match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }

    match backend_cranelift::compile(module) {
        Ok(compiled) => match compiled.run() {
            Ok(()) => {
                let _ = io::stdout().flush();
                0
            }
            Err(error) => {
                eprintln!("{error}");
                1
            }
        },
        Err(error) => {
            eprintln!("error: native code generation failed: {error}");
            1
        }
    }
}

fn report_compat_error(error: CompatError) -> i32 {
    match error {
        CompatError::Exit(code) => code,
        other => {
            eprintln!("{other}");
            1
        }
    }
}

/// `orb inspect`: print the native IR, never fall back.
pub(crate) fn command_inspect(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if config.help {
        print!("{USAGE}");
        return 0;
    }

    let Some(file) = config.file.as_deref() else {
        eprintln!("error: inspect requires an input file");
        return 2;
    };
    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{file}': {error}");
            return 1;
        }
    };

    match compile_native(&source, config.optimize) {
        Ok(module) => {
            print!("{module}");
            0
        }
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(file, &source));
            1
        }
    }
}

/// `orb check` / `-c`: parse without executing.
pub(crate) fn command_check(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if let Err(code) = prepare(&config) {
        return code;
    }
    check(&config)
}

fn check(config: &RunConfig) -> i32 {
    if let Some(source) = &config.eval {
        return match compat::check(source, "<eval>") {
            Ok(()) => 0,
            Err(error) => report_compat_error(error),
        };
    }

    let Some(file) = config.file.as_deref() else {
        let mut source = String::new();
        if let Err(error) = io::stdin().read_to_string(&mut source) {
            eprintln!("error: cannot read stdin: {error}");
            return 1;
        }
        return match compat::check(&source, "<stdin>") {
            Ok(()) => 0,
            Err(error) => report_compat_error(error),
        };
    };

    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{file}': {error}");
            return 1;
        }
    };
    match compat::check(&source, file) {
        Ok(()) => 0,
        Err(error) => report_compat_error(error),
    }
}

/// `orb repl` and bare `orb` on a terminal.
pub(crate) fn command_repl(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if let Err(code) = prepare(&config) {
        return code;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut repl = match compat::Repl::new(cwd) {
        Ok(repl) => repl,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };

    println!(
        "orb {} REPL (JavaScript). Type .help for help, .exit or Ctrl-D to quit.",
        env!("CARGO_PKG_VERSION")
    );
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("error: {error}");
                return 1;
            }
        }
        let source = line.trim_end();
        if source.trim().is_empty() {
            continue;
        }
        match source {
            ".exit" | ".quit" => break,
            ".help" => {
                println!("Enter JavaScript expressions or statements.");
                println!(".help  show this message");
                println!(".exit  leave the REPL");
                continue;
            }
            _ => {}
        }
        match repl.evaluate(source) {
            Ok(Some(value)) => println!("{value}"),
            Ok(None) => {}
            Err(CompatError::Exit(code)) => return code,
            Err(error) => eprintln!("{error}"),
        }
    }
    0
}

/// Runs a `package.json` script when the positional argument is not a file.
fn run_package_script(name: &str, config: &RunConfig) -> Option<i32> {
    let command = package::find_script(name)?;
    Some(package::run_script(name, &command, &config.program_args))
}

/// `orb build --native <file> [-o out]`: emit a standalone native binary.
///
/// Only the statically typed native subset can be compiled this way (the same
/// subset served by the JIT); the compatibility runtime's JS/TS programs keep
/// using `orb run` / Bun bundling.
pub(crate) fn command_build_native(args: &[String]) -> i32 {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if let Some(value) = arg
            .strip_prefix("--outfile=")
            .or_else(|| arg.strip_prefix("--output="))
        {
            output = Some(value.to_owned());
        } else if arg == "-o" || arg == "--outfile" || arg == "--output" {
            index += 1;
            match args.get(index) {
                Some(value) => output = Some(value.clone()),
                None => {
                    eprintln!("error: {arg} requires a path");
                    return 2;
                }
            }
        } else if arg.starts_with('-') {
            eprintln!("error: unknown flag for native build: {arg}");
            return 2;
        } else if input.is_none() {
            input = Some(arg.to_owned());
        } else {
            eprintln!("error: native build takes exactly one input file");
            return 2;
        }
        index += 1;
    }

    let Some(input) = input else {
        eprintln!("error: build --native requires an input file");
        return 2;
    };
    let path = Path::new(&input);
    if !path.is_file() {
        eprintln!("error: cannot find file '{input}'");
        return 1;
    }
    if !util::is_supported_extension(path) {
        eprintln!("error: unsupported file extension for native build");
        return 1;
    }
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{input}': {error}");
            return 1;
        }
    };
    let filename = path.to_string_lossy();
    let module = match compiler::compile_optimized(&source) {
        Ok(module) => module,
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(&filename, &source));
            eprintln!("note: AOT output supports the native subset only; run `orb inspect {input}` for details");
            return 1;
        }
    };
    let object = match backend_cranelift::compile_object(&module) {
        Ok(object) => object,
        Err(error) => {
            eprintln!("error: native code generation failed: {error}");
            return 1;
        }
    };

    let out = output.unwrap_or_else(|| {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("program");
        format!("{stem}.native{}", std::env::consts::EXE_SUFFIX)
    });
    let out_path = Path::new(&out);
    if let Err(error) = backend_cranelift::link_executable(&object, out_path, None) {
        eprintln!("error: linking failed: {error}");
        return 1;
    }
    println!("{}", out_path.display());
    0
}
