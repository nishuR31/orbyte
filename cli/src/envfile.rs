//! Minimal `.env` loading, matching Bun's "do not override existing variables"
//! rule. Supports comments, `export`, and single/double quoted values.

use std::env;
use std::fs;
use std::path::Path;

/// Loads the automatic `.env` chain plus any explicit `--env-file` paths.
pub(crate) fn load(explicit: &[String], auto: bool) -> Result<(), String> {
    if auto {
        let mut files = vec![".env".to_owned(), ".env.local".to_owned()];
        let mode = env::var("ORB_ENV")
            .or_else(|_| env::var("NODE_ENV"))
            .or_else(|_| env::var("BUN_ENV"));
        if let Ok(mode) = mode {
            if !mode.is_empty() {
                files.push(format!(".env.{mode}"));
                files.push(format!(".env.{mode}.local"));
            }
        }
        for file in files {
            let path = Path::new(&file);
            if path.is_file() {
                load_file(path)?;
            }
        }
    }

    for file in explicit {
        let path = Path::new(file);
        if !path.is_file() {
            return Err(format!("env file not found: '{file}'"));
        }
        load_file(path)?;
    }
    Ok(())
}

fn load_file(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read env file '{}': {error}", path.display()))?;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = parse_value(value.trim());
        // Explicit process environment always wins, matching Node/Bun.
        if env::var_os(key).is_none() {
            env::set_var(key, value);
        }
        let _ = line_number;
    }
    Ok(())
}

fn parse_value(raw: &str) -> String {
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if (bytes[0] == b'"' && bytes[raw.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\'')
        {
            let inner = &raw[1..raw.len() - 1];
            let mut output = String::with_capacity(inner.len());
            let mut characters = inner.chars();
            while let Some(character) = characters.next() {
                if character == '\\' {
                    match characters.next() {
                        Some('n') => output.push('\n'),
                        Some('r') => output.push('\r'),
                        Some('t') => output.push('\t'),
                        Some('\\') => output.push('\\'),
                        Some('"') => output.push('"'),
                        Some('\'') => output.push('\''),
                        Some(other) => {
                            output.push('\\');
                            output.push(other);
                        }
                        None => output.push('\\'),
                    }
                } else {
                    output.push(character);
                }
            }
            return output;
        }
        if raw.starts_with('"') || raw.starts_with('\'') {
            return raw.trim_matches(['"', '\'']).to_owned();
        }
    }
    // Strip an unquoted trailing comment.
    raw.split(" #").next().unwrap_or(raw).trim_end().to_owned()
}
