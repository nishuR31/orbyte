//! Integration tests for the command surface added in 3.0: eval/print, check,
//! CommonJS, TypeScript, the test runner, delegation refusal, and the
//! `orb install bun` dry run.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn orb() -> Command {
    Command::new(env!("CARGO_BIN_EXE_orb"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orb-cli-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory should be creatable");
    dir
}

fn run(args: &[&str]) -> (String, String, i32) {
    let output = orb()
        .args(args)
        .output()
        .expect("the orb binary should start");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn evaluates_and_prints_expressions() {
    let (stdout, stderr, code) = run(&["-e", "console.log(6 * 7)"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty(), "{stderr}");

    let (stdout, stderr, code) = run(&["-p", "6 * 7"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "42\n");

    let (stdout, stderr, code) = run(&["--print", "\"a\" + \"b\""]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "ab\n");
}

#[test]
fn check_parses_without_executing() {
    let dir = scratch("check");
    let good = dir.join("good.js");
    let bad = dir.join("bad.js");
    fs::write(&good, "console.log('not printed');\n").unwrap();
    fs::write(&bad, "console.log(1\n").unwrap();

    let (stdout, stderr, code) = run(&["check", good.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");

    let (_, stderr, code) = run(&["--check", bad.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("SyntaxError"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commonjs_require_and_cache_work() {
    let dir = scratch("commonjs");
    fs::write(
        dir.join("counter.cjs"),
        "let count = 0;\nmodule.exports = { next: () => ++count };\n",
    )
    .unwrap();
    let entry = dir.join("main.cjs");
    fs::write(
        &entry,
        "const a = require('./counter.cjs');\n\
         const b = require('./counter.cjs');\n\
         console.log(a.next(), b.next(), __dirname.endsWith('commonjs'));\n",
    )
    .unwrap();

    let (stdout, stderr, code) = run(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "1 2 true\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn esm_imports_and_typescript_run() {
    let dir = scratch("modules");
    fs::write(dir.join("dep.ts"), "export const value: number = 21;\n").unwrap();
    let entry = dir.join("main.ts");
    fs::write(
        &entry,
        "import { value } from './dep.ts';\n\
         const doubled: number = value * 2;\n\
         console.log(doubled);\n",
    )
    .unwrap();

    let (stdout, stderr, code) = run(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "42\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_runner_reports_pass_fail_and_skip() {
    let dir = scratch("tests");
    fs::write(
        dir.join("math.test.ts"),
        "describe('math', () => {\n\
           test('adds', () => { expect(1 + 1).toBe(2); });\n\
           test.skip('later', () => { throw new Error('no'); });\n\
         });\n",
    )
    .unwrap();
    fs::write(
        dir.join("fail.test.js"),
        "test('fails', () => { expect(2).toBe(3); });\n",
    )
    .unwrap();

    let (stdout, stderr, code) = run(&["test", dir.to_str().unwrap()]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.contains("(pass) math > adds"), "{stdout}");
    assert!(stdout.contains("(skip) math > later"), "{stdout}");
    assert!(stdout.contains("(fail) fails"), "{stdout}");
    assert!(stdout.contains("1 pass, 1 fail, 1 skip"), "{stdout}");

    fs::write(
        dir.join("fail.test.js"),
        "test('passes', () => { expect(2).toBe(2); });\n",
    )
    .unwrap();
    let (stdout, stderr, code) = run(&["test", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("2 pass, 0 fail"), "{stdout}");

    let (stdout, stderr, code) = run(&[
        "test",
        dir.to_str().unwrap(),
        "--test-name-pattern",
        "adds",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("(pass) math > adds"), "{stdout}");
    assert!(!stdout.contains("(pass) passes"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn install_bun_dry_run_prints_the_installer() {
    let (stdout, stderr, code) = run(&["install", "bun", "--force", "--dry-run"]);
    assert_eq!(code, 0, "{stderr}");
    if cfg!(windows) {
        assert!(stdout.contains("bun.sh/install.ps1"), "{stdout}");
    } else {
        assert!(stdout.contains("bun.sh/install"), "{stdout}");
    }

    let (stdout, stderr, code) = run(&["install", "bun", "--force", "--dry-run", "--npm"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("npm install -g bun"), "{stdout}");
}

#[test]
fn delegation_can_be_disabled() {
    let output = orb()
        .args(["add", "left-pad"])
        .env("ORB_NO_DELEGATE", "1")
        .output()
        .expect("the orb binary should start");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("delegation is disabled"), "{stderr}");
}

#[test]
fn stdin_programs_run() {
    let mut child = orb()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the orb binary should start");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"console.log('stdin', 40 + 2);\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "stdin 42\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_dash_reads_stdin() {
    let mut child = orb()
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the orb binary should start");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"console.log('dash', 40 + 2);\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "dash 42\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn bare_package_specifiers_resolve_for_esm_and_cjs() {
    let dir = scratch("bare-packages");
    let package = dir.join("node_modules/demo-pkg");
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("package.json"),
        r#"{"name":"demo-pkg","version":"1.0.0","main":"index.js"}"#,
    )
    .unwrap();
    fs::write(
        package.join("index.js"),
        "exports.greet = (name) => 'Hello ' + name;\n",
    )
    .unwrap();
    fs::write(
        dir.join("entry.mjs"),
        "import { greet } from 'demo-pkg'; console.log(greet('ESM'));\n",
    )
    .unwrap();
    fs::write(
        dir.join("entry.cjs"),
        "console.log(require('demo-pkg').greet('CJS'));\n",
    )
    .unwrap();

    let esm = dir.join("entry.mjs");
    let cjs = dir.join("entry.cjs");
    let (stdout, stderr, code) = run(&["run", esm.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "Hello ESM\n");
    let (stdout, stderr, code) = run(&["run", cjs.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "Hello CJS\n");
}

#[test]
fn node_builtins_are_available_in_eval() {
    let (stdout, stderr, code) = run(&[
        "-e",
        "const path = require('node:path'); const assert = require('node:assert'); \
         console.log(path.join('a', 'b')); assert.strictEqual(2 + 2, 4); console.log('ok');",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "a/b\nok\n");
}

#[test]
fn build_target_js_transpiles_locally() {
    let dir = scratch("build-js");
    let source = dir.join("input.ts");
    let output = dir.join("out.js");
    fs::write(
        &source,
        "const x: number = 2;\nconsole.log('value', x * 3);\n",
    )
    .unwrap();
    let (stdout, stderr, code) = run(&[
        "build",
        "--target",
        "js",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("built"), "{stdout}");
    let generated = fs::read_to_string(&output).expect("compiled JS should exist");
    assert!(generated.contains("console.log('value', x * 3)"), "{generated}");
    assert!(!generated.contains(": number"), "{generated}");
}

#[test]
fn unknown_flags_are_rejected_before_the_entry_file() {
    let (_, stderr, code) = run(&["run", "--not-a-real-flag"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown flag"), "{stderr}");
}

#[test]
fn unsupported_extension_is_reported() {
    let dir = scratch("extension");
    let file = dir.join("program.txt");
    fs::write(&file, "console.log(1);").unwrap();
    let (_, stderr, code) = run(&["run", file.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("unsupported file extension"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn package_script_fallback_is_discoverable() {
    // Without Bun or delegation the orb must still explain the script
    // route instead of emitting "file not found" for a declared script.
    let dir = scratch("scripts");
    fs::write(
        dir.join("package.json"),
        r#"{"name":"scripts","scripts":{"hello":"echo hello"}}"#,
    )
    .unwrap();
    let output = orb()
        .current_dir(&dir)
        .args(["run", "hello"])
        .env("ORB_NO_DELEGATE", "1")
        .output()
        .expect("the orb binary should start");
    assert!(output.status.success(), "{:?}", output.status);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("hello"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn native_rejection_is_located() {
    let dir = scratch("native");
    let file = dir.join("big.js");
    fs::write(&file, "const big = 10n;\n").unwrap();
    let (stdout, stderr, code) = run(&["run", file.to_str().unwrap(), "--native"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("big.js:1:"), "{stderr}");
    assert!(stderr.contains("BigInt"), "{stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn typecheck_reports_errors_from_embedded_tsz() {
    let dir = scratch("typecheck");
    let good = dir.join("good.ts");
    let bad = dir.join("bad.ts");
    fs::write(&good, "const x: number = 1;\nconsole.log(x);\n").unwrap();
    fs::write(&bad, "const x: number = 'oops';\n").unwrap();

    let (out, err, code) = run(&["typecheck", good.to_str().unwrap()]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("No type errors"), "{out}");

    let (out, err, code) = run(&["typecheck", bad.to_str().unwrap()]);
    assert_eq!(code, 1, "{out} {err}");
    assert!(out.contains("TS"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}
