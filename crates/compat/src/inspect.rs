//! Node-shaped value inspection for `console.log`.
//!
//! Boa's built-in inspector is designed for humans debugging a context: it
//! indents over several lines, double-quotes strings, and abbreviates nested
//! arrays as `Array(2)`. Node prints a compact, single-line representation
//! with single-quoted strings and fully expanded nesting. This module bridges
//! that gap for the values a CLI program normally logs. Exotic built-ins fall
//! back to Boa's display rather than guessing their format.

use boa_engine::object::JsObject;
use boa_engine::object::builtins::{JsMap, JsSet};
use boa_engine::property::PropertyKey;
use boa_engine::{Context, JsString, JsValue, js_string};

const MAX_DEPTH: usize = 4;
const MAX_ITEMS: usize = 100;

/// Formats one logged value. Strings are unquoted at the top level, as in
/// `console.log("hello")`, but quoted when nested inside an object or array.
pub(crate) fn format_value(value: &JsValue, context: &mut Context) -> String {
    let mut seen = Vec::new();
    format_inner(value, context, 0, &mut seen, true)
}

fn format_inner(
    value: &JsValue,
    context: &mut Context,
    depth: usize,
    seen: &mut Vec<JsObject>,
    top: bool,
) -> String {
    if let Some(string) = value.as_string() {
        let text = string.to_std_string_escaped();
        return if top { text } else { quote(&text) };
    }
    if value.is_undefined() {
        return "undefined".to_owned();
    }
    if value.is_null() {
        return "null".to_owned();
    }
    if value.as_boolean().is_some() || value.as_number().is_some() {
        return value.display().to_string();
    }
    if let Some(bigint) = value.as_bigint() {
        return format!("{}n", bigint.to_string());
    }
    if value.as_symbol().is_some() {
        return value.display().to_string();
    }
    let Some(object) = value.as_object() else {
        return value.display().to_string();
    };
    if object.is_callable() {
        let name = object
            .get(js_string!("name"), context)
            .ok()
            .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()))
            .filter(|name| !name.is_empty());
        return match name {
            Some(name) => format!("[Function: {name}]"),
            None => "[Function (anonymous)]".to_owned(),
        };
    }
    if seen.iter().any(|previous| JsObject::equals(previous, &object)) {
        return "[Circular *1]".to_owned();
    }
    if depth >= MAX_DEPTH {
        return if object.is_array() { "[Array]".into() } else { "[Object]".into() };
    }
    seen.push(object.clone());

    // Maps and sets keep their contents in internal slots, not own properties.
    if let Ok(map) = JsMap::from_object(object.clone()) {
        let mut parts = Vec::new();
        let result = map.for_each_native(|key, value| {
            let k = format_inner(&key, context, depth + 1, seen, false);
            let v = format_inner(&value, context, depth + 1, seen, false);
            parts.push(format!("{k} => {v}"));
            Ok(())
        });
        if result.is_ok() {
            let size = map
                .get_size(context)
                .ok()
                .and_then(|value| value.as_number())
                .unwrap_or(parts.len() as f64) as usize;
            seen.pop();
            return format!("Map({size}) {{ {} }}", parts.join(", "));
        }
    }
    if let Ok(set) = JsSet::from_object(object.clone()) {
        let mut parts = Vec::new();
        let result = set.for_each_native(|value| {
            parts.push(format_inner(&value, context, depth + 1, seen, false));
            Ok(())
        });
        if result.is_ok() {
            let size = set.size();
            seen.pop();
            return format!("Set({size}) {{ {} }}", parts.join(", "));
        }
    }

    let result = if object.is_array() {
        format_array(&object, context, depth, seen)
    } else {
        format_object(&object, context, depth, seen)
    };

    seen.pop();
    result
}

fn format_array(
    array: &JsObject,
    context: &mut Context,
    depth: usize,
    seen: &mut Vec<JsObject>,
) -> String {
    let Ok(length_value) = array.get(js_string!("length"), context) else {
        return JsValue::from(array.clone()).display().to_string();
    };
    let length = length_value.as_number().unwrap_or(0.0) as usize;
    if length == 0 {
        return "[]".to_owned();
    }
    let mut parts = Vec::new();
    for index in 0..length.min(MAX_ITEMS) {
        let value = match array.get(index as u32, context) {
            Ok(value) => value,
            Err(_) => continue,
        };
        // Holes print as empty slots in Node.
        if value.is_undefined() && !array.has_own_property(index as u32, context).unwrap_or(false) {
            parts.push(String::new());
        } else {
            parts.push(format_inner(&value, context, depth + 1, seen, false));
        }
    }
    if length > MAX_ITEMS {
        parts.push(format!("... {} more items", length - MAX_ITEMS));
    }
    format!("[ {} ]", parts.join(", "))
}

fn format_object(
    object: &JsObject,
    context: &mut Context,
    depth: usize,
    seen: &mut Vec<JsObject>,
) -> String {
    let constructor = object
        .get(js_string!("constructor"), context)
        .ok()
        .and_then(|value| value.as_object())
        .and_then(|constructor| constructor.get(js_string!("name"), context).ok())
        .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();

    // Errors render as `Name: message`, the way Node prints them in an array.
    if constructor.contains("Error") {
        let message = object
            .get(js_string!("message"), context)
            .ok()
            .and_then(|value| value.as_string().map(|s| s.to_std_string_escaped()))
            .unwrap_or_default();
        return if message.is_empty() {
            constructor
        } else {
            format!("{constructor}: {message}")
        };
    }

    let keys = match object.own_property_keys(context) {
        Ok(keys) => keys,
        Err(_) => return JsValue::from(object.clone()).display().to_string(),
    };
    let mut parts = Vec::new();
    for key in keys {
        // Symbol keys are not shown by console.log; numeric keys are.
        let name = match key {
            PropertyKey::String(name) => name.to_std_string_escaped(),
            PropertyKey::Index(index) => index.get().to_string(),
            PropertyKey::Symbol(_) => continue,
        };
        let value = match object.get(JsString::from(name.as_str()), context) {
            Ok(value) => value,
            Err(_) => continue,
        };
        parts.push(format!(
            "{}: {}",
            property_key(&name),
            format_inner(&value, context, depth + 1, seen, false)
        ));
    }
    let prefix = if constructor.is_empty() || constructor == "Object" {
        String::new()
    } else {
        format!("{constructor} ")
    };
    format!("{prefix}{{ {} }}", parts.join(", "))
}

fn property_key(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    {
        name.to_owned()
    } else {
        quote(name)
    }
}

/// Node uses single quotes for strings unless that would need more escaping.
fn quote(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_quotes_match_node() {
        assert_eq!(property_key("a"), "a");
        assert_eq!(property_key("first-name"), "'first-name'");
        assert_eq!(quote("two"), "'two'");
        assert_eq!(quote("it's"), "\"it's\"");
        assert_eq!(quote("a\nb"), "'a\\nb'");
    }
}
