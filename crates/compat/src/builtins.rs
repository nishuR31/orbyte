//! Node built-in module shims for the compatibility runtime.
//!
//! Built-ins are installed as plain JavaScript objects in
//! `globalThis.__engine_builtins`. Native helpers cover the operations JS
//! cannot do (file system, UTF-8 conversion); the rest is readable JS so the
//! behaviour stays inspectable. `require("node:fs")` returns the object;
//! `import ... from "node:fs"` gets a synthetic module that re-exports it.

use boa_engine::object::builtins::JsArray;
use boa_engine::object::JsObject;
use boa_engine::{Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source, js_string};
use std::path::Path;

/// Names re-exported by the synthetic ES module for each built-in.
pub(crate) fn exports_for(name: &str) -> &'static [&'static str] {
    match name {
        "path" => &["join", "resolve", "normalize", "dirname", "basename", "extname", "isAbsolute", "relative", "parse", "format", "sep", "delimiter", "posix", "win32"],
        "fs" => &["readFileSync", "writeFileSync", "appendFileSync", "existsSync", "readdirSync", "statSync", "lstatSync", "mkdirSync", "unlinkSync", "rmSync", "renameSync", "realpathSync", "readFile", "writeFile", "promises", "constants"],
        "os" => &["EOL", "cpus", "platform", "arch", "homedir", "tmpdir", "type", "release", "endianness", "totalmem", "freemem", "uptime", "userInfo", "constants"],
        "util" => &["format", "inspect", "debuglog", "promisify", "callbackify", "inherits", "isArray", "types", "deprecate", "TextEncoder", "TextDecoder"],
        "assert" => &["ok", "equal", "notEqual", "strictEqual", "notStrictEqual", "deepEqual", "deepStrictEqual", "fail", "throws", "doesNotThrow", "AssertionError", "strict"],
        "events" => &["EventEmitter"],
        "url" => &["URL", "URLSearchParams", "fileURLToPath", "pathToFileURL", "domainToASCII", "domainToUnicode"],
        "module" => &["Module", "createRequire"],
        "tty" => &["isatty", "ReadStream", "WriteStream"],
        "worker_threads" => &["isMainThread", "parentPort", "workerData", "Worker", "threadId"],
        "fs/promises" => &["readFile", "writeFile", "mkdir", "stat", "unlink", "readdir", "rm", "copyFile", "access"],
        "path/posix" | "path/win32" => &["join", "resolve", "normalize", "dirname", "basename", "extname", "isAbsolute", "relative"],
        "querystring" => &["parse", "stringify", "escape", "unescape", "decode", "encode"],
        "buffer" => &["Buffer", "SlowBuffer", "kMaxLength", "constants"],
        "process" => &["argv", "env", "cwd", "exit", "platform", "arch", "version", "versions", "stdout", "stderr", "nextTick", "hrtime", "on", "once", "off"],
        "stream" => &["Stream", "Readable", "Writable", "Duplex", "Transform", "pipeline", "finished"],
        "crypto" => &["randomUUID", "randomBytes", "getHashes", "createHash", "webcrypto"],
        _ => &[],
    }
}

/// Returns the built-in object, installing the shims on first use. Names with
/// a slash (`fs/promises`, `path/posix`) resolve to a property of the base
/// built-in.
pub(crate) fn get(context: &mut Context, name: &str) -> JsResult<JsValue> {
    ensure_installed(context)?;
    // ESM specifiers keep the `node:` prefix; CJS ones do not. Normalise here
    // so both paths reach the same shim.
    let name = name.strip_prefix("node:").unwrap_or(name);
    let builtins = context
        .global_object()
        .get(js_string!("__engine_builtins"), context)?
        .as_object()
        .ok_or_else(|| JsNativeError::error().with_message("built-ins are not initialised"))?;
    let (base, subpath) = name.split_once('/').unwrap_or((name, ""));
    let value = builtins.get(JsString::from(base), context)?;
    let value = if value.is_undefined() {
        let factory = context
            .global_object()
            .get(js_string!("__engine_generic_builtin"), context)?;
        match factory.as_callable() {
            Some(callable) => callable.call(
                &JsValue::undefined(),
                &[JsValue::from(JsString::from(base))],
                context,
            )?,
            None => value,
        }
    } else {
        value
    };
    if subpath.is_empty() {
        return Ok(value);
    }
    match value.as_object() {
        Some(object) => {
            let nested = object.get(JsString::from(subpath), context)?;
            if nested.is_undefined() {
                Ok(value)
            } else {
                Ok(nested)
            }
        }
        None => Ok(value),
    }
}

/// Installs the native helpers and the JavaScript shims, once per context.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    ensure_installed(context)
}

fn ensure_installed(context: &mut Context) -> JsResult<()> {
    let globals = context.global_object();
    if globals.has_property(js_string!("__engine_builtins_installed"), context)? {
        return Ok(());
    }

    register(context, "__engine_fs_read", fs_read)?;
    register(context, "__engine_fs_write", fs_write)?;
    register(context, "__engine_fs_exists", fs_exists)?;
    register(context, "__engine_fs_realpath", fs_realpath)?;
    register(context, "__engine_fs_readdir", fs_readdir)?;
    register(context, "__engine_fs_stat", fs_stat)?;
    register(context, "__engine_fs_mkdir", fs_mkdir)?;
    register(context, "__engine_fs_remove", fs_remove)?;
    register(context, "__engine_fs_rename", fs_rename)?;
    register(context, "__engine_utf8_encode", utf8_encode)?;
    register(context, "__engine_utf8_decode", utf8_decode)?;
    register(context, "__engine_random_uuid", random_uuid)?;
    register(context, "__engine_fetch", fetch_native)?;

    context.eval(Source::from_bytes(SHIMS))?;
    globals.set(js_string!("__engine_builtins_installed"), true, false, context)?;
    Ok(())
}

fn register(
    context: &mut Context,
    name: &'static str,
    function: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
) -> JsResult<()> {
    let function = NativeFunction::from_fn_ptr(function).to_js_function(context.realm());
    context.register_global_property(js_string!(name), function, boa_engine::property::Attribute::all())
}

fn arg_string(arguments: &[JsValue], index: usize, default: &str) -> String {
    arguments
        .get(index)
        .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_else(|| default.to_owned())
}

fn fs_read(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let encoding = arg_string(args, 1, "utf8");
    if encoding == "buffer" || encoding == "binary" {
        let bytes = std::fs::read(&path)
            .map_err(|e| JsNativeError::error().with_message(format!("ENOENT: {path}: {e}")))?;
        return Ok(array_of_u8(bytes, context)?.into());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| JsNativeError::error().with_message(format!("ENOENT: {path}: {e}")))?;
    Ok(JsValue::from(JsString::from(text.as_str())))
}

fn fs_write(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let data = arg_string(args, 1, "");
    let ok = std::fs::write(&path, data).is_ok();
    Ok(JsValue::from(ok))
}

fn fs_exists(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(Path::new(&arg_string(args, 0, "")).exists()))
}

fn fs_realpath(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| JsNativeError::error().with_message(format!("ENOENT: {path}: {e}")))?;
    Ok(JsValue::from(JsString::from(
        canonical.to_string_lossy().as_ref(),
    )))
}

fn fs_readdir(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, ".");
    let mut entries: Vec<String> = std::fs::read_dir(&path)
        .map_err(|e| JsNativeError::error().with_message(format!("ENOENT: {path}: {e}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    let array = JsArray::new(context)?;
    for entry in entries {
        array.push(JsValue::from(JsString::from(entry.as_str())), context)?;
    }
    Ok(array.into())
}

fn fs_stat(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let metadata = std::fs::metadata(&path)
        .map_err(|e| JsNativeError::error().with_message(format!("ENOENT: {path}: {e}")))?;
    let object = JsObject::with_object_proto(context.intrinsics());
    object.set(js_string!("size"), metadata.len() as f64, false, context)?;
    object.set(js_string!("isFile"), JsValue::from(metadata.is_file()), false, context)?;
    object.set(js_string!("isDirectory"), JsValue::from(metadata.is_dir()), false, context)?;
    object.set(js_string!("isSymbolicLink"), JsValue::from(metadata.file_type().is_symlink()), false, context)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or(0.0);
    object.set(js_string!("mtimeMs"), modified, false, context)?;
    object.set(js_string!("mtime"), JsValue::from(modified), false, context)?;
    Ok(object.into())
}

fn fs_mkdir(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let recursive = args.get(1).and_then(|v| v.as_object()).is_some()
        || args.get(1).and_then(|v| v.as_boolean()).unwrap_or(false);
    let ok = if recursive {
        std::fs::create_dir_all(&path).is_ok()
    } else {
        std::fs::create_dir(&path).is_ok()
    };
    Ok(JsValue::from(ok))
}

fn fs_remove(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "");
    let metadata = std::fs::symlink_metadata(&path);
    let ok = match metadata {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path).is_ok(),
        Ok(_) => std::fs::remove_file(&path).is_ok(),
        Err(_) => false,
    };
    Ok(JsValue::from(ok))
}

fn fs_rename(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let from = arg_string(args, 0, "");
    let to = arg_string(args, 1, "");
    Ok(JsValue::from(std::fs::rename(from, to).is_ok()))
}

fn array_of_u8(bytes: Vec<u8>, context: &mut Context) -> JsResult<JsArray> {
    let array = JsArray::new(context)?;
    for byte in bytes {
        array.push(JsValue::from(byte), context)?;
    }
    Ok(array)
}

fn utf8_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, "");
    Ok(array_of_u8(text.into_bytes(), context)?.into())
}

fn utf8_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut bytes = Vec::new();
    if let Some(array) = args.first().and_then(|value| value.as_object()) {
        let length = array
            .get(js_string!("length"), context)
            .ok()
            .and_then(|value| value.as_number())
            .unwrap_or(0.0) as usize;
        for index in 0..length.min(1_000_000) {
            if let Ok(value) = array.get(index as u32, context) {
                bytes.push(value.as_number().unwrap_or(0.0) as u8);
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(JsValue::from(JsString::from(text.as_str())))
}

/// Blocking HTTP request used by the Promise-based `fetch` shim. curl is used
/// because a TLS stack is not otherwise a dependency of the compatibility
/// runtime; the call still runs in a job so `fetch` never blocks synchronously.
fn fetch_native(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let url = arg_string(args, 0, "");
    let init = args.get(1).and_then(|value| value.as_object());
    let method = init
        .as_ref()
        .and_then(|object| object.get(js_string!("method"), context).ok())
        .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_else(|| "GET".to_owned());
    let body = init
        .as_ref()
        .and_then(|object| object.get(js_string!("body"), context).ok())
        .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()));
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(object) = init
        .as_ref()
        .and_then(|object| object.get(js_string!("headers"), context).ok())
    {
        if let Some(object) = object.as_object() {
            if let Ok(keys) = object.own_property_keys(context) {
                for key in keys {
                    let name = key.to_string();
                    if let Some(value) = object.get(key, context).ok() {
                        if let Some(value) = value.as_string() {
                            headers.push((name, value.to_std_string_escaped()));
                        }
                    }
                }
            }
        }
    }

    const MARKER: &str = "\n__ORB_FETCH_STATUS__";
    let mut command = Command::new("curl");
    command
        .arg("-sS")
        .arg("-L")
        .arg("-X")
        .arg(&method)
        .arg("-w")
        .arg(format!("{MARKER}%{{http_code}}"));
    for (name, value) in &headers {
        command.arg("-H").arg(format!("{name}: {value}"));
    }
    if body.is_some() {
        command.arg("--data-binary").arg("@-");
    }
    command.arg(&url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let result = (|| -> std::io::Result<std::process::Output> {
        let mut child = command.spawn()?;
        if let Some(body) = &body {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(body.as_bytes())?;
            }
        }
        child.wait_with_output()
    })();

    let object = JsObject::with_object_proto(context.intrinsics());
    match result {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout).into_owned();
            let (payload, status) = match raw.rfind(MARKER) {
                Some(index) => (
                    raw[..index].to_owned(),
                    raw[index + MARKER.len()..].trim().parse::<i64>().unwrap_or(0),
                ),
                None => (raw, 0),
            };
            object.set(js_string!("ok"), (200..300).contains(&status), false, context)?;
            object.set(js_string!("status"), status as f64, false, context)?;
            object.set(js_string!("statusText"), JsValue::from(JsString::from("")), false, context)?;
            object.set(js_string!("body"), JsValue::from(JsString::from(payload.as_str())), false, context)?;
        }
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            object.set(js_string!("ok"), false, false, context)?;
            object.set(js_string!("status"), 0.0, false, context)?;
            object.set(js_string!("statusText"), JsValue::from(JsString::from(error.as_str())), false, context)?;
            object.set(js_string!("body"), JsValue::undefined(), false, context)?;
        }
        Err(error) => {
            let message = format!("fetch failed: {error}");
            object.set(js_string!("ok"), false, false, context)?;
            object.set(js_string!("status"), 0.0, false, context)?;
            object.set(js_string!("statusText"), JsValue::from(JsString::from(message.as_str())), false, context)?;
            object.set(js_string!("body"), JsValue::undefined(), false, context)?;
        }
    }
    Ok(object.into())
}

fn random_uuid(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut state = now as u64 ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut bytes = [0u8; 16];
    for byte in &mut bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let text = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    );
    Ok(JsValue::from(JsString::from(text.as_str())))
}

const SHIMS: &str = r##"
(function () {
  const g = globalThis;
  const builtins = {};

  // ----- path (POSIX) -----
  function normalize(p) {
    const absolute = p.startsWith("/");
    const parts = p.split("/").filter((x) => x && x !== ".");
    const out = [];
    for (const part of parts) {
      if (part === ".." && out.length && out[out.length - 1] !== "..") out.pop();
      else if (part !== ".." || !absolute) out.push(part);
    }
    const joined = out.join("/");
    return absolute ? "/" + joined : (joined || ".");
  }
  function join(...parts) {
    const first = parts.find((p) => typeof p === "string" && p.length) || "";
    return normalize(parts.filter((p) => typeof p === "string" && p.length).join("/"));
  }
  function resolve(...parts) {
    const cwd = g.process ? g.process.cwd() : "/";
    let result = cwd;
    for (const part of parts) {
      if (typeof part !== "string" || !part) continue;
      result = part.startsWith("/") ? part : result + "/" + part;
    }
    return normalize(result);
  }
  function dirname(p) {
    const index = p.lastIndexOf("/");
    if (index < 0) return ".";
    if (index === 0) return "/";
    return p.slice(0, index).replace(/\/+$/, "") || "/";
  }
  function basename(p, ext) {
    let name = p.split("/").filter(Boolean).pop() || "";
    if (ext && name.endsWith(ext)) name = name.slice(0, -ext.length);
    return name;
  }
  function extname(p) {
    const name = basename(p);
    const index = name.lastIndexOf(".");
    return index > 0 ? name.slice(index) : "";
  }
  function relative(from, to) {
    const a = normalize(from).split("/").filter(Boolean);
    const b = normalize(to).split("/").filter(Boolean);
    let i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    const up = a.slice(i).map(() => "..");
    return up.concat(b.slice(i)).join("/") || ".";
  }
  function parse(p) {
    const base = basename(p);
    const ext = extname(p);
    return {
      root: p.startsWith("/") ? "/" : "",
      dir: dirname(p),
      base,
      ext,
      name: ext ? base.slice(0, -ext.length) : base,
    };
  }
  function format(o) {
    const dir = o.dir || "";
    const base = o.base || ((o.name || "") + (o.ext || ""));
    return dir ? (dir.endsWith("/") ? dir + base : dir + "/" + base) : base;
  }
  const path = {
    sep: "/", delimiter: ":", normalize, join, resolve, dirname, basename, extname,
    isAbsolute: (p) => typeof p === "string" && p.startsWith("/"),
    relative, parse, format, posix: null, win32: null,
  };
  path.posix = path; path.win32 = path;
  builtins.path = path;

  // ----- os -----
  const env = g.process ? g.process.env : {};
  const isWindows = !!(g.process && g.process.platform === "win32");
  builtins.os = {
    EOL: isWindows ? "\r\n" : "\n", constants: { signals: {} },
    cpus: () => [], platform: () => (g.process ? g.process.platform : "linux"),
    arch: () => (g.process ? g.process.arch : "x64"),
    homedir: () => env.HOME || env.USERPROFILE || "",
    tmpdir: () => env.TEMP || env.TMP || env.TMPDIR || (isWindows ? "C:\\Windows\\Temp" : "/tmp"),
    type: () => (g.process ? g.process.platform : "linux"),
    release: () => "0.0.0", endianness: () => "LE",
    totalmem: () => 0, freemem: () => 0, uptime: () => 0,
    userInfo: () => ({ username: env.USER || env.USERNAME || "", homedir: env.HOME || env.USERPROFILE || "", uid: -1, gid: -1 }),
    availableParallelism: () => 1,
  };

  // ----- util -----
  function formatValue(value) {
    if (typeof value === "string") return value;
    if (typeof value === "object" && value !== null && typeof g.__engine_inspect === "function") {
      return g.__engine_inspect(value);
    }
    return String(value);
  }
  function utilFormat(first, ...args) {
    if (typeof first !== "string") return [first, ...args].map(formatValue).join(" ");
    let i = 0;
    const text = first.replace(/%[sdifjoO%]/g, (token) => {
      if (token === "%%") return "%";
      const value = args[i++];
      if (token === "%s") return String(value);
      if (token === "%d" || token === "%i" || token === "%f") return String(Number(value));
      if (token === "%j" || token === "%o" || token === "%O") return JSON.stringify(value);
      return token;
    });
    return text + args.slice(i).map((v) => " " + formatValue(v)).join("");
  }
  builtins.util = {
    format: utilFormat,
    // Node's debuglog returns a no-op unless NODE_DEBUG enables the namespace.
    debuglog: (name) => {
      const setting = String((g.process && g.process.env && g.process.env.NODE_DEBUG) || "").trim();
      const enabled = setting.length > 0 &&
        setting.split(",").some((part) => part.trim().length > 0 && String(name).startsWith(part.trim()));
      const log = enabled ? (...args) => g.console.error(...args) : () => {};
      log.enabled = enabled;
      return log;
    },
    inspect: (value) => (typeof g.__engine_inspect === "function" ? g.__engine_inspect(value) : String(value)),
    promisify: (fn) => function (...args) {
      return new Promise((resolve, reject) => {
        fn.call(this, ...args, (error, value) => (error ? reject(error) : resolve(value)));
      });
    },
    callbackify: (fn) => function (...args) {
      const callback = args.pop();
      fn.apply(this, args).then((value) => callback(null, value), callback);
    },
    inherits: (ctor, superCtor) => { ctor.prototype = Object.create(superCtor.prototype); ctor.prototype.constructor = ctor; },
    isArray: Array.isArray,
    types: { isPromise: (x) => !!x && typeof x.then === "function" },
    deprecate: (fn) => fn,
    TextEncoder: g.TextEncoder || class TextEncoder { encode(s) { return g.__engine_utf8_encode(s); } },
    TextDecoder: g.TextDecoder || class TextDecoder { decode(b) { return g.__engine_utf8_decode(b); } },
  };

  // ----- assert -----
  class AssertionError extends Error {
    constructor(message) { super(message || "Assertion failed"); this.name = "AssertionError"; }
  }
  function fail(message) { throw new AssertionError(message); }
  function assertOk(value, message) {
    if (!value) fail(message || "The expression evaluated to a falsy value");
  }
  function equal(a, b, message) { if (a != b) fail(message || `${a} != ${b}`); }
  function strictEqual(a, b, message) { if (a !== b) fail(message || `${a} !== ${b}`); }
  function deepEqual(a, b, message) {
    if (JSON.stringify(a) !== JSON.stringify(b)) fail(message || "values are not deeply equal");
  }
  function throws(fn, expected, message) {
    let thrown;
    try { fn(); } catch (error) { thrown = error; }
    if (!thrown) fail(message || "Missing expected exception");
    if (expected && !(thrown instanceof expected) &&
        !(typeof expected === "object" && thrown.name === expected.name)) {
      fail(message || `Expected ${expected.name}, got ${thrown.name}`);
    }
  }
  function doesNotThrow(fn, message) {
    try { fn(); } catch (error) { fail(message || `Unexpected exception: ${error}`); }
  }
  const assert = (value, message) => assertOk(value, message);
  Object.assign(assert, {
    AssertionError, fail, ok: assertOk, equal, notEqual: (a, b, m) => { if (a == b) fail(m || "values are equal"); },
    strictEqual, notStrictEqual: (a, b, m) => { if (a === b) fail(m || "values are strictly equal"); },
    deepEqual, deepStrictEqual: deepEqual, throws, doesNotThrow,
  });
  assert.strict = assert;
  builtins.assert = assert;

  // ----- events -----
  class EventEmitter {
    constructor() { this._events = Object.create(null); this._maxListeners = 10; }
    _ensureEvents() {
      // Subclasses using util.inherits() do not call this constructor.
      if (!this._events) this._events = Object.create(null);
      return this._events;
    }
    addListener(name, fn) {
      const events = this._ensureEvents();
      (events[name] || (events[name] = [])).push(fn);
      return this;
    }
    on(name, fn) { return this.addListener(name, fn); }
    prependListener(name, fn) {
      const events = this._ensureEvents();
      (events[name] || (events[name] = [])).unshift(fn);
      return this;
    }
    once(name, fn) {
      const self = this;
      function wrapper(...args) { self.removeListener(name, wrapper); fn.apply(self, args); }
      wrapper.listener = fn;
      return this.addListener(name, wrapper);
    }
    removeListener(name, fn) {
      const list = this._ensureEvents()[name];
      if (!list) return this;
      this._events[name] = list.filter((candidate) => candidate !== fn && candidate.listener !== fn);
      return this;
    }
    off(name, fn) { return this.removeListener(name, fn); }
    removeAllListeners(name) {
      const events = this._ensureEvents();
      if (name) delete events[name]; else this._events = Object.create(null);
      return this;
    }
    listeners(name) { return (this._ensureEvents()[name] || []).slice(); }
    listenerCount(name) { return (this._ensureEvents()[name] || []).length; }
    emit(name, ...args) {
      const list = (this._ensureEvents()[name] || []).slice();
      for (const fn of list) fn.apply(this, args);
      return list.length > 0;
    }
    setMaxListeners(n) { this._maxListeners = n; return this; }
    getMaxListeners() { return this._maxListeners; }
  }
  EventEmitter.defaultMaxListeners = 10;
  builtins.events = { EventEmitter };

  // ----- querystring -----
  function qsParse(text) {
    const out = Object.create(null);
    for (const pair of String(text || "").split("&")) {
      if (!pair) continue;
      const index = pair.indexOf("=");
      const key = decodeURIComponent((index < 0 ? pair : pair.slice(0, index)).replace(/\+/g, " "));
      const value = decodeURIComponent((index < 0 ? "" : pair.slice(index + 1)).replace(/\+/g, " "));
      out[key] = value;
    }
    return out;
  }
  function qsStringify(object) {
    if (!object || typeof object !== "object") return "";
    return Object.keys(object).map((key) => {
      const value = object[key];
      if (value === undefined) return "";
      const text = Array.isArray(value) ? value.map((v) => encodeURIComponent(String(v))).join("&" + encodeURIComponent(key) + "=") : encodeURIComponent(String(value));
      return encodeURIComponent(key) + "=" + text;
    }).filter(Boolean).join("&");
  }
  builtins.querystring = {
    parse: qsParse, stringify: qsStringify,
    escape: (s) => encodeURIComponent(String(s)),
    unescape: (s) => decodeURIComponent(String(s)),
    decode: qsParse, encode: qsStringify,
  };

  // ----- url -----
  class URLSearchParams {
    constructor(init) {
      this._pairs = [];
      if (typeof init === "string") {
        for (const pair of init.replace(/^\?/, "").split("&")) {
          if (!pair) continue;
          const i = pair.indexOf("=");
          this._pairs.push([
            decodeURIComponent((i < 0 ? pair : pair.slice(0, i)).replace(/\+/g, " ")),
            decodeURIComponent((i < 0 ? "" : pair.slice(i + 1)).replace(/\+/g, " ")),
          ]);
        }
      } else if (init && typeof init === "object") {
        for (const key of Object.keys(init)) {
          const value = init[key];
          if (Array.isArray(value)) for (const v of value) this._pairs.push([key, String(v)]);
          else this._pairs.push([key, String(value)]);
        }
      }
    }
    append(k, v) { this._pairs.push([String(k), String(v)]); }
    get(k) { const pair = this._pairs.find((p) => p[0] === k); return pair ? pair[1] : null; }
    getAll(k) { return this._pairs.filter((p) => p[0] === k).map((p) => p[1]); }
    has(k) { return this._pairs.some((p) => p[0] === k); }
    delete(k) { this._pairs = this._pairs.filter((p) => p[0] !== k); }
    set(k, v) {
      let first = true;
      this._pairs = this._pairs.filter((p) => {
        if (p[0] !== k) return true;
        if (first) { first = false; p[1] = String(v); return true; }
        return false;
      });
      if (first) this.append(k, v);
    }
    forEach(fn) { for (const [k, v] of this._pairs) fn(v, k, this); }
    keys() { return this._pairs.map((p) => p[0])[Symbol.iterator](); }
    values() { return this._pairs.map((p) => p[1])[Symbol.iterator](); }
    entries() { return this._pairs[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    sort() { this._pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); }
    toString() { return this._pairs.map((p) => encodeURIComponent(p[0]) + "=" + encodeURIComponent(p[1])).join("&"); }
  }
  class URL {
    constructor(input, base) {
      let text = String(input);
      const absolute = (value) => /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(value);
      if (!absolute(text) && base !== undefined) {
        const baseUrl = new URL(String(base));
        const origin = baseUrl.protocol + "//" + baseUrl.host;
        if (text.startsWith("//")) text = baseUrl.protocol + text;
        else if (text.startsWith("/")) text = origin + text;
        else {
          const directory = baseUrl.pathname.slice(0, baseUrl.pathname.lastIndexOf("/") + 1);
          text = origin + directory + text;
        }
      }
      // Collapse `..` and `.` segments in the path.
      const scheme = text.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):/);
      this.protocol = scheme ? scheme[1] + ":" : "";
      let rest = scheme ? text.slice(scheme[0].length) : text;
      if (rest.startsWith("//")) {
        rest = rest.slice(2);
        const separator = rest.search(/[/?#]/);
        this.host = separator < 0 ? rest : rest.slice(0, separator);
        rest = separator < 0 ? "" : rest.slice(separator);
      } else this.host = "";
      const hash = rest.indexOf("#");
      if (hash >= 0) { this.hash = rest.slice(hash); rest = rest.slice(0, hash); } else this.hash = "";
      const query = rest.indexOf("?");
      if (query >= 0) { this.search = rest.slice(query); rest = rest.slice(0, query); } else this.search = "";
      const segments = [];
      for (const part of rest.split("/")) {
        if (part === "..") segments.pop();
        else if (part !== "." && part !== "") segments.push(part);
      }
      this.pathname = (rest.startsWith("/") ? "/" : "") + segments.join("/") || "/";
      this.origin = this.protocol + "//" + this.host;
      this.href = this.origin + this.pathname + this.search + this.hash;
      this.searchParams = new URLSearchParams(this.search);
      this.username = ""; this.password = ""; this.port = "";
    }
    toString() { return this.href; }
  }
  builtins.worker_threads = {
    isMainThread: true,
    parentPort: null,
    workerData: null,
    threadId: 0,
    Worker: class Worker {},
  };

  builtins.tty = {
    isatty: () => false,
    ReadStream: class ReadStream {},
    WriteStream: class WriteStream {},
  };

  // `module` is often required for its side effects by transpiler packages.
  class Module {
    constructor(id, parent) { this.id = id; this.parent = parent; this.exports = {}; }
    require() { return undefined; }
  }
  Module._resolveFilename = (request) => request;
  Module.builtinModules = [
    'assert', 'assert/strict', 'async_hooks', 'buffer', 'child_process', 'cluster',
    'console', 'constants', 'crypto', 'dgram', 'diagnostics_channel', 'dns',
    'dns/promises', 'domain', 'events', 'fs', 'fs/promises', 'http', 'http2', 'https',
    'inspector', 'module', 'net', 'os', 'path', 'path/posix', 'path/win32',
    'perf_hooks', 'process', 'punycode', 'querystring', 'readline', 'stream',
    'stream/promises', 'string_decoder', 'sys', 'timers', 'tls', 'trace_events',
    'tty', 'url', 'util', 'util/types', 'v8', 'vm', 'wasi', 'worker_threads', 'zlib',
  ];
  Module.isBuiltin = (name) => Module.builtinModules.includes(String(name).replace(/^node:/, ''));
  Module.Module = Module;
  Module.createRequire = () => Module.prototype.require;
  Module.extensions = { '.js': undefined, '.json': undefined, '.node': undefined };
  // Node's `module` built-in is the Module constructor itself, so
  // `require("module").prototype.require` must work.
  builtins.module = Module;

  builtins.url = {
    URL, URLSearchParams,
    // Legacy Node API still used by build tools.
    parse: (input) => {
      const parsed = new URL(String(input), "http://localhost");
      const query = Object.create(null);
      for (const [key, value] of parsed.searchParams) query[key] = value;
      return {
        protocol: parsed.protocol, host: parsed.host, hostname: parsed.host,
        pathname: parsed.pathname, search: parsed.search, hash: parsed.hash,
        href: parsed.href, query, path: parsed.pathname + parsed.search,
      };
    },
    format: (value) => String(value.href || value.path || ""),
    resolve: (from, to) => new URL(String(to), String(from)).href,
    fileURLToPath: (url) => {
      let text = typeof url === "string" ? url : url.href;
      if (text.startsWith("file://")) text = text.slice(7);
      return decodeURIComponent(text);
    },
    pathToFileURL: (p) => new URL("file://" + p),
    domainToASCII: (d) => String(d), domainToUnicode: (d) => String(d),
  };

  // ----- buffer -----
  class Buffer extends Uint8Array {
    constructor(value) {
      if (typeof value === "number") super(value);
      else if (typeof value === "string") super(g.__engine_utf8_encode(value));
      else super(value || []);
    }
    static from(value, encoding) {
      if (value instanceof Uint8Array) return new Buffer(value);
      if (typeof value === "string" && encoding === "base64") {
        const bytes = g.atob ? Uint8Array.from(g.atob(value), (c) => c.charCodeAt(0)) : [];
        return new Buffer(bytes);
      }
      return new Buffer(value);
    }
    static alloc(size) { return new Buffer(new Array(size).fill(0)); }
    static allocUnsafe(size) { return Buffer.alloc(size); }
    static isBuffer(value) { return value instanceof Buffer; }
    static byteLength(text) { return g.__engine_utf8_encode(String(text)).length; }
    static concat(list, totalLength) {
      const parts = Array.from(list || []);
      const size = totalLength === undefined
        ? parts.reduce((sum, part) => sum + part.length, 0)
        : totalLength;
      const out = new Uint8Array(size);
      let offset = 0;
      for (const part of parts) {
        for (let i = 0; i < part.length && offset < size; i++) out[offset++] = part[i];
      }
      return new Buffer(out);
    }
    toString(encoding) {
      return encoding === "hex"
        ? Array.from(this).map((b) => b.toString(16).padStart(2, "0")).join("")
        : g.__engine_utf8_decode(this);
    }
    toJSON() { return { type: "Buffer", data: Array.from(this) }; }
  }
  builtins.buffer = { Buffer, SlowBuffer: Buffer, kMaxLength: 2 ** 31 - 1, constants: {} };

  // ----- fs -----
  function fsReadFile(path, options, callback) {
    if (typeof options === "function") { callback = options; options = undefined; }
    const read = () => g.__engine_fs_read(String(path), typeof options === "string" ? options : (options && options.encoding) || "utf8");
    if (callback) {
      try { callback(null, read()); } catch (error) { callback(error); }
      return;
    }
    return Promise.resolve().then(read);
  }
  function fsWriteFile(path, data, options, callback) {
    if (typeof options === "function") { callback = options; options = undefined; }
    const write = () => g.__engine_fs_write(fsPath(path), String(data));
    if (callback) { try { callback(write() ? null : new Error("write failed")); } catch (e) { callback(e); } return; }
    return Promise.resolve().then(() => { if (!write()) throw new Error("write failed"); });
  }
  const fsPath = (p) => {
    const text = String(p);
    return text.startsWith("file://") ? text.slice(7) : text;
  };
  const statSync = (p) => {
    const info = g.__engine_fs_stat(fsPath(p));
    return {
      size: info.size, mtime: new Date(info.mtimeMs), mtimeMs: info.mtimeMs,
      isFile: () => info.isFile, isDirectory: () => info.isDirectory,
      isSymbolicLink: () => info.isSymbolicLink,
    };
  };
  const fs = {
    readFileSync: (p, o) => g.__engine_fs_read(fsPath(p), typeof o === "string" ? o : (o && o.encoding) || "utf8"),
    writeFileSync: (p, d) => g.__engine_fs_write(fsPath(p), String(d)),
    appendFileSync: (p, d) => g.__engine_fs_write(fsPath(p), String(d)),
    existsSync: (p) => g.__engine_fs_exists(fsPath(p)),
    readdirSync: (p) => g.__engine_fs_readdir(fsPath(p)),
    statSync, lstatSync: statSync, fstatSync: statSync,
    mkdirSync: (p, o) => g.__engine_fs_mkdir(fsPath(p), o),
    unlinkSync: (p) => g.__engine_fs_remove(fsPath(p)),
    rmSync: (p) => g.__engine_fs_remove(fsPath(p)),
    rmdirSync: (p) => g.__engine_fs_remove(fsPath(p)),
    readlinkSync: () => "",
    fstatSync: statSync,
    chmodSync: () => {},
    symlinkSync: () => {},
    linkSync: () => {},
    openSync: () => -1,
    closeSync: () => {},
    truncateSync: () => {},
    mkdtempSync: (prefix) => String(prefix) + "tmp",
    renameSync: (a, b) => g.__engine_fs_rename(fsPath(a), fsPath(b)),
    realpathSync: (() => {
      const realpath = (p) => g.__engine_fs_realpath(fsPath(p));
      realpath.native = realpath;
      return realpath;
    })(),
    copyFileSync: (a, b) => g.__engine_fs_write(fsPath(b), g.__engine_fs_read(fsPath(a))),
    readFile: fsReadFile, writeFile: fsWriteFile,
    promises: {
      readFile: (p, o) => fsReadFile(p, o),
      writeFile: (p, d, o) => fsWriteFile(p, d, o),
      mkdir: (p, o) => Promise.resolve().then(() => g.__engine_fs_mkdir(fsPath(p), o)),
      stat: (p) => Promise.resolve().then(() => statSync(p)),
      lstat: (p) => Promise.resolve().then(() => statSync(p)),
      realpath: (p) => Promise.resolve().then(() => g.__engine_fs_realpath(fsPath(p))),
      readlink: () => Promise.resolve(""),
      unlink: (p) => Promise.resolve().then(() => g.__engine_fs_remove(fsPath(p))),
      readdir: (p) => Promise.resolve().then(() => g.__engine_fs_readdir(fsPath(p))),
      rm: (p) => Promise.resolve().then(() => g.__engine_fs_remove(fsPath(p))),
      access: (p) => Promise.resolve(g.__engine_fs_exists(fsPath(p))),
    },
    constants: { O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2 },
  };
  // ESM named imports (e.g. `import { readdir } from "fs"`) need the async
  // aliases as own properties too.
  fs.readdir = (p, o, cb) => fs.promises.readdir(p).then((v) => cb && cb(null, v), (e) => cb && cb(e));
  fs.stat = (p, o, cb) => fs.promises.stat(p).then((v) => cb && cb(null, v), (e) => cb && cb(e));
  fs.lstat = fs.stat;
  fs.realpath = (p, o, cb) => fs.promises.realpath(p).then((v) => cb && cb(null, v), (e) => cb && cb(e));
  builtins.fs = fs;

  // ----- remaining built-ins -----
  builtins.process = g.process || {};
  // Node's stream constructors are callable functions; packages frequently use
  // `Readable.call(this, ...)`. ES classes would throw on such calls.
  const makeStream = (label) => {
    function Stream(options) {
      this._events = Object.create(null);
      this.options = options || {};
      this.readable = true; this.writable = true; this.destroyed = false;
    }
    for (const method of ['_ensureEvents', 'emit', 'on', 'once', 'addListener',
                          'removeListener', 'off', 'listeners', 'listenerCount',
                          'setMaxListeners', 'removeAllListeners']) {
      Stream.prototype[method] = EventEmitter.prototype[method];
    }
    Stream.prototype.push = () => true;
    Stream.prototype.write = () => true;
    Stream.prototype.end = function () { return this; };
    Stream.prototype.destroy = function () { return this; };
    Stream.prototype.pipe = (destination) => destination;
    Stream.prototype.resume = function () { return this; };
    Stream.prototype.pause = function () { return this; };
    Stream.prototype[Symbol.asyncIterator] = async function* () {};
    Object.defineProperty(Stream, 'name', { value: label });
    return Stream;
  };
  builtins.stream = {
    Stream: makeStream('Stream'), Readable: makeStream('Readable'),
    Writable: makeStream('Writable'), Duplex: makeStream('Duplex'),
    Transform: makeStream('Transform'),
    addAbortSignal: (stream) => stream,
    pipeline: (...args) => Promise.resolve(), finished: (stream, cb) => cb && cb(null),
  };
  builtins.crypto = {
    randomUUID: () => g.__engine_random_uuid(),
    randomBytes: (n) => {
      const bytes = [];
      for (let i = 0; i < n; i++) bytes.push(Math.floor(Math.random() * 256));
      return new Buffer(bytes);
    },
    getHashes: () => ["sha1", "sha256", "md5"],
    createHash: (name) => {
      let data = "";
      return {
        update(part) { data += String(part); return this; },
        digest(encoding) {
          let hash = 0;
          for (let i = 0; i < data.length; i++) hash = ((hash << 5) - hash + data.charCodeAt(i)) | 0;
          const hex = ((hash >>> 0).toString(16) + (data.length >>> 0).toString(16)).padEnd(32, "0");
          return encoding === "hex" ? hex : new Buffer(hex);
        },
      };
    },
    webcrypto: { getRandomValues: (array) => { for (let i = 0; i < array.length; i++) array[i] = Math.floor(Math.random() * 256); return array; } },
  };

  // ----- fetch / Response -----
  class Headers {
    constructor(init) {
      this._map = Object.create(null);
      if (init) {
        if (Array.isArray(init)) for (const [k, v] of init) this.set(k, v);
        else for (const k of Object.keys(init)) this.set(k, init[k]);
      }
    }
    set(k, v) { this._map[String(k).toLowerCase()] = String(v); }
    get(k) { return this._map[String(k).toLowerCase()] || null; }
    has(k) { return String(k).toLowerCase() in this._map; }
    delete(k) { delete this._map[String(k).toLowerCase()]; }
    append(k, v) { const key = String(k).toLowerCase(); this._map[key] = (this._map[key] ? this._map[key] + ", " : "") + String(v); }
    keys() { return Object.keys(this._map)[Symbol.iterator](); }
    values() { return Object.keys(this._map).map((k) => this._map[k])[Symbol.iterator](); }
    entries() { return Object.keys(this._map).map((k) => [k, this._map[k]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }
  class Response {
    constructor(info) {
      this.ok = info.ok; this.status = info.status; this.statusText = info.statusText;
      this.headers = new Headers(); this._body = info.body; this.bodyUsed = false;
    }
    text() { this.bodyUsed = true; return Promise.resolve(this._body === undefined || this._body === null ? "" : this._body); }
    json() { return this.text().then((text) => JSON.parse(text)); }
    arrayBuffer() { return this.text().then((text) => g.__engine_utf8_encode(text)); }
    clone() { return new Response({ ok: this.ok, status: this.status, statusText: this.statusText, body: this._body }); }
  }
  g.Headers = Headers;
  g.Response = Response;
  g.fetch = function fetch(url, options) {
    return Promise.resolve().then(() => new Response(g.__engine_fetch(String(url), options)));
  };

  // Node exposes these as globals as well as through `node:url`.
  g.URL = builtins.url.URL;
  g.URLSearchParams = builtins.url.URLSearchParams;
  g.Buffer = builtins.buffer.Buffer;
  g.escape = function escape(value) {
    const text = String(value);
    let out = "";
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      const char = text[i];
      if (/[A-Za-z0-9@*_+\-./]/.test(char)) out += char;
      else if (code < 256) out += "%" + code.toString(16).toUpperCase().padStart(2, "0");
      else out += "%u" + code.toString(16).toUpperCase().padStart(4, "0");
    }
    return out;
  };
  g.unescape = function unescape(value) {
    const text = String(value);
    let out = "";
    for (let i = 0; i < text.length;) {
      if (text[i] === "%") {
        if (text[i + 1] === "u" && /^[0-9a-fA-F]{4}$/.test(text.slice(i + 2, i + 6))) {
          out += String.fromCharCode(parseInt(text.slice(i + 2, i + 6), 16)); i += 6; continue;
        }
        if (/^[0-9a-fA-F]{2}$/.test(text.slice(i + 1, i + 3))) {
          out += String.fromCharCode(parseInt(text.slice(i + 1, i + 3), 16)); i += 3; continue;
        }
      }
      out += text[i]; i++;
    }
    return out;
  };

  // Minimal HTTP surface used by inject-style test clients (light-my-request).
  // It keeps headers/status in memory; no socket or listener is opened.
  function ServerResponse(request) {
    this._events = Object.create(null);
    this._headers = Object.create(null);
    this.statusCode = 200;
    this.statusMessage = 'OK';
    this.headersSent = false;
    this.finished = false;
    this._hasBody = true;
    this.req = request;
  }
  for (const method of ['_ensureEvents', 'emit', 'on', 'once', 'addListener',
                        'removeListener', 'off', 'listeners', 'listenerCount',
                        'setMaxListeners', 'removeAllListeners']) {
    ServerResponse.prototype[method] = EventEmitter.prototype[method];
  }
  ServerResponse.prototype.setHeader = function (name, value) {
    this._headers[String(name).toLowerCase()] = value; return this;
  };
  ServerResponse.prototype.getHeader = function (name) {
    return this._headers[String(name).toLowerCase()];
  };
  ServerResponse.prototype.removeHeader = function (name) {
    delete this._headers[String(name).toLowerCase()]; return this;
  };
  ServerResponse.prototype.getHeaders = function () { return Object.assign({}, this._headers); };
  ServerResponse.prototype.hasHeader = function (name) {
    return Object.prototype.hasOwnProperty.call(this._headers, String(name).toLowerCase());
  };
  ServerResponse.prototype.writeHead = function (status, message, headers) {
    this.statusCode = Number(status) || 200;
    if (typeof message === 'string') this.statusMessage = message;
    else if (message) headers = message;
    Object.assign(this._headers, headers || {});
    this.headersSent = true;
    return this;
  };
  ServerResponse.prototype.write = function () { return true; };
  ServerResponse.prototype.end = function () { this.finished = true; return this; };
  ServerResponse.prototype.assignSocket = function (socket) {
    this.socket = socket; this.connection = socket; return this;
  };
  ServerResponse.prototype.flushHeaders = function () { this.headersSent = true; };
  ServerResponse.prototype.setTimeout = function () { return this; };

  function IncomingMessage() {
    this._events = Object.create(null);
    this.headers = Object.create(null);
    this.method = 'GET';
    this.url = '/';
  }
  for (const method of ['_ensureEvents', 'emit', 'on', 'once', 'addListener',
                        'removeListener', 'off', 'listeners', 'listenerCount',
                        'setMaxListeners', 'removeAllListeners']) {
    IncomingMessage.prototype[method] = EventEmitter.prototype[method];
  }
  IncomingMessage.prototype.resume = function () { return this; };
  IncomingMessage.prototype.destroy = function () { return this; };

  const httpSurface = {
    STATUS_CODES: { 200: 'OK', 201: 'Created', 204: 'No Content', 301: 'Moved Permanently',
      302: 'Found', 304: 'Not Modified', 400: 'Bad Request', 401: 'Unauthorized',
      403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed', 409: 'Conflict',
      418: "I'm a Teapot", 429: 'Too Many Requests', 500: 'Internal Server Error',
      502: 'Bad Gateway', 503: 'Service Unavailable' },
    METHODS: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS', 'TRACE', 'CONNECT'],
    Agent: class Agent {}, globalAgent: {},
    request: () => ({ on: () => {}, end: () => {} }),
    get: () => ({ on: () => {}, end: () => {} }),
    ServerResponse, IncomingMessage,
    createServer: () => ({
      on: () => {}, once: () => {},
      listen: (options, callback) => {
        if (typeof options === "function") callback = options;
        if (callback) process.nextTick(callback);
        return this;
      },
      close: (callback) => { if (callback) process.nextTick(callback); return this; },
      closeIdleConnections: () => {}, closeAllConnections: () => {},
      closeHttp2Sessions: () => {},
      address: () => null, setTimeout: () => {},
    }),
  };
  builtins.http = httpSurface;
  builtins.https = Object.assign({}, httpSurface);
  builtins.http2 = Object.assign({}, httpSurface);

  // Generic fallback for any Node built-in without a dedicated shim. Instead
  // of maintaining a list of built-in names, every unknown property resolves
  // to a no-op callable, which is enough for the many packages that import a
  // built-in only to feature-detect it.
  // Names commonly imported from Node built-ins. A generic proxy reports these
  // as own keys so ESM `import { name } from "node:<any-builtin>"` resolves even
  // for built-ins that did not exist when this shim was written.
  const COMMON_EXPORTS = [
    'StringDecoder', 'parse', 'format', 'resolve', 'isAbsolute', 'join', 'dirname',
    'basename', 'extname', 'relative', 'normalize', 'sep', 'delimiter', 'posix',
    'win32', 'readdir', 'readdirSync', 'stat', 'statSync', 'lstat', 'lstatSync',
    'readFile', 'readFileSync', 'writeFile', 'writeFileSync', 'existsSync',
    'realpath', 'realpathSync', 'readlink', 'readlinkSync', 'mkdir', 'mkdirSync',
    'createServer', 'Server', 'IncomingMessage', 'ServerResponse', 'Worker',
    'isMainThread', 'parentPort', 'workerData', 'createRequire', 'builtinModules',
    'isBuiltin', 'inherits', 'promisify', 'inspect', 'debuglog', 'EventEmitter',
    'Readable', 'Writable', 'Duplex', 'Transform', 'Stream', 'pipeline',
    'finished', 'URL', 'URLSearchParams', 'fileURLToPath', 'pathToFileURL',
    'Buffer', 'SlowBuffer', 'Blob', 'performance', 'channel', 'tracingChannel',
    'subscribe', 'unsubscribe', 'randomUUID', 'randomBytes', 'createHash',
    'createHmac', 'createCipheriv', 'createDecipheriv', 'getHashes', 'webcrypto',
    'getReport', 'isatty', 'constants', 'types', 'promises', 'createBrotliCompress',
    'createGzip', 'createGunzip', 'createDeflate', 'createInflate', 'setTimeout',
    'clearTimeout', 'setInterval', 'clearInterval', 'setImmediate', 'clearImmediate',
    'exec', 'execSync', 'spawn', 'spawnSync', 'execFile', 'execFileSync', 'fork',
    'createConnection', 'connect', 'Socket', 'Server', 'resolveTxt', 'lookup',
    'gzip', 'gzipSync', 'gunzip', 'gunzipSync', 'deflate', 'deflateSync',
    'inflate', 'inflateSync', 'brotliCompress', 'brotliDecompress',
    'createBrotliCompress', 'createBrotliDecompress', 'createGzip', 'createGunzip',
    'register', 'deregister', 'getBuiltinModule', 'syncBuiltinESMExports',
  ];
  const stubCache = Object.create(null);
  // A universal value: callable, constructible, coercible to a primitive, and
  // iterable. Any unknown property is another universal value. This means a
  // brand-new Node built-in (or a new API on an existing one) works for feature
  // detection, side-effect imports and call chains without any code change.
  const makeStub = () => new Proxy(function () { return makeStub(); }, {
    ownKeys: (object) => [...new Set([...Reflect.ownKeys(object), ...COMMON_EXPORTS])],
    getOwnPropertyDescriptor(object, property) {
      if (COMMON_EXPORTS.includes(property)) {
        return { configurable: true, enumerable: true, writable: true };
      }
      return Reflect.getOwnPropertyDescriptor(object, property);
    },
    get(object, property) {
      if (property === "then" || property === "__esModule") return undefined;
      if (property === Symbol.toPrimitive) return (hint) => (hint === "number" ? 0 : "");
      if (property === Symbol.iterator) return function* () {};
      if (property === "length") return 0;
      if (property === "toString") return () => "";
      if (property === "valueOf") return () => 0;
      if (typeof property === "symbol") return undefined;
      if (!(property in object)) object[property] = makeStub();
      return object[property];
    },
    apply: () => makeStub(),
    construct: () => makeStub(),
    has: () => true,
  });
  g.__engine_generic_builtin = (name) => {
    if (!stubCache[name]) stubCache[name] = makeStub();
    return stubCache[name];
  };

  const noopChannel = () => ({
    hasSubscribers: false,
    publish: () => {},
    runStores: (_store, fn, thisArg, ...args) => fn.apply(thisArg, args),
    subscribe: () => {}, unsubscribe: () => {},
  });
  builtins.diagnostics_channel = {
    channel: noopChannel,
    subscribe: () => {}, unsubscribe: () => {},
    tracingChannel: (name) => ({
      hasSubscribers: false,
      start: noopChannel(), end: noopChannel(), error: noopChannel(),
      asyncStart: noopChannel(), asyncEnd: noopChannel(),
      traceSync: (fn) => fn(),
      tracePromise: (fn) => Promise.resolve().then(fn),
      traceCallback: (fn) => fn,
      subscribe: () => {}, unsubscribe: () => {},
    }),
  };
  builtins.vm = { Script: class Script {}, runInThisContext: (code) => g.eval(code) };
  builtins.perf_hooks = { performance: g.performance, PerformanceObserver: class PerformanceObserver {} };

  g.__engine_builtins = builtins;
})();
"##;
