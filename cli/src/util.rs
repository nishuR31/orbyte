//! Small shared utilities: cwd handling, executable lookup, file classification.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Changes the process working directory, reporting a friendly error.
pub(crate) fn change_dir(directory: &str) -> Result<(), String> {
    env::set_current_dir(directory)
        .map_err(|error| format!("cannot change directory to '{directory}': {error}"))
}

/// Resolves a path against the current working directory.
pub(crate) fn absolute(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Node/Bun-style executable lookup.
pub(crate) fn which(command: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(command);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        for extension in ["exe", "cmd", "bat"] {
            let candidate = directory.join(format!("{command}.{extension}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Locates Bun, including the installer's default `$HOME/.bun/bin` location.
pub(crate) fn find_bun() -> Option<PathBuf> {
    if let Some(path) = which("bun") {
        return Some(path);
    }
    let install_dir = env::var_os("BUN_INSTALL")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .or_else(|| env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".bun"))
        });
    let candidate = install_dir?.join("bin").join(if cfg!(windows) {
        "bun.exe"
    } else {
        "bun"
    });
    candidate.is_file().then_some(candidate)
}

/// Returns whether the extension is one the runtime accepts.
pub(crate) fn is_supported_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
    )
}

/// True when the file name looks like a test file.
pub(crate) fn is_test_file(path: &Path) -> bool {
    if !is_supported_extension(path) {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with('.') {
        return false;
    }
    name.ends_with(".test.js")
        || name.ends_with(".test.mjs")
        || name.ends_with(".test.cjs")
        || name.ends_with(".test.jsx")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.mts")
        || name.ends_with(".test.cts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".spec.js")
        || name.ends_with(".spec.mjs")
        || name.ends_with(".spec.cjs")
        || name.ends_with(".spec.jsx")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.mts")
        || name.ends_with(".spec.cts")
        || name.ends_with(".spec.tsx")
        || name.ends_with("_test.js")
        || name.ends_with("_test.ts")
        || name.ends_with("_test.tsx")
        || name.ends_with("_spec.js")
        || name.ends_with("_spec.ts")
        || name.ends_with("_spec.tsx")
}

/// True when a relative path sits under a conventional test directory.
pub(crate) fn is_in_test_directory(relative: &Path) -> bool {
    relative.parent().map_or(false, |parent| {
        parent.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("__tests__") | Some("test") | Some("tests")
            )
        })
    })
}

/// Directories the recursive walkers never enter.
pub(crate) fn is_skipped_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("node_modules")
            | Some(".git")
            | Some(".hg")
            | Some(".svn")
            | Some("target")
            | Some("dist")
            | Some("build")
            | Some(".next")
            | Some("coverage")
            | Some(".cache")
            | Some(".turbo")
            | Some("vendor")
    )
}

/// Prints the standard install/delegation hint.
pub(crate) fn bun_hint() -> &'static str {
    "Bun is required for this command. Run `orb install bun` to install it."
}

/// Maps the exit code of a child process onto this process's exit code.
pub(crate) fn status_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

/// A coarse file-tree fingerprint used by `--watch` / `--hot`.
///
/// Only supported source files, `package.json`, and `.env*` files participate,
/// and the walk skips dependency/build directories. That keeps the watcher
/// cheap even in a repository with a large `node_modules`.
pub(crate) struct Watcher {
    root: PathBuf,
    entries: HashMap<PathBuf, (u128, u64)>,
}

impl Watcher {
    pub(crate) fn new(root: PathBuf) -> Self {
        let entries = fingerprint(&root);
        Self { root, entries }
    }

    /// Blocks until a relevant file changes, then refreshes the fingerprint.
    pub(crate) fn wait(&mut self) {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let current = fingerprint(&self.root);
            if current != self.entries {
                self.entries = current;
                return;
            }
        }
    }
}

fn fingerprint(root: &Path) -> HashMap<PathBuf, (u128, u64)> {
    let mut entries = HashMap::new();
    if root.is_file() {
        insert_stamp(&mut entries, root);
    } else {
        walk(root, &mut entries);
    }
    entries
}

fn walk(directory: &Path, entries: &mut HashMap<PathBuf, (u128, u64)>) {
    let Ok(read_dir) = std::fs::read_dir(directory) else {
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
            if !is_skipped_directory(&path) {
                walk(&path, entries);
            }
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if is_supported_extension(&path) || name == "package.json" || name.starts_with(".env") {
            insert_stamp(entries, &path);
        }
    }
}

fn insert_stamp(entries: &mut HashMap<PathBuf, (u128, u64)>, path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    entries.insert(path.to_path_buf(), (modified, metadata.len()));
}
