//! Node-shaped module resolution for the compatibility runtime.
//!
//! Resolves relative files, bare package specifiers through `node_modules`
//! (including subpaths such as `next/image` and `next/font/google`),
//! `package.json` `exports`/`main` fields, TypeScript `paths` aliases from the
//! nearest `tsconfig.json`, and Node built-in names. Shared by the ES-module
//! loader and CommonJS `require` so both systems agree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) const EXTENSIONS: &[&str] = &[
    "js", "cjs", "json", "ts", "tsx", "jsx", "mjs", "mts", "cts",
];

pub(crate) const INDEX_FILES: &[&str] = &[
    "index.js",
    "index.cjs",
    "index.json",
    "index.ts",
    "index.tsx",
    "index.jsx",
    "index.mjs",
    "index.mts",
    "index.cts",
];

/// Core module names that resolve without a `node:` prefix. Any specifier
/// starting with `node:` is a built-in regardless of this list; unknown ones
/// get a generic no-op shim from `builtins::get`.
pub(crate) const BUILTINS: &[&str] = &[
    "assert", "assert/strict", "async_hooks", "buffer", "child_process", "cluster",
    "console", "constants", "crypto", "dgram", "diagnostics_channel", "dns",
    "dns/promises", "domain", "events", "fs", "fs/promises", "http", "http2", "https",
    "inspector", "inspector/promises", "module", "net", "os", "path", "path/posix",
    "path/win32", "perf_hooks", "process", "punycode", "querystring", "readline",
    "readline/promises", "repl", "stream", "stream/consumers", "stream/promises",
    "stream/web", "string_decoder", "sys", "timers", "timers/promises", "tls",
    "trace_events", "tty", "url", "util", "util/types", "v8", "vm", "wasi",
    "worker_threads", "zlib",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    File(PathBuf),
    Builtin(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Condition {
    Import,
    Require,
}

/// The parts of `compilerOptions` that affect specifier resolution.
#[derive(Debug, Clone)]
struct TsConfig {
    /// Directory that relative `paths` targets are resolved against (`baseUrl`).
    base: PathBuf,
    /// `(pattern, targets)` pairs from `compilerOptions.paths`.
    paths: Vec<(String, Vec<String>)>,
}

thread_local! {
    /// Parsing `tsconfig.json` per import is wasteful; cache by canonical path.
    static TSCONFIG_CACHE: RefCell<HashMap<PathBuf, Option<TsConfig>>> =
        RefCell::new(HashMap::new());
}

pub(crate) fn is_builtin(specifier: &str) -> bool {
    if specifier.starts_with("node:") {
        return true;
    }
    BUILTINS.contains(&specifier)
}

pub(crate) fn builtin_name(specifier: &str) -> String {
    specifier.strip_prefix("node:").unwrap_or(specifier).to_owned()
}

/// Resolves `specifier` imported from `parent_file`.
pub(crate) fn resolve(
    parent_file: &Path,
    specifier: &str,
    condition: Condition,
) -> Result<Resolved, String> {
    if is_builtin(specifier) {
        return Ok(Resolved::Builtin(builtin_name(specifier)));
    }
    if is_relative(specifier) {
        if let Some(path) = try_specifier_file(parent_file, specifier) {
            return Ok(Resolved::File(path));
        }
    } else {
        // Bare specifiers must never fall back to a sibling file: a directory
        // containing `ajv.js` should not make `require("ajv")` load it.
        // TypeScript `paths` aliases take precedence over node_modules, the
        // same order tsc and bundlers use.
        if let Some(path) = resolve_tsconfig_path(parent_file, specifier) {
            return Ok(Resolved::File(path));
        }
        if let Some(path) = resolve_package(parent_file, specifier, condition) {
            return Ok(Resolved::File(path));
        }
    }
    Err(format!("Cannot find module '{specifier}' from '{}'", parent_file.display()))
}

pub(crate) fn is_relative(specifier: &str) -> bool {
    specifier == "." || specifier == ".." || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with(".\\")
        || specifier.starts_with("..\\")
        || Path::new(specifier).is_absolute()
}

/// Directory to start ancestor walks from. `parent_file` may be the synthetic
/// `<eval>`/REPL path, which is not a real file but lives under the cwd.
fn start_dir(parent_file: &Path) -> PathBuf {
    if parent_file.is_dir() {
        parent_file.to_path_buf()
    } else {
        parent_file.parent().unwrap_or(Path::new(".")).to_path_buf()
    }
}

fn try_specifier_file(parent_file: &Path, specifier: &str) -> Option<PathBuf> {
    let base = start_dir(parent_file);
    let candidate = if Path::new(specifier).is_absolute() {
        PathBuf::from(specifier)
    } else {
        base.join(specifier)
    };
    try_file(&candidate)
}

// ---------------------------------------------------------------------------
// TypeScript `paths` aliases
// ---------------------------------------------------------------------------

/// Finds the nearest `tsconfig.json` at or above `start` and resolves
/// `specifier` through its `compilerOptions.paths` mapping.
fn resolve_tsconfig_path(parent_file: &Path, specifier: &str) -> Option<PathBuf> {
    let start = start_dir(parent_file);
    let config = find_tsconfig(&start)?;

    // TypeScript prefers the pattern with the longest prefix before `*`.
    let mut best: Option<(String, Vec<String>, usize)> = None;
    for (pattern, targets) in &config.paths {
        let score = if let Some(star) = pattern.find('*') {
            let prefix = &pattern[..star];
            let suffix = &pattern[star + 1..];
            if specifier.starts_with(prefix)
                && specifier.ends_with(suffix)
                && specifier.len() >= prefix.len() + suffix.len()
            {
                Some(prefix.len())
            } else {
                None
            }
        } else if pattern == specifier {
            Some(pattern.len())
        } else {
            None
        };
        if let Some(score) = score {
            if best.as_ref().map_or(true, |(_, _, current)| score > *current) {
                best = Some((pattern.clone(), targets.clone(), score));
            }
        }
    }

    let (pattern, targets, _) = best?;
    let wildcard = if let Some(star) = pattern.find('*') {
        let prefix = &pattern[..star];
        let suffix = &pattern[star + 1..];
        Some(&specifier[prefix.len()..specifier.len() - suffix.len()])
    } else {
        None
    };
    for target in targets {
        let expanded = match wildcard {
            Some(value) => target.replace('*', value),
            None => target.clone(),
        };
        if let Some(path) = try_file(&config.base.join(expanded)) {
            return Some(path);
        }
    }
    None
}

fn find_tsconfig(start: &Path) -> Option<TsConfig> {
    for directory in start.ancestors() {
        // Packages under node_modules belong to their own resolution realm;
        // the application's tsconfig must not rewrite their imports.
        if directory.file_name().map_or(false, |name| name == "node_modules") {
            return None;
        }
        let path = directory.join("tsconfig.json");
        if !path.is_file() {
            continue;
        }
        let key = canonical(&path);
        if let Some(cached) = TSCONFIG_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
            return cached;
        }
        let parsed = parse_tsconfig(&path, directory);
        TSCONFIG_CACHE.with(|cache| {
            cache.borrow_mut().insert(key, parsed.clone());
        });
        return parsed;
    }
    None
}

fn parse_tsconfig(path: &Path, directory: &Path) -> Option<TsConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    // tsconfig.json is JSONC: comments and trailing commas are common.
    let cleaned = strip_trailing_commas(&strip_jsonc(&text));
    let value: Value = serde_json::from_str(&cleaned).ok()?;
    let options = value.get("compilerOptions")?;
    let base = options
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(|base_url| directory.join(base_url))
        .unwrap_or_else(|| directory.to_path_buf());
    let paths = options
        .get("paths")
        .and_then(Value::as_object)?
        .iter()
        .filter_map(|(pattern, targets)| {
            let targets = targets
                .as_array()?
                .iter()
                .filter_map(|target| target.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            Some((pattern.clone(), targets))
        })
        .collect();
    Some(TsConfig { base, paths })
}

/// Removes `//` and `/* */` comments outside string literals.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            out.push(character);
            continue;
        }
        if character == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                }
                _ => out.push(character),
            }
            continue;
        }
        out.push(character);
    }
    out
}

fn strip_trailing_commas(text: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    for character in text.chars() {
        if character == '}' || character == ']' {
            while out.last().map_or(false, |previous| previous.is_whitespace()) {
                out.pop();
            }
            if out.last() == Some(&',') {
                out.pop();
            }
        }
        out.push(character);
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// node_modules resolution
// ---------------------------------------------------------------------------

/// Splits `next/font/google` into package `next` and subpath `font/google`.
/// Bare root specifiers use `"."` as their subpath.
fn split_package_subpath(specifier: &str) -> (&str, &str) {
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or(specifier);
    if first.starts_with('@') {
        let Some(second) = parts.next() else {
            return (specifier, ".");
        };
        let end = first.len() + 1 + second.len();
        if specifier.len() == end {
            (specifier, ".")
        } else {
            (&specifier[..end], &specifier[end + 1..])
        }
    } else {
        match specifier.find('/') {
            Some(index) => (&specifier[..index], &specifier[index + 1..]),
            None => (specifier, "."),
        }
    }
}

/// Resolves a bare specifier by walking `node_modules` from the importing file.
fn resolve_package(parent_file: &Path, specifier: &str, condition: Condition) -> Option<PathBuf> {
    let start = start_dir(parent_file);
    let (package, subpath) = split_package_subpath(specifier);
    for directory in start.ancestors() {
        let package_dir = directory.join("node_modules").join(package);
        if !package_dir.is_dir() {
            continue;
        }
        // 1. `exports` map (handles both "." and named subpaths).
        if let Some(path) = try_package_exports(&package_dir, subpath, condition) {
            return Some(path);
        }
        // 2. Plain filesystem subpath: `next/image` -> `next/image.js`,
        //    `next/font/google` -> `next/font/google/index.js`.
        if subpath != "." {
            if let Some(path) = try_file(&package_dir.join(subpath)) {
                return Some(path);
            }
        }
        // 3. Package root: `main`/`module`, then `index.*`.
        if let Some(path) = try_package(&package_dir, condition) {
            return Some(path);
        }
    }
    None
}

fn try_package_exports(directory: &Path, subpath: &str, condition: Condition) -> Option<PathBuf> {
    let manifest_text = std::fs::read_to_string(directory.join("package.json")).ok()?;
    let manifest: Value = serde_json::from_str(&manifest_text).ok()?;
    let exports = manifest.get("exports")?;
    // `exports` keys are written as "./config"; `split_package_subpath`
    // returns "config" for `eslint/config`.
    let export_subpath = if subpath == "." || subpath.starts_with("./") {
        subpath.to_owned()
    } else {
        format!("./{subpath}")
    };
    let target = select_export(exports, &export_subpath, condition)?;
    try_file(&directory.join(target.trim_start_matches("./")))
}

fn try_package(directory: &Path, condition: Condition) -> Option<PathBuf> {
    if let Some(path) = try_package_exports(directory, ".", condition) {
        return Some(path);
    }
    let manifest_text = std::fs::read_to_string(directory.join("package.json")).ok()?;
    let manifest: Value = serde_json::from_str(&manifest_text).ok()?;
    for field in ["main", "module"] {
        if let Some(main) = manifest.get(field).and_then(Value::as_str) {
            if let Some(resolved) = try_file(&directory.join(main)) {
                return Some(resolved);
            }
        }
    }
    for index in INDEX_FILES {
        let path = directory.join(index);
        if path.is_file() {
            return Some(canonical(&path));
        }
    }
    None
}

/// Selects an `exports` target for `subpath` under `condition`.
fn select_export(exports: &Value, subpath: &str, condition: Condition) -> Option<String> {
    if let Some(text) = exports.as_str() {
        return if subpath == "." { Some(text.to_owned()) } else { None };
    }
    let object = exports.as_object()?;

    if let Some(value) = object.get(subpath) {
        return select_from_value(value, condition);
    }

    // Wildcard patterns such as "./features/*": "./src/features/*.js".
    let mut best: Option<(usize, String)> = None;
    for (key, value) in object {
        let Some(star) = key.find('*') else {
            continue;
        };
        let prefix = &key[..star];
        let suffix = &key[star + 1..];
        if !subpath.starts_with(prefix)
            || !subpath.ends_with(suffix)
            || subpath.len() < prefix.len() + suffix.len()
        {
            continue;
        }
        let wildcard = &subpath[prefix.len()..subpath.len() - suffix.len()];
        if let Some(target) = select_from_value(value, condition) {
            let expanded = target.replace('*', wildcard);
            if best.as_ref().map_or(true, |(score, _)| prefix.len() > *score) {
                best = Some((prefix.len(), expanded));
            }
        }
    }
    if let Some((_, target)) = best {
        return Some(target);
    }

    // A root conditional map (`{ "import": "...", "default": "..." }`).
    if subpath == "." {
        for key in condition_keys(condition) {
            if let Some(value) = object.get(*key) {
                if let Some(target) = select_from_value(value, condition) {
                    return Some(target);
                }
            }
        }
    }
    None
}

fn select_from_value(value: &Value, condition: Condition) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => items.iter().find_map(|item| select_from_value(item, condition)),
        Value::Object(_) => {
            for key in condition_keys(condition) {
                if let Some(nested) = value.get(*key) {
                    if let Some(target) = select_from_value(nested, condition) {
                        return Some(target);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn condition_keys(condition: Condition) -> &'static [&'static str] {
    match condition {
        Condition::Import => &["import", "node", "default", "require"],
        Condition::Require => &["require", "node", "default", "import"],
    }
}

pub(crate) fn try_file(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(canonical(candidate));
    }
    // Node appends extensions to the whole specifier, even when the basename
    // already contains a dot (`./head-manager-context.shared-runtime` ->
    // `head-manager-context.shared-runtime.js`).
    for extension in EXTENSIONS {
        let mut text = candidate.as_os_str().to_owned();
        text.push(".");
        text.push(extension);
        let path = PathBuf::from(text);
        if path.is_file() {
            return Some(canonical(&path));
        }
    }
    // Bun/TypeScript convenience: `./dep.js` finds `dep.ts`.
    let extension = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if matches!(extension, "js" | "mjs" | "cjs") {
        for replacement in ["ts", "tsx", "jsx", "mts", "cts"] {
            let path = candidate.with_extension(replacement);
            if path.is_file() {
                return Some(canonical(&path));
            }
        }
    }
    if candidate.is_dir() {
        for index in INDEX_FILES {
            let path = candidate.join(index);
            if path.is_file() {
                return Some(canonical(&path));
            }
        }
    }
    None
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "engine-resolve-{}-{name}-{}",
            std::process::id(),
            SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_names_drop_the_node_prefix() {
        assert!(is_builtin("node:fs"));
        assert!(is_builtin("path"));
        assert!(!is_builtin("./path"));
        assert_eq!(builtin_name("node:path"), "path");
    }

    #[test]
    fn exports_object_honours_import_and_require_conditions() {
        let exports: Value = serde_json::from_str(
            r#"{ ".": { "import": "./esm.mjs", "require": "./cjs.cjs" } }"#,
        )
        .unwrap();
        assert_eq!(select_export(&exports, ".", Condition::Import).as_deref(), Some("./esm.mjs"));
        assert_eq!(select_export(&exports, ".", Condition::Require).as_deref(), Some("./cjs.cjs"));
    }

    #[test]
    fn resolves_package_subpath_files_and_directories() {
        let dir = scratch("subpath");
        let package = dir.join("node_modules/next");
        std::fs::create_dir_all(package.join("font/google")).unwrap();
        std::fs::write(dir.join("main.js"), "").unwrap();
        std::fs::write(package.join("package.json"), r#"{"main":"./index.js"}"#).unwrap();
        std::fs::write(package.join("index.js"), "module.exports = {};").unwrap();
        std::fs::write(package.join("image.js"), "module.exports = function(){};").unwrap();
        std::fs::write(package.join("font/google/index.js"), "").unwrap();

        let parent = dir.join("main.js");
        let file = |specifier| match resolve(&parent, specifier, Condition::Import) {
            Ok(Resolved::File(path)) => path,
            other => panic!("expected a file for {specifier}, got {other:?}"),
        };
        assert!(file("next/image").ends_with("next/image.js"));
        assert!(file("next/font/google").ends_with("font/google/index.js"));
        assert!(file("next").ends_with("next/index.js"));
    }

    #[test]
    fn bare_specifier_does_not_resolve_to_a_sibling_file() {
        let dir = scratch("bare-sibling");
        std::fs::write(dir.join("ajv.js"), "module.exports = 'wrong';").unwrap();
        let package = dir.join("node_modules/ajv");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), r#"{"main":"./index.js"}"#).unwrap();
        std::fs::write(package.join("index.js"), "module.exports = 'right';").unwrap();

        let resolved = resolve(&dir.join("ajv-user.js"), "ajv", Condition::Require).unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("node_modules/ajv/index.js")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn appends_extensions_to_dotted_basenames() {
        let dir = scratch("dotted");
        std::fs::write(dir.join("main.js"), "").unwrap();
        std::fs::write(dir.join("context.shared-runtime.js"), "module.exports = 1;").unwrap();
        let resolved = resolve(&dir.join("main.js"), "./context.shared-runtime", Condition::Require)
            .unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("context.shared-runtime.js")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn resolves_package_subpath_exports() {
        let dir = scratch("exports");
        let package = dir.join("node_modules/eslint");
        std::fs::create_dir_all(package.join("lib")).unwrap();
        std::fs::write(dir.join("main.js"), "").unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"exports":{".":{"default":"./lib/api.js"},"./config":{"types":"./lib/types.d.ts","default":"./lib/config-api.js"}}}"#,
        )
        .unwrap();
        std::fs::write(package.join("lib/api.js"), "module.exports = {};").unwrap();
        std::fs::write(package.join("lib/config-api.js"), "module.exports = {};").unwrap();

        let parent = dir.join("main.js");
        let resolved = resolve(&parent, "eslint/config", Condition::Import).unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("lib/config-api.js")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn resolves_scoped_package_subpath() {
        let dir = scratch("scoped");
        let package = dir.join("node_modules/@scope/pkg");
        std::fs::create_dir_all(package.join("sub")).unwrap();
        std::fs::write(dir.join("main.js"), "").unwrap();
        std::fs::write(package.join("package.json"), r#"{"main":"./index.js"}"#).unwrap();
        std::fs::write(package.join("index.js"), "").unwrap();
        std::fs::write(package.join("sub/index.js"), "").unwrap();

        let resolved = resolve(&dir.join("main.js"), "@scope/pkg/sub", Condition::Import).unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("sub/index.js")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn resolves_tsconfig_path_alias() {
        let dir = scratch("tsconfig");
        std::fs::create_dir_all(dir.join("src/lib")).unwrap();
        std::fs::write(dir.join("tsconfig.json"), r#"{
            // JSONC comment
            "compilerOptions": {
                "paths": { "@/*": ["./src/*"] },
            }
        }"#).unwrap();
        std::fs::write(dir.join("src/lib/utils.ts"), "export const x = 1;").unwrap();
        std::fs::write(dir.join("main.ts"), "").unwrap();

        let resolved = resolve(&dir.join("main.ts"), "@/lib/utils", Condition::Import).unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("src/lib/utils.ts")),
            other => panic!("expected a file, got {other:?}"),
        }
        // Exports also work from files nested under src/.
        let nested = resolve(&dir.join("src/main.ts"), "@/lib/utils", Condition::Import).unwrap();
        match nested {
            Resolved::File(path) => assert!(path.ends_with("src/lib/utils.ts")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn tsconfig_paths_do_not_apply_inside_node_modules() {
        let dir = scratch("tsconfig-node-modules");
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("tsconfig.json"), r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#).unwrap();
        std::fs::write(dir.join("node_modules/pkg/package.json"), r#"{"main":"./index.js"}"#).unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "").unwrap();
        let parent = dir.join("node_modules/pkg/index.js");
        assert!(resolve(&parent, "@/lib/utils", Condition::Import).is_err());
    }

    #[test]
    fn wildcard_exports_expand_the_subpath() {
        let exports: Value = serde_json::from_str(
            r#"{ "./features/*": { "default": "./src/features/*.js" } }"#,
        )
        .unwrap();
        assert_eq!(
            select_export(&exports, "./features/alpha", Condition::Import).as_deref(),
            Some("./src/features/alpha.js")
        );
    }
}
