//! `orb test`: discovery, filtering, parallel execution, reporting, watch.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use compat::{RunOptions, TestResult};

use crate::{envfile, util};

#[derive(Debug, Clone)]
struct TestConfig {
    roots: Vec<PathBuf>,
    filters: Vec<String>,
    name_pattern: Option<String>,
    concurrency: usize,
    bail: bool,
    watch: bool,
    cwd: Option<String>,
    env_files: Vec<String>,
    auto_env: bool,
    preloads: Vec<String>,
    help: bool,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            filters: Vec::new(),
            name_pattern: None,
            concurrency: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(4),
            bail: false,
            watch: false,
            cwd: None,
            env_files: Vec::new(),
            auto_env: true,
            preloads: Vec::new(),
            help: false,
        }
    }
}

fn parse_config(args: &[String]) -> Result<TestConfig, String> {
    let mut config = TestConfig::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--help" | "-h" => config.help = true,
            "--bail" => config.bail = true,
            "--watch" | "--hot" => config.watch = true,
            "--no-env-file" => config.auto_env = false,
            "--concurrency" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--concurrency requires an argument".to_owned())?;
                config.concurrency = parse_concurrency(value)?;
            }
            "--test-name-pattern" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--test-name-pattern requires an argument".to_owned())?;
                config.name_pattern = Some(value.clone());
            }
            "--env-file" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--env-file requires an argument".to_owned())?;
                config.env_files.push(value.clone());
            }
            "--preload" | "--require" | "-r" | "--import" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{arg} requires an argument"))?;
                config.preloads.push(value.clone());
            }
            "--cwd" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cwd requires an argument".to_owned())?;
                config.cwd = Some(value.clone());
            }
            "--" => {
                index += 1;
                while index < args.len() {
                    push_positional(&mut config, &args[index]);
                    index += 1;
                }
                break;
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--concurrency=") {
                    config.concurrency = parse_concurrency(value)?;
                } else if let Some(value) = arg.strip_prefix("--test-name-pattern=") {
                    config.name_pattern = Some(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--env-file=") {
                    config.env_files.push(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--preload=") {
                    config.preloads.push(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--cwd=") {
                    config.cwd = Some(value.to_owned());
                } else if arg.starts_with('-') {
                    return Err(format!("unknown flag '{arg}'"));
                } else {
                    push_positional(&mut config, arg);
                }
            }
        }
        index += 1;
    }
    Ok(config)
}

fn parse_concurrency(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(value) if value > 0 => Ok(value),
        _ => Err("--concurrency must be a positive integer".to_owned()),
    }
}

fn push_positional(config: &mut TestConfig, value: &str) {
    let path = PathBuf::from(value);
    if path.exists() {
        config.roots.push(path);
    } else {
        config.filters.push(value.to_owned());
    }
}

pub(crate) fn command_test(args: &[String]) -> i32 {
    let config = match parse_config(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if config.help {
        print!(
            "\
orb test - run *.test.* and *.spec.* files

USAGE:
    orb test [path|filter...] [FLAGS]

FLAGS:
    --bail                     stop scheduling new files after the first failure
    --concurrency <N>          number of test files to run in parallel
    --test-name-pattern <text> only report tests whose names contain <text>
    --watch, --hot             re-run when test files change
    --preload <file>           evaluate <file> before each test file
    --env-file <file>          load environment variables from <file>
    --cwd <dir>                resolve paths relative to <dir>
"
        );
        return 0;
    }

    if let Some(directory) = &config.cwd {
        if let Err(error) = util::change_dir(directory) {
            eprintln!("error: {error}");
            return 1;
        }
    }
    if let Err(error) = envfile::load(&config.env_files, config.auto_env) {
        eprintln!("error: {error}");
        return 1;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let files = match discover(&config, &cwd) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if files.is_empty() {
        println!("No test files found.");
        return 0;
    }

    let mut watcher = config.watch.then(|| util::Watcher::new(cwd.clone()));
    loop {
        let code = run_suite(&files, &config, &cwd);
        let Some(watcher) = watcher.as_mut() else {
            return code;
        };
        watcher.wait();
        eprintln!("\norb: change detected, re-running tests");
    }
}

fn discover(config: &TestConfig, cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let roots = if config.roots.is_empty() {
        vec![cwd.to_path_buf()]
    } else {
        config
            .roots
            .iter()
            .map(|root| {
                if root.is_absolute() {
                    root.clone()
                } else {
                    cwd.join(root)
                }
            })
            .collect()
    };

    let mut files = Vec::new();
    for root in &roots {
        if root.is_file() {
            if !util::is_supported_extension(root) {
                return Err(format!(
                    "unsupported test file extension: '{}'",
                    root.display()
                ));
            }
            files.push(root.clone());
        } else if root.is_dir() {
            let root_is_test_dir = matches!(
                root.file_name().and_then(|name| name.to_str()),
                Some("test") | Some("tests") | Some("__tests__")
            );
            walk(root, root, root_is_test_dir, &mut files);
        } else {
            return Err(format!("path does not exist: '{}'", root.display()));
        }
    }

    files.sort();
    files.dedup();

    if !config.filters.is_empty() {
        files.retain(|file| {
            let display = file.to_string_lossy().to_ascii_lowercase();
            config
                .filters
                .iter()
                .any(|filter| display.contains(&filter.to_ascii_lowercase()))
        });
    }
    Ok(files)
}

fn walk(root: &Path, directory: &Path, root_is_test_dir: bool, files: &mut Vec<PathBuf>) {
    let Ok(read_dir) = fs::read_dir(directory) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .map_or(false, |name| name.starts_with('.'));
            if !hidden && !util::is_skipped_directory(&path) {
                walk(root, &path, root_is_test_dir, files);
            }
            continue;
        }
        if !util::is_supported_extension(&path) {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if root_is_test_dir || util::is_test_file(&path) || util::is_in_test_directory(relative) {
            files.push(path);
        }
    }
}

struct FileOutcome {
    path: PathBuf,
    rows: Vec<TestResult>,
    output: String,
    error: Option<String>,
    elapsed_ms: f64,
}

fn run_suite(files: &[PathBuf], config: &TestConfig, cwd: &Path) -> i32 {
    let started = Instant::now();
    let workers = config.concurrency.min(files.len()).max(1);
    let next = AtomicUsize::new(0);
    let bail = AtomicBool::new(false);
    let collected: Mutex<Vec<(usize, FileOutcome)>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            // Boa's VM uses the native stack for host calls. Dependency-heavy
            // programs need far more than the default 2 MiB worker stack.
            std::thread::Builder::new()
                .name("orb-test-worker".to_owned())
                .stack_size(16 * 1024 * 1024)
                .spawn_scoped(scope, || loop {
                if config.bail && bail.load(Ordering::Acquire) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= files.len() {
                    break;
                }
                let outcome = run_one(&files[index], config, cwd);
                let failed = outcome.failed();
                collected.lock().unwrap().push((index, outcome));
                if config.bail && failed {
                    bail.store(true, Ordering::Release);
                }
            })
            .expect("spawn test worker");
        }
    });

    let mut outcomes = collected.into_inner().unwrap();
    outcomes.sort_by_key(|(index, _)| *index);

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut total_tests = 0usize;

    for (_, outcome) in &outcomes {
        let display = display_path(&outcome.path, cwd);
        println!("{display} ({:.1}ms):", outcome.elapsed_ms);
        if !outcome.output.is_empty() {
            print!("{}", indent_output(&outcome.output));
        }
        if let Some(error) = &outcome.error {
            println!("  (fail) {error}");
            failed += 1;
            continue;
        }
        for row in &outcome.rows {
            total_tests += 1;
            if row.skipped {
                skipped += 1;
                println!("  (skip) {}", row.name);
            } else if row.passed {
                passed += 1;
                println!("  (pass) {} [{:.2}ms]", row.name, row.duration_ms);
            } else {
                failed += 1;
                println!("  (fail) {} [{:.2}ms]", row.name, row.duration_ms);
                if let Some(error) = &row.error {
                    println!("    {error}");
                }
            }
        }
        if outcome.rows.is_empty() && outcome.error.is_none() {
            println!("  (no tests)");
        }
    }

    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    if total_tests > 0 || failed == 0 {
        println!();
    }
    println!(
        "{passed} pass, {failed} fail, {skipped} skip across {} test files in {elapsed:.0}ms",
        outcomes.len()
    );

    if failed > 0 {
        1
    } else {
        0
    }
}

fn run_one(path: &Path, config: &TestConfig, cwd: &Path) -> FileOutcome {
    let started = Instant::now();
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return FileOutcome {
                path: path.to_path_buf(),
                rows: Vec::new(),
                output: String::new(),
                error: Some(format!("cannot read file: {error}")),
                elapsed_ms: 0.0,
            }
        }
    };

    let options = RunOptions {
        filename: path.to_string_lossy().into_owned(),
        cwd: Some(cwd.to_path_buf()),
        argv: vec![
            "orb".to_owned(),
            "test".to_owned(),
            path.to_string_lossy().into_owned(),
        ],
        preloads: config.preloads.clone(),
        ..RunOptions::default()
    };

    let (result, output) = match compat::run_tests_collect(&source, &options) {
        Ok((rows, output)) => (Ok(rows), output),
        Err(error) => (Err(error), String::new()),
    };

    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    match result {
        Ok(mut rows) => {
            if let Some(pattern) = &config.name_pattern {
                rows.retain(|row| row.name.contains(pattern));
            }
            FileOutcome {
                path: path.to_path_buf(),
                rows,
                output,
                error: None,
                elapsed_ms,
            }
        }
        Err(error) => FileOutcome {
            path: path.to_path_buf(),
            rows: Vec::new(),
            output,
            error: Some(error.to_string()),
            elapsed_ms,
        },
    }
}

impl FileOutcome {
    fn failed(&self) -> bool {
        self.error.is_some() || self.rows.iter().any(TestResult::failed)
    }
}

fn display_path(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn indent_output(output: &str) -> String {
    let mut result = String::with_capacity(output.len() + 8);
    for line in output.lines() {
        result.push_str("  ");
        result.push_str(line);
        result.push('\n');
    }
    result
}
