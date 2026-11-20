//! Randomised differential test against Node.js.
//!
//! A tiny seeded generator produces programs in the subset the orb supports
//! (non-negative integer arithmetic kept below 1e12, comparisons, `&&`/`||`/`!`,
//! `if`/`else`, bounded `for` loops with `break`/`continue`, `console.log`).
//! Each program is run by real Node.js and by the orb in all four
//! configurations (JIT and interpreter, with and without the optimiser); every
//! output must be identical to Node's. The test is skipped, loudly, when `node`
//! is not installed.
//!
//! Values stay non-negative on purpose: the orb prints `-0` as `0` and
//! formats very large or tiny numbers differently from Node, and none of that
//! is what this test is about.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn expr(rng: &mut Rng, vars: &[String], depth: u32) -> String {
    if depth == 0 || rng.below(3) == 0 {
        return if rng.below(2) == 0 {
            rng.below(10).to_string()
        } else {
            vars[rng.below(vars.len() as u64) as usize].clone()
        };
    }
    match rng.below(3) {
        0 => format!("({} + {})", expr(rng, vars, depth - 1), expr(rng, vars, depth - 1)),
        1 => format!("({} * {})", expr(rng, vars, depth - 1), expr(rng, vars, depth - 1)),
        _ => format!("({} % {})", expr(rng, vars, depth - 1), rng.below(9) + 1),
    }
}

fn cond(rng: &mut Rng, vars: &[String], depth: u32) -> String {
    if depth == 0 || rng.below(2) == 0 {
        let ops = ["<", "<=", "==", "!=", ">", ">="];
        let op = ops[rng.below(ops.len() as u64) as usize];
        return format!("({} {} {})", expr(rng, vars, 2), op, expr(rng, vars, 2));
    }
    match rng.below(3) {
        0 => format!("({} && {})", cond(rng, vars, depth - 1), cond(rng, vars, depth - 1)),
        1 => format!("({} || {})", cond(rng, vars, depth - 1), cond(rng, vars, depth - 1)),
        _ => format!("!{}", cond(rng, vars, depth - 1)),
    }
}

fn stmt(rng: &mut Rng, vars: &[String], depth: u32, in_loop: bool, out: &mut String) {
    let target = vars[rng.below(3) as usize].clone(); // only a, b, c are assigned
    let kinds = if depth < 2 { 6 } else { 3 };
    match rng.below(kinds) {
        0 => out.push_str(&format!("{target} = {} % 1000;\n", expr(rng, vars, 3))),
        1 => {
            let op = ["+=", "*="][rng.below(2) as usize];
            out.push_str(&format!("{target} {op} {};\n{target} %= 1000;\n", expr(rng, vars, 2)));
        }
        2 => out.push_str(&format!("console.log({});\n", expr(rng, vars, 3))),
        3 => {
            out.push_str(&format!("if ({}) {{\n", cond(rng, vars, 2)));
            stmt(rng, vars, depth + 1, in_loop, out);
            out.push_str("} else {\n");
            stmt(rng, vars, depth + 1, in_loop, out);
            out.push_str("}\n");
        }
        4 => {
            let name = format!("i{depth}");
            let bound = rng.below(4) + 1;
            out.push_str(&format!("for (let {name} = 0; {name} < {bound}; {name}++) {{\n"));
            let mut inner: Vec<String> = vars.to_vec();
            inner.push(name);
            for _ in 0..rng.below(3) + 1 {
                stmt(rng, &inner, depth + 1, true, out);
            }
            out.push_str("}\n");
        }
        _ => {
            if in_loop {
                let word = if rng.below(2) == 0 { "break" } else { "continue" };
                out.push_str(&format!("if ({}) {{ {word}; }}\n", cond(rng, vars, 1)));
            } else {
                out.push_str(&format!("console.log({});\n", expr(rng, vars, 2)));
            }
        }
    }
}

fn program(seed: u64) -> String {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xDEAD_BEEF);
    let vars: Vec<String> = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let mut out = String::new();
    for v in &vars {
        out.push_str(&format!("let {v} = {};\n", rng.below(20)));
    }
    for _ in 0..(rng.below(7) + 6) {
        stmt(&mut rng, &vars, 0, false, &mut out);
    }
    for v in &vars {
        out.push_str(&format!("console.log({v});\n"));
    }
    out
}

fn node_available() -> bool {
    Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn run_orb(file: &Path, flags: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_orb"))
        .arg("run")
        .arg(file)
        .args(flags)
        .output()
        .expect("failed to launch the orb binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn random_programs_match_node_on_every_backend() {
    if !node_available() {
        eprintln!("SKIPPED: `node` is not installed, so there is no independent oracle");
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("orb-random-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    let configs: [(&str, &[&str]); 4] = [
        ("jit", &[]),
        ("jit --no-opt", &["--no-opt"]),
        ("interp", &["--interp"]),
        ("interp --no-opt", &["--interp", "--no-opt"]),
    ];

    for seed in 0..40u64 {
        let source = program(seed);
        let file = dir.join(format!("p{seed}.js"));
        fs::write(&file, &source).unwrap();

        let node = Command::new("node").arg(&file).output().expect("failed to launch node");
        assert!(
            node.status.success(),
            "seed {seed}: node rejected the generated program (generator bug):\n{}\n{source}",
            String::from_utf8_lossy(&node.stderr)
        );
        let expected = String::from_utf8_lossy(&node.stdout).into_owned();

        for (label, flags) in configs {
            let (stdout, stderr, ok) = run_orb(&file, flags);
            assert!(ok, "seed {seed} failed under `{label}`: {stderr}\n{source}");
            assert_eq!(stdout, expected, "seed {seed}: `{label}` differs from node\n{source}");
        }
    }
    let _ = fs::remove_dir_all(&dir);
}
