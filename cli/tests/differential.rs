//! Differential test harness.
//!
//! Every `examples/javascript/*.js` program is run through the real `engine`
//! binary four ways -- JIT and reference interpreter, each with and without the
//! optimiser -- and all four outputs must agree. If a `<name>.expected` file sits
//! next to the example, the output must also match it. The `.expected` files are
//! generated from real Node.js (`node file.js`), so they are an independent oracle.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/javascript")
}

fn run_engine(file: &Path, flags: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_engine"))
        .arg("run")
        .arg(file)
        .args(flags)
        .output()
        .expect("failed to launch the engine binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn every_example_agrees_across_backends_and_optimiser() {
    let mut files: Vec<PathBuf> = fs::read_dir(examples_dir())
        .expect("examples/javascript exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map_or(false, |e| e == "js"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no examples found");

    let configs: [(&str, &[&str]); 4] = [
        ("jit", &[]),
        ("jit --no-opt", &["--no-opt"]),
        ("interp", &["--interp"]),
        ("interp --no-opt", &["--interp", "--no-opt"]),
    ];

    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let expected = fs::read_to_string(file.with_extension("expected")).ok();

        let (reference_label, reference_flags) = configs[2];
        let (reference, stderr, ok) = run_engine(file, reference_flags);
        assert!(ok, "{name} failed under {reference_label}: {stderr}");
        if let Some(expected) = &expected {
            assert_eq!(&reference, expected, "{name}: output differs from the Node.js oracle");
        }

        for (label, flags) in configs {
            let (stdout, stderr, ok) = run_engine(file, flags);
            assert!(ok, "{name} failed under {label}: {stderr}");
            assert_eq!(stdout, reference, "{name}: `{label}` disagrees with `{reference_label}`");
        }
    }
}

#[test]
fn compile_errors_exit_nonzero_with_a_located_message() {
    let dir = std::env::temp_dir().join(format!("engine-diff-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bad.js");
    fs::write(&file, "let x = 1;\nconsole.log(x + true);\n").unwrap();
    let (stdout, stderr, ok) = run_engine(&file, &[]);
    assert!(!ok);
    assert!(stdout.is_empty());
    assert!(stderr.contains("bad.js:2:"), "{stderr}");
    assert!(stderr.contains("error:"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn runaway_recursion_fails_cleanly_in_both_backends() {
    let dir = std::env::temp_dir().join(format!("engine-overflow-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("overflow.js");
    fs::write(&file, "function f(n) { return f(n + 1); }\nconsole.log(f(0));\n").unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let (stdout, stderr, ok) = run_engine(&file, flags);
        assert!(!ok, "expected a non-zero exit with {flags:?}");
        assert!(stdout.is_empty());
        assert!(stderr.contains("Maximum call stack size exceeded"), "{flags:?}: {stderr}");
    }
    let _ = fs::remove_dir_all(&dir);
}
