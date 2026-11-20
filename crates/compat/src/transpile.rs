//! TypeScript/JSX erasure with an on-disk cache.
//!
//! Plain JavaScript never reaches SWC; it is passed through unchanged. Files
//! that do need erasure are keyed by content and parser mode, so repeated runs
//! (and repeated test invocations) skip the transform entirely.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use swc::config::{IsModule, Options};
use swc_common::{errors::Handler, sync::Lrc, FileName, Globals, SourceMap, GLOBALS as SWC_GLOBALS};
use swc_ecma_ast::{EsVersion, ModuleItem, Program};
use swc_ecma_parser::{EsSyntax, Syntax, TsSyntax};

use crate::CompatError;

thread_local! {
    /// SWC's `GLOBALS` is required around every operation and is relatively
    /// expensive to allocate. One per thread is enough for the whole process.
    static GLOBALS: Globals = Globals::new();
    /// Small in-process cache in front of the disk cache. This matters for the
    /// test runner and the REPL, which transform many files in one process.
    static MEMORY_CACHE: std::cell::RefCell<HashMap<String, String>> =
        std::cell::RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    JavaScript,
    Jsx,
    TypeScript,
}

pub(crate) fn source_kind(filename: &str) -> SourceKind {
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "jsx" => SourceKind::Jsx,
        "ts" | "mts" | "cts" | "tsx" => SourceKind::TypeScript,
        _ => SourceKind::JavaScript,
    }
}

/// Confirms that a source really contains ESM declarations, using the SWC
/// parser so string/comment contents cannot fool the line-based heuristic in
/// `runtime::has_module_syntax`. Called only for candidates, so ordinary
/// CommonJS files keep the cheap fast path.
pub(crate) fn contains_module_decl(source: &str, filename: &str) -> bool {
    let typescript = source_kind(filename) == SourceKind::TypeScript;
    GLOBALS.with(|globals| {
        SWC_GLOBALS.set(globals, || {
            let source_map: Lrc<SourceMap> = Default::default();
            let file = source_map.new_source_file(
                FileName::Custom(filename.to_owned()).into(),
                source.to_owned(),
            );
            let diagnostics = SharedBuffer::default();
            let handler =
                Handler::with_emitter_writer(Box::new(diagnostics.clone()), Some(source_map.clone()));
            let compiler = swc::Compiler::new(source_map);
            let syntax = if typescript {
                Syntax::Typescript(TsSyntax {
                    tsx: filename.ends_with(".tsx"),
                    decorators: true,
                    ..Default::default()
                })
            } else {
                Syntax::Es(EsSyntax {
                    jsx: true,
                    decorators: true,
                    ..Default::default()
                })
            };
            match compiler.parse_js(
                file,
                &handler,
                EsVersion::EsNext,
                syntax,
                IsModule::Bool(true),
                None,
            ) {
                Ok(Program::Module(module)) => module
                    .body
                    .iter()
                    .any(|item| matches!(item, ModuleItem::ModuleDecl(_))),
                // If parsing fails, trust the caller's heuristic rather than
                // changing behaviour on malformed input.
                _ => true,
            }
        })
    })
}

/// Removes a leading `#!` line, which Boa does not accept.
pub(crate) fn strip_shebang(source: &str) -> &str {
    if source.starts_with("#!") {
        match source.find('\n') {
            Some(index) => &source[index + 1..],
            None => "",
        }
    } else {
        source
    }
}

/// Erases TypeScript/JSX when the filename calls for it.
pub fn transpile_source(source: &str, filename: &str) -> Result<String, CompatError> {
    let source = strip_shebang(source);
    match source_kind(filename) {
        SourceKind::JavaScript => Ok(source.to_owned()),
        kind => {
            let key = cache_key(source, filename, kind);
            if let Some(cached) = MEMORY_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
                return Ok(cached);
            }

            if let Some(cached) = read_disk_cache(&key) {
                remember(&key, &cached);
                return Ok(cached);
            }

            let code = transpile_uncached(source, filename, kind)?;
            remember(&key, &code);
            write_disk_cache(&key, &code);
            Ok(code)
        }
    }
}

fn remember(key: &str, code: &str) {
    MEMORY_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        // A CLI process is short-lived, but the test runner and REPL can touch
        // many files. Bound the map so a pathological session cannot grow it
        // without limit.
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(key.to_owned(), code.to_owned());
    });
}

fn transpile_uncached(
    source: &str,
    filename: &str,
    kind: SourceKind,
) -> Result<String, CompatError> {
    GLOBALS.with(|globals| {
        SWC_GLOBALS.set(globals, || {
            // `.ts` normally means "no JSX", but the acceptance file contains
            // JSX despite the `.ts` extension. Try strict mode first, then the
            // lenient TSX mode.
            let typescript = kind == SourceKind::TypeScript;
            let first = transpile_once(source, filename, typescript, false);
            match first {
                Ok(code) => Ok(code),
                Err(_) if typescript => {
                    transpile_once(source, filename, true, true).map_err(CompatError::Transpile)
                }
                Err(message) => Err(CompatError::Transpile(message)),
            }
        })
    })
}

fn transpile_once(
    source: &str,
    filename: &str,
    typescript: bool,
    tsx: bool,
) -> Result<String, String> {
    let source_map: Lrc<SourceMap> = Default::default();
    // Use a non-`Real` file name: `Options::build_as_input` calls
    // `Config::adjust` for `FileName::Real`, which would force `tsx = false`
    // for a `.ts` path and undo the TSX retry. A `Custom` name keeps the
    // original filename visible in diagnostics without extension-based
    // config rewriting.
    let file = source_map.new_source_file(
        FileName::Custom(filename.to_owned()).into(),
        source.to_owned(),
    );
    let diagnostics = SharedBuffer::default();
    let handler =
        Handler::with_emitter_writer(Box::new(diagnostics.clone()), Some(source_map.clone()));
    let compiler = swc::Compiler::new(source_map);

    let syntax = if typescript {
        Syntax::Typescript(TsSyntax {
            tsx,
            decorators: true,
            ..Default::default()
        })
    } else {
        Syntax::Es(EsSyntax {
            jsx: true,
            decorators: true,
            ..Default::default()
        })
    };

    let mut options: Options = serde_json::from_str(&options_json(typescript, tsx))
        .map_err(|error| format!("internal SWC configuration error: {error}"))?;
    // Do not set `options.filename`: `Config::adjust` would force `tsx = false`
    // for a `.ts` filename and undo the TSX retry. The real path is already in
    // the source map for diagnostics.
    options.swcrc = false;
    options.config.jsc.syntax = Some(syntax.clone());

    let program = compiler
        .parse_js(
            file,
            &handler,
            EsVersion::EsNext,
            syntax,
            IsModule::Bool(true),
            None,
        )
        .map_err(|error| diagnostics_message(&diagnostics, error))?;
    // SWC recovers from some parse errors and still returns a program. Treat
    // any emitted error as a failed attempt so `.ts` can be retried as TSX.
    if handler.has_errors() {
        return Err(diagnostics_text(&diagnostics, "parsing failed"));
    }

    let output = compiler
        .process_js(&handler, program, &options)
        .map_err(|error| diagnostics_message(&diagnostics, error))?;
    if handler.has_errors() {
        return Err(diagnostics_text(&diagnostics, "transformation failed"));
    }

    Ok(output.code)
}

fn options_json(typescript: bool, tsx: bool) -> String {
    let parser = if typescript {
        format!(r#"{{"syntax":"typescript","tsx":{tsx},"decorators":true}}"#)
    } else {
        r#"{"syntax":"ecmascript","jsx":true,"decorators":true}"#.to_owned()
    };
    format!(
        r#"{{"jsc":{{"parser":{parser},"target":"es2022","transform":{{"legacyDecorator":true,"react":{{"runtime":"classic"}}}}}},"module":{{"type":"es6"}}}}"#
    )
}

/// SWC emits diagnostics through a [`Handler`]; prefer that text over the
/// terse top-level error value.
fn diagnostics_message<E: std::fmt::Debug>(buffer: &SharedBuffer, error: E) -> String {
    let captured = buffer.contents();
    if captured.trim().is_empty() {
        format!("{error:?}")
    } else {
        captured
    }
}

fn diagnostics_text(buffer: &SharedBuffer, fallback: &str) -> String {
    let captured = buffer.contents();
    if captured.trim().is_empty() {
        fallback.to_owned()
    } else {
        captured
    }
}

#[derive(Debug, Clone, Default)]
struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn contents(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn cache_key(source: &str, filename: &str, kind: SourceKind) -> String {
    let mode = match kind {
        SourceKind::JavaScript => 0,
        SourceKind::Jsx => 1,
        SourceKind::TypeScript => 2,
    };
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let first = hash(0xcbf2_9ce4_8422_2325, mode, extension.as_bytes(), source.as_bytes());
    let second = hash(0x9e37_79b9_7f4a_7c15, mode, source.as_bytes(), extension.as_bytes());
    format!("{first:016x}{second:016x}-{}", source.len())
}

fn hash(seed: u64, mode: u8, first: &[u8], second: &[u8]) -> u64 {
    let mut value = seed ^ u64::from(mode);
    for byte in first.iter().chain(second) {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x100_0000_01b3);
    }
    value
}

fn cache_root() -> Option<&'static PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        if std::env::var_os("ORB_NO_CACHE").is_some() {
            return None;
        }
        let base = std::env::var_os("ORB_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            })
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join("engine").join("transpile");
        std::fs::create_dir_all(&root).ok()?;
        Some(root)
    })
    .as_ref()
}

fn read_disk_cache(key: &str) -> Option<String> {
    let path = cache_root()?.join(format!("{key}.js"));
    std::fs::read_to_string(path).ok()
}

fn write_disk_cache(key: &str, code: &str) {
    let Some(root) = cache_root() else {
        return;
    };
    let path = root.join(format!("{key}.js"));
    // Write-then-rename keeps concurrent readers (parallel test workers) from
    // observing a partial file.
    let temporary = root.join(format!(".{key}.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, code).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
    let _ = std::fs::remove_file(&temporary);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_javascript_is_passed_through() {
        assert_eq!(transpile_source("let x = 1;", "a.js").unwrap(), "let x = 1;");
    }

    #[test]
    fn shebang_is_removed() {
        assert_eq!(
            transpile_source("#!/usr/bin/env engine\nconsole.log(1);", "a.js").unwrap(),
            "console.log(1);"
        );
    }

    #[test]
    fn cache_key_depends_on_mode_and_content() {
        let js = cache_key("let x = 1;", "a.js", SourceKind::JavaScript);
        let ts = cache_key("let x: number = 1;", "a.ts", SourceKind::TypeScript);
        assert_ne!(js, ts);
    }
}
