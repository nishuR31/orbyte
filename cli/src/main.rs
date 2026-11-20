use std::process;

mod envfile;
mod execute;
mod package;
mod testrunner;
mod util;
mod update;
mod typecheck;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const USAGE: &str = "\
orb - the native JavaScript/TypeScript runtime from orbyte (version 3.0.0)

USAGE:
    orb [file] [args...]                 run a file (Node/Bun-shaped surface)
    orb run <file|script> [args...]      run a file or a package.json script
    orb run -e <code>                    evaluate JavaScript/TypeScript
    orb run -p <code>                    evaluate and print the result
    orb test [filter...]                 run *.test.* / *.spec.* files
    orb repl                             start an interactive session
    orb check <file>                     parse without executing (-c)
    orb inspect <file> [--no-opt]        print the generated native IR
    orb build [args...]                  bundle (delegated to Bun)
    orb build --native <file> [-o out]   compile a standalone native executable
    orb init [-y]                        create package.json / index.ts
    orb install bun [--force]            install Bun (extra command)
    orb install|add|remove|update ...    package management (delegated)
    orb x <package> [args...]            execute a package binary (delegated)
    orb create|link|unlink|pm ...        Bun package commands (delegated)
    orb upgrade                          install/upgrade Bun
    orb self-update [--check]            update orb to the latest release
    orb typecheck <file.ts>              type-check with embedded tsz
    orb --version | --revision | --help

ENGINE FLAGS:
    --interp         run with the reference IR interpreter instead of Cranelift
    --no-opt         skip the IR optimiser
    --threads <N>    worker threads for parallel runtime jobs (default: 1)
    --native         disable the compatibility fallback; fail outside the native subset
    --compat         force the embedded compatibility runtime (Boa + SWC)
    --watch, --hot   re-run when files change
    --env-file <f>   load environment variables from <f> (repeatable)
    --no-env-file    disable automatic .env loading
    --input-type <t> auto | module | commonjs
    --cwd <dir>      resolve files relative to <dir>
    --preload <f>    evaluate <f> before the entry (repeatable)
    --               everything after this belongs to the program

TEST FLAGS:
    --bail           stop scheduling files after the first failure
    --concurrency <N> parallel test files
    --test-name-pattern <text>  only report tests whose names contain <text>
    --watch, --hot   re-run when test files change

By default `run` tries the native pipeline first and falls back to the
compatibility runtime when the program is outside the native subset. TypeScript,
JSX, ES modules, BigInt, private fields, async functions, and CommonJS are served
by the compatibility runtime. Relative files, bare packages in `node_modules`,
TypeScript `paths` aliases, and Node built-ins are resolved; missing specifiers
get a clear error. `orb test` runs the in-process harness (not Bun).
Package-management and bundling commands are delegated to Bun when available;
`orb install bun` is implemented by this binary.
Supported extensions: .js .mjs .cjs .jsx .ts .mts .cts .tsx
";

fn main() {
    process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    route(&args)
}

fn route(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        return execute::run_stdin_or_repl(&[]);
    };

    match command {
        // `orb node ...` and `orb bun ...` run the same surface. They are
        // aliases, not emulations of the other binaries.
        "node" | "bun" => route(&args[1..]),
        "run" => execute::command_run(&args[1..]),
        "inspect" => execute::command_inspect(&args[1..]),
        "test" => testrunner::command_test(&args[1..]),
        "repl" => execute::command_repl(&args[1..]),
        "check" => execute::command_check(&args[1..]),
        "build" => package::command_build(&args[1..]),
        "init" => package::command_init(&args[1..]),
        "upgrade" => package::command_upgrade(&args[1..]),
        "self-update" => update::command_self_update(&args[1..]),
        "typecheck" => typecheck::command_typecheck(&args[1..]),
        "exec" => package::command_exec(&args[1..]),
        "x" | "bunx" => package::command_x(&args[1..]),
        "install" | "i" => {
            if args.get(1).map(String::as_str) == Some("bun") {
                package::command_install_bun(&args[2..])
            } else {
                package::delegate("install", &args[1..])
            }
        }
        "add" | "a" => package::delegate("add", &args[1..]),
        "remove" | "rm" => package::delegate("remove", &args[1..]),
        "update" => package::delegate("update", &args[1..]),
        "audit" => package::delegate("audit", &args[1..]),
        "dedupe" => package::delegate("dedupe", &args[1..]),
        "prune" => package::delegate("prune", &args[1..]),
        "outdated" => package::delegate("outdated", &args[1..]),
        "link" => package::delegate("link", &args[1..]),
        "unlink" => package::delegate("unlink", &args[1..]),
        "pm" => package::delegate("pm", &args[1..]),
        "publish" => package::delegate("publish", &args[1..]),
        "create" | "c" => package::command_create(&args[1..]),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            0
        }
        "--version" | "-v" | "-V" => {
            println!("{VERSION}");
            0
        }
        "--revision" => {
            println!("{VERSION}");
            0
        }
        "--test" => testrunner::command_test(&args[1..]),
        _ if command.starts_with("--test-") => testrunner::command_test(args),
        _ if command.starts_with('-') => execute::command_run(args),
        _ => execute::command_run(args),
    }
}
