//! TypeScript type checking powered by tsz (tsz-org/tsz).
//!
//! `tsz-core` is embedded as a library: no external process, no Node, no
//! network at check time. `WasmProgram` is tsz's native-capable multi-file
//! program API; the bundled `lib.esnext.full.d.ts` supplies global types.

#[derive(Debug)]
pub struct Report {
    /// Rendered diagnostics (one per line), or a success message.
    pub text: String,
    /// Number of error-level diagnostics; used as the CLI exit code.
    pub errors: usize,
}

/// Type-checks one TypeScript source with the bundled ESNext standard library.
pub fn check_file(path: &str, source: &str) -> Result<Report, String> {
    let mut program = tsz::WasmProgram::new();
    program
        .set_compiler_options(
            r#"{"strict":true,"target":12,"module":99}"#,
        )
        .map_err(|error| format!("invalid tsz compiler options: {error:?}"))?;

    // esnext.full covers the language; dom adds browser globals such as `console`.
    for name in ["es5.d.ts", "esnext.full.d.ts", "dom.d.ts"] {
        if let Some(lib) = tsz::embedded_libs::get_lib_content(name) {
            program.add_lib_file(name.to_owned(), lib.to_owned());
        }
    }
    program.add_file(path.to_owned(), source.to_owned());

    let json = program.check_all();
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|error| format!("invalid tsz result: {error}"))?;

    let mut lines = Vec::new();
    let mut errors = 0;
    if let Some(files) = value.get("files").and_then(|v| v.as_array()) {
        for file in files {
            let name = file
                .get("fileName")
                .and_then(|v| v.as_str())
                .unwrap_or(path);
            for key in ["parseDiagnostics", "checkDiagnostics"] {
                let Some(diagnostics) = file.get(key).and_then(|v| v.as_array()) else {
                    continue;
                };
                for diagnostic in diagnostics {
                    let message = diagnostic
                        .get("messageText")
                        .or_else(|| diagnostic.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown type error");
                    let code = diagnostic.get("code").and_then(|v| v.as_u64()).unwrap_or(0);
                    let start = diagnostic.get("start").and_then(|v| v.as_u64()).unwrap_or(0);
                    let category = diagnostic
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Error");
                    let (line, column) = line_column(source, start);
                    if category == "Error" {
                        errors += 1;
                    }
                    lines.push(format!(
                        "{name}({line},{column}): {category} TS{code}: {message}"
                    ));
                }
            }
        }
    }

    let text = if lines.is_empty() {
        format!("No type errors in {path} (checked with tsz).")
    } else {
        lines.join("\n")
    };
    Ok(Report { text, errors })
}

/// Converts a tsz byte offset into a 1-based line and column.
fn line_column(source: &str, offset: u64) -> (usize, usize) {
    let limit = (offset as usize).min(source.len());
    let mut boundary = 0;
    for (index, _) in source.char_indices() {
        if index >= limit {
            break;
        }
        boundary = index;
    }
    if limit == source.len() {
        boundary = limit;
    }
    let before = &source[..boundary];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len() + 1, |newline| before.len() - newline);
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_typescript_has_no_errors() {
        let report = check_file("good.ts", "const x: number = 1;\nconsole.log(x);\n").unwrap();
        assert_eq!(report.errors, 0, "{}", report.text);
    }

    #[test]
    fn bad_typescript_reports_an_error() {
        let report = check_file("bad.ts", "const x: number = 'oops';\n").unwrap();
        assert!(report.errors > 0, "{}", report.text);
        assert!(report.text.contains("TS"), "{}", report.text);
    }
}
