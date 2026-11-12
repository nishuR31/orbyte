//! Cranelift JIT backend: lowers the architecture-independent IR to machine
//! code in memory and runs it on the host CPU.
//!
//! Type mapping: `F64` -> `f64`, `Bool` -> `i8` (0 or 1).
//! IR locals become Cranelift `Variable`s; Cranelift's frontend performs the
//! SSA construction (inserting block parameters where control flow merges).
//!
//! Execution context: every generated function takes a hidden first argument,
//! a pointer to a `runtime::ExecContext` owned by the caller of
//! [`CompiledModule::run`]. It holds the call-depth counter and an error flag,
//! so each invocation has its own state and generated code has no mutable
//! globals. `run` takes `&self` and creates a fresh context per call; repeated
//! runs of one compiled module are independent, and a failed run cannot affect
//! the next one. (`CompiledModule` is deliberately not marked `Send`/`Sync`:
//! the JIT memory owner is not, so moving it across threads is not supported.)
//!
//! Error propagation: past `runtime::MAX_CALL_DEPTH` a function records
//! `ERR_STACK_OVERFLOW` in the context and returns a zero value. After every
//! call, generated code checks the flag and returns immediately if it is set,
//! so the whole native stack unwinds by ordinary returns (unwinding Rust panics
//! through JIT frames is not possible). `run` converts the flag into a
//! `RuntimeError`; nothing here ever terminates the host process.
//!
//! Runtime helpers are plain Rust `extern "C"` functions.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{self as clif, types, AbiParam, InstBuilder, MemFlags};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module as ClifModule};
use ir::{BinOp, CmpOp, Inst, Terminator, Type, UnOp};
use runtime::{ExecContext, RuntimeError};

/// Machine code for a whole program. Owns the JIT memory, which must outlive
/// every call through `entry`.
pub struct CompiledModule {
    _jit: JITModule,
    entry: extern "C" fn(*mut ExecContext),
}

impl CompiledModule {
    /// Runs the program's entry function with a fresh execution context.
    ///
    /// Returns the runtime error (for example a stack overflow) instead of
    /// terminating the process. Output produced before the error stays written.
    pub fn run(&self) -> Result<(), RuntimeError> {
        let mut ctx = ExecContext::default();
        (self.entry)(&mut ctx as *mut ExecContext);
        ctx.status()
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn clif_type(t: Type) -> clif::Type {
    match t {
        Type::F64 => types::F64,
        Type::Bool => types::I8,
        Type::Str | Type::Arr | Type::Obj | Type::Null => types::I64,
    }
}

fn make_sig(jit: &JITModule, params: &[clif::Type], ret: Option<clif::Type>) -> clif::Signature {
    let mut sig = jit.make_signature();
    for p in params {
        sig.params.push(AbiParam::new(*p));
    }
    if let Some(r) = ret {
        sig.returns.push(AbiParam::new(r));
    }
    sig
}

struct RtIds {
    print_f64: FuncId,
    print_bool: FuncId,
    print_str: FuncId,
    alloc_str: FuncId,
    concat_str: FuncId,
    str_eq: FuncId,
    num_to_str: FuncId,
    bool_to_str: FuncId,
    str_to_num: FuncId,
    bool_to_num: FuncId,
    str_len: FuncId,
    str_upper: FuncId,
    str_includes: FuncId,
    str_slice: FuncId,
    str_replace: FuncId,
    str_split_count: FuncId,
    alloc_array: FuncId,
    array_push: FuncId,
    array_get: FuncId,
    array_set: FuncId,
    array_len: FuncId,
    alloc_obj: FuncId,
    obj_set_num: FuncId,
    obj_get_num: FuncId,
    obj_set_str: FuncId,
    obj_get_str: FuncId,
    obj_set_obj: FuncId,
    obj_get_obj: FuncId,
    fmod: FuncId,
}

struct RtRefs {
    print_f64: clif::FuncRef,
    print_bool: clif::FuncRef,
    print_str: clif::FuncRef,
    alloc_str: clif::FuncRef,
    concat_str: clif::FuncRef,
    str_eq: clif::FuncRef,
    num_to_str: clif::FuncRef,
    bool_to_str: clif::FuncRef,
    str_len: clif::FuncRef,
    str_upper: clif::FuncRef,
    str_includes: clif::FuncRef,
    str_slice: clif::FuncRef,
    str_replace: clif::FuncRef,
    str_split_count: clif::FuncRef,
    alloc_array: clif::FuncRef,
    array_push: clif::FuncRef,
    array_get: clif::FuncRef,
    array_set: clif::FuncRef,
    array_len: clif::FuncRef,
    alloc_obj: clif::FuncRef,
    obj_set_num: clif::FuncRef,
    obj_get_num: clif::FuncRef,
    obj_set_str: clif::FuncRef,
    obj_get_str: clif::FuncRef,
    obj_set_obj: clif::FuncRef,
    obj_get_obj: clif::FuncRef,
    str_to_num: clif::FuncRef,
    bool_to_num: clif::FuncRef,
    fmod: clif::FuncRef,
}

pub fn compile(module: &ir::Module) -> Result<CompiledModule, String> {
    // The backend validates its own input; it never trusts the caller to have done so.
    ir::verify(module).map_err(|e| format!("invalid IR: {e}"))?;
    if std::mem::size_of::<usize>() != 8 {
        return Err("the native backend currently supports 64-bit targets only".to_string());
    }
    let entry_fn = module
        .functions
        .get(module.entry.0 as usize)
        .ok_or("entry function out of range")?;
    if !entry_fn.params.is_empty() || entry_fn.ret.is_some() {
        return Err("entry function must take no arguments and return nothing".to_string());
    }

    // ---- target machine ----
    let mut flags = settings::builder();
    flags.set("use_colocated_libcalls", "false").map_err(err)?;
    flags.set("is_pic", "false").map_err(err)?;
    flags.set("opt_level", "speed").map_err(err)?;
    let isa_builder = cranelift_native::builder().map_err(err)?;
    let isa = isa_builder.finish(settings::Flags::new(flags)).map_err(err)?;

    let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
    jit_builder.symbol("rt_print_f64", runtime::rt_print_f64 as *const u8);
    jit_builder.symbol("rt_print_bool", runtime::rt_print_bool as *const u8);
    jit_builder.symbol("rt_print_str", runtime::rt_print_str as *const u8);
    jit_builder.symbol("rt_alloc_str", runtime::rt_alloc_str as *const u8);
    jit_builder.symbol("rt_concat_str", runtime::rt_concat_str as *const u8);
    jit_builder.symbol("rt_str_eq", runtime::rt_str_eq as *const u8);
    jit_builder.symbol("rt_num_to_str", runtime::rt_num_to_str as *const u8);
    jit_builder.symbol("rt_bool_to_str", runtime::rt_bool_to_str as *const u8);
    jit_builder.symbol("rt_str_len", runtime::rt_str_len as *const u8);
    jit_builder.symbol("rt_str_upper", runtime::rt_str_upper as *const u8);
    jit_builder.symbol("rt_str_includes", runtime::rt_str_includes as *const u8);
    jit_builder.symbol("rt_str_slice", runtime::rt_str_slice as *const u8);
    jit_builder.symbol("rt_str_replace", runtime::rt_str_replace as *const u8);
    jit_builder.symbol("rt_str_split_count", runtime::rt_str_split_count as *const u8);
    jit_builder.symbol("rt_str_to_num", runtime::rt_str_to_num as *const u8);
    jit_builder.symbol("rt_bool_to_num", runtime::rt_bool_to_num as *const u8);
    jit_builder.symbol("rt_alloc_array", runtime::rt_alloc_array as *const u8);
    jit_builder.symbol("rt_array_push", runtime::rt_array_push as *const u8);
    jit_builder.symbol("rt_array_get", runtime::rt_array_get as *const u8);
    jit_builder.symbol("rt_array_set", runtime::rt_array_set as *const u8);
    jit_builder.symbol("rt_array_len", runtime::rt_array_len as *const u8);
    jit_builder.symbol("rt_alloc_obj", runtime::rt_alloc_obj as *const u8);
    jit_builder.symbol("rt_obj_set_num", runtime::rt_obj_set_num as *const u8);
    jit_builder.symbol("rt_obj_get_num", runtime::rt_obj_get_num as *const u8);
    jit_builder.symbol("rt_obj_set_str", runtime::rt_obj_set_str as *const u8);
    jit_builder.symbol("rt_obj_get_str", runtime::rt_obj_get_str as *const u8);
    jit_builder.symbol("rt_obj_set_obj", runtime::rt_obj_set_obj as *const u8);
    jit_builder.symbol("rt_obj_get_obj", runtime::rt_obj_get_obj as *const u8);
    jit_builder.symbol("rt_fmod", runtime::rt_fmod as *const u8);
    let mut jit = JITModule::new(jit_builder);

    // ---- runtime imports ----
    let rt_ids = RtIds {
        print_f64: jit
            .declare_function("rt_print_f64", Linkage::Import, &make_sig(&jit, &[types::F64], None))
            .map_err(err)?,
        print_bool: jit
            .declare_function("rt_print_bool", Linkage::Import, &make_sig(&jit, &[types::I32], None))
            .map_err(err)?,
        print_str: jit
            .declare_function("rt_print_str", Linkage::Import, &make_sig(&jit, &[types::I64], None))
            .map_err(err)?,
        alloc_str: jit
            .declare_function("rt_alloc_str", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I64)))
            .map_err(err)?,
        concat_str: jit
            .declare_function("rt_concat_str", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I64)))
            .map_err(err)?,
        str_eq: jit
            .declare_function("rt_str_eq", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I32)))
            .map_err(err)?,
        num_to_str: jit
            .declare_function("rt_num_to_str", Linkage::Import, &make_sig(&jit, &[types::F64], Some(types::I64)))
            .map_err(err)?,
        bool_to_str: jit
            .declare_function("rt_bool_to_str", Linkage::Import, &make_sig(&jit, &[types::I32], Some(types::I64)))
            .map_err(err)?,
        str_len: jit
            .declare_function("rt_str_len", Linkage::Import, &make_sig(&jit, &[types::I64], Some(types::F64)))
            .map_err(err)?,
        str_upper: jit
            .declare_function("rt_str_upper", Linkage::Import, &make_sig(&jit, &[types::I64], Some(types::I64)))
            .map_err(err)?,
        str_includes: jit
            .declare_function("rt_str_includes", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I32)))
            .map_err(err)?,
        str_slice: jit
            .declare_function("rt_str_slice", Linkage::Import, &make_sig(&jit, &[types::I64, types::F64, types::F64], Some(types::I64)))
            .map_err(err)?,
        str_replace: jit
            .declare_function("rt_str_replace", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64, types::I64], Some(types::I64)))
            .map_err(err)?,
        str_split_count: jit
            .declare_function("rt_str_split_count", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::F64)))
            .map_err(err)?,
        str_to_num: jit
            .declare_function("rt_str_to_num", Linkage::Import, &make_sig(&jit, &[types::I64], Some(types::F64)))
            .map_err(err)?,
        bool_to_num: jit
            .declare_function("rt_bool_to_num", Linkage::Import, &make_sig(&jit, &[types::I32], Some(types::F64)))
            .map_err(err)?,
        alloc_array: jit
            .declare_function("rt_alloc_array", Linkage::Import, &make_sig(&jit, &[types::I64], Some(types::I64)))
            .map_err(err)?,
        array_push: jit
            .declare_function("rt_array_push", Linkage::Import, &make_sig(&jit, &[types::I64, types::F64], None))
            .map_err(err)?,
        array_get: jit
            .declare_function("rt_array_get", Linkage::Import, &make_sig(&jit, &[types::I64, types::F64], Some(types::F64)))
            .map_err(err)?,
        array_set: jit
            .declare_function("rt_array_set", Linkage::Import, &make_sig(&jit, &[types::I64, types::F64, types::F64], None))
            .map_err(err)?,
        array_len: jit
            .declare_function("rt_array_len", Linkage::Import, &make_sig(&jit, &[types::I64], Some(types::F64)))
            .map_err(err)?,
        alloc_obj: jit
            .declare_function("rt_alloc_obj", Linkage::Import, &make_sig(&jit, &[], Some(types::I64)))
            .map_err(err)?,
        obj_set_num: jit
            .declare_function("rt_obj_set_num", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64, types::F64], None))
            .map_err(err)?,
        obj_get_num: jit
            .declare_function("rt_obj_get_num", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::F64)))
            .map_err(err)?,
        obj_set_str: jit
            .declare_function("rt_obj_set_str", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64, types::I64], None))
            .map_err(err)?,
        obj_get_str: jit
            .declare_function("rt_obj_get_str", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I64)))
            .map_err(err)?,
        obj_set_obj: jit
            .declare_function("rt_obj_set_obj", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64, types::I64], None))
            .map_err(err)?,
        obj_get_obj: jit
            .declare_function("rt_obj_get_obj", Linkage::Import, &make_sig(&jit, &[types::I64, types::I64], Some(types::I64)))
            .map_err(err)?,
        fmod: jit
            .declare_function(
                "rt_fmod",
                Linkage::Import,
                &make_sig(&jit, &[types::F64, types::F64], Some(types::F64)),
            )
            .map_err(err)?,
    };

    // ---- declare every function first so calls can refer to later ones ----
    let mut ids: Vec<FuncId> = Vec::with_capacity(module.functions.len());
    let mut sigs: Vec<clif::Signature> = Vec::with_capacity(module.functions.len());
    for (i, f) in module.functions.iter().enumerate() {
        // Hidden first parameter: pointer to the execution context.
        let mut params: Vec<clif::Type> = vec![types::I64];
        params.extend(f.params.iter().map(|t| clif_type(*t)));
        let sig = make_sig(&jit, &params, f.ret.map(clif_type));
        // JS names can collide with runtime symbols (or contain `$`), so use index-based names.
        let symbol = format!("js_fn_{i}");
        let id = jit.declare_function(&symbol, Linkage::Export, &sig).map_err(err)?;
        ids.push(id);
        sigs.push(sig);
    }

    // ---- define ----
    let mut ctx = jit.make_context();
    let mut fb_ctx = FunctionBuilderContext::new();
    for (i, f) in module.functions.iter().enumerate() {
        ctx.func.signature = sigs[i].clone();
        let refs: Vec<clif::FuncRef> = ids
            .iter()
            .map(|id| jit.declare_func_in_func(*id, &mut ctx.func))
            .collect();
        let rt = RtRefs {
            print_f64: jit.declare_func_in_func(rt_ids.print_f64, &mut ctx.func),
            print_bool: jit.declare_func_in_func(rt_ids.print_bool, &mut ctx.func),
            print_str: jit.declare_func_in_func(rt_ids.print_str, &mut ctx.func),
            alloc_str: jit.declare_func_in_func(rt_ids.alloc_str, &mut ctx.func),
            concat_str: jit.declare_func_in_func(rt_ids.concat_str, &mut ctx.func),
            str_eq: jit.declare_func_in_func(rt_ids.str_eq, &mut ctx.func),
            num_to_str: jit.declare_func_in_func(rt_ids.num_to_str, &mut ctx.func),
            bool_to_str: jit.declare_func_in_func(rt_ids.bool_to_str, &mut ctx.func),
            str_len: jit.declare_func_in_func(rt_ids.str_len, &mut ctx.func),
            str_upper: jit.declare_func_in_func(rt_ids.str_upper, &mut ctx.func),
            str_includes: jit.declare_func_in_func(rt_ids.str_includes, &mut ctx.func),
            str_slice: jit.declare_func_in_func(rt_ids.str_slice, &mut ctx.func),
            str_replace: jit.declare_func_in_func(rt_ids.str_replace, &mut ctx.func),
            str_split_count: jit.declare_func_in_func(rt_ids.str_split_count, &mut ctx.func),
            alloc_array: jit.declare_func_in_func(rt_ids.alloc_array, &mut ctx.func),
            array_push: jit.declare_func_in_func(rt_ids.array_push, &mut ctx.func),
            array_get: jit.declare_func_in_func(rt_ids.array_get, &mut ctx.func),
            array_set: jit.declare_func_in_func(rt_ids.array_set, &mut ctx.func),
            array_len: jit.declare_func_in_func(rt_ids.array_len, &mut ctx.func),
            alloc_obj: jit.declare_func_in_func(rt_ids.alloc_obj, &mut ctx.func),
            obj_set_num: jit.declare_func_in_func(rt_ids.obj_set_num, &mut ctx.func),
            obj_get_num: jit.declare_func_in_func(rt_ids.obj_get_num, &mut ctx.func),
            obj_set_str: jit.declare_func_in_func(rt_ids.obj_set_str, &mut ctx.func),
            obj_get_str: jit.declare_func_in_func(rt_ids.obj_get_str, &mut ctx.func),
            obj_set_obj: jit.declare_func_in_func(rt_ids.obj_set_obj, &mut ctx.func),
            obj_get_obj: jit.declare_func_in_func(rt_ids.obj_get_obj, &mut ctx.func),
            str_to_num: jit.declare_func_in_func(rt_ids.str_to_num, &mut ctx.func),
            bool_to_num: jit.declare_func_in_func(rt_ids.bool_to_num, &mut ctx.func),
            fmod: jit.declare_func_in_func(rt_ids.fmod, &mut ctx.func),
        };
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            translate(&mut builder, f, &refs, &rt)
                .map_err(|e| format!("in function '{}': {e}", f.name))?;
            builder.finalize();
        }
        jit.define_function(ids[i], &mut ctx)
            .map_err(|e| format!("in function '{}': {e}", f.name))?;
        jit.clear_context(&mut ctx);
    }

    jit.finalize_definitions().map_err(err)?;
    let code = jit.get_finalized_function(ids[module.entry.0 as usize]);
    // SAFETY: the entry function was declared with the signature `(i64) -> ()` (the
    // hidden execution-context pointer) and the default (host) calling convention,
    // which matches `extern "C" fn(*mut ExecContext)` on a 64-bit target (checked
    // above). The code stays mapped for as long as `jit` lives, and
    // `CompiledModule` owns `jit`.
    let entry = unsafe { std::mem::transmute::<*const u8, extern "C" fn(*mut ExecContext)>(code) };
    Ok(CompiledModule { _jit: jit, entry })
}

fn get(values: &[Option<clif::Value>], v: ir::Value) -> Result<clif::Value, String> {
    values
        .get(v.0 as usize)
        .copied()
        .flatten()
        .ok_or_else(|| format!("undefined IR value {v}"))
}

/// Emits `return <zero of the function's return type>`.
fn return_zero(b: &mut FunctionBuilder, ret: Option<Type>) {
    match ret {
        None => {
            b.ins().return_(&[]);
        }
        Some(Type::F64) => {
            let z = b.ins().f64const(0.0);
            b.ins().return_(&[z]);
        }
        Some(Type::Bool) => {
            let z = b.ins().iconst(types::I8, 0);
            b.ins().return_(&[z]);
        }
        Some(Type::Str | Type::Arr | Type::Obj | Type::Null) => {
            let z = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[z]);
        }
    }
}

fn translate(
    b: &mut FunctionBuilder,
    f: &ir::Function,
    refs: &[clif::FuncRef],
    rt: &RtRefs,
) -> Result<(), String> {
    let blocks: Vec<clif::Block> = f.blocks.iter().map(|_| b.create_block()).collect();
    // A separate prologue block receives the parameters and runs the depth check
    // before control reaches IR block 0.
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);

    // IR locals -> Cranelift variables; parameters (after the hidden context
    // pointer) initialise the first locals.
    let vars: Vec<Variable> = f
        .locals
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let var = Variable::from_u32(i as u32);
            b.declare_var(var, clif_type(*t));
            var
        })
        .collect();
    let ctx_ptr = b.block_params(entry)[0];
    for i in 0..f.params.len() {
        let param = b.block_params(entry)[i + 1];
        b.def_var(vars[i], param);
    }

    // Shared exit used when an error has been recorded: return a zero value so
    // callers can notice the flag and unwind.
    let error_exit = b.create_block();
    let overflow = b.create_block();

    // ---- call-depth guard ----
    let depth = b.ins().load(types::I32, MemFlags::new(), ctx_ptr, runtime::CTX_DEPTH_OFFSET);
    let depth = b.ins().iadd_imm(depth, 1);
    b.ins().store(MemFlags::new(), depth, ctx_ptr, runtime::CTX_DEPTH_OFFSET);
    let too_deep = b.ins().icmp_imm(IntCC::UnsignedGreaterThan, depth, runtime::MAX_CALL_DEPTH as i64);
    b.ins().brif(too_deep, overflow, &[], blocks[0], &[]);

    b.switch_to_block(overflow);
    let code = b.ins().iconst(types::I32, runtime::ERR_STACK_OVERFLOW as i64);
    b.ins().store(MemFlags::new(), code, ctx_ptr, runtime::CTX_ERROR_OFFSET);
    b.ins().jump(error_exit, &[]);

    b.switch_to_block(error_exit);
    return_zero(b, f.ret);

    let mut values: Vec<Option<clif::Value>> = vec![None; f.value_types.len()];

    for (bi, data) in f.blocks.iter().enumerate() {
        b.switch_to_block(blocks[bi]);

        for inst in &data.insts {
            match inst {
                Inst::ConstF64 { dst, value } => {
                    let v = b.ins().f64const(*value);
                    values[dst.0 as usize] = Some(v);
                }
                Inst::ConstBool { dst, value } => {
                    let v = b.ins().iconst(types::I8, *value as i64);
                    values[dst.0 as usize] = Some(v);
                }
                Inst::Unary { dst, op, src } => {
                    let s = get(&values, *src)?;
                    let v = match op {
                        UnOp::NegF64 => b.ins().fneg(s),
                        UnOp::NotBool => b.ins().bxor_imm(s, 1),
                    };
                    values[dst.0 as usize] = Some(v);
                }
                Inst::Binary { dst, op, lhs, rhs } => {
                    let l = get(&values, *lhs)?;
                    let r = get(&values, *rhs)?;
                    let v = match op {
                        BinOp::AddF64 => b.ins().fadd(l, r),
                        BinOp::SubF64 => b.ins().fsub(l, r),
                        BinOp::MulF64 => b.ins().fmul(l, r),
                        BinOp::DivF64 => b.ins().fdiv(l, r),
                        BinOp::RemF64 => {
                            // Cranelift has no float remainder; call fmod semantics in the runtime.
                            let call = b.ins().call(rt.fmod, &[l, r]);
                            b.inst_results(call)[0]
                        }
                    };
                    values[dst.0 as usize] = Some(v);
                }
                Inst::Compare { dst, op, ty, lhs, rhs } => {
                    let l = get(&values, *lhs)?;
                    let r = get(&values, *rhs)?;
                    let v = match ty {
                        Type::F64 => {
                            // Ordered comparisons are false for NaN; NotEqual is true for NaN,
                            // matching ECMAScript's `!=` / `!==`.
                            let cc = match op {
                                CmpOp::Eq => FloatCC::Equal,
                                CmpOp::Ne => FloatCC::NotEqual,
                                CmpOp::Lt => FloatCC::LessThan,
                                CmpOp::Le => FloatCC::LessThanOrEqual,
                                CmpOp::Gt => FloatCC::GreaterThan,
                                CmpOp::Ge => FloatCC::GreaterThanOrEqual,
                            };
                            b.ins().fcmp(cc, l, r)
                        }
                        Type::Bool => {
                            let cc = match op {
                                CmpOp::Eq => IntCC::Equal,
                                CmpOp::Ne => IntCC::NotEqual,
                                _ => return Err("ordering comparison on booleans".to_string()),
                            };
                            b.ins().icmp(cc, l, r)
                        }
                        Type::Str => {
                            let call = b.ins().call(rt.str_eq, &[l, r]);
                            let eq = b.inst_results(call)[0];
                            match op {
                                CmpOp::Eq => b.ins().icmp_imm(IntCC::NotEqual, eq, 0),
                                CmpOp::Ne => b.ins().icmp_imm(IntCC::Equal, eq, 0),
                                _ => return Err("ordering comparison on strings is not supported yet".to_string()),
                            }
                        }
                        Type::Arr | Type::Obj | Type::Null => {
                            let cc = match op {
                                CmpOp::Eq => IntCC::Equal,
                                CmpOp::Ne => IntCC::NotEqual,
                                _ => return Err("ordering comparison on objects is not supported".to_string()),
                            };
                            b.ins().icmp(cc, l, r)
                        }
                    };
                    values[dst.0 as usize] = Some(v);
                }
                Inst::GetLocal { dst, local } => {
                    let v = b.use_var(vars[local.0 as usize]);
                    values[dst.0 as usize] = Some(v);
                }
                Inst::SetLocal { local, src } => {
                    let s = get(&values, *src)?;
                    b.def_var(vars[local.0 as usize], s);
                }
                Inst::Call { dst, callee, args } => {
                    let mut argv: Vec<clif::Value> = vec![ctx_ptr];
                    for a in args {
                        argv.push(get(&values, *a)?);
                    }
                    let call = b.ins().call(refs[callee.0 as usize], &argv);
                    if let Some(d) = dst {
                        let r = b.inst_results(call)[0];
                        values[d.0 as usize] = Some(r);
                    }
                    // If the callee recorded an error, unwind: return right away.
                    let flag = b.ins().load(types::I32, MemFlags::new(), ctx_ptr, runtime::CTX_ERROR_OFFSET);
                    let next = b.create_block();
                    b.ins().brif(flag, error_exit, &[], next, &[]);
                    b.switch_to_block(next);
                }
                Inst::Print { ty, value } => {
                    let v = get(&values, *value)?;
                    match ty {
                        Type::F64 => {
                            b.ins().call(rt.print_f64, &[v]);
                        }
                        Type::Bool => {
                            let wide = b.ins().uextend(types::I32, v);
                            b.ins().call(rt.print_bool, &[wide]);
                        }
                        Type::Str => {
                            b.ins().call(rt.print_str, &[v]);
                        }
                        Type::Arr | Type::Obj => {
                            let static_str: &'static str = "[object Object]";
                            let ptr_val = b.ins().iconst(types::I64, static_str.as_ptr() as i64);
                            let len_val = b.ins().iconst(types::I64, static_str.len() as i64);
                            let call = b.ins().call(rt.alloc_str, &[ptr_val, len_val]);
                            let handle = b.inst_results(call)[0];
                            b.ins().call(rt.print_str, &[handle]);
                        }
                        Type::Null => {
                            let static_str: &'static str = "undefined";
                            let ptr_val = b.ins().iconst(types::I64, static_str.as_ptr() as i64);
                            let len_val = b.ins().iconst(types::I64, static_str.len() as i64);
                            let call = b.ins().call(rt.alloc_str, &[ptr_val, len_val]);
                            let handle = b.inst_results(call)[0];
                            b.ins().call(rt.print_str, &[handle]);
                        }
                    }
                }
                Inst::ConstStr { dst, value } => {
                    // String literals are leaked into a static buffer so the pointer remains valid for the JIT process
                    let static_str: &'static str = Box::leak(value.clone().into_boxed_str());
                    let ptr_val = b.ins().iconst(types::I64, static_str.as_ptr() as i64);
                    let len_val = b.ins().iconst(types::I64, static_str.len() as i64);
                    let call = b.ins().call(rt.alloc_str, &[ptr_val, len_val]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ConcatStr { dst, lhs, rhs } => {
                    let l = get(&values, *lhs)?;
                    let r = get(&values, *rhs)?;
                    let call = b.ins().call(rt.concat_str, &[l, r]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::NumToStr { dst, src } => {
                    let v = get(&values, *src)?;
                    let call = b.ins().call(rt.num_to_str, &[v]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::BoolToStr { dst, src } => {
                    let v = get(&values, *src)?;
                    let wide = b.ins().uextend(types::I32, v);
                    let call = b.ins().call(rt.bool_to_str, &[wide]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrLen { dst, src } => {
                    let v = get(&values, *src)?;
                    let call = b.ins().call(rt.str_len, &[v]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrUpper { dst, src } => {
                    let v = get(&values, *src)?;
                    let call = b.ins().call(rt.str_upper, &[v]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrIncludes { dst, src, needle } => {
                    let s = get(&values, *src)?;
                    let n = get(&values, *needle)?;
                    let call = b.ins().call(rt.str_includes, &[s, n]);
                    let wide = b.inst_results(call)[0];
                    let b_val = b.ins().ireduce(types::I8, wide);
                    values[dst.0 as usize] = Some(b_val);
                }
                Inst::StrSlice { dst, src, start, end } => {
                    let s = get(&values, *src)?;
                    let st = get(&values, *start)?;
                    let en = get(&values, *end)?;
                    let call = b.ins().call(rt.str_slice, &[s, st, en]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrReplace { dst, src, from, to } => {
                    let s = get(&values, *src)?;
                    let f = get(&values, *from)?;
                    let t = get(&values, *to)?;
                    let call = b.ins().call(rt.str_replace, &[s, f, t]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrSplitCount { dst, src, sep } => {
                    let s = get(&values, *src)?;
                    let sp = get(&values, *sep)?;
                    let call = b.ins().call(rt.str_split_count, &[s, sp]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::StrToNum { dst, src } => {
                    let v = get(&values, *src)?;
                    let call = b.ins().call(rt.str_to_num, &[v]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::BoolToNum { dst, src } => {
                    let v = get(&values, *src)?;
                    let wide = b.ins().uextend(types::I32, v);
                    let call = b.ins().call(rt.bool_to_num, &[wide]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::MathSqrt { dst, src } => {
                    let v = get(&values, *src)?;
                    let res = b.ins().sqrt(v);
                    values[dst.0 as usize] = Some(res);
                }
                Inst::MathAbs { dst, src } => {
                    let v = get(&values, *src)?;
                    let res = b.ins().fabs(v);
                    values[dst.0 as usize] = Some(res);
                }
                Inst::AllocArray { dst, capacity } => {
                    let cap_val = b.ins().iconst(types::I64, *capacity as i64);
                    let call = b.ins().call(rt.alloc_array, &[cap_val]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ArrayPush { arr, val } => {
                    let a = get(&values, *arr)?;
                    let v = get(&values, *val)?;
                    b.ins().call(rt.array_push, &[a, v]);
                }
                Inst::ArrayGet { dst, arr, index } => {
                    let a = get(&values, *arr)?;
                    let idx = get(&values, *index)?;
                    let call = b.ins().call(rt.array_get, &[a, idx]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ArraySet { arr, index, val } => {
                    let a = get(&values, *arr)?;
                    let idx = get(&values, *index)?;
                    let v = get(&values, *val)?;
                    b.ins().call(rt.array_set, &[a, idx, v]);
                }
                Inst::ArrayLen { dst, arr } => {
                    let a = get(&values, *arr)?;
                    let call = b.ins().call(rt.array_len, &[a]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::AllocObj { dst } => {
                    let call = b.ins().call(rt.alloc_obj, &[]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ObjSetNum { obj, key, val } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let v = get(&values, *val)?;
                    b.ins().call(rt.obj_set_num, &[o, k, v]);
                }
                Inst::ObjGetNum { dst, obj, key } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let call = b.ins().call(rt.obj_get_num, &[o, k]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ObjSetStr { obj, key, val } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let v = get(&values, *val)?;
                    b.ins().call(rt.obj_set_str, &[o, k, v]);
                }
                Inst::ObjGetStr { dst, obj, key } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let call = b.ins().call(rt.obj_get_str, &[o, k]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ObjSetObj { obj, key, val } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let v = get(&values, *val)?;
                    b.ins().call(rt.obj_set_obj, &[o, k, v]);
                }
                Inst::ObjGetObj { dst, obj, key } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let call = b.ins().call(rt.obj_get_obj, &[o, k]);
                    let handle = b.inst_results(call)[0];
                    values[dst.0 as usize] = Some(handle);
                }
                Inst::ConstNull { dst } => {
                    let z = b.ins().iconst(types::I64, 0);
                    values[dst.0 as usize] = Some(z);
                }
                Inst::NullishCoalesce { dst, lhs, rhs } => {
                    let l = get(&values, *lhs)?;
                    let r = get(&values, *rhs)?;
                    let zero = b.ins().iconst(types::I64, 0);
                    let is_null = b.ins().icmp(IntCC::Equal, l, zero);
                    let res = b.ins().select(is_null, r, l);
                    values[dst.0 as usize] = Some(res);
                }
                Inst::OptionalChain { dst, obj, key } => {
                    let o = get(&values, *obj)?;
                    let k = get(&values, *key)?;
                    let call = b.ins().call(rt.obj_get_num, &[o, k]);
                    let handle = b.inst_results(call)[0];
                    let zero = b.ins().iconst(types::I64, 0);
                    let is_null = b.ins().icmp(IntCC::Equal, o, zero);
                    let res = b.ins().select(is_null, zero, handle);
                    values[dst.0 as usize] = Some(res);
                }
            }
        }

        match data.terminator.as_ref().ok_or("unterminated block")? {
            Terminator::Jump(t) => {
                b.ins().jump(blocks[t.0 as usize], &[]);
            }
            Terminator::Branch { cond, then_block, else_block } => {
                let c = get(&values, *cond)?;
                b.ins().brif(c, blocks[then_block.0 as usize], &[], blocks[else_block.0 as usize], &[]);
            }
            Terminator::Return(v) => {
                // Leave the call-depth counter as we found it.
                let d = b.ins().load(types::I32, MemFlags::new(), ctx_ptr, runtime::CTX_DEPTH_OFFSET);
                let d = b.ins().iadd_imm(d, -1);
                b.ins().store(MemFlags::new(), d, ctx_ptr, runtime::CTX_DEPTH_OFFSET);
                match v {
                    Some(v) => {
                        let x = get(&values, *v)?;
                        b.ins().return_(&[x]);
                    }
                    None => {
                        b.ins().return_(&[]);
                    }
                }
            }
        }
    }

    b.seal_all_blocks();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::{BlockData, FuncRef, Function, Module};

    fn jit(src: &str) -> CompiledModule {
        let module = compiler::compile_optimized(src)
            .unwrap_or_else(|d| panic!("compile error: {}", d.message));
        compile(&module).expect("native compilation failed")
    }

    const RUNAWAY: &str = "function f(n) { return f(n + 1); }\nlet r = f(0);";
    const DEEP: &str = "function down(n) { if (n == 0) { return 0; } return down(n - 1) + 1; }\nlet r = down(5000);";

    #[test]
    fn stack_overflow_is_an_error_not_a_process_exit() {
        let m = jit(RUNAWAY);
        assert_eq!(m.run(), Err(RuntimeError::StackOverflow));
    }

    #[test]
    fn a_failed_run_does_not_affect_the_next_run() {
        let m = jit(RUNAWAY);
        assert_eq!(m.run(), Err(RuntimeError::StackOverflow));
        // A fresh context starts at depth 0 with no error recorded.
        assert_eq!(m.run(), Err(RuntimeError::StackOverflow));

        let ok = jit(DEEP);
        for _ in 0..3 {
            assert_eq!(ok.run(), Ok(()));
        }
    }

    #[test]
    fn independent_modules_run_concurrently_without_sharing_state() {
        let handles: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    let m = jit(if i % 2 == 0 { DEEP } else { RUNAWAY });
                    (i, m.run())
                })
            })
            .collect();
        for h in handles {
            let (i, result) = h.join().expect("thread panicked");
            if i % 2 == 0 {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(result, Err(RuntimeError::StackOverflow));
            }
        }
    }

    #[test]
    fn malformed_ir_is_rejected_with_an_error_not_a_panic() {
        let f = Function {
            name: "main".to_string(),
            params: vec![],
            ret: None,
            locals: vec![],
            blocks: vec![BlockData { insts: vec![], terminator: None }],
            value_types: vec![],
        };
        let module = Module { functions: vec![f], entry: FuncRef(0) };
        let e = compile(&module).err().expect("expected an error");
        assert!(e.contains("invalid IR"), "{e}");
    }

    #[test]
    fn undefined_values_are_rejected_before_code_generation() {
        let f = Function {
            name: "main".to_string(),
            params: vec![],
            ret: None,
            locals: vec![],
            blocks: vec![BlockData {
                insts: vec![ir::Inst::Print { ty: ir::Type::F64, value: ir::Value(0) }],
                terminator: Some(ir::Terminator::Return(None)),
            }],
            value_types: vec![ir::Type::F64],
        };
        let module = Module { functions: vec![f], entry: FuncRef(0) };
        assert!(compile(&module).is_err());
    }
}
