//! Context construction, global prelude, promise/timer draining, and the
//! ES-module file loader.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use boa_engine::builtins::promise::{OperationType, Promise, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::module::{ModuleLoader, ModuleRequest, Referrer};
use boa_engine::object::JsObject;
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::property::PropertyKey;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsResult, JsString, JsValue, Module, Source,
};
use boa_engine::script::Script;

use crate::builtins;
use crate::commonjs;
use crate::harness;
use crate::inspect;
use crate::resolve::{self, Condition, Resolved};
use crate::transpile::{SourceKind, source_kind, transpile_source};
use crate::{CompatError, InputType, RunOptions};

thread_local! {
    static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static EXIT_CODE: Cell<Option<i32>> = const { Cell::new(None) };
}

/// Runs `f` while intercepting everything written through `console` and
/// `process.stdout` / `process.stderr`.
pub(crate) fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<u8>) {
    let previous = CAPTURE.with(|capture| capture.borrow_mut().replace(Vec::new()));
    let result = f();
    let output = CAPTURE
        .with(|capture| capture.borrow_mut().take())
        .unwrap_or_default();
    CAPTURE.with(|capture| *capture.borrow_mut() = previous);
    (result, output)
}

pub(crate) fn take_exit_code() -> Option<i32> {
    EXIT_CODE.with(|code| code.take())
}

fn emit_bytes(text: &[u8]) {
    emit_bytes_to(1, text);
}

fn emit_bytes_to(fd: u8, text: &[u8]) {
    let captured = CAPTURE.with(|capture| {
        let mut capture = capture.borrow_mut();
        if let Some(buffer) = capture.as_mut() {
            buffer.extend_from_slice(text);
            true
        } else {
            false
        }
    });

    if !captured {
        if fd == 2 {
            let stderr = io::stderr();
            let mut out = stderr.lock();
            let _ = out.write_all(text);
        } else {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            let _ = out.write_all(text);
        }
    }
}

fn emit(text: &str) {
    emit_bytes(text.as_bytes());
}

/// Executes one program in a fresh context.
pub(crate) fn execute(source: &str, options: &RunOptions) -> Result<(), CompatError> {
    let module_mode = module_mode(source, options);
    let mut runtime = Runtime::new(options)?;

    for preload in &options.preloads {
        runtime.preload(preload)?;
    }

    if module_mode {
        runtime.eval_module(source)?;
    } else {
        runtime.eval_commonjs(source)?;
    }

    runtime.finish()
}

/// Executes one program and returns the formatted result of its last
/// expression. Used by `--print`.
pub(crate) fn evaluate(source: &str, options: &RunOptions) -> Result<Option<String>, CompatError> {
    let module_mode = module_mode(source, options);
    let mut runtime = Runtime::new(options)?;

    for preload in &options.preloads {
        runtime.preload(preload)?;
    }

    let value = if module_mode {
        runtime.eval_module(source)?;
        None
    } else {
        Some(runtime.eval_script(source)?)
    };
    runtime.finish()?;

    Ok(value.and_then(|value| {
        if value.is_undefined() {
            None
        } else {
            Some(format_value(&value, &mut runtime.context))
        }
    }))
}

/// Parses without executing. Used by `--check`.
pub(crate) fn check(code: &str, filename: &str) -> Result<(), CompatError> {
    let source = Source::from_reader(code.as_bytes(), Some(Path::new(filename)));
    let mut context = Context::default();
    if needs_module(code, filename) {
        Module::parse(source, None, &mut context)
            .map(|_| ())
            .map_err(|error| runtime_error(filename, &error))
    } else {
        Script::parse(source, None, &mut context)
            .map(|_| ())
            .map_err(|error| runtime_error(filename, &error))
    }
}

/// Executes one test file, runs the registered tests, and returns the results.
pub(crate) fn run_tests(
    source: &str,
    options: &RunOptions,
) -> Result<Vec<harness::TestResult>, CompatError> {
    let module_mode = module_mode(source, options);
    let mut runtime = Runtime::new(options)?;

    for preload in &options.preloads {
        runtime.preload(preload)?;
    }

    if module_mode {
        runtime.eval_module(source)?;
    } else {
        runtime.eval_commonjs(source)?;
    }

    runtime.start_test_runner()?;
    let results = harness::extract_results(&mut runtime.context)
        .map_err(|error| runtime.error(error))?;
    runtime.finish()?;
    Ok(results)
}

pub(crate) fn module_mode(source: &str, options: &RunOptions) -> bool {
    match options.input_type {
        InputType::Module => true,
        InputType::CommonJs => false,
        InputType::Auto => needs_module(source, &options.filename),
    }
}

/// A reusable execution context for the REPL.
pub struct Repl {
    runtime: Runtime,
}

impl Repl {
    pub fn new(cwd: PathBuf) -> Result<Self, CompatError> {
        let options = RunOptions {
            filename: cwd.join("__engine_repl__.js").to_string_lossy().into_owned(),
            cwd: Some(cwd),
            argv: vec!["engine".to_owned(), "repl".to_owned()],
            ..RunOptions::default()
        };
        Ok(Self {
            runtime: Runtime::new(&options)?,
        })
    }

    pub fn evaluate(&mut self, source: &str) -> Result<Option<String>, CompatError> {
        let value = self.runtime.eval_script(source)?;
        self.runtime.finish()?;
        Ok(if value.is_undefined() {
            None
        } else {
            Some(format_value(&value, &mut self.runtime.context))
        })
    }
}

struct Runtime {
    context: Context,
    tracker: Rc<RejectionTracker>,
    loader: Rc<FileModuleLoader>,
    filename: String,
    entry: PathBuf,
}

impl Runtime {
    fn new(options: &RunOptions) -> Result<Self, CompatError> {
        EXIT_CODE.with(|code| code.set(None));
        let cwd = options
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let entry = absolute_path(&cwd, &options.filename);
        let loader = Rc::new(FileModuleLoader::new(cwd.clone()));
        let tracker = Rc::new(RejectionTracker::default());

        let builder = Context::builder()
            .host_hooks(Rc::new(RejectionHooks {
                tracker: tracker.clone(),
            }))
            .module_loader(loader.clone());
        let mut context = builder
            .build()
            .map_err(|error| runtime_error(&options.filename, &error))?;
        // Boa's defaults (10,240 VM stack slots, 512 nested calls) are tuned for
        // small scripts. Real applications with large module graphs and
        // declaration-only packages need more headroom.
        let mut limits = context.runtime_limits();
        limits.set_stack_size_limit(1024 * 1024);
        limits.set_recursion_limit(4096);
        context.set_runtime_limits(limits);
        install_prelude(&mut context, options, &cwd, &entry)
            .map_err(|error| runtime_error(&options.filename, &error))?;
        builtins::install(&mut context)
            .map_err(|error| runtime_error(&options.filename, &error))?;
        if options.test_mode {
            harness::install(&mut context)
                .map_err(|error| runtime_error(&options.filename, &error))?;
        }

        Ok(Self {
            context,
            tracker,
            loader,
            filename: options.filename.clone(),
            entry,
        })
    }

    fn preload(&mut self, preload: &str) -> Result<(), CompatError> {
        let path = if Path::new(preload).is_absolute() {
            PathBuf::from(preload)
        } else {
            self.loader.base_dir.join(preload)
        };
        let source = std::fs::read_to_string(&path).map_err(|error| {
            CompatError::Runtime(format!(
                "{}: cannot preload '{}': {error}",
                self.filename,
                path.display()
            ))
        })?;
        let code = transpile_source(&source, &path.to_string_lossy())?;
        if needs_module(&code, &path.to_string_lossy()) {
            self.eval_module_with_path(&code, Some(&path))
        } else {
            commonjs::execute_file(&mut self.context, &code, &path)
                .map_err(|error| self.error(error))
        }
    }

    fn eval_script(&mut self, code: &str) -> Result<JsValue, CompatError> {
        let source = Source::from_reader(code.as_bytes(), Some(&self.entry));
        self.context
            .eval(source)
            .map_err(|error| self.error(error))
    }

    fn eval_module(&mut self, code: &str) -> Result<(), CompatError> {
        let entry = if self.entry.is_file() {
            Some(self.entry.clone())
        } else {
            None
        };
        self.eval_module_with_path(code, entry.as_deref())
    }

    fn eval_module_with_path(
        &mut self,
        code: &str,
        path: Option<&Path>,
    ) -> Result<(), CompatError> {
        // Always give the parser a path, even for `--eval`: imports in an
        // evaluated module resolve relative to the synthetic entry file's
        // directory, matching Node/Bun.
        let source = Source::from_reader(code.as_bytes(), Some(path.unwrap_or(&self.entry)));
        let parsed = Module::parse(source, None, &mut self.context)
            .map_err(|error| self.error(error))?;

        if let Some(path) = path {
            self.loader.insert(path.to_path_buf(), parsed.clone());
        } else if self.entry.is_file() {
            self.loader.insert(self.entry.clone(), parsed.clone());
        }

        let promise = parsed.load_link_evaluate(&mut self.context);
        self.settle()?;

        if let PromiseState::Rejected(value) = promise.state() {
            return Err(CompatError::Runtime(format!(
                "{}: Uncaught {}",
                self.filename,
                strip_native_marker(&format_value(&value, &mut self.context))
            )));
        }
        Ok(())
    }

    fn eval_commonjs(&mut self, code: &str) -> Result<(), CompatError> {
        commonjs::execute_entry(&mut self.context, code, &self.entry)
            .map_err(|error| self.error(error))
    }

    fn finish(&mut self) -> Result<(), CompatError> {
        self.settle()?;
        if let Some(reason) = self.tracker.first_unhandled() {
            let reason = format_value(&reason, &mut self.context);
            return Err(CompatError::Runtime(format!(
                "{}: Uncaught {reason}",
                self.filename
            )));
        }
        if let Some(code) = take_exit_code() {
            return Err(CompatError::Exit(code));
        }
        if let Some(code) = self.process_exit_code() {
            if code != 0 {
                return Err(CompatError::Exit(code));
            }
        }
        Ok(())
    }

    /// Invokes the JavaScript harness runner and waits for every async test.
    fn start_test_runner(&mut self) -> Result<(), CompatError> {
        let runner = self
            .context
            .global_object()
            .get(js_string!("__engine_run_tests"), &mut self.context)
            .map_err(|error| self.error(error))?;
        let Some(callable) = runner.as_callable() else {
            return Err(CompatError::Runtime(format!(
                "{}: the test harness is not installed",
                self.filename
            )));
        };
        callable
            .call(&JsValue::undefined(), &[], &mut self.context)
            .map_err(|error| self.error(error))?;
        self.settle()
    }

    fn process_exit_code(&mut self) -> Option<i32> {
        let process = self
            .context
            .global_object()
            .get(js_string!("process"), &mut self.context)
            .ok()?;
        let process = process.as_object()?;
        let code = process.get(js_string!("exitCode"), &mut self.context).ok()?;
        code.as_number().map(|value| value as i32)
    }

    /// Drains promise jobs and timers until neither can make progress.
    fn settle(&mut self) -> Result<(), CompatError> {
        let mut fired = 0usize;
        loop {
            self.context.run_jobs().map_err(|error| self.error(error))?;

            let delay = self.call_timer_next()?;
            if delay < 0.0 {
                break;
            }
            if delay > 0.0 {
                // Sleep in bounded chunks so a long timer does not make the
                // process unresponsive to signals.
                std::thread::sleep(Duration::from_millis((delay as u64).min(50)));
                continue;
            }
            if !self.call_timer_fire()? {
                break;
            }
            fired += 1;
            if fired > 100_000 {
                return Err(CompatError::Runtime(format!(
                    "{}: timers did not settle after {fired} callbacks",
                    self.filename
                )));
            }
        }
        Ok(())
    }

    fn call_timer_next(&mut self) -> Result<f64, CompatError> {
        let function = self
            .context
            .global_object()
            .get(js_string!("__engine_timer_next"), &mut self.context)
            .ok()
            .filter(JsValue::is_callable);
        let Some(function) = function else {
            return Ok(-1.0);
        };
        let value = function
            .as_callable()
            .expect("checked callable")
            .call(&JsValue::undefined(), &[], &mut self.context)
            .map_err(|error| self.error(error))?;
        Ok(value.as_number().unwrap_or(-1.0))
    }

    fn call_timer_fire(&mut self) -> Result<bool, CompatError> {
        let function = self
            .context
            .global_object()
            .get(js_string!("__engine_timer_fire"), &mut self.context)
            .ok()
            .filter(JsValue::is_callable);
        let Some(function) = function else {
            return Ok(false);
        };
        let value = function
            .as_callable()
            .expect("checked callable")
            .call(&JsValue::undefined(), &[], &mut self.context)
            .map_err(|error| self.error(error))?;
        Ok(value.as_boolean().unwrap_or(false))
    }

    fn error(&self, error: JsError) -> CompatError {
        if let Some(code) = take_exit_code() {
            return CompatError::Exit(code);
        }
        runtime_error(&self.filename, &error)
    }
}

fn absolute_path(cwd: &Path, filename: &str) -> PathBuf {
    let path = Path::new(filename);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn runtime_error(filename: &str, error: &JsError) -> CompatError {
    CompatError::Runtime(format!(
        "{filename}: {}",
        strip_native_marker(&error.to_string())
    ))
}

/// Boa marks errors created without source information with a trailing
/// `(native)`. That is Boa's internal marker, not this engine's native
/// pipeline, so hide it from users.
fn strip_native_marker(message: &str) -> String {
    message
        .trim_end()
        .strip_suffix(" (native)")
        .unwrap_or_else(|| message.trim_end())
        .to_owned()
}

/// Formats strings the way a console does (unquoted) and falls back to Boa's
/// inspector for every other value.
pub(crate) fn format_value(value: &JsValue, context: &mut Context) -> String {
    inspect::format_value(value, context)
}

#[derive(Default)]
struct RejectionTracker {
    unhandled: RefCell<Vec<(JsObject<Promise>, JsValue)>>,
}

impl RejectionTracker {
    fn first_unhandled(&self) -> Option<JsValue> {
        self.unhandled
            .borrow()
            .first()
            .map(|(_, reason)| reason.clone())
    }
}

struct RejectionHooks {
    tracker: Rc<RejectionTracker>,
}

impl HostHooks for RejectionHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject<Promise>,
        operation: OperationType,
        _context: &mut Context,
    ) {
        let mut unhandled = self.tracker.unhandled.borrow_mut();
        match operation {
            OperationType::Reject => {
                if let PromiseState::Rejected(reason) = JsPromise::from(promise.clone()).state() {
                    if !unhandled.iter().any(|(entry, _)| entry == promise) {
                        unhandled.push((promise.clone(), reason));
                    }
                }
            }
            OperationType::Handle => {
                unhandled.retain(|(entry, _)| entry != promise);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ES module loading
// ---------------------------------------------------------------------------

/// Loads imported files relative to the importing module.
///
/// Delegates specifier resolution to [`crate::resolve`], which handles relative
/// files, TypeScript `paths` aliases, bare packages in `node_modules` (including
/// subpaths and `exports` maps), and Node built-ins.
struct FileModuleLoader {
    base_dir: PathBuf,
    cache: RefCell<HashMap<PathBuf, Module>>,
}

impl FileModuleLoader {
    fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn insert(&self, path: PathBuf, module: Module) {
        self.cache.borrow_mut().insert(path, module);
    }

    fn load(
        &self,
        referrer: &Referrer,
        request: &ModuleRequest,
        context: &mut Context,
    ) -> JsResult<Module> {
        let specifier = request.specifier().to_std_string_escaped();
        if is_test_specifier(&specifier) {
            let key = PathBuf::from("engine:test");
            if let Some(module) = self.cache.borrow().get(&key).cloned() {
                return Ok(module);
            }
            let module = Module::parse(
                Source::from_bytes(TEST_MODULE_SOURCE),
                None,
                context,
            )?;
            self.insert(key, module.clone());
            return Ok(module);
        }

        let parent = referrer.path().unwrap_or(&self.base_dir);
        let resolved = resolve::resolve(parent, &specifier, Condition::Import).map_err(|error| {
            JsError::from(JsNativeError::typ().with_message(format!(
                "cannot resolve module `{specifier}`: {error}"
            )))
        })?;
        let path = match resolved {
            Resolved::Builtin(name) => return builtin_module(&name, context),
            Resolved::File(path) => path,
        };

        if let Some(module) = self.cache.borrow().get(&path).cloned() {
            return Ok(module);
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Bundlers turn style imports into empty side-effect modules. The
        // compatibility runtime does the same so an app entry can be loaded
        // without a CSS build step.
        if matches!(extension.as_str(), "css" | "scss" | "sass" | "less") {
            let module = Module::parse(Source::from_bytes(b"export default {};"), None, context)?;
            self.insert(path, module.clone());
            return Ok(module);
        }
        if extension == "json" {
            let is_json_import = request.attributes().iter().any(|attribute| {
                attribute.key().to_std_string_escaped() == "type"
                    && attribute.value().to_std_string_escaped() == "json"
            });
            if !is_json_import {
                return Err(JsNativeError::typ()
                    .with_message(format!(
                        "module `{specifier}` needs an import attribute of type \"json\""
                    ))
                    .into());
            }
            let json = std::fs::read_to_string(&path).map_err(|error| {
                JsError::from(
                    JsNativeError::typ()
                        .with_message(format!("cannot open module `{specifier}`: {error}")),
                )
            })?;
            let module = Module::parse_json(js_string!(json.as_str()), context)?;
            self.insert(path, module.clone());
            return Ok(module);
        }

        let text = std::fs::read_to_string(&path).map_err(|error| {
            JsError::from(
                JsNativeError::typ()
                    .with_message(format!("cannot open module `{specifier}`: {error}")),
            )
        })?;
        let path_text = path.to_string_lossy().into_owned();

        // Build-time-only packages can ship an empty JS entry whose exports
        // are declared in a sibling .d.ts. Synthesise the named exports so
        // importing programs can still be executed.
        if text.trim().is_empty() {
            if let Some(stub) = declaration_stub_module(&path) {
                let module = Module::parse(
                    Source::from_reader(stub.as_bytes(), Some(&path)),
                    None,
                    context,
                )?;
                self.insert(path, module.clone());
                return Ok(module);
            }
        }

        // CommonJS interop: `import value from "./legacy.cjs"` (and `.cts`,
        // plus `.js` files without ESM syntax) loads through `require` and
        // exposes the result as the default export.
        let kind = source_kind(&path_text);
        if matches!(extension.as_str(), "cjs" | "cts")
            || (kind == SourceKind::JavaScript && !needs_module(&text, &path_text))
        {
            let exported = commonjs::require_path(context, &path).map_err(|error| {
                JsError::from(JsNativeError::typ().with_message(format!(
                    "cannot import CommonJS module `{specifier}`: {error}"
                )))
            })?;
            let quoted = serde_json::to_string(&path_text)
                .unwrap_or_else(|_| format!("{path_text:?}"));
            // Node exposes named imports from CommonJS by statically finding
            // `exports.name = ...` assignments, but a module can also assign
            // `module.exports = require(...)` whose keys are only known after
            // evaluation. Enumerate the live object as well so namespace and
            // named imports work for packages like `react`.
            let mut names = cjs_named_exports(&text);
            if let Some(object) = exported.as_object() {
                for key in object.own_property_keys(context)? {
                    if let PropertyKey::String(name) = key {
                        let name = name.to_std_string_escaped();
                        if is_export_identifier(&name) && !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
            let mut wrapper = format!(
                "const __engine_cjs = globalThis.__engine_module_cache[{quoted}].exports;\nexport default __engine_cjs;\n"
            );
            for name in names {
                wrapper.push_str(&format!("export const {name} = __engine_cjs.{name};\n"));
            }
            let module = Module::parse(
                Source::from_reader(wrapper.as_bytes(), Some(&path)),
                None,
                context,
            )?;
            self.insert(path, module.clone());
            return Ok(module);
        }

        let code = transpile_source(&text, &path_text).map_err(|error| {
            JsError::from(JsNativeError::syntax().with_message(error.to_string()))
        })?;
        let module = Module::parse(
            Source::from_reader(code.as_bytes(), Some(&path)),
            None,
            context,
        )?;
        self.insert(path, module.clone());
        Ok(module)
    }
}

impl ModuleLoader for FileModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = self.load(&referrer, &request, &mut context.borrow_mut());
        std::future::ready(result)
    }

    fn init_import_meta(
        self: Rc<Self>,
        import_meta: &JsObject,
        module: &Module,
        context: &mut Context,
    ) {
        let url = module.path().map(|path| {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.base_dir.join(path)
            };
            let text = absolute.to_string_lossy().replace('\\', "/");
            if let Some(share) = text.strip_prefix("//") {
                // UNC: \\server\share -> file://server/share
                format!("file://{share}")
            } else {
                format!("file:///{}", text.trim_start_matches('/'))
            }
        });
        if let Some(url) = url {
            let _ = import_meta.set(
                js_string!("url"),
                JsString::from(url.as_str()),
                false,
                context,
            );
        }
    }
}

/// Builds a stub module for an empty JS entry whose public names exist only in
/// an adjacent `.d.ts` (for example `next/font/google`, whose font loaders are
/// generated by Next's build-time transform). Following `export * from` keeps
/// the stub tied to the package's own declarations instead of a hardcoded
/// list of font names.
fn declaration_stub_module(path: &Path) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut queue = vec![path.with_extension("d.ts")];

    for _ in 0..8 {
        let Some(declaration) = queue.pop() else {
            break;
        };
        if !seen.insert(declaration.clone()) {
            continue;
        }
        let text = std::fs::read_to_string(&declaration).ok()?;
        for line in text.lines() {
            let line = line.trim();
            for prefix in [
                "export declare function ",
                "export declare const ",
                "export declare class ",
                "export declare let ",
            ] {
                if let Some(rest) = line.strip_prefix(prefix) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                        .collect();
                    if !name.is_empty() && !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            if let Some(rest) = line.strip_prefix("export * from ") {
                let quote = rest.find(['\'', '"'])?;
                let end = rest[quote + 1..].find(['\'', '"'])? + quote + 1;
                let specifier = &rest[quote + 1..end];
                let target = if resolve::is_relative(specifier) {
                    declaration.parent().unwrap_or(Path::new(".")).join(specifier)
                } else {
                    match resolve::resolve(&declaration, specifier, resolve::Condition::Import) {
                        Ok(Resolved::File(file)) => file,
                        _ => continue,
                    }
                };
                queue.push(target.with_extension("d.ts"));
            }
        }
    }

    if names.is_empty() {
        return None;
    }
    let mut source = String::from("export default {};\n");
    for name in names {
        source.push_str(&format!(
            "export const {name} = (options) => ({{ className: \"\", style: {{}}, variable: options && options.variable }});\n"
        ));
    }
    Some(source)
}

/// Synthetic ES module that re-exports a built-in object as named exports plus
/// a default export, so both `import { join } from "path"` and
/// `import path from "path"` work.
fn builtin_module(name: &str, context: &mut Context) -> JsResult<Module> {
    let value = builtins::get(context, name)?;
    context.register_global_property(
        js_string!("__engine_builtin_value"),
        value.clone(),
        boa_engine::property::Attribute::all(),
    )?;
    // Derive named exports from the shim's live keys. This avoids maintaining a
    // separate list per built-in and keeps ESM `import { x } from "node:..."`
    // in sync with whatever the shim actually provides.
    let mut names: Vec<String> = Vec::new();
    if let Some(object) = value.as_object() {
        for key in object.own_property_keys(context)? {
            if let PropertyKey::String(name) = key {
                let name = name.to_std_string_escaped();
                if is_export_identifier(&name) && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    for export in builtins::exports_for(name) {
        if is_export_identifier(export) && !names.iter().any(|existing| existing == export) {
            names.push((*export).to_owned());
        }
    }
    let mut source = String::from(
        "const __engine_builtin = globalThis.__engine_builtin_value;\nexport default __engine_builtin;\n",
    );
    for export in names {
        source.push_str(&format!(
            "export const {export} = __engine_builtin.{export};\n"
        ));
    }
    Module::parse(Source::from_bytes(source.as_bytes()), None, context)
}

/// Whether a CommonJS export name can be emitted as `export const <name>`.
/// Reserved words such as `default` must be skipped: they are valid object
/// keys but cannot be ESM binding identifiers.
fn is_export_identifier(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "await", "break", "case", "catch", "class", "const", "continue", "debugger",
        "default", "delete", "do", "else", "enum", "export", "extends", "false",
        "finally", "for", "function", "if", "implements", "import", "in", "instanceof",
        "interface", "let", "new", "null", "package", "private", "protected", "public",
        "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof",
        "var", "void", "while", "with", "yield",
    ];
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !RESERVED.contains(&name)
}

/// Finds statically assignable `exports.name` / `module.exports.name` names in
/// CommonJS source. This mirrors the small part of Node's `cjs-module-lexer`
/// that a compatibility runtime needs for named ESM imports.
fn cjs_named_exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |name: &str| {
        if !is_export_identifier(name) {
            return;
        }
        if !names.iter().any(|existing: &String| existing == name) {
            names.push(name.to_owned());
        }
    };
    for needle in ["exports.", "module.exports."] {
        let mut offset = 0;
        while let Some(index) = source[offset..].find(needle) {
            let start = offset + index + needle.len();
            let rest = &source[start..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
                .unwrap_or(rest.len());
            push(&rest[..end]);
            offset = start + end.max(1);
        }
    }
    for needle in ["Object.defineProperty(exports, \"", "Object.defineProperty(exports, '"] {
        let mut offset = 0;
        while let Some(index) = source[offset..].find(needle) {
            let start = offset + index + needle.len();
            let rest = &source[start..];
            let end = rest.find(['\"', '\'']).unwrap_or(rest.len());
            push(&rest[..end]);
            offset = start + end.max(1);
        }
    }
    names
}

fn is_test_specifier(specifier: &str) -> bool {
    matches!(specifier, "bun:test" | "node:test")
}

const TEST_MODULE_SOURCE: &str = r#"
const api = globalThis.__engine_test_api;
if (!api) {
    throw new Error("bun:test is only available when running `engine test`");
}
export const test = api.test;
export const it = api.it;
export const describe = api.describe;
export const expect = api.expect;
export const assert = api.assert;
export const beforeEach = api.beforeEach;
export const afterEach = api.afterEach;
export const beforeAll = api.beforeAll;
export const afterAll = api.afterAll;
export default api.test;
"#;

/// Detects whether the program needs ES-module semantics. Boa's module
/// evaluator is required for top-level `import`/`export`, and `.mjs`/`.mts`
/// files are modules by definition (including top-level `await`).
pub(crate) fn needs_module(source: &str, filename: &str) -> bool {
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "mjs" | "mts") {
        return true;
    }
    // The line-based `has_module_syntax` is only a heuristic and can miss real
    // declarations (or see them inside comments). Use the SWC AST so module
    // detection is exact; plain JS files are cheap to parse.
    crate::transpile::contains_module_decl(source, filename)
}

/// Detects whether a source file contains top-level `import`/`export` syntax.
/// Used both for module selection and as a fast path that skips a native
/// compilation attempt which cannot succeed.
pub fn has_module_syntax(source: &str) -> bool {
    masked_source(source).lines().any(|line| {
        let trimmed = line.trim_start();
        is_module_keyword(trimmed, "import") || is_module_keyword(trimmed, "export")
    })
}

/// Blanks out comments and string/template contents while preserving line
/// structure, so the line-based module heuristic cannot mistake documentation
/// examples such as `import ...` inside a block comment for real ESM syntax.
fn masked_source(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut line_comment = false;
    let mut block_comment = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if line_comment {
            if character == '\n' {
                line_comment = false;
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if block_comment {
            if character == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
                out.push_str("  ");
            } else if character == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                out.push(' ');
            } else if character == '\\' {
                escaped = true;
                out.push(' ');
            } else if character == active {
                quote = None;
                out.push(' ');
            } else if character == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        match character {
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                line_comment = true;
                out.push_str("  ");
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                block_comment = true;
                out.push_str("  ");
            }
            '"' | '\'' | '`' => {
                quote = Some(character);
                out.push(' ');
            }
            _ => out.push(character),
        }
    }
    out
}

fn is_module_keyword(line: &str, keyword: &str) -> bool {
    line.strip_prefix(keyword).map_or(false, |rest| {
        rest.is_empty()
            || rest.starts_with('{')
            || rest.starts_with('*')
            || rest.starts_with('"')
            || rest.starts_with('\'')
            || rest
                .chars()
                .next()
                .map_or(false, |character| character.is_whitespace())
    })
}

// ---------------------------------------------------------------------------
// Global prelude
// ---------------------------------------------------------------------------

const PRELUDE: &str = r#"
globalThis.global = globalThis;

globalThis.console = {
    log: (...args) => __engine_print(...args),
    info: (...args) => __engine_print(...args),
    debug: (...args) => __engine_print(...args),
    warn: (...args) => __engine_print(...args),
    error: (...args) => __engine_print(...args),
    trace: (...args) => __engine_print(...args),
    dir: (...args) => __engine_print(...args),
    table: (...args) => __engine_print(...args),
    assert: (value, ...args) => { if (!value) __engine_print("Assertion failed", ...args); },
    time: () => undefined,
    timeEnd: () => undefined,
};

globalThis.__engine_module_cache = Object.create(null);

globalThis.__engine_create_module = (filename) => ({
    id: filename,
    filename,
    exports: {},
    loaded: false,
    children: [],
    paths: [],
});

globalThis.__engine_make_require = (filename) => {
    const require = (specifier) => __engine_require(filename, specifier);
    require.resolve = (specifier) => __engine_resolve(filename, specifier);
    require.cache = globalThis.__engine_module_cache;
    require.extensions = { '.js': undefined, '.json': undefined, '.node': undefined };
    return require;
};

globalThis.process = {
    argv: __engine_argv,
    env: __engine_env,
    platform: __engine_platform,
    arch: __engine_arch,
    version: "v22.0.0",
    versions: { engine: "3.0.0", node: "22.0.0" },
    report: {
        getReport: () => ({
            header: {
                glibcVersionRuntime: "2.31",
                arch: __engine_arch,
                platform: __engine_platform,
                nodejsVersion: "v22.0.0",
            },
        }),
    },
    pid: __engine_pid,
    cwd: () => __engine_cwd(),
    chdir: (directory) => __engine_chdir(String(directory)),
    exit: (code = 0) => __engine_exit(Number(code) | 0),
    exitCode: 0,
    nextTick: (fn, ...args) => Promise.resolve().then(() => fn(...args)),
    hrtime: Object.assign((previous) => {
        const total = Date.now() / 1000;
        const seconds = Math.floor(total);
        const nanos = Math.floor((total - seconds) * 1e9);
        if (previous) {
            let ds = seconds - previous[0];
            let dn = nanos - previous[1];
            if (dn < 0) { ds -= 1; dn += 1e9; }
            return [ds, dn];
        }
        return [seconds, nanos];
    }, { bigint: () => BigInt(Math.round(Date.now() * 1e6)) }),
    stdout: {
        write: (text) => { __engine_write(1, String(text)); return true; },
        isTTY: false,
    },
    stderr: {
        write: (text) => { __engine_write(2, String(text)); return true; },
        isTTY: false,
    },
    on: () => process,
    once: () => process,
    off: () => process,
    emit: () => false,
    kill: () => undefined,
};

globalThis.queueMicrotask = (fn) => Promise.resolve().then(fn);
globalThis.setImmediate = (fn, ...args) => setTimeout(fn, 0, ...args);
globalThis.clearImmediate = (id) => clearTimeout(id);

globalThis.__engine_timer_id = 0;
globalThis.__engine_timers = [];
globalThis.setTimeout = (fn, delay = 0, ...args) => {
    const id = ++__engine_timer_id;
    __engine_timers.push({
        id,
        fn,
        args,
        due: Date.now() + Number(delay || 0),
        interval: null,
        cancelled: false,
    });
    return id;
};
globalThis.setInterval = (fn, delay = 0, ...args) => {
    const id = ++__engine_timer_id;
    const interval = Math.max(1, Number(delay || 0));
    __engine_timers.push({
        id,
        fn,
        args,
        due: Date.now() + interval,
        interval,
        cancelled: false,
    });
    return id;
};
globalThis.clearTimeout = (id) => {
    for (const timer of __engine_timers) {
        if (timer.id === id) timer.cancelled = true;
    }
};
globalThis.clearInterval = globalThis.clearTimeout;
globalThis.__engine_timer_next = () => {
    const now = Date.now();
    let next = -1;
    for (const timer of __engine_timers) {
        if (timer.cancelled) continue;
        const delay = timer.due - now;
        if (next < 0 || delay < next) next = delay;
    }
    return next;
};
globalThis.__engine_timer_fire = () => {
    const now = Date.now();
    const index = __engine_timers.findIndex((timer) => {
        return !timer.cancelled && timer.due <= now;
    });
    if (index < 0) return false;
    const [timer] = __engine_timers.splice(index, 1);
    if (timer.interval !== null) {
        timer.due = now + timer.interval;
        __engine_timers.push(timer);
    }
    timer.fn(...timer.args);
    return true;
};

globalThis.performance = globalThis.performance || {
    now: () => Date.now() - __engine_started_at,
};

globalThis.document = {
    querySelector: () => undefined,
    querySelectorAll: () => [],
    addEventListener: () => undefined,
    createElement: (tag) => ({ tagName: tag, style: {}, appendChild() {}, setAttribute() {} }),
};
globalThis.React = {
    createElement: (tag, props, ...children) => ({ type: tag, props, children }),
};

if (typeof globalThis.structuredClone !== "function") {
    globalThis.structuredClone = (value) => JSON.parse(JSON.stringify(value));
}

// Boa does not implement the legacy (but widely used) String#substr.
if (typeof String.prototype.substr !== "function") {
    String.prototype.substr = function (start, length) {
        const text = String(this);
        const size = text.length;
        let from = Math.trunc(Number(start) || 0);
        if (from < 0) from = Math.max(size + from, 0);
        else from = Math.min(from, size);
        const end = length === undefined
            ? size
            : Math.min(size, from + Math.max(0, Math.trunc(Number(length) || 0)));
        return text.slice(from, end);
    };
}

// Config files often use `require()` even when they are loaded as ES modules
// (tailwind.config.ts, for example). Provide a global require resolved from the
// entry file so those configs can be executed.
if (typeof globalThis.require !== "function") {
    globalThis.require = (specifier) => __engine_require(__engine_entry, String(specifier));
}

// V8 stack-trace API used by tooling such as typescript-eslint.
if (typeof Error.captureStackTrace !== "function") {
    Error.stackTraceLimit = 10;
    Error.prepareStackTrace = undefined;
    Error.captureStackTrace = (target) => {
        const stack = String(new Error().stack || "");
        const frames = stack.split("\n").slice(1).map((line) => {
            const match = line.match(/\(?([^()\s]+):\d+:\d+\)?$/);
            return {
                getFileName: () => (match ? match[1] : null),
                getLineNumber: () => 0,
                getColumnNumber: () => 0,
            };
        });
        Object.defineProperty(target, "stack", {
            value: frames, configurable: true, writable: true,
        });
    };
}
"#;

fn install_prelude(
    context: &mut Context,
    options: &RunOptions,
    cwd: &Path,
    entry: &Path,
) -> JsResult<()> {
    let print = boa_engine::NativeFunction::from_fn_ptr(print_values).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_print"), print, Default::default())?;

    let inspect =
        boa_engine::NativeFunction::from_fn_ptr(inspect_value).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_inspect"), inspect, Default::default())?;

    let write = boa_engine::NativeFunction::from_fn_ptr(write_values).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_write"), write, Default::default())?;

    let exit = boa_engine::NativeFunction::from_fn_ptr(exit_process).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_exit"), exit, Default::default())?;

    let cwd_fn = boa_engine::NativeFunction::from_fn_ptr(current_directory).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_cwd"), cwd_fn, Default::default())?;

    let chdir = boa_engine::NativeFunction::from_fn_ptr(change_directory).to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_chdir"), chdir, Default::default())?;

    let require = boa_engine::NativeFunction::from_fn_ptr(commonjs::require_native)
        .to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_require"), require, Default::default())?;
    context.register_global_property(
        js_string!("__engine_entry"),
        JsString::from(entry.to_string_lossy().as_ref()),
        Default::default(),
    )?;

    let resolve = boa_engine::NativeFunction::from_fn_ptr(commonjs::resolve_native)
        .to_js_function(context.realm());
    context.register_global_property(js_string!("__engine_resolve"), resolve, Default::default())?;

    let argv = JsArray::new(context)?;
    for argument in &options.argv {
        argv.push(JsString::from(argument.as_str()), context)?;
    }
    context.register_global_property(js_string!("__engine_argv"), argv, Default::default())?;

    let env = JsObject::with_object_proto(context.intrinsics());
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        env.set(JsString::from(key), JsString::from(value), false, context)?;
    }
    context.register_global_property(js_string!("__engine_env"), env, Default::default())?;

    context.register_global_property(
        js_string!("__engine_platform"),
        JsString::from(node_platform()),
        Default::default(),
    )?;
    context.register_global_property(
        js_string!("__engine_arch"),
        JsString::from(node_arch()),
        Default::default(),
    )?;
    context.register_global_property(
        js_string!("__engine_pid"),
        std::process::id() as f64,
        Default::default(),
    )?;
    context.register_global_property(
        js_string!("__engine_started_at"),
        js_timestamp() as f64,
        Default::default(),
    )?;
    context.register_global_property(
        js_string!("__engine_entry"),
        JsString::from(entry.to_string_lossy().as_ref()),
        Default::default(),
    )?;
    context.register_global_property(
        js_string!("__engine_cwd_value"),
        JsString::from(cwd.to_string_lossy().as_ref()),
        Default::default(),
    )?;

    context.eval(Source::from_bytes(PRELUDE))?;
    Ok(())
}

fn js_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

fn print_values(
    _this: &JsValue,
    arguments: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let mut parts = Vec::with_capacity(arguments.len());
    for value in arguments {
        parts.push(format_value(value, context));
    }
    let line = parts.join(" ");
    emit(&line);
    emit("\n");
    Ok(JsValue::undefined())
}

fn inspect_value(
    _this: &JsValue,
    arguments: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let value = arguments.first().cloned().unwrap_or_else(JsValue::undefined);
    Ok(JsValue::from(JsString::from(
        format_value(&value, context).as_str(),
    )))
}

fn write_values(
    _this: &JsValue,
    arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let fd = arguments
        .first()
        .and_then(JsValue::as_number)
        .unwrap_or(1.0);
    let text = arguments
        .get(1)
        .and_then(JsValue::as_string)
        .map(|string| string.to_std_string_escaped())
        .unwrap_or_default();
    if fd == 2.0 {
        emit_bytes_to(2, text.as_bytes());
    } else {
        emit_bytes(text.as_bytes());
    }
    Ok(JsValue::undefined())
}

fn exit_process(
    _this: &JsValue,
    arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let code = arguments
        .first()
        .and_then(JsValue::as_number)
        .unwrap_or(0.0) as i32;
    EXIT_CODE.with(|slot| slot.set(Some(code)));
    Err(JsNativeError::error()
        .with_message("process.exit")
        .into())
}

fn current_directory(
    _this: &JsValue,
    _arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Ok(JsValue::from(JsString::from(cwd.to_string_lossy().as_ref())))
}

fn change_directory(
    _this: &JsValue,
    arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let directory = arguments
        .first()
        .and_then(JsValue::as_string)
        .map(|string| string.to_std_string_escaped())
        .unwrap_or_default();
    std::env::set_current_dir(&directory).map_err(|error| {
        JsError::from(JsNativeError::error().with_message(format!(
            "ENOENT: no such file or directory, chdir '{directory}': {error}"
        )))
    })?;
    Ok(JsValue::undefined())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_module_syntax() {
        assert!(needs_module("import x from './x.js';", "a.js"));
        assert!(needs_module("export const x = 1;", "a.js"));
        assert!(!needs_module("const x = import('./x.js');", "a.js"));
        assert!(!needs_module("const importer = 1;", "a.js"));
        assert!(needs_module("await Promise.resolve(1);", "a.mjs"));
    }

    #[test]
    fn module_syntax_in_comments_and_strings_is_ignored() {
        assert!(!needs_module(
            "// import x from 'x';\n/*\nexport default x;\n*/\nmodule.exports = 1;",
            "a.js"
        ));
        assert!(!needs_module("const doc = `\nimport x from 'x';\n`;", "a.js"));
        assert!(needs_module("// note\nexport const x = 1;", "a.js"));
    }

    #[test]
    fn strips_boa_native_marker() {
        assert_eq!(strip_native_marker("boom (native)"), "boom");
        assert_eq!(strip_native_marker("boom"), "boom");
    }
}
