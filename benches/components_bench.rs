use std::time::Instant;

fn main() {
    println!("=== ENGINE COMPONENT BENCHMARK SUITE ===");

    // Construct a synthetic ~5,000 line JS program to stress every component
    let mut large_src = String::new();
    large_src.push_str("function compute(a, b) {\n");
    large_src.push_str("    let x = a * 2 + b;\n");
    large_src.push_str("    let y = 0xFF + 0b1010 + 0o77 + 1_000_000;\n");
    large_src.push_str("    return x + y;\n");
    large_src.push_str("}\n\n");

    for i in 0..1000 {
        large_src.push_str(&format!(
            "function fn_{i}(val) {{\n    let res = compute(val, {i});\n    if (res > 100) {{ return res; }} else {{ return {i}; }}\n}}\n\n"
        ));
    }
    large_src.push_str("console.log(fn_999(42));\n");

    println!("Benchmark source size: {} bytes, {} lines", large_src.len(), large_src.lines().count());

    let iterations = 10;

    // 1. Lexer Benchmark
    let mut lex_times = Vec::new();
    let mut tokens_count = 0;
    for _ in 0..iterations {
        let t0 = Instant::now();
        let tokens = lexer::tokenize(&large_src).expect("lexing failed");
        tokens_count = tokens.len();
        lex_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    lex_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_lex = lex_times[lex_times.len() / 2];
    println!("1. Lexer (Tokens: {tokens_count}): {median_lex:.3} ms ({:.2} MB/s)", (large_src.len() as f64 / 1_000_000.0) / (median_lex / 1000.0));

    // 2. Parser Benchmark
    let mut parse_times = Vec::new();
    let mut ast_stmts = 0;
    for _ in 0..iterations {
        let t0 = Instant::now();
        let ast = parser::parse(&large_src).expect("parsing failed");
        ast_stmts = ast.body.len();
        parse_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    parse_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_parse = parse_times[parse_times.len() / 2];
    println!("2. Parser (AST statements: {ast_stmts}): {median_parse:.3} ms");

    let ast = parser::parse(&large_src).unwrap();

    // 3. Semantic Analysis / Type Checking Benchmark
    let mut sem_times = Vec::new();
    let mut functions_count = 0;
    for _ in 0..iterations {
        let t0 = Instant::now();
        let hir = semantic::analyze(&ast).expect("semantic analysis failed");
        functions_count = hir.functions.len();
        sem_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    sem_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_sem = sem_times[sem_times.len() / 2];
    println!("3. Semantic Analyzer / Type Checker (Functions: {functions_count}): {median_sem:.3} ms");

    let hir = semantic::analyze(&ast).unwrap();

    // 4. IR Builder / Lowering Benchmark
    let mut ir_times = Vec::new();
    let mut ir_blocks = 0;
    for _ in 0..iterations {
        let t0 = Instant::now();
        let ir = ir_builder::lower(&hir);
        ir_blocks = ir.functions.iter().map(|f| f.blocks.len()).sum();
        ir_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    ir_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_ir = ir_times[ir_times.len() / 2];
    println!("4. IR Lowering / Builder (Basic Blocks: {ir_blocks}): {median_ir:.3} ms");

    let mut unopt_ir = ir_builder::lower(&hir);

    // 5. Optimizer Benchmark (Constant folding, dead-code, CFG pruning)
    let mut opt_times = Vec::new();
    for _ in 0..iterations {
        let mut ir_clone = unopt_ir.clone();
        let t0 = Instant::now();
        optimizer::optimize(&mut ir_clone).expect("optimizer failed");
        opt_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    opt_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_opt = opt_times[opt_times.len() / 2];
    println!("5. IR Optimizer (Fixed-point passes): {median_opt:.3} ms");

    optimizer::optimize(&mut unopt_ir).unwrap();

    // 6. Native Cranelift JIT Compilation Benchmark
    let mut jit_times = Vec::new();
    for _ in 0..iterations {
        let t0 = Instant::now();
        let compiled = backend_cranelift::compile(&unopt_ir).expect("cranelift compilation failed");
        jit_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = compiled;
    }
    jit_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_jit = jit_times[jit_times.len() / 2];
    println!("6. Cranelift Machine Codegen (JIT Native Emit): {median_jit:.3} ms");

    // 7. Reference Interpreter Execution Benchmark
    let mut interp_times = Vec::new();
    for _ in 0..iterations {
        let mut out = Vec::new();
        let t0 = Instant::now();
        runtime::interp::run(&unopt_ir, &mut out).expect("interpreter execution failed");
        interp_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    interp_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_interp = interp_times[interp_times.len() / 2];
    println!("7. Reference IR Interpreter Execution: {median_interp:.3} ms");

    // 8. Native JIT Compiled Execution Benchmark
    let compiled = backend_cranelift::compile(&unopt_ir).unwrap();
    let mut exec_times = Vec::new();
    for _ in 0..iterations {
        let t0 = Instant::now();
        compiled.run().expect("native execution failed");
        exec_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    exec_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_exec = exec_times[exec_times.len() / 2];
    println!("8. Native CPU Machine Code Execution: {median_exec:.4} ms");

    let total_compile = median_lex + median_parse + median_sem + median_ir + median_opt + median_jit;
    println!("\nTotal Full-Pipeline JIT Compile Time: {total_compile:.2} ms");
}
