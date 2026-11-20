//! Minimal CommonJS support.
//!
//! Scope is deliberately small and matches what the compatibility runtime can
//! honestly support:
//!
//! * relative or absolute `require` of `.js`, `.cjs`, `.json`, `.ts`, `.cts`,
//!   `.tsx`, and `.jsx` files,
//! * directory resolution through `package.json#main` and `index.*`,
//! * a shared module cache (so cycles see partial exports),
//! * `module.exports`, `exports`, `__filename`, `__dirname`, and
//!   `require.resolve`.
//!
//! Bare specifiers and Node built-ins are rejected with an actionable message.

use std::path::{Path, PathBuf};

use boa_engine::builtins::promise::PromiseState;
use boa_engine::object::JsObject;
use boa_engine::object::builtins::JsArray;
use boa_engine::property::PropertyKey;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsResult, JsString, JsValue, Module, Source,
};

use crate::builtins;
use crate::transpile::transpile_source;
use crate::runtime::{format_value, needs_module};
use crate::resolve::{self, Condition, Resolved};

/// Executes the entry file through a CommonJS wrapper.
pub(crate) fn execute_entry(context: &mut Context, code: &str, path: &Path) -> JsResult<()> {
    load_module(context, path, Some(code)).map(|_| ())
}

/// Executes a file as a CommonJS module without treating it as the entry.
pub(crate) fn execute_file(context: &mut Context, code: &str, path: &Path) -> JsResult<()> {
    load_module(context, path, Some(code)).map(|_| ())
}

/// Loads a file from disk through the CommonJS loader and returns its exports.
pub(crate) fn require_path(context: &mut Context, path: &Path) -> JsResult<JsValue> {
    load_module(context, path, None)
}

/// The native function behind `require(specifier)`.
pub(crate) fn require_native(
    _this: &JsValue,
    arguments: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let parent = string_argument(arguments, 0, "filename")?;
    let specifier = string_argument(arguments, 1, "specifier")?;

    if is_test_specifier(&specifier) {
        let api = context
            .global_object()
            .get(js_string!("__engine_test_api"), context)?;
        if api.is_object() {
            return Ok(api);
        }
        return Err(JsNativeError::typ()
            .with_message(
                "bun:test is only available when running `engine test`",
            )
            .into());
    }

    let parent = PathBuf::from(parent);
    let resolved = resolve::resolve(&parent, &specifier, Condition::Require)
        .map_err(|message| JsNativeError::typ().with_message(message))?;
    match resolved {
        Resolved::File(path) => load_module(context, &path, None),
        Resolved::Builtin(name) => crate::builtins::get(context, &name),
    }
}

/// The native function behind `require.resolve(specifier)`.
pub(crate) fn resolve_native(
    _this: &JsValue,
    arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let parent = string_argument(arguments, 0, "filename")?;
    let specifier = string_argument(arguments, 1, "specifier")?;
    let resolved = resolve::resolve(&PathBuf::from(parent), &specifier, Condition::Require)
        .map_err(|message| JsNativeError::typ().with_message(message))?;
    let text = match resolved {
        Resolved::File(path) => path.to_string_lossy().into_owned(),
        Resolved::Builtin(name) => name,
    };
    Ok(JsValue::from(JsString::from(text.as_str())))
}

fn is_test_specifier(specifier: &str) -> bool {
    matches!(specifier, "bun:test" | "node:test")
}

fn string_argument(arguments: &[JsValue], index: usize, name: &str) -> JsResult<String> {
    arguments
        .get(index)
        .and_then(JsValue::as_string)
        .map(|string| string.to_std_string_escaped())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("the \"{name}\" argument must be a string"))
                .into()
        })
}

fn load_module(
    context: &mut Context,
    path: &Path,
    source_override: Option<&str>,
) -> JsResult<JsValue> {
    let key = path.to_string_lossy().into_owned();
    let cache = module_cache(context)?;

    if let Some(exports) = cached_exports(&cache, &key, context)? {
        return Ok(exports);
    }

    let module = create_module(context, &key)?;
    cache.set(
        JsString::from(key.as_str()),
        module.clone(),
        false,
        context,
    )?;

    let result = evaluate_module(context, path, &module, source_override);
    match result {
        Ok(()) => {
            module.set(js_string!("loaded"), true, false, context)?;
            module.get(js_string!("exports"), context)
        }
        Err(error) => {
            // A failed module must not poison the cache for a later require.
            let _ = cache.delete_property_or_throw(JsString::from(key.as_str()), context);
            Err(error)
        }
    }
}

/// Evaluates an ES module reached through `require()` and returns its namespace
/// as a CommonJS exports object. Top-level await is unsupported because
/// `require` is synchronous; such modules fail with a clear error.
fn require_esm(context: &mut Context, path: &Path, source: &str) -> JsResult<JsValue> {
    let parsed = Module::parse(Source::from_reader(source.as_bytes(), Some(path)), None, context)?;
    let promise = parsed.load_link_evaluate(context);
    // Drain microtasks so the namespace is populated before we copy it.
    for _ in 0..1000 {
        context.run_jobs()?;
        if !matches!(promise.state(), PromiseState::Pending) {
            break;
        }
    }
    if let PromiseState::Rejected(reason) = promise.state() {
        let message = format_value(&reason, context);
        return Err(JsNativeError::error()
            .with_message(format!("cannot require ES module '{}': {message}", path.display()))
            .into());
    }
    if matches!(promise.state(), PromiseState::Pending) {
        return Err(JsNativeError::error()
            .with_message("cannot require an ES module with top-level await")
            .into());
    }

    let namespace = parsed.namespace(context);
    let exports = JsObject::with_null_proto();
    for key in namespace.own_property_keys(context)? {
        if let PropertyKey::String(name) = key {
            let value = namespace.get(name.clone(), context)?;
            exports.set(name, value, false, context)?;
        }
    }
    Ok(exports.into())
}

fn evaluate_module(
    context: &mut Context,
    path: &Path,
    module: &JsObject,
    source_override: Option<&str>,
) -> JsResult<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Native addons are binary files. They cannot be loaded into the JS VM, so
    // expose the universal no-op shim; feature detection and config loading can
    // proceed even though the addon's real functions are unavailable.
    if extension == "node" {
        let exports = builtins::get(context, "node:__native_addon")?;
        module.set(js_string!("exports"), exports, false, context)?;
        return Ok(());
    }

    let source = match source_override {
        Some(source) => source.to_owned(),
        None => std::fs::read_to_string(path).map_err(|error| {
            JsError::from(JsNativeError::typ().with_message(format!(
                "Cannot find module '{}': {error}",
                path.display()
            )))
        })?,
    };
    let filename = path.to_string_lossy().into_owned();

    if extension == "json" {
        let json: serde_json::Value = serde_json::from_str(&source).map_err(|error| {
            JsError::from(JsNativeError::syntax().with_message(format!(
                "{filename}: invalid JSON: {error}"
            )))
        })?;
        let value = json_to_js(&json, context)?;
        module.set(js_string!("exports"), value, false, context)?;
        return Ok(());
    }

    if needs_module(&source, &filename) {
        // Node 22+ allows `require()` of a synchronous ES module and exposes
        // its namespace as the exports object. Mirror that here so CommonJS
        // dependencies can consume ESM-only packages.
        let exports = require_esm(context, path, &source)?;
        module.set(js_string!("exports"), exports, false, context)?;
        return Ok(());
    }

    let code = match source_override {
        // The entry source was already transpiled by the caller.
        Some(_) => source,
        None => transpile_source(&source, &filename)
            .map_err(|error| JsError::from(JsNativeError::syntax().with_message(error.to_string())))?,
    };

    let dirname = path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_owned());

    // The opening brace is on the same line as the first source line, so
    // diagnostics keep their original line numbers.
    let wrapper = format!(
        "(function (exports, require, module, __filename, __dirname) {{{code}\n}})\n//# sourceURL={filename}\n"
    );
    let function = context.eval(Source::from_reader(wrapper.as_bytes(), Some(path)))?;
    let callable = function.as_callable().ok_or_else(|| {
        JsError::from(JsNativeError::typ().with_message("CommonJS wrapper is not callable"))
    })?;

    let exports = module.get(js_string!("exports"), context)?;
    let require = make_require(context, &filename)?;
    let filename_value = JsValue::from(JsString::from(filename.as_str()));
    let dirname_value = JsValue::from(JsString::from(dirname.as_str()));

    callable.call(
        &exports,
        &[
            exports.clone(),
            require,
            JsValue::from(module.clone()),
            filename_value,
            dirname_value,
        ],
        context,
    )?;
    Ok(())
}

fn module_cache(context: &mut Context) -> JsResult<JsObject> {
    context
        .global_object()
        .get(js_string!("__engine_module_cache"), context)?
        .as_object()
        .ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("module cache is not initialised"))
        })
}

fn cached_exports(
    cache: &JsObject,
    key: &str,
    context: &mut Context,
) -> JsResult<Option<JsValue>> {
    let entry = cache.get(JsString::from(key), context)?;
    if entry.is_undefined() {
        return Ok(None);
    }
    let module = entry.as_object().ok_or_else(|| {
        JsError::from(JsNativeError::error().with_message("corrupt CommonJS module cache entry"))
    })?;
    module.get(js_string!("exports"), context).map(Some)
}

fn create_module(context: &mut Context, filename: &str) -> JsResult<JsObject> {
    let creator = context
        .global_object()
        .get(js_string!("__engine_create_module"), context)?;
    let creator = creator
        .as_callable()
        .ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("module factory is not initialised"))
        })?;
    let module = creator.call(
        &JsValue::undefined(),
        &[JsValue::from(JsString::from(filename))],
        context,
    )?;
    module.as_object().ok_or_else(|| {
        JsError::from(JsNativeError::error().with_message("module factory returned a non-object"))
    })
}

fn make_require(context: &mut Context, filename: &str) -> JsResult<JsValue> {
    let maker = context
        .global_object()
        .get(js_string!("__engine_make_require"), context)?;
    let maker = maker.as_callable().ok_or_else(|| {
        JsError::from(JsNativeError::error().with_message("require factory is not initialised"))
    })?;
    maker.call(
        &JsValue::undefined(),
        &[JsValue::from(JsString::from(filename))],
        context,
    )
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn json_to_js(value: &serde_json::Value, context: &mut Context) -> JsResult<JsValue> {
    Ok(match value {
        serde_json::Value::Null => JsValue::null(),
        serde_json::Value::Bool(value) => JsValue::from(*value),
        serde_json::Value::Number(number) => {
            JsValue::from(number.as_f64().unwrap_or(f64::NAN))
        }
        serde_json::Value::String(text) => JsValue::from(JsString::from(text.as_str())),
        serde_json::Value::Array(items) => {
            let array = JsArray::new(context)?;
            for item in items {
                array.push(json_to_js(item, context)?, context)?;
            }
            JsValue::from(array)
        }
        serde_json::Value::Object(entries) => {
            let object = JsObject::with_object_proto(context.intrinsics());
            for (key, item) in entries {
                object.set(
                    JsString::from(key.as_str()),
                    json_to_js(item, context)?,
                    false,
                    context,
                )?;
            }
            JsValue::from(object)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "engine-cjs-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_extensions_and_directories() {
        let dir = scratch("resolve");
        std::fs::write(dir.join("a.ts"), "export const a = 1;").unwrap();
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("folder/index.js"), "module.exports = 1;").unwrap();

        let parent = dir.join("main.js");
        let file = |specifier| match resolve::resolve(&parent, specifier, Condition::Require) {
            Ok(Resolved::File(path)) => path,
            other => panic!("expected a file for {specifier}, got {other:?}"),
        };
        assert!(file("./a").ends_with("a.ts"));
        assert!(file("./folder").ends_with("index.js"));
        assert!(resolve::resolve(&parent, "./missing", Condition::Require).is_err());
        assert_eq!(
            resolve::resolve(&parent, "node:fs", Condition::Require).unwrap(),
            Resolved::Builtin("fs".to_owned())
        );
    }

    #[test]
    fn resolves_js_specifier_to_typescript_file() {
        let dir = scratch("js-to-ts");
        std::fs::write(dir.join("dep.ts"), "module.exports = 1;").unwrap();
        let resolved = resolve::resolve(&dir.join("main.js"), "./dep.js", Condition::Require)
            .unwrap();
        match resolved {
            Resolved::File(path) => assert!(path.ends_with("dep.ts")),
            other => panic!("expected a file, got {other:?}"),
        }
    }
}
