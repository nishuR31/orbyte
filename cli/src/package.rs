//! Package-management and project commands.
//!
//! Installing dependencies, bundling, and running package binaries is delegated
//! to Bun when it is available, with npm/npx fallbacks for the commands npm
//! owns. `orb install bun` is implemented here and does not need an existing
//! JavaScript runtime.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util;

const INSTALL_USAGE: &str = "\
orb install bun - install the Bun JavaScript runtime

USAGE:
    orb install bun [FLAGS]

FLAGS:
    --force             reinstall even when Bun is already on PATH
    --version <v>       install a specific version (for example 1.2.3)
    --npm               install with `npm install -g bun` instead of the script
    --dry-run           print the installer command without running it
";

/// Runs a package-manager subcommand via Bun (preferred) or npm.
pub(crate) fn delegate(subcommand: &str, args: &[String]) -> i32 {
    if env::var_os("ORB_NO_DELEGATE").is_some() {
        eprintln!(
            "orb: '{subcommand}' is delegated to an external package manager; \
             delegation is disabled by ORB_NO_DELEGATE"
        );
        return 2;
    }

    let npm_only = matches!(subcommand, "dedupe" | "prune" | "exec");
    if !npm_only {
        if let Some(bun) = util::find_bun() {
            return run_status(Command::new(bun).arg(subcommand).args(args));
        }
    }

    let npm = match util::which("npm") {
        Some(npm) => npm,
        None => {
            eprintln!("orb: {}\n{}", util::bun_hint(), "npm was not found either.");
            return 127;
        }
    };

    let mapped = match subcommand {
        "add" => "install",
        "remove" => "uninstall",
        "x" => "exec",
        other => other,
    };
    let mut command = Command::new(npm);
    if mapped == "exec" {
        if let Some(npx) = util::which("npx") {
            return run_status(Command::new(npx).args(args));
        }
    }
    command.arg(mapped).args(args);
    run_status(&mut command)
}

fn run_status(command: &mut Command) -> i32 {
    match command.status() {
        Ok(status) => util::status_code(status),
        Err(error) => {
            eprintln!("error: cannot start '{}': {error}", command_debug(command));
            127
        }
    }
}

fn command_debug(command: &Command) -> String {
    command.get_program().to_string_lossy().into_owned()
}

/// `orb x` / `orb bunx`.
pub(crate) fn command_x(args: &[String]) -> i32 {
    delegate("x", args)
}

/// `orb exec`.
pub(crate) fn command_exec(args: &[String]) -> i32 {
    delegate("exec", args)
}

/// `orb build`: transpile TS/TSX/JSX to JS locally, or delegate to Bun's
/// bundler for the other targets.
pub(crate) fn command_build(args: &[String]) -> i32 {
    // Native AOT is implemented here; every other build target is delegated.
    if args.iter().any(|arg| arg == "--native") {
        let rest: Vec<String> = args
            .iter()
            .filter(|arg| arg.as_str() != "--native")
            .cloned()
            .collect();
        return crate::execute::command_build_native(&rest);
    }

    let mut target: Option<String> = None;
    let mut output: Option<String> = None;
    let mut input: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if let Some(value) = arg.strip_prefix("--target=") {
            target = Some(value.to_owned());
        } else if arg == "--target" {
            index += 1;
            match args.get(index) {
                Some(value) => target = Some(value.clone()),
                None => {
                    eprintln!("error: --target requires js (expected .js .mjs .cjs .jsx .ts .mts .cts .tsx)");
                    return 2;
                }
            }
        } else if let Some(value) = arg.strip_prefix("--outfile=") {
            output = Some(value.to_owned());
        } else if arg == "-o" || arg == "--outfile" {
            index += 1;
            match args.get(index) {
                Some(value) => output = Some(value.clone()),
                None => {
                    eprintln!("error: {arg} requires a path");
                    return 2;
                }
            }
        } else if arg.starts_with('-') {
            // Unknown flags go to Bun's bundler below.
            return delegate("build", args);
        } else if input.is_none() {
            input = Some(arg.to_owned());
        } else {
            return delegate("build", args);
        }
        index += 1;
    }

    if target.as_deref() == Some("js") {
        let Some(input) = input else {
            eprintln!("error: build --target js requires an input file");
            return 2;
        };
        return build_js(&input, output.as_deref());
    }

    delegate("build", args)
}

fn build_js(input: &str, output: Option<&str>) -> i32 {
    let path = std::path::Path::new(input);
    if !path.is_file() {
        eprintln!("error: cannot find file '{input}'");
        return 1;
    }
    if !crate::util::is_supported_extension(path) {
        eprintln!("error: unsupported file extension (expected .js .mjs .cjs .jsx .ts .mts .cts .tsx)");
        return 1;
    }
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{input}': {error}");
            return 1;
        }
    };
    let code = match compat::transpile_source(&source, input) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let output = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| path.with_extension("js"));
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
            eprintln!("error: cannot create output directory '{}'", parent.display());
            return 1;
        }
    }
    if let Err(error) = std::fs::write(&output, code) {
        eprintln!("error: cannot write '{}': {error}", output.display());
        return 1;
    }
    println!("built {}", output.display());
    0
}

/// `orb install bun`.
pub(crate) fn command_install_bun(args: &[String]) -> i32 {
    let mut force = false;
    let mut dry_run = false;
    let mut use_npm = false;
    let mut version: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{INSTALL_USAGE}");
                return 0;
            }
            "--force" | "-f" => force = true,
            "--dry-run" => dry_run = true,
            "--npm" => use_npm = true,
            "--version" => {
                index += 1;
                match args.get(index) {
                    Some(value) if valid_version(value) => version = Some(value.clone()),
                    _ => {
                        eprintln!("error: --version requires a version such as 1.2.3");
                        return 2;
                    }
                }
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--version=") {
                    if !valid_version(value) {
                        eprintln!("error: invalid Bun version '{value}'");
                        return 2;
                    }
                    version = Some(value.to_owned());
                } else {
                    eprintln!("error: unknown flag '{arg}'\n");
                    eprint!("{INSTALL_USAGE}");
                    return 2;
                }
            }
        }
        index += 1;
    }

    if !force {
        if let Some(bun) = util::find_bun() {
            let reported = Command::new(&bun)
                .arg("--version")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|version| version.trim().to_owned())
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "unknown version".to_owned());
            println!(
                "Bun {reported} is already installed at {} (use --force to reinstall).",
                bun.display()
            );
            return 0;
        }
    }

    let command = installer_command(use_npm, version.as_deref());
    if dry_run {
        println!("orb: would run: {command}");
        return 0;
    }

    println!("orb: running: {command}");
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &command,
            ])
            .status()
    } else {
        Command::new("sh").arg("-c").arg(&command).status()
    };

    match status {
        Ok(status) if status.success() => {
            if let Some(bun) = util::find_bun() {
                let version = Command::new(&bun)
                    .arg("--version")
                    .output()
                    .ok()
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .map(|version| version.trim().to_owned())
                    .unwrap_or_default();
                println!("Bun {version} installed at {}.", bun.display());
            } else {
                let bin = if cfg!(windows) {
                    "%USERPROFILE%\\.bun\\bin"
                } else {
                    "$HOME/.bun/bin"
                };
                println!(
                    "Bun installed. Open a new shell, or add {bin} to PATH, then run `bun --version`."
                );
            }
            0
        }
        Ok(status) => {
            eprintln!("error: Bun installer exited with {}", util::status_code(status));
            1
        }
        Err(error) => {
            eprintln!("error: cannot start the installer: {error}");
            1
        }
    }
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'))
}

fn installer_command(use_npm: bool, version: Option<&str>) -> String {
    if use_npm {
        return match version {
            Some(version) => format!("npm install -g bun@{version}"),
            None => "npm install -g bun".to_owned(),
        };
    }

    if cfg!(windows) {
        return "irm bun.sh/install.ps1 | iex".to_owned();
    }

    let downloader = if util::which("curl").is_some() {
        "curl -fsSL https://bun.sh/install"
    } else if util::which("wget").is_some() {
        "wget -qO- https://bun.sh/install"
    } else {
        // The installer itself downloads with curl or wget; without either the
        // command will fail with the underlying tool's error, which is still
        // more useful than pretending this orb can fetch binaries itself.
        "curl -fsSL https://bun.sh/install"
    };
    match version {
        Some(version) => format!("{downloader} | bash -s -- bun-v{version}"),
        None => format!("{downloader} | bash"),
    }
}

/// `orb upgrade`: upgrade Bun, installing it when absent.
pub(crate) fn command_upgrade(args: &[String]) -> i32 {
    match util::find_bun() {
        Some(bun) => run_status(Command::new(bun).arg("upgrade").args(args)),
        None => {
            println!("Bun is not installed; installing it instead.");
            command_install_bun(args)
        }
    }
}

/// `orb create`: delegate to `bun create` / `npm create`.
pub(crate) fn command_create(args: &[String]) -> i32 {
    delegate("create", args)
}

/// `orb init [-y] [--force] [dir]`.
pub(crate) fn command_init(args: &[String]) -> i32 {
    let mut force = false;
    let mut directory: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "orb init - create package.json and index.ts\n\nUSAGE:\n    orb init [-y] [--force] [dir]"
                );
                return 0;
            }
            "-y" | "--yes" => {}
            "--force" | "-f" => force = true,
            other if other.starts_with('-') => {
                eprintln!("error: unknown flag '{other}'");
                return 2;
            }
            other => {
                if directory.is_some() {
                    eprintln!("error: expected at most one directory");
                    return 2;
                }
                directory = Some(other.to_owned());
            }
        }
    }

    let target = directory
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if let Err(error) = fs::create_dir_all(&target) {
        eprintln!("error: cannot create '{}': {error}", target.display());
        return 1;
    }

    let name = target
        .canonicalize()
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "orb-project".to_owned());
    let package_name = sanitize_name(&name);

    let package_path = target.join("package.json");
    if package_path.exists() && !force {
        println!("package.json already exists (use --force to overwrite).");
    } else {
        let contents = format!(
            "{{\n  \"name\": \"{package_name}\",\n  \"version\": \"0.0.0\",\n  \"type\": \"module\",\n  \"private\": true,\n  \"module\": \"index.ts\",\n  \"scripts\": {{\n    \"start\": \"orb run index.ts\",\n    \"test\": \"orb test\"\n  }}\n}}\n"
        );
        if let Err(error) = fs::write(&package_path, contents) {
            eprintln!("error: cannot write '{}': {error}", package_path.display());
            return 1;
        }
        println!("created {}", package_path.display());
    }

    let index_path = target.join("index.ts");
    if index_path.exists() && !force {
        println!("index.ts already exists (use --force to overwrite).");
    } else {
        if let Err(error) = fs::write(&index_path, "console.log(\"Hello from orb!\");\n") {
            eprintln!("error: cannot write '{}': {error}", index_path.display());
            return 1;
        }
        println!("created {}", index_path.display());
    }
    0
}

fn sanitize_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            result.push(character.to_ascii_lowercase());
        } else {
            result.push('-');
        }
    }
    let trimmed = result.trim_matches(['-', '.']).to_owned();
    if trimmed.is_empty() {
        "orb-project".to_owned()
    } else {
        trimmed
    }
}

/// Looks up a `package.json` script without interpreting the file as a module.
pub(crate) fn find_script(name: &str) -> Option<String> {
    let cwd = env::current_dir().ok()?;
    find_script_in(&cwd, name)
}

fn find_script_in(cwd: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(cwd.join("package.json")).ok()?;
    let package: serde_json::Value = serde_json::from_str(&text).ok()?;
    package
        .get("scripts")?
        .get(name)?
        .as_str()
        .map(str::to_owned)
}

/// Runs a package script. Bun owns the semantics when present; otherwise the
/// script is executed by the platform shell with `node_modules/.bin` on PATH.
pub(crate) fn run_script(name: &str, command: &str, args: &[String]) -> i32 {
    if env::var_os("ORB_NO_DELEGATE").is_none() {
        if let Some(bun) = util::find_bun() {
            let mut invocation = Command::new(bun);
            invocation.arg("run").arg(name);
            if !args.is_empty() {
                invocation.arg("--");
                invocation.args(args);
            }
            return run_status(&mut invocation);
        }
    }

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = augmented_path(&cwd);
    println!("orb: running script '{name}': {command}");
    let status = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(command)
            .args(args)
            .env("PATH", &path)
            .status()
    } else {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .arg(name)
            .args(args)
            .env("PATH", &path)
            .status()
    };
    match status {
        Ok(status) => util::status_code(status),
        Err(error) => {
            eprintln!("error: cannot run script '{name}': {error}");
            127
        }
    }
}

fn augmented_path(cwd: &Path) -> std::ffi::OsString {
    let mut paths = vec![cwd.join("node_modules").join(".bin")];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).unwrap_or_default()
}
