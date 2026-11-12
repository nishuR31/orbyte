//! A small reference interpreter for the IR.
//!
//! It exists so the frontend, IR lowering and verifier can be tested without a
//! native backend, and so the JIT can be checked against it differentially.
//!
//! Calls do not recurse on the Rust stack: the interpreter keeps an explicit
//! stack of [`Frame`]s, so the call-depth limit is a plain counter and deep
//! (but bounded) JavaScript recursion needs no special thread or stack size.

use std::io::Write;

use ir::{BinOp, CmpOp, Inst, Module, Terminator, Type, UnOp, Value};

use crate::{format_number, RuntimeError, MAX_CALL_DEPTH};

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    F64(f64),
    Bool(bool),
    Str(String),
    Arr(std::rc::Rc<std::cell::RefCell<Vec<f64>>>),
    Obj(std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Val>>>),
    Null,
}

/// One activation of an IR function.
struct Frame {
    func: usize,
    block: usize,
    /// Index of the next instruction in `block`; equal to the block's length
    /// when the terminator is next.
    pc: usize,
    locals: Vec<Val>,
    vals: Vec<Option<Val>>,
    /// Where the caller wants the result (a value in the caller's frame).
    ret_dst: Option<Value>,
}

fn internal(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::Internal(msg.into())
}

fn zero(ty: Type) -> Val {
    match ty {
        Type::F64 => Val::F64(0.0),
        Type::Bool => Val::Bool(false),
        Type::Str => Val::Str(String::new()),
        Type::Arr => Val::Arr(std::rc::Rc::new(std::cell::RefCell::new(Vec::new()))),
        Type::Obj => Val::Obj(std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()))),
        Type::Null => Val::Null,
    }
}

fn get(vals: &[Option<Val>], v: Value) -> Result<Val, RuntimeError> {
    vals.get(v.0 as usize)
        .cloned()
        .flatten()
        .ok_or_else(|| internal(format!("use of undefined value {v}")))
}

fn new_frame(
    module: &Module,
    fid: usize,
    args: &[Val],
    ret_dst: Option<Value>,
) -> Result<Frame, RuntimeError> {
    let f = module
        .functions
        .get(fid)
        .ok_or_else(|| internal(format!("call to unknown function f{fid}")))?;
    if args.len() > f.locals.len() {
        return Err(internal(format!("too many arguments for '{}'", f.name)));
    }
    let mut locals: Vec<Val> = f.locals.iter().map(|t| zero(*t)).collect();
    for (i, a) in args.iter().enumerate() {
        locals[i] = a.clone();
    }
    Ok(Frame {
        func: fid,
        block: 0,
        pc: 0,
        locals,
        vals: vec![None; f.value_types.len()],
        ret_dst,
    })
}

/// Runs the module's entry function, writing `console.log` output to `out`.
///
/// Exceeding [`MAX_CALL_DEPTH`] returns [`RuntimeError::StackOverflow`], the
/// same condition the native backend reports.
pub fn run(module: &Module, out: &mut dyn Write) -> Result<(), RuntimeError> {
    let mut stack: Vec<Frame> = vec![new_frame(module, module.entry.0 as usize, &[], None)?];

    loop {
        let frame = stack.last_mut().ok_or_else(|| internal("empty call stack"))?;
        let f = &module.functions[frame.func];
        let data = f
            .blocks
            .get(frame.block)
            .ok_or_else(|| internal("jump to a missing block"))?;

        if frame.pc < data.insts.len() {
            let inst = &data.insts[frame.pc];
            frame.pc += 1;
            match inst {
                Inst::ConstF64 { dst, value } => frame.vals[dst.0 as usize] = Some(Val::F64(*value)),
                Inst::ConstBool { dst, value } => frame.vals[dst.0 as usize] = Some(Val::Bool(*value)),
                Inst::ConstStr { dst, value } => frame.vals[dst.0 as usize] = Some(Val::Str(value.clone())),
                Inst::ConcatStr { dst, lhs, rhs } => {
                    let (Val::Str(a), Val::Str(b)) = (get(&frame.vals, *lhs)?, get(&frame.vals, *rhs)?) else {
                        return Err(internal("type error in string concatenation"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(format!("{a}{b}")));
                }
                Inst::NumToStr { dst, src } => {
                    let Val::F64(v) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in num_to_str"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(format_number(v)));
                }
                Inst::BoolToStr { dst, src } => {
                    let Val::Bool(b) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in bool_to_str"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(b.to_string()));
                }
                Inst::StrToNum { dst, src } => {
                    let Val::Str(s) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in str_to_num"));
                    };
                    let trimmed = s.trim();
                    let val = if trimmed.is_empty() { 0.0 } else { trimmed.parse::<f64>().unwrap_or(f64::NAN) };
                    frame.vals[dst.0 as usize] = Some(Val::F64(val));
                }
                Inst::BoolToNum { dst, src } => {
                    let Val::Bool(b) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in bool_to_num"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(if b { 1.0 } else { 0.0 }));
                }
                Inst::StrLen { dst, src } => {
                    let Val::Str(s) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in str_len"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(s.len() as f64));
                }
                Inst::StrUpper { dst, src } => {
                    let Val::Str(s) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in str_upper"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(s.to_uppercase()));
                }
                Inst::StrIncludes { dst, src, needle } => {
                    let (Val::Str(s), Val::Str(n)) = (get(&frame.vals, *src)?, get(&frame.vals, *needle)?) else {
                        return Err(internal("type error in str_includes"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Bool(s.contains(&n)));
                }
                Inst::StrSlice { dst, src, start, end } => {
                    let (Val::Str(s), Val::F64(st), Val::F64(en)) = (get(&frame.vals, *src)?, get(&frame.vals, *start)?, get(&frame.vals, *end)?) else {
                        return Err(internal("type error in str_slice"));
                    };
                    let len = s.len() as i64;
                    let mut start_idx = st as i64;
                    let mut end_idx = en as i64;
                    if start_idx < 0 { start_idx = (len + start_idx).max(0); } else { start_idx = start_idx.min(len); }
                    if end_idx < 0 { end_idx = (len + end_idx).max(0); } else { end_idx = end_idx.min(len); }
                    if end_idx < start_idx { end_idx = start_idx; }
                    let sub = &s[start_idx as usize..end_idx as usize];
                    frame.vals[dst.0 as usize] = Some(Val::Str(sub.to_string()));
                }
                Inst::StrReplace { dst, src, from, to } => {
                    let (Val::Str(s), Val::Str(f), Val::Str(t)) = (get(&frame.vals, *src)?, get(&frame.vals, *from)?, get(&frame.vals, *to)?) else {
                        return Err(internal("type error in str_replace"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(s.replacen(&f, &t, 1)));
                }
                Inst::StrSplitCount { dst, src, sep } => {
                    let (Val::Str(s), Val::Str(sp)) = (get(&frame.vals, *src)?, get(&frame.vals, *sep)?) else {
                        return Err(internal("type error in str_split_count"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(s.split(&sp).count() as f64));
                }
                Inst::MathSqrt { dst, src } => {
                    let Val::F64(v) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in math.sqrt"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(v.sqrt()));
                }
                Inst::MathAbs { dst, src } => {
                    let Val::F64(v) = get(&frame.vals, *src)? else {
                        return Err(internal("type error in math.abs"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(v.abs()));
                }
                Inst::AllocArray { dst, capacity } => {
                    frame.vals[dst.0 as usize] = Some(Val::Arr(std::rc::Rc::new(std::cell::RefCell::new(Vec::with_capacity(*capacity)))));
                }
                Inst::ArrayPush { arr, val } => {
                    let (Val::Arr(a), Val::F64(v)) = (get(&frame.vals, *arr)?, get(&frame.vals, *val)?) else {
                        return Err(internal("type error in array_push"));
                    };
                    a.borrow_mut().push(v);
                }
                Inst::ArrayGet { dst, arr, index } => {
                    let (Val::Arr(a), Val::F64(idx)) = (get(&frame.vals, *arr)?, get(&frame.vals, *index)?) else {
                        return Err(internal("type error in array_get"));
                    };
                    let i = idx as usize;
                    let v = a.borrow().get(i).copied().unwrap_or(f64::NAN);
                    frame.vals[dst.0 as usize] = Some(Val::F64(v));
                }
                Inst::ArraySet { arr, index, val } => {
                    let (Val::Arr(a), Val::F64(idx), Val::F64(v)) = (get(&frame.vals, *arr)?, get(&frame.vals, *index)?, get(&frame.vals, *val)?) else {
                        return Err(internal("type error in array_set"));
                    };
                    let i = idx as usize;
                    let mut mut_a = a.borrow_mut();
                    if i < mut_a.len() {
                        mut_a[i] = v;
                    } else if i == mut_a.len() {
                        mut_a.push(v);
                    }
                }
                Inst::ArrayLen { dst, arr } => {
                    let Val::Arr(a) = get(&frame.vals, *arr)? else {
                        return Err(internal("type error in array_len"));
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(a.borrow().len() as f64));
                }
                Inst::AllocObj { dst } => {
                    frame.vals[dst.0 as usize] = Some(Val::Obj(std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()))));
                }
                Inst::ObjSetNum { obj, key, val } => {
                    let (Val::Obj(o), Val::Str(k), Val::F64(v)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?, get(&frame.vals, *val)?) else {
                        return Err(internal("type error in obj_set_num"));
                    };
                    o.borrow_mut().insert(k.clone(), Val::F64(v));
                }
                Inst::ObjGetNum { dst, obj, key } => {
                    let (Val::Obj(o), Val::Str(k)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?) else {
                        return Err(internal("type error in obj_get_num"));
                    };
                    let v = match o.borrow().get(&k) {
                        Some(Val::F64(num)) => *num,
                        _ => f64::NAN,
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(v));
                }
                Inst::ObjSetStr { obj, key, val } => {
                    let (Val::Obj(o), Val::Str(k), Val::Str(v)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?, get(&frame.vals, *val)?) else {
                        return Err(internal("type error in obj_set_str"));
                    };
                    o.borrow_mut().insert(k.clone(), Val::Str(v.clone()));
                }
                Inst::ObjGetStr { dst, obj, key } => {
                    let (Val::Obj(o), Val::Str(k)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?) else {
                        return Err(internal("type error in obj_get_str"));
                    };
                    let s = match o.borrow().get(&k) {
                        Some(Val::Str(s)) => s.clone(),
                        _ => String::new(),
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Str(s));
                }
                Inst::ObjSetObj { obj, key, val } => {
                    let (Val::Obj(o), Val::Str(k), Val::Obj(v)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?, get(&frame.vals, *val)?) else {
                        return Err(internal("type error in obj_set_obj"));
                    };
                    o.borrow_mut().insert(k.clone(), Val::Obj(v));
                }
                Inst::ObjGetObj { dst, obj, key } => {
                    let (Val::Obj(o), Val::Str(k)) = (get(&frame.vals, *obj)?, get(&frame.vals, *key)?) else {
                        return Err(internal("type error in obj_get_obj"));
                    };
                    let v = match o.borrow().get(&k) {
                        Some(Val::Obj(nested)) => Val::Obj(nested.clone()),
                        _ => Val::Null,
                    };
                    frame.vals[dst.0 as usize] = Some(v);
                }
                Inst::Unary { dst, op, src } => {
                    let r = match (op, get(&frame.vals, *src)?) {
                        (UnOp::NegF64, Val::F64(x)) => Val::F64(-x),
                        (UnOp::NotBool, Val::Bool(x)) => Val::Bool(!x),
                        _ => return Err(internal("type error in unary operation")),
                    };
                    frame.vals[dst.0 as usize] = Some(r);
                }
                Inst::Binary { dst, op, lhs, rhs } => {
                    let (Val::F64(a), Val::F64(b)) = (get(&frame.vals, *lhs)?, get(&frame.vals, *rhs)?)
                    else {
                        return Err(internal("type error in binary operation"));
                    };
                    let r = match op {
                        BinOp::AddF64 => a + b,
                        BinOp::SubF64 => a - b,
                        BinOp::MulF64 => a * b,
                        BinOp::DivF64 => a / b,
                        BinOp::RemF64 => a % b,
                    };
                    frame.vals[dst.0 as usize] = Some(Val::F64(r));
                }
                Inst::Compare { dst, op, lhs, rhs, .. } => {
                    let res = match (get(&frame.vals, *lhs)?, get(&frame.vals, *rhs)?) {
                        (Val::F64(a), Val::F64(b)) => match op {
                            CmpOp::Eq => a == b,
                            CmpOp::Ne => a != b,
                            CmpOp::Lt => a < b,
                            CmpOp::Le => a <= b,
                            CmpOp::Gt => a > b,
                            CmpOp::Ge => a >= b,
                        },
                        (Val::Bool(a), Val::Bool(b)) => match op {
                            CmpOp::Eq => a == b,
                            CmpOp::Ne => a != b,
                            _ => return Err(internal("ordering comparison on booleans")),
                        },
                        (Val::Str(a), Val::Str(b)) => match op {
                            CmpOp::Eq => a == b,
                            CmpOp::Ne => a != b,
                            _ => return Err(internal("ordering comparison on strings")),
                        },
                        _ => return Err(internal("comparison type mismatch")),
                    };
                    frame.vals[dst.0 as usize] = Some(Val::Bool(res));
                }
                Inst::GetLocal { dst, local } => {
                    frame.vals[dst.0 as usize] = Some(frame.locals[local.0 as usize].clone());
                }
                Inst::SetLocal { local, src } => {
                    frame.locals[local.0 as usize] = get(&frame.vals, *src)?;
                }
                Inst::Call { dst, callee, args } => {
                    let argv: Vec<Val> = args
                        .iter()
                        .map(|a| get(&frame.vals, *a))
                        .collect::<Result<_, _>>()?;
                    // Same rule as native code: the new frame may not push the depth past the limit.
                    if stack.len() >= MAX_CALL_DEPTH as usize {
                        return Err(RuntimeError::StackOverflow);
                    }
                    let callee_frame = new_frame(module, callee.0 as usize, &argv, *dst)?;
                    stack.push(callee_frame);
                }
                Inst::Print { value, .. } => {
                    let text = match get(&frame.vals, *value)? {
                        Val::F64(x) => format_number(x),
                        Val::Bool(b) => b.to_string(),
                        Val::Str(s) => s,
                        Val::Arr(a) => format!("{:?}", a.borrow()),
                        Val::Obj(_) => "[object Object]".to_string(),
                        Val::Null => "undefined".to_string(),
                    };
                    writeln!(out, "{text}").map_err(|e| internal(e.to_string()))?;
                }
                Inst::ConstNull { dst } => {
                    frame.vals[dst.0 as usize] = Some(Val::Null);
                }
                Inst::NullishCoalesce { dst, lhs, rhs } => {
                    let l = get(&frame.vals, *lhs)?;
                    let res = match l {
                        Val::Null => get(&frame.vals, *rhs)?,
                        other => other,
                    };
                    frame.vals[dst.0 as usize] = Some(res);
                }
                Inst::OptionalChain { dst, obj, key } => {
                    let o = get(&frame.vals, *obj)?;
                    match o {
                        Val::Null => {
                            frame.vals[dst.0 as usize] = Some(Val::Null);
                        }
                        Val::Obj(map) => {
                            let k = match get(&frame.vals, *key)? {
                                Val::Str(s) => s,
                                _ => return Err(internal("optional chain key must be string")),
                            };
                            let val = map.borrow().get(&k).cloned().unwrap_or(Val::Null);
                            frame.vals[dst.0 as usize] = Some(val);
                        }
                        _ => {
                            frame.vals[dst.0 as usize] = Some(Val::Null);
                        }
                    }
                }
            }
            continue;
        }

        match data
            .terminator
            .as_ref()
            .ok_or_else(|| internal("unterminated block"))?
        {
            Terminator::Jump(b) => {
                frame.block = b.0 as usize;
                frame.pc = 0;
            }
            Terminator::Branch { cond, then_block, else_block } => {
                let Val::Bool(c) = get(&frame.vals, *cond)? else {
                    return Err(internal("branch condition is not a bool"));
                };
                let next = if c { then_block.0 } else { else_block.0 };
                frame.block = next as usize;
                frame.pc = 0;
            }
            Terminator::Return(v) => {
                let result = match v {
                    Some(v) => Some(get(&frame.vals, *v)?),
                    None => None,
                };
                let finished = stack.pop().ok_or_else(|| internal("empty call stack"))?;
                match stack.last_mut() {
                    None => return Ok(()),
                    Some(caller) => {
                        if let Some(d) = finished.ret_dst {
                            caller.vals[d.0 as usize] = result;
                        }
                    }
                }
            }
        }
    }
}
