//! Runtime support: helpers called from generated machine code, number
//! formatting, the per-invocation execution context, runtime errors, and a
//! reference IR interpreter used for differential testing.
//!
//! The runtime never terminates the host process. Failures are recorded in the
//! [`ExecContext`] (native code) or returned as [`RuntimeError`] (interpreter),
//! and the embedder decides how to report them.

pub mod interp;

use std::fmt;
use std::io::Write;

/// Maximum call depth shared by the interpreter and native code, so both
/// report the same `Maximum call stack size exceeded` condition.
pub const MAX_CALL_DEPTH: u32 = 10_000;

/// A failure that stops program execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// `MAX_CALL_DEPTH` was exceeded.
    StackOverflow,
    /// Malformed IR or an I/O failure; indicates a bug or a broken host.
    Internal(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::StackOverflow => write!(f, "RangeError: Maximum call stack size exceeded"),
            RuntimeError::Internal(msg) => write!(f, "internal runtime error: {msg}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Error codes stored in [`ExecContext::error`].
pub const ERR_NONE: u32 = 0;
pub const ERR_STACK_OVERFLOW: u32 = 1;

/// Mutable state of one program execution.
///
/// Native code receives a pointer to this struct as a hidden first argument of
/// every generated function, so each invocation owns its call-depth counter and
/// error flag. Nothing is shared between invocations of the same compiled code.
/// The layout is part of the ABI with the code generator.
#[repr(C)]
#[derive(Debug, Default)]
pub struct ExecContext {
    /// Current call depth.
    pub depth: u32,
    /// `ERR_NONE` while the program is healthy; once non-zero, generated code
    /// unwinds by returning from every frame without running further code.
    pub error: u32,
}

/// Byte offsets of the [`ExecContext`] fields, used by the code generator.
pub const CTX_DEPTH_OFFSET: i32 = 0;
pub const CTX_ERROR_OFFSET: i32 = 4;

impl ExecContext {
    /// Converts the recorded error code into a result.
    pub fn status(&self) -> Result<(), RuntimeError> {
        match self.error {
            ERR_NONE => Ok(()),
            ERR_STACK_OVERFLOW => Err(RuntimeError::StackOverflow),
            other => Err(RuntimeError::Internal(format!("unknown error code {other}"))),
        }
    }
}

/// Formats a number the way ECMAScript specifies for `console.log` / `String()`.
///
/// Implements the ECMAScript Number-to-String algorithm (section 7.1.12.1):
/// - NaN → `"NaN"`, ±Infinity → `"Infinity"` / `"-Infinity"`, -0 → `"0"`
/// - Integers in `(-1e21, 1e21)` print without a decimal point
/// - Values with absolute value ≥ 1e21 or in `(0, 1e-7)` use `Xe+Y` / `Xe-Y` notation
/// - Other non-integers use shortest round-trip decimal representation
const TWO_POW_32: f64 = 4_294_967_296.0;

/// ECMAScript `ToUint32` (<https://tc39.es/ecma262/#sec-touint32>).
pub fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let modulo = value.trunc().rem_euclid(TWO_POW_32);
    modulo as u32
}

/// ECMAScript `ToInt32` (<https://tc39.es/ecma262/#sec-toint32>).
pub fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

pub fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }

    // ECMAScript Number::toString uses scientific notation when the decimal
    // exponent is >= 21 or <= -7, i.e. |value| >= 1e21 or |value| < 1e-6.
    let abs = value.abs();
    if !(1e-6..1e21).contains(&abs) {
        let mut formatted = format!("{value:e}");
        if let Some(split) = formatted.find('e') {
            let (mantissa, exp) = formatted.split_at(split);
            let exp_digits = exp.strip_prefix('e').unwrap_or("");
            let (sign, digits) = if let Some(digits) = exp_digits.strip_prefix('-') {
                ('-', digits)
            } else {
                ('+', exp_digits)
            };
            let trimmed = digits.trim_start_matches('0');
            let digits = if trimmed.is_empty() { "0" } else { trimmed };
            formatted = format!("{mantissa}e{sign}{digits}");
        }
        return formatted;
    }

    // Integer values below 1e21 print without decimal point.
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }

    value.to_string()
}

// ---- helpers called from JIT-compiled code (C ABI) ----

pub extern "C" fn rt_print_f64(v: f64) {
    let _ = writeln!(std::io::stdout(), "{}", format_number(v));
}

/// Booleans are passed zero-extended to 32 bits to keep the ABI unambiguous.
pub extern "C" fn rt_print_bool(v: u32) {
    let _ = writeln!(std::io::stdout(), "{}", if v != 0 { "true" } else { "false" });
}

pub struct JsString {
    pub ptr: *const u8,
    pub len: usize,
    /// When true, `ptr` was heap allocated by `rt_concat_str` and owned.
    pub owned: bool,
}

/// Creates a new `JsString` handle on the heap and returns its pointer.
pub extern "C" fn rt_alloc_str(ptr: *const u8, len: usize) -> *mut JsString {
    Box::into_raw(Box::new(JsString { ptr, len, owned: false }))
}

/// Concatenates two `JsString` handles into a new owned heap-allocated `JsString`.
pub extern "C" fn rt_concat_str(a: *const JsString, b: *const JsString) -> *mut JsString {
    let (ptr_a, len_a) = if a.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*a).ptr, (*a).len) } };
    let (ptr_b, len_b) = if b.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*b).ptr, (*b).len) } };

    let str_a = if !ptr_a.is_null() && len_a > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_a, len_a)) }
    } else {
        ""
    };
    let str_b = if !ptr_b.is_null() && len_b > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_b, len_b)) }
    } else {
        ""
    };

    let joined = format!("{str_a}{str_b}");
    let bytes = joined.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    Box::into_raw(Box::new(JsString { ptr, len, owned: true }))
}

/// Prints a `JsString` handle.
pub extern "C" fn rt_print_str(s: *const JsString) {
    if !s.is_null() {
        let (ptr, len) = unsafe { ((*s).ptr, (*s).len) };
        if !ptr.is_null() && len > 0 {
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            let text = String::from_utf8_lossy(bytes);
            let _ = writeln!(std::io::stdout(), "{text}");
            return;
        }
    }
    let _ = writeln!(std::io::stdout());
}

/// Compares two `JsString` handles for equality. Returns 1 if equal, 0 if not.
pub extern "C" fn rt_str_eq(a: *const JsString, b: *const JsString) -> u32 {
    let (ptr_a, len_a) = if a.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*a).ptr, (*a).len) } };
    let (ptr_b, len_b) = if b.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*b).ptr, (*b).len) } };

    if len_a != len_b {
        return 0;
    }
    if len_a == 0 {
        return 1;
    }
    let bytes_a = unsafe { std::slice::from_raw_parts(ptr_a, len_a) };
    let bytes_b = unsafe { std::slice::from_raw_parts(ptr_b, len_b) };
    if bytes_a == bytes_b { 1 } else { 0 }
}

/// Converts an f64 number to a JsString handle using ECMAScript format.
pub extern "C" fn rt_num_to_str(val: f64) -> *mut JsString {
    let text = format_number(val);
    let bytes = text.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    Box::into_raw(Box::new(JsString { ptr, len, owned: true }))
}

/// Converts a boolean (passed as u32) to a JsString handle.
pub extern "C" fn rt_bool_to_str(val: u32) -> *mut JsString {
    let text = if val != 0 { "true" } else { "false" };
    rt_alloc_str(text.as_ptr(), text.len())
}

/// Parses a JsString handle into an f64 number per ECMAScript rules.
pub extern "C" fn rt_str_to_num(s: *const JsString) -> f64 {
    let (ptr, len) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let text = if !ptr.is_null() && len > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
    } else {
        ""
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// Converts a boolean (passed as u32) to f64 (1.0 or 0.0).
pub extern "C" fn rt_bool_to_num(val: u32) -> f64 {
    if val != 0 { 1.0 } else { 0.0 }
}

/// Returns the byte/character length of a JsString handle as f64.
pub extern "C" fn rt_str_len(s: *const JsString) -> f64 {
    if s.is_null() {
        return 0.0;
    }
    unsafe { (*s).len as f64 }
}

pub extern "C" fn rt_str_upper(s: *const JsString) -> *mut JsString {
    let (ptr, len) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let text = if !ptr.is_null() && len > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
    } else {
        ""
    };
    let upper = text.to_uppercase();
    let bytes = upper.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    Box::into_raw(Box::new(JsString { ptr, len, owned: true }))
}

pub extern "C" fn rt_str_includes(s: *const JsString, needle: *const JsString) -> u32 {
    let (ptr_s, len_s) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let (ptr_n, len_n) = if needle.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*needle).ptr, (*needle).len) } };
    let text = if !ptr_s.is_null() && len_s > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_s, len_s)) }
    } else {
        ""
    };
    let n = if !ptr_n.is_null() && len_n > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_n, len_n)) }
    } else {
        ""
    };
    if text.contains(n) { 1 } else { 0 }
}

pub extern "C" fn rt_str_slice(s: *const JsString, start: f64, end: f64) -> *mut JsString {
    let (ptr, len) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let text = if !ptr.is_null() && len > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
    } else {
        ""
    };
    let text_len = text.len() as i64;
    let mut st = start as i64;
    let mut en = end as i64;
    if st < 0 { st = (text_len + st).max(0); } else { st = st.min(text_len); }
    if en < 0 { en = (text_len + en).max(0); } else { en = en.min(text_len); }
    if en < st { en = st; }
    let sub = &text[st as usize..en as usize];
    let bytes = sub.to_string().into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    Box::into_raw(Box::new(JsString { ptr, len, owned: true }))
}

pub extern "C" fn rt_str_replace(s: *const JsString, from: *const JsString, to: *const JsString) -> *mut JsString {
    let (ptr_s, len_s) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let (ptr_f, len_f) = if from.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*from).ptr, (*from).len) } };
    let (ptr_t, len_t) = if to.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*to).ptr, (*to).len) } };
    let text = if !ptr_s.is_null() && len_s > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_s, len_s)) }
    } else {
        ""
    };
    let f = if !ptr_f.is_null() && len_f > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_f, len_f)) }
    } else {
        ""
    };
    let t = if !ptr_t.is_null() && len_t > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_t, len_t)) }
    } else {
        ""
    };
    let replaced = text.replacen(f, t, 1);
    let bytes = replaced.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    Box::into_raw(Box::new(JsString { ptr, len, owned: true }))
}

pub extern "C" fn rt_str_split_count(s: *const JsString, sep: *const JsString) -> f64 {
    let (ptr_s, len_s) = if s.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*s).ptr, (*s).len) } };
    let (ptr_sep, len_sep) = if sep.is_null() { (std::ptr::null(), 0) } else { unsafe { ((*sep).ptr, (*sep).len) } };
    let text = if !ptr_s.is_null() && len_s > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_s, len_s)) }
    } else {
        ""
    };
    let sep_str = if !ptr_sep.is_null() && len_sep > 0 {
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr_sep, len_sep)) }
    } else {
        ""
    };
    text.split(sep_str).count() as f64
}

pub extern "C" fn rt_fmod(a: f64, b: f64) -> f64 {
    a % b
}

pub struct JsArray {
    pub elements: Vec<f64>,
}

pub extern "C" fn rt_alloc_array(capacity: usize) -> *mut JsArray {
    Box::into_raw(Box::new(JsArray {
        elements: Vec::with_capacity(capacity),
    }))
}

pub extern "C" fn rt_array_push(arr: *mut JsArray, val: f64) {
    if !arr.is_null() {
        unsafe {
            (*arr).elements.push(val);
        }
    }
}

pub extern "C" fn rt_array_get(arr: *const JsArray, index: f64) -> f64 {
    if !arr.is_null() && index >= 0.0 {
        let i = index as usize;
        unsafe {
            let elems = &(*arr).elements;
            if let Some(&v) = elems.get(i) {
                return v;
            }
        }
    }
    f64::NAN
}

pub extern "C" fn rt_array_set(arr: *mut JsArray, index: f64, val: f64) {
    if !arr.is_null() && index >= 0.0 {
        let i = index as usize;
        unsafe {
            let elems = &mut (*arr).elements;
            if i < elems.len() {
                elems[i] = val;
            } else if i == elems.len() {
                elems.push(val);
            }
        }
    }
}

pub extern "C" fn rt_array_len(arr: *const JsArray) -> f64 {
    if arr.is_null() {
        0.0
    } else {
        unsafe { (*arr).elements.len() as f64 }
    }
}

pub struct JsObject {
    pub num_props: std::collections::HashMap<String, f64>,
    pub str_props: std::collections::HashMap<String, *mut JsString>,
    pub obj_props: std::collections::HashMap<String, *mut JsObject>,
}

pub extern "C" fn rt_alloc_obj() -> *mut JsObject {
    Box::into_raw(Box::new(JsObject {
        num_props: std::collections::HashMap::new(),
        str_props: std::collections::HashMap::new(),
        obj_props: std::collections::HashMap::new(),
    }))
}

pub extern "C" fn rt_obj_set_num(obj: *mut JsObject, key: *const JsString, val: f64) {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            (*obj).num_props.insert(k, val);
        }
    }
}

pub extern "C" fn rt_obj_get_num(obj: *const JsObject, key: *const JsString) -> f64 {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            if let Some(&v) = (*obj).num_props.get(&k) {
                return v;
            }
        }
    }
    f64::NAN
}

pub extern "C" fn rt_obj_set_str(obj: *mut JsObject, key: *const JsString, val: *mut JsString) {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            (*obj).str_props.insert(k, val);
        }
    }
}

pub extern "C" fn rt_obj_get_str(obj: *const JsObject, key: *const JsString) -> *mut JsString {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            if let Some(&v) = (*obj).str_props.get(&k) {
                return v;
            }
        }
    }
    std::ptr::null_mut()
}

pub extern "C" fn rt_obj_set_obj(obj: *mut JsObject, key: *const JsString, val: *mut JsObject) {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            (*obj).obj_props.insert(k, val);
        }
    }
}

pub extern "C" fn rt_obj_get_obj(obj: *const JsObject, key: *const JsString) -> *mut JsObject {
    if !obj.is_null() && !key.is_null() {
        let k = unsafe {
            let bytes = std::slice::from_raw_parts((*key).ptr, (*key).len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe {
            if let Some(&v) = (*obj).obj_props.get(&k) {
                return v;
            }
        }
    }
    std::ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_layout_matches_the_code_generator() {
        assert_eq!(std::mem::offset_of!(ExecContext, depth) as i32, CTX_DEPTH_OFFSET);
        assert_eq!(std::mem::offset_of!(ExecContext, error) as i32, CTX_ERROR_OFFSET);
    }

    #[test]
    fn context_status_maps_error_codes() {
        let mut ctx = ExecContext::default();
        assert_eq!(ctx.status(), Ok(()));
        ctx.error = ERR_STACK_OVERFLOW;
        assert_eq!(ctx.status(), Err(RuntimeError::StackOverflow));
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(610.0), "610");
        assert_eq!(format_number(-3.0), "-3");
        assert_eq!(format_number(0.25), "0.25");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(f64::NAN), "NaN");
        assert_eq!(format_number(f64::INFINITY), "Infinity");
        assert_eq!(format_number(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(format_number(1e20), "100000000000000000000");
        // ECMAScript: values >= 1e21 use exponential notation.
        assert_eq!(format_number(1e21), "1e+21");
        assert_eq!(format_number(1.5e21), "1.5e+21");
        assert_eq!(format_number(-1e21), "-1e+21");
        // ECMAScript: values in (0, 1e-7) use exponential notation.
        assert_eq!(format_number(5e-324), "5e-324");
        assert_eq!(format_number(1e-7), "1e-7");
        // Values just above 1e-7 do NOT use exponential.
        assert_eq!(format_number(0.000001), "0.000001");
    }
}
