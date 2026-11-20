//! `orb typecheck <file>`: TypeScript diagnostics from the embedded tsz checker.

use std::fs;
use std::path::Path;

pub(crate) fn command_typecheck(args: &[String]) -> i32 {
    let mut input: Option<String> = None;
    for arg in args {
        if arg.starts_with('-') {
            eprintln!("error: unknown flag for typecheck: {arg}");
            return 2;
        }
        if input.is_some() {
            eprintln!("error: typecheck takes exactly one file");
            return 2;
        }
        input = Some(arg.clone());
    }
    let Some(input) = input else {
        eprintln!("error: typecheck requires a file (for example: orb typecheck app.ts)");
        return 2;
    };
    let path = Path::new(&input);
    if !path.is_file() {
        eprintln!("error: cannot find file '{input}'");
        return 1;
    }
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read '{input}': {error}");
            return 1;
        }
    };
    match ::typecheck::check_file(&input, &source) {
        Ok(report) => {
            println!("{}", report.text);
            if report.errors > 0 { 1 } else { 0 }
        }
        Err(error) => {
            eprintln!("error: type checking failed: {error}");
            1
        }
    }
}
