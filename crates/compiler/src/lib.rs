//! Orchestrates the frontend and IR pipeline:
//! source -> tokens -> AST -> typed HIR -> verified IR -> (optional) optimised IR.

use diagnostics::{Diagnostic, Result, Span};

fn verified(module: ir::Module, stage: &str) -> Result<ir::Module> {
    ir::verify(&module).map_err(|e| {
        Diagnostic::new(
            format!("internal compiler error: invalid IR after {stage}: {e}"),
            Span::default(),
        )
    })?;
    Ok(module)
}

/// Compiles source to verified, unoptimised IR.
pub fn compile_to_ir(source: &str) -> Result<ir::Module> {
    let program = parser::parse(source)?;
    let hir = semantic::analyze(&program)?;
    verified(ir_builder::lower(&hir), "lowering")
}

/// Compiles source to IR and runs the optimiser, re-verifying the result.
pub fn compile_optimized(source: &str) -> Result<ir::Module> {
    let mut module = compile_to_ir(source)?;
    // The optimiser verifies the module after every pass and names the culprit.
    optimizer::optimize(&mut module).map_err(|e| {
        Diagnostic::new(format!("internal compiler error: {e}"), Span::default())
    })?;
    verified(module, "optimisation")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interpret(module: &ir::Module) -> std::result::Result<String, runtime::RuntimeError> {
        let mut out = Vec::new();
        runtime::interp::run(module, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    /// Runs `src` through the interpreter on both the unoptimised and the
    /// optimised IR, asserts they agree, and returns the output.
    fn run(src: &str) -> String {
        let plain = compile_to_ir(src).unwrap_or_else(|d| panic!("compile error: {}", d.message));
        let opt = compile_optimized(src).unwrap_or_else(|d| panic!("compile error: {}", d.message));
        let a = interpret(&plain).expect("runtime error");
        let b = interpret(&opt).expect("runtime error (optimised)");
        assert_eq!(a, b, "optimiser changed behaviour for:\n{src}");
        a
    }

    fn error(src: &str) -> String {
        compile_to_ir(src).expect_err("expected a compile error").message
    }

    #[test]
    fn milestone_example_prints_610() {
        let src = "function calculate(a, b) {\n    return a * b + 10;\n}\n\nconsole.log(calculate(20, 30));\n";
        assert_eq!(run(src), "610\n");
    }

    #[test]
    fn arithmetic_precedence() {
        assert_eq!(run("console.log(2 + 3 * 4 - 6 / 2);"), "11\n");
        assert_eq!(run("console.log(-5 + 2);"), "-3\n");
        assert_eq!(run("console.log(7 % 3);"), "1\n");
        assert_eq!(run("console.log(1 / 4);"), "0.25\n");
    }

    #[test]
    fn special_float_values() {
        assert_eq!(run("console.log(0 / 0);"), "NaN\n");
        assert_eq!(run("console.log(1 / 0);"), "Infinity\n");
        assert_eq!(run("console.log(-1 / 0);"), "-Infinity\n");
        assert_eq!(run("console.log(0 / 0 == 0 / 0);"), "false\n");
        assert_eq!(run("console.log(0 / 0 != 0 / 0);"), "true\n");
    }

    #[test]
    fn booleans_and_comparisons() {
        assert_eq!(run("console.log(1 < 2);\nconsole.log(!(2 == 3));"), "true\ntrue\n");
        assert_eq!(run("console.log(true === false);"), "false\n");
        assert_eq!(run("const t = true;\nconsole.log(t);"), "true\n");
    }

    #[test]
    fn recursion() {
        let src = "function fib(n) {\n  if (n < 2) { return n; }\n  return fib(n - 1) + fib(n - 2);\n}\nconsole.log(fib(10));";
        assert_eq!(run(src), "55\n");
    }

    #[test]
    fn while_loop() {
        let src = "let i = 0;\nlet s = 0;\nwhile (i < 5) {\n  s = s + i;\n  i = i + 1;\n}\nconsole.log(s);";
        assert_eq!(run(src), "10\n");
    }

    #[test]
    fn if_else_with_returns_in_both_branches() {
        let src = "function max(a, b) { if (a > b) { return a; } else { return b; } }\nconsole.log(max(3, 9));";
        assert_eq!(run(src), "9\n");
    }

    #[test]
    fn functions_are_hoisted() {
        assert_eq!(run("console.log(twice(2));\nfunction twice(x) { return x * 2; }"), "4\n");
    }

    #[test]
    fn void_functions() {
        assert_eq!(run("function show(n) { console.log(n); }\nshow(3);"), "3\n");
    }

    #[test]
    fn boolean_returning_functions() {
        let src = "function isPos(n) { return n > 0; }\nconsole.log(isPos(5));\nconsole.log(isPos(-5));";
        assert_eq!(run(src), "true\nfalse\n");
    }

    #[test]
    fn block_scoping_and_shadowing() {
        assert_eq!(run("let x = 1;\n{ let x = 2; console.log(x); }\nconsole.log(x);"), "2\n1\n");
    }

    #[test]
    fn semicolon_insertion_and_comments() {
        assert_eq!(run("let a = 1\nlet b = 2\nconsole.log(a + b)"), "3\n");
        assert_eq!(run("// hi\nconsole.log(1); /* x */ console.log(2);"), "1\n2\n");
    }

    #[test]
    fn code_after_return_is_accepted_and_ignored() {
        assert_eq!(run("function f() { return 1; console.log(99); }\nconsole.log(f());"), "1\n");
    }

    #[test]
    fn ir_dump_mentions_functions() {
        let module = compile_to_ir("function f(a) { return a + 1; }\nconsole.log(f(1));").unwrap();
        let text = module.to_string();
        assert!(text.contains("func f("));
        assert!(text.contains("func __main("));
    }

    // ---- things we must reject rather than miscompile ----

    #[test]
    fn rejects_undefined_variable() {
        assert!(error("console.log(x);").contains("not defined"));
    }

    #[test]
    fn rejects_implicit_conversions() {
        assert!(error("console.log(1 + true);").contains("must be number"));
        assert!(error("if (1) { console.log(1); }").contains("must be boolean"));
        assert!(error("console.log(1 == true);").contains("cannot compare"));
    }

    #[test]
    fn rejects_type_changing_assignment() {
        assert!(error("let x = 1;\nx = true;").contains("cannot assign"));
    }

    #[test]
    fn rejects_assignment_to_const() {
        assert!(error("const c = 1;\nc = 2;").contains("constant"));
    }

    #[test]
    fn rejects_top_level_return() {
        assert!(error("return 1;").contains("outside of a function"));
    }

    #[test]
    fn rejects_missing_return_path() {
        assert!(error("function f(x) { if (x < 1) { return 1; } }").contains("without returning"));
    }

    #[test]
    fn rejects_uninitialized_declaration() {
        assert!(error("let x;").contains("initializer"));
    }

    #[test]
    fn rejects_wrong_argument_count() {
        assert!(error("function f(a) { return a; }\nconsole.log(f(1, 2));").contains("expects 1"));
    }

    #[test]
    fn rejects_use_before_declaration() {
        assert!(error("console.log(x);\nlet x = 1;").contains("before its initialization"));
    }

    #[test]
    fn reports_syntax_errors() {
        assert!(error("console.log(1;").contains("expected"));
    }

    // ---- logic, loops, compound assignment ----

    #[test]
    fn boolean_function_used_in_conditions() {
        let src = "function isPos(n) { return n > 0; }\n\
                   function both(a, b) { if (isPos(a) && isPos(b)) { return 1; } return 0; }\n\
                   console.log(both(1, 2));\nconsole.log(both(1, -2));";
        assert_eq!(run(src), "1\n0\n");
    }

    #[test]
    fn mutually_recursive_boolean_functions() {
        let src = "function isEven(n) { if (n == 0) { return true; } return isOdd(n - 1); }\n\
                   function isOdd(n) { if (n == 0) { return false; } return isEven(n - 1); }\n\
                   console.log(isEven(10));\nconsole.log(isOdd(7));\nconsole.log(isEven(3));";
        assert_eq!(run(src), "true\ntrue\nfalse\n");
    }

    #[test]
    fn logical_operators_short_circuit() {
        let src = "function t(x) { console.log(x); return true; }\n\
                   function f(x) { console.log(x); return false; }\n\
                   console.log(f(1) && t(2));\n\
                   console.log(t(3) || f(4));\n\
                   console.log(t(5) && f(6));\n\
                   console.log(f(7) || t(8));";
        assert_eq!(run(src), "1\nfalse\n3\ntrue\n5\n6\nfalse\n7\n8\ntrue\n");
    }

    #[test]
    fn earlier_operand_survives_a_branching_later_operand() {
        let src = "let a = 1;\nlet b = 2;\n\
                   console.log((a > 0) == (b > 5 && a > 0));\n\
                   console.log((a > 0) != (b > 5 || a > 5));";
        assert_eq!(run(src), "false\ntrue\n");
    }

    #[test]
    fn for_loop_with_continue_and_break() {
        let src = "let sum = 0;\n\
                   for (let i = 0; i < 10; i++) {\n\
                     if (i % 2 == 0) { continue; }\n\
                     if (i > 7) { break; }\n\
                     sum += i;\n\
                   }\nconsole.log(sum);";
        assert_eq!(run(src), "16\n");
    }

    #[test]
    fn break_only_leaves_the_inner_loop() {
        let src = "let count = 0;\n\
                   for (let i = 0; i < 3; i++) {\n\
                     for (let j = 0; j < 3; j++) { if (j == 2) { break; } count += 1; }\n\
                   }\nconsole.log(count);";
        assert_eq!(run(src), "6\n");
    }

    #[test]
    fn continue_in_while_skips_to_the_condition() {
        let src = "let i = 0;\nlet s = 0;\n\
                   while (i < 6) { i++; if (i == 3) { continue; } s += i; }\n\
                   console.log(s);";
        assert_eq!(run(src), "18\n");
    }

    #[test]
    fn for_with_empty_clauses() {
        assert_eq!(run("let n = 0;\nfor (;;) { n++; if (n >= 4) { break; } }\nconsole.log(n);"), "4\n");
    }

    #[test]
    fn for_loop_variable_does_not_leak() {
        assert!(error("for (let i = 0; i < 1; i++) { }\nconsole.log(i);").contains("not defined"));
    }

    #[test]
    fn compound_assignment_and_increments() {
        assert_eq!(run("let x = 10;\nx -= 3;\nx *= 4;\nx /= 7;\nx %= 3;\nconsole.log(x);"), "1\n");
        assert_eq!(run("let k = 5;\n++k;\nk--;\n--k;\nconsole.log(k);"), "4\n");
        assert_eq!(run("let k = 4;\nlet m = ++k;\nconsole.log(m + k);"), "10\n");
    }

    #[test]
    fn gcd_and_primes() {
        let gcd = "function gcd(a, b) { while (b != 0) { let t = b; b = a % b; a = t; } return a; }\nconsole.log(gcd(48, 18));";
        assert_eq!(run(gcd), "6\n");
        let primes = "function isPrime(n) {\n\
                        if (n < 2) { return false; }\n\
                        for (let d = 2; d * d <= n; d++) { if (n % d == 0) { return false; } }\n\
                        return true;\n\
                      }\n\
                      let c = 0;\nfor (let n = 0; n < 100; n++) { if (isPrime(n)) { c++; } }\nconsole.log(c);";
        assert_eq!(run(primes), "25\n");
    }

    #[test]
    fn rejects_postfix_increment_as_a_value() {
        assert!(error("let a = 1;\nlet b = a++;").contains("postfix"));
    }

    #[test]
    fn rejects_break_outside_a_loop() {
        assert!(error("break;").contains("outside of a loop"));
        assert!(error("function f() { continue; }").contains("outside of a loop"));
    }

    #[test]
    fn rejects_non_boolean_logical_operands() {
        assert!(error("console.log(1 && true);").contains("must be boolean"));
    }

    #[test]
    fn rejects_increment_of_constants_and_booleans() {
        assert!(error("const c = 1;\nc++;").contains("constant"));
        assert!(error("let b = true;\nb++;").contains("must be number"));
    }

    // ---- binding semantics: var visibility and the temporal dead zone ----

    #[test]
    fn rejects_reading_a_conditionally_initialised_var() {
        // In JavaScript this returns `undefined` when `c <= 0`; we must not return 0.
        let src = "function f(c) { if (c > 0) { var r = 100; } return r; }\nconsole.log(f(1));";
        assert!(error(src).contains("may be uninitialized"));
        let in_loop = "function g() { for (let i = 0; i < 1; i++) { var v = 1; } return v; }\nconsole.log(g());";
        assert!(error(in_loop).contains("may be uninitialized"));
    }

    #[test]
    fn allows_var_inside_the_block_that_declares_it() {
        let src = "function f(c) { if (c > 0) { var r = 100; return r + 1; } return 0; }\n\
                   console.log(f(1));\nconsole.log(f(0));";
        assert_eq!(run(src), "101\n0\n");
        assert_eq!(run("function h() { var a = 2; { a = a + 1; } return a; }\nconsole.log(h());"), "3\n");
    }

    #[test]
    fn var_is_function_scoped_but_let_is_block_scoped() {
        assert!(error("{ let a = 1; }\nconsole.log(a);").contains("not defined"));
        assert!(error("{ var a = 1; }\nconsole.log(a);").contains("may be uninitialized"));
    }

    #[test]
    fn rejects_the_temporal_dead_zone() {
        // JavaScript throws a ReferenceError for each of these.
        assert!(error("let x = 10;\n{ let x = x + 1; }").contains("before its initialization"));
        assert!(error("let a = 1;\n{ console.log(a); let a = 2; }").contains("before its initialization"));
        assert!(error("let a = a;").contains("before its initialization"));
        assert!(error("{ a = 1; let a = 0; }").contains("before its initialization"));
    }

    #[test]
    fn shadowing_after_initialisation_is_fine() {
        let src = "let x = 1;\n{ let y = x + 1; let x = y * 10; console.log(x); }\nconsole.log(x);";
        // `let y = x + 1` is inside the block that later declares its own `x`,
        // so JavaScript throws there; the engine must reject it too.
        assert!(error(src).contains("before its initialization"));
        let ok = "let x = 1;\n{ let z = 5; let x = z * 10; console.log(x); }\nconsole.log(x);";
        assert_eq!(run(ok), "50\n1\n");
    }

    #[test]
    fn rejects_redeclaration_conflicts() {
        assert!(error("let a = 1;\nlet a = 2;").contains("already declared"));
        assert!(error("let a = 1;\nvar a = 2;").contains("already declared"));
        assert!(error("{ let a = 1; var a = 2; }").contains("already declared"));
        assert!(error("function f(a) { let a = 1; return a; }\nconsole.log(f(1));").contains("already declared"));
    }

    #[test]
    fn for_loop_header_variables_are_in_their_dead_zone_too() {
        assert!(error("for (let i = i; i < 1; i++) { }").contains("before its initialization"));
    }

    // ---- optimiser ----

    #[test]
    fn optimiser_folds_constants_and_branches() {
        let module = compile_optimized("console.log(2 * 3 + 4);").unwrap();
        let text = module.to_string();
        assert!(!text.contains("MulF64") && !text.contains("AddF64"), "{text}");
        assert_eq!(interpret(&module).unwrap(), "10\n");

        let module = compile_optimized("if (1 < 2) { console.log(1); } else { console.log(2); }").unwrap();
        let text = module.to_string();
        assert!(!text.contains("branch"), "{text}");
        assert_eq!(interpret(&module).unwrap(), "1\n");
    }

    #[test]
    fn optimiser_keeps_side_effects_and_nan_semantics() {
        assert_eq!(run("console.log(0 / 0 == 0 / 0);\nconsole.log(0 / 0 != 0 / 0);"), "false\ntrue\n");
        assert_eq!(run("console.log(-7 % 3);\nconsole.log(5 % 0);"), "-1\nNaN\n");
    }

    // ---- runtime limits ----

    #[test]
    fn runaway_recursion_reports_a_stack_overflow() {
        let module = compile_to_ir("function f(n) { return f(n + 1); }\nconsole.log(f(0));").unwrap();
        let err = interpret(&module).unwrap_err();
        assert_eq!(err, runtime::RuntimeError::StackOverflow);
        assert!(err.to_string().contains("Maximum call stack size exceeded"), "{err}");
    }

    #[test]
    fn one_ir_module_can_be_interpreted_repeatedly() {
        let module = compile_to_ir("function f(n) { return f(n + 1); }\nconsole.log(f(0));").unwrap();
        assert_eq!(interpret(&module).unwrap_err(), runtime::RuntimeError::StackOverflow);
        // A failed run leaves nothing behind that could affect the next one.
        assert_eq!(interpret(&module).unwrap_err(), runtime::RuntimeError::StackOverflow);
        let ok = compile_to_ir("console.log(1);").unwrap();
        assert_eq!(interpret(&ok).unwrap(), "1\n");
        assert_eq!(interpret(&ok).unwrap(), "1\n");
    }

    #[test]
    fn output_before_a_stack_overflow_is_preserved() {
        let src = "function f(n) { if (n == 0) { console.log(7); } return f(n + 1); }\nconsole.log(f(0));";
        let module = compile_to_ir(src).unwrap();
        let mut out = Vec::new();
        let err = runtime::interp::run(&module, &mut out).unwrap_err();
        assert_eq!(err, runtime::RuntimeError::StackOverflow);
        assert_eq!(String::from_utf8(out).unwrap(), "7\n");
    }

    #[test]
    fn deep_but_bounded_recursion_works() {
        let src = "function down(n) { if (n == 0) { return 0; } return down(n - 1) + 1; }\nconsole.log(down(5000));";
        assert_eq!(run(src), "5000\n");
    }

    #[test]
    fn string_literals_concatenation_and_printing() {
        let src = r#"
            let greeting = "Hello, ";
            let name = "world";
            let full = greeting + name + "!";
            console.log(full);
        "#;
        let module = compile_to_ir(src).unwrap();
        let mut out = Vec::new();
        runtime::interp::run(&module, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "Hello, world!\n");
    }

    #[test]
    fn hex_binary_octal_and_numeric_separators_execute() {
        let src = r#"
            let hex = 0xFF;
            let bin = 0b1010;
            let oct = 0o77;
            let sep = 1_000_000;
            console.log(hex + bin + oct + sep);
        "#;
        assert_eq!(run(src), "1000328\n");
    }

    #[test]
    fn unicode_identifiers_execute() {
        let src = r#"
            let π = 3;
            let val_réel = 4;
            console.log(π * val_réel);
        "#;
        assert_eq!(run(src), "12\n");
    }
}
