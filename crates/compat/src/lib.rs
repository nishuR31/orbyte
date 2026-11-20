//! Embedded compatibility runtime for programs outside the native subset.
//!
//! The native Cranelift pipeline remains the primary execution path. When the
//! native frontend rejects a program (or the CLI is asked for a Node/Bun-shaped
//! command), this crate:
//!
//! 1. erases TypeScript and JSX with SWC (`.ts`, `.mts`, `.cts`, `.tsx`,
//!    `.jsx`) and caches the result on disk,
//! 2. executes the result with the Boa ECMAScript engine, and
//! 3. supplies the host surface a CLI program expects: `process`, `console`,
//!    timers, `performance`, minimal CommonJS, relative ES modules, and the
//!    `bun:test` harness.
//!
//! This is an execution fallback, not part of the native pipeline. It exists so
//! the engine can run complete JavaScript/TypeScript programs while the 3.0
//! native runtime described in `docs/architecture/redesign-v3.md` is built out.
//!
//! The fallback is deliberately self-contained: SWC's helper functions are
//! inlined by enabling `swc_ecma_transforms_base/inline-helpers`, so transpiled
//! output never imports `@swc/helpers`. Relative file imports are resolved from
//! the importing module, and imported TypeScript/JSX is transpiled with the
//! same pipeline as the entry file. Bare specifiers (packages, `node:` builtins)
//! are rejected with a clear error instead of a panic.

mod builtins;
mod commonjs;
mod harness;
mod inspect;
mod resolve;
mod runtime;
mod transpile;

use std::fmt;
use std::path::PathBuf;

pub use harness::TestResult;
pub use runtime::Repl;
pub use transpile::transpile_source;

/// Which module system an input should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputType {
    /// Detect from syntax and file extension (Node/Bun default).
    #[default]
    Auto,
    /// Force ES-module semantics.
    Module,
    /// Force CommonJS semantics.
    CommonJs,
}

impl InputType {
    /// Parses the value of `--input-type`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "module" => Some(Self::Module),
            "commonjs" | "cjs" => Some(Self::CommonJs),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// Options for one compatibility-runtime execution.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Entry filename. Selects the parser mode and appears in diagnostics.
    pub filename: String,
    /// Working directory used to resolve the entry and relative imports.
    pub cwd: Option<PathBuf>,
    /// `process.argv` contents. The CLI passes the real command line.
    pub argv: Vec<String>,
    /// Module-system override from `--input-type`.
    pub input_type: InputType,
    /// Install the test harness and leave tests registered for the runner.
    pub test_mode: bool,
    /// Files evaluated before the entry, in order.
    pub preloads: Vec<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            filename: "<eval>".to_owned(),
            cwd: None,
            argv: vec!["engine".to_owned()],
            input_type: InputType::Auto,
            test_mode: false,
            preloads: Vec::new(),
        }
    }
}

/// A failure in the compatibility path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatError {
    /// SWC could not erase TypeScript/JSX from the source.
    Transpile(String),
    /// Boa failed to parse or evaluate the resulting program.
    Runtime(String),
    /// The program called `process.exit(code)`.
    Exit(i32),
}

impl fmt::Display for CompatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transpile(message) | Self::Runtime(message) => write!(f, "{message}"),
            Self::Exit(code) => write!(f, "process exited with code {code}"),
        }
    }
}

impl std::error::Error for CompatError {}

/// Transpiles (if needed) and executes `source`.
pub fn run(source: &str, filename: &str) -> Result<(), CompatError> {
    let options = RunOptions {
        filename: filename.to_owned(),
        ..RunOptions::default()
    };
    run_with_options(source, &options)
}

/// Transpiles (if needed) and executes `source` with explicit options.
pub fn run_with_options(source: &str, options: &RunOptions) -> Result<(), CompatError> {
    let code = transpile_source(source, &options.filename)?;
    runtime::execute(&code, options)
}

/// Runs `source` and captures everything written through `console`.
///
/// This is the testable form of [`run`]. Production output goes straight to
/// stdout; this helper buffers it on the current thread.
pub fn run_collect(source: &str, filename: &str) -> Result<String, CompatError> {
    let options = RunOptions {
        filename: filename.to_owned(),
        ..RunOptions::default()
    };
    run_collect_with_options(source, &options)
}

/// Capturing form of [`run_with_options`].
pub fn run_collect_with_options(
    source: &str,
    options: &RunOptions,
) -> Result<String, CompatError> {
    let code = transpile_source(source, &options.filename)?;
    let (result, output) = runtime::capture(|| runtime::execute(&code, options));
    result?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

/// Executes `source` and returns the formatted value of its final expression.
///
/// Module inputs have no completion value, matching Node's behaviour, and
/// return `Ok(None)`.
pub fn evaluate(source: &str, options: &RunOptions) -> Result<Option<String>, CompatError> {
    let code = transpile_source(source, &options.filename)?;
    runtime::evaluate(&code, options)
}

/// Alias for [`evaluate`], named for symmetry with `--print`.
pub fn run_value(source: &str, options: &RunOptions) -> Result<Option<String>, CompatError> {
    evaluate(source, options)
}

/// Parses `source` without executing it. Backs `--check` / `engine check`.
pub fn check(source: &str, filename: &str) -> Result<(), CompatError> {
    let code = transpile_source(source, filename)?;
    runtime::check(&code, filename)
}

/// Runs one test file and returns its rows plus captured console output.
pub fn run_tests_collect(
    source: &str,
    options: &RunOptions,
) -> Result<(Vec<TestResult>, String), CompatError> {
    let code = transpile_source(source, &options.filename)?;
    let mut test_options = options.clone();
    test_options.test_mode = true;
    let (result, output) = runtime::capture(|| runtime::run_tests(&code, &test_options));
    result.map(|results| (results, String::from_utf8_lossy(&output).into_owned()))
}

/// Runs one test file with the harness installed.
pub fn run_tests(source: &str, options: &RunOptions) -> Result<Vec<TestResult>, CompatError> {
    run_tests_collect(source, options).map(|(results, _)| results)
}

/// Returns whether `source` contains top-level `import`/`export` syntax.
///
/// The CLI uses this to skip a native compilation attempt that cannot succeed.
pub fn has_module_syntax(source: &str) -> bool {
    runtime::has_module_syntax(source)
}

/// Returns whether `source` needs the ES-module evaluator.
pub fn needs_module(source: &str, filename: &str) -> bool {
    runtime::needs_module(source, filename)
}

/// Trivial programs that the native pipeline is known to reject for reasons
/// that are expensive to discover: TypeScript/JSX, modules, BigInt literals,
/// private fields, async functions, and `require` calls.
///
/// The CLI calls this *before* compiling so it does not pay for a guaranteed
/// failure. It is a hint, never a rejection: returning `true` only means "try
/// the compatibility runtime directly".
pub fn should_skip_native(source: &str, filename: &str) -> bool {
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "ts" | "tsx" | "jsx" | "mts" | "cts") {
        return true;
    }
    if runtime::needs_module(source, filename) {
        return true;
    }
    let bytes = source.as_bytes();
    // BigInt literals and private fields are cheap textual signals.
    for (offset, character) in source.char_indices() {
        if character == '#' {
            if let Some(next) = source[offset + 1..].chars().next() {
                if next.is_alphanumeric() || next == '_' {
                    return true;
                }
            }
        }
        if character.is_ascii_digit() {
            // Skip digits that continue an identifier or the literal already
            // scanned at its first digit; this keeps the pass linear.
            if offset > 0 {
                let previous = bytes[offset - 1];
                if previous.is_ascii_alphanumeric() || previous == b'_' {
                    continue;
                }
            }
            let rest = &bytes[offset..];
            let end = rest
                .iter()
                .position(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_' && *byte != b'.')
                .unwrap_or(rest.len());
            if end > 0 && rest[end - 1] == b'n' {
                return true;
            }
        }
    }
    let keyword = |needle: &str| {
        source.match_indices(needle).any(|(index, _)| {
            let before = source[..index].chars().next_back();
            let after = source[index + needle.len()..].chars().next();
            let boundary_before = before.map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$');
            let boundary_after = after.map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$');
            boundary_before && boundary_after
        })
    };
    if keyword("async") || keyword("await") || keyword("require") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transpiled(source: &str, filename: &str) -> String {
        transpile_source(source, filename).expect("transpilation should succeed")
    }

    #[test]
    fn strips_type_annotations() {
        let output = transpiled(
            "let value: number = 1;\nexport const doubled: number = value * 2;",
            "sample.ts",
        );
        assert!(!output.contains(": number"), "{output}");
        assert!(output.contains("let value = 1;"), "{output}");
        assert!(output.contains("doubled"), "{output}");
    }

    #[test]
    fn transpiles_tsx_to_create_element() {
        let output = transpiled(
            "const element = <div className=\"box\">Hi</div>;",
            "sample.tsx",
        );
        assert!(output.contains("React.createElement(\"div\""), "{output}");
        assert!(output.contains("\"Hi\""), "{output}");
    }

    #[test]
    fn retries_typescript_as_tsx() {
        let output = transpiled("const element = <div>Hello</div>;", "sample.ts");
        assert!(output.contains("React.createElement"), "{output}");
    }

    #[test]
    fn erases_enums_and_namespaces() {
        let output = transpiled(
            "enum Color { Red, Blue }\nnamespace N { export const x = 1; }\nconsole.log(Color.Blue, N.x);",
            "sample.ts",
        );
        assert!(output.contains("var Color"), "{output}");
        assert!(!output.contains("namespace"), "{output}");
    }

    #[test]
    fn legacy_decorators_use_an_inline_helper() {
        let output = transpiled(
            "function log(target: any, key: string) {}\nclass C { @log greet() { return 1; } }\nconsole.log(new C().greet());",
            "sample.ts",
        );
        assert!(output.contains("function _ts_decorate"), "{output}");
        assert!(!output.contains("@swc/helpers"), "{output}");
    }

    #[test]
    fn executes_modules_and_drains_promise_jobs() {
        let output = run_collect(
            "export const value = 1;\nPromise.resolve().then(() => console.log(\"async\", value));",
            "sample.js",
        )
        .expect("execution should succeed");
        assert_eq!(output, "async 1\n");
    }

    #[test]
    fn console_uses_node_shaped_inspection() {
        let output = run_collect(
            "console.log({ a: 1, b: 'two' }, [1, [2, 3]], null, undefined, true);",
            "sample.js",
        )
        .expect("execution should succeed");
        assert_eq!(output, "{ a: 1, b: 'two' } [ 1, [ 2, 3 ] ] null undefined true\n");
    }

    #[test]
    fn executes_typescript() {
        let output = run_collect(
            "const add = (a: number, b: number): number => a + b;\nconsole.log(add(2, 3));",
            "sample.ts",
        )
        .expect("execution should succeed");
        assert_eq!(output, "5\n");
    }

    #[test]
    fn executes_jsx() {
        let output = run_collect(
            "const element = <div>Hello</div>;\nconsole.log(element.type, element.children[0]);",
            "sample.tsx",
        )
        .expect("execution should succeed");
        assert_eq!(output, "div Hello\n");
    }

    #[test]
    fn reports_runtime_errors_with_the_filename() {
        let error = run_collect("throw new Error(\"boom\");", "sample.js")
            .expect_err("the program should fail");
        let message = error.to_string();
        assert!(message.contains("sample.js"), "{message}");
        assert!(message.contains("boom"), "{message}");
    }

    #[test]
    fn executes_bigint() {
        let output = run_collect("console.log(12345678901234567890n + 1n);", "sample.js")
            .expect("execution should succeed");
        assert!(output.contains("12345678901234567891"), "{output}");
    }

    #[test]
    fn executes_private_class_fields() {
        let output = run_collect(
            "class Counter { #balance = 0; add(n) { this.#balance += n; return this.#balance; } }\nconsole.log(new Counter().add(3));",
            "sample.js",
        )
        .expect("execution should succeed");
        assert_eq!(output, "3\n");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("engine-compat-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch directory should be creatable");
        dir
    }

    fn run_file(path: &std::path::Path) -> Result<String, CompatError> {
        let source = std::fs::read_to_string(path).expect("test file should be readable");
        run_collect(&source, &path.to_string_lossy())
    }

    #[test]
    fn resolves_relative_imports() {
        let dir = scratch_dir("relative");
        std::fs::write(dir.join("dep.js"), "export const value = 41;").unwrap();
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import { value } from \"./dep.js\";\nconsole.log(value + 1);",
        )
        .unwrap();

        assert_eq!(run_file(&entry).expect("import should resolve"), "42\n");
    }

    #[test]
    fn resolves_imported_typescript() {
        let dir = scratch_dir("typescript-import");
        std::fs::write(dir.join("dep.ts"), "export const value: number = 21;").unwrap();
        let entry = dir.join("main.ts");
        std::fs::write(
            &entry,
            "import { value } from \"./dep.ts\";\nconst doubled: number = value * 2;\nconsole.log(doubled);",
        )
        .unwrap();

        assert_eq!(
            run_file(&entry).expect("TypeScript import should resolve"),
            "42\n"
        );
    }

    #[test]
    fn missing_import_is_a_clean_error() {
        let dir = scratch_dir("missing-import");
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import { nope } from \"./missing.js\";\nconsole.log(nope);",
        )
        .unwrap();

        let error = run_file(&entry).expect_err("the import should fail");
        let message = error.to_string();
        assert!(message.contains("missing.js"), "{message}");
        assert!(
            message.contains("cannot resolve module") || message.contains("Cannot find module"),
            "{message}"
        );
        assert!(!message.contains("(native)"), "{message}");
    }

    #[test]
    fn node_builtin_imports_work() {
        let dir = scratch_dir("builtin-import");
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import { readFileSync } from \"node:fs\";\nconsole.log(typeof readFileSync);",
        )
        .unwrap();

        assert_eq!(run_file(&entry).expect("node:fs import should resolve"), "function\n");
    }

    #[test]
    fn legacy_string_substr_is_available() {
        let output = run_collect(
            "console.log('abcdef'.substr(2, 3), 'abcdef'.substr(-2));",
            "sample.js",
        )
        .expect("substr polyfill should be installed");
        assert_eq!(output, "cde ef\n");
    }

    #[test]
    fn buffer_and_fs_promises_are_available() {
        let output = run_collect(
            "const { realpathSync } = require('fs');\nconst promises = require('node:fs/promises');\nconsole.log(typeof Buffer, typeof promises.readFile, typeof realpathSync.native);",
            "sample.js",
        )
        .expect("built-ins should be available");
        assert_eq!(output, "function function function\n");
    }

    #[test]
    fn unknown_node_builtins_get_a_universal_stub() {
        let output = run_collect(
            "const m = require('node:some-future-builtin');\nconst x = m.newApi();\nconsole.log(typeof m, typeof m.newApi, String(x), typeof new m.Thing());",
            "sample.js",
        )
        .expect("unknown built-ins should still be usable");
        assert_eq!(output, "function function  function\n");
    }

    #[test]
    fn node_globals_are_available() {
        let output = run_collect(
            "const params = new URLSearchParams('a=1');\nconsole.log(typeof URL, params.get('a'), escape('a b'), unescape('%41'));",
            "sample.js",
        )
        .expect("Node globals should be installed");
        assert_eq!(output, "function 1 a%20b A\n");
    }

    #[test]
    fn namespace_import_from_commonjs_exposes_runtime_exports() {
        let dir = scratch_dir("cjs-namespace");
        std::fs::write(
            dir.join("legacy.cjs"),
            "module.exports = { helper: () => 42 };",
        )
        .unwrap();
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import * as ns from \"./legacy.cjs\";\nconsole.log(typeof ns.helper, ns.helper());",
        )
        .unwrap();
        assert_eq!(
            run_file(&entry).expect("namespace import should expose the live exports"),
            "function 42\n"
        );
    }

    #[test]
    fn commonjs_default_key_is_not_emitted_as_a_named_binding() {
        let dir = scratch_dir("cjs-default-key");
        std::fs::write(
            dir.join("legacy.cjs"),
            "module.exports = { default: 1, helper: () => 2 };",
        )
        .unwrap();
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import legacy, * as ns from \"./legacy.cjs\";\nconsole.log(legacy.default, ns.helper());",
        )
        .unwrap();
        assert_eq!(
            run_file(&entry).expect("reserved export names must be skipped"),
            "1 2\n"
        );
    }

    #[test]
    fn resolves_tsconfig_path_alias_end_to_end() {
        let dir = scratch_dir("tsconfig-alias");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("src/value.ts"), "export const value: number = 42;").unwrap();
        let entry = dir.join("main.ts");
        std::fs::write(
            &entry,
            "import { value } from \"@/value\";\nconsole.log(value);",
        )
        .unwrap();
        assert_eq!(run_file(&entry).expect("alias should resolve"), "42\n");
    }

    #[test]
    fn commonjs_can_require_an_es_module() {
        let dir = scratch_dir("require-esm");
        std::fs::write(dir.join("modern.mjs"), "export const value = 42;\nexport default 'hi';")
            .unwrap();
        let entry = dir.join("main.cjs");
        std::fs::write(
            &entry,
            "const m = require(\"./modern.mjs\");\nconsole.log(m.value, m.default);",
        )
        .unwrap();
        assert_eq!(run_file(&entry).expect("require(esm) should work"), "42 hi\n");
    }

    #[test]
    fn empty_module_exports_are_synthesised_from_declarations() {
        let dir = scratch_dir("declaration-stub");
        std::fs::write(dir.join("widget.js"), "").unwrap();
        std::fs::write(
            dir.join("widget.d.ts"),
            "export declare function make(options?: { variable?: string }): unknown;\n",
        )
        .unwrap();
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import { make } from \"./widget.js\";\nconsole.log(typeof make, make({ variable: '--x' }).variable);",
        )
        .unwrap();
        assert_eq!(
            run_file(&entry).expect("declaration stub should provide exports"),
            "function --x\n"
        );
    }

    #[test]
    fn module_builtin_can_be_required_for_side_effects() {
        let output = run_collect(
            "const m = require(\"module\");\nconsole.log(typeof m.Module, typeof m.createRequire);",
            "sample.cjs",
        )
        .expect("node:module should resolve");
        assert_eq!(output, "function function\n");
    }

    #[test]
    fn style_imports_are_empty_modules() {
        let dir = scratch_dir("style-import");
        std::fs::write(dir.join("app.css"), "body { color: red; }").unwrap();
        let entry = dir.join("main.js");
        std::fs::write(
            &entry,
            "import styles from \"./app.css\";\nconsole.log(typeof styles);",
        )
        .unwrap();
        assert_eq!(run_file(&entry).expect("CSS import should load"), "object\n");
    }

    #[test]
    fn reports_unhandled_promise_rejections() {
        let error = run_collect("Promise.reject(new Error(\"nope\"));", "sample.js")
            .expect_err("an unhandled rejection should fail the run");
        let message = error.to_string();
        assert!(message.contains("sample.js"), "{message}");
        assert!(message.contains("nope"), "{message}");
    }

    #[test]
    fn handled_promise_rejections_do_not_fail() {
        let output = run_collect(
            "Promise.reject(new Error(\"nope\")).catch((error) => console.log(\"caught\", error.message));",
            "sample.js",
        )
        .expect("a handled rejection should succeed");
        assert_eq!(output, "caught nope\n");
    }

    #[test]
    fn process_exit_returns_the_exit_code() {
        let options = RunOptions {
            filename: "exit.js".to_owned(),
            ..RunOptions::default()
        };
        let error = run_with_options("process.exit(7);", &options)
            .expect_err("process.exit should end the run");
        assert_eq!(error, CompatError::Exit(7));
    }

    #[test]
    fn commonjs_require_and_cache_work() {
        let dir = scratch_dir("commonjs");
        std::fs::write(
            dir.join("counter.cjs"),
            "let count = 0;\nmodule.exports = { next: () => ++count };",
        )
        .unwrap();
        let entry = dir.join("main.cjs");
        std::fs::write(
            &entry,
            "const a = require(\"./counter.cjs\");\nconst b = require(\"./counter.cjs\");\nconsole.log(a.next(), b.next(), __dirname.endsWith(\"commonjs\"));",
        )
        .unwrap();

        assert_eq!(
            run_file(&entry).expect("CommonJS entry should run"),
            "1 2 true\n"
        );
    }

    #[test]
    fn require_resolves_builtins() {
        let output = run_collect(
            "const path = require(\"node:path\");\nconsole.log(path.join(\"a\", \"b\"));",
            "sample.cjs",
        )
        .expect("node:path should resolve through require");
        assert_eq!(output, "a/b\n");
    }

    #[test]
    fn skip_native_detects_expensive_rejections() {
        assert!(should_skip_native("let x: number = 1;", "a.ts"));
        assert!(should_skip_native("import x from './x.js';", "a.js"));
        assert!(should_skip_native("const x = 10n;", "a.js"));
        assert!(should_skip_native("class A { #x = 1; }", "a.js"));
        assert!(should_skip_native("async function f() {}", "a.js"));
        assert!(!should_skip_native("let x = 1; console.log(x);", "a.js"));
    }

    #[test]
    fn evaluates_expression_values() {
        let options = RunOptions {
            filename: "eval.js".to_owned(),
            ..RunOptions::default()
        };
        assert_eq!(
            evaluate("1 + 2", &options).expect("evaluation should succeed"),
            Some("3".to_owned())
        );
    }
}
