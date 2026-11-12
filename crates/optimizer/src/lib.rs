//! IR optimisation passes. Each pass preserves ECMAScript-observable behaviour
//! (output order and values) and the module is re-verified by the caller.
//!
//! * `fold_constants`: per-block constant folding and store-to-load forwarding
//!   of locals, plus folding of branches on constant conditions.
//! * `eliminate_dead_code`: removes pure instructions whose result is unused.
//! * `remove_unreachable_blocks`: drops blocks no path from the entry reaches.
//! * `compact_values`: renumbers values densely after removals.
//!
//! `optimize` verifies the module after every pass.
//!
//! Values are block-local, so every analysis here is local to one block and
//! needs no dataflow framework.

use std::collections::HashMap;

use ir::{BinOp, Block, BlockData, CmpOp, Function, Inst, Local, Module, Terminator, UnOp, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Const {
    F64(f64),
    Bool(bool),
}

type Pass = (&'static str, fn(&mut Function));

const PASSES: [Pass; 3] = [
    ("fold_constants", fold_constants),
    ("remove_unreachable_blocks", remove_unreachable_blocks),
    ("eliminate_dead_code", eliminate_dead_code),
];

/// Runs all passes to a fixed point (bounded) over every function.
///
/// After every single pass the function's value numbering is compacted and the
/// whole module is re-verified, so a faulty pass is reported by name instead of
/// surfacing later as a backend or runtime failure. The input must already be
/// valid IR.
pub fn optimize(module: &mut Module) -> Result<(), String> {
    for fi in 0..module.functions.len() {
        for _ in 0..8 {
            let before = module.functions[fi].clone();
            for (name, pass) in PASSES {
                pass(&mut module.functions[fi]);
                compact_values(&mut module.functions[fi]);
                ir::verify(module).map_err(|e| {
                    format!("invalid IR after pass '{name}' on '{}': {e}", module.functions[fi].name)
                })?;
            }
            if module.functions[fi] == before {
                break;
            }
        }
    }
    Ok(())
}

/// The value an instruction defines, pure or not.
fn defined_value(inst: &Inst) -> Option<Value> {
    match inst {
        Inst::ConstF64 { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstStr { dst, .. }
        | Inst::ConcatStr { dst, .. }
        | Inst::NumToStr { dst, .. }
        | Inst::BoolToStr { dst, .. }
        | Inst::StrToNum { dst, .. }
        | Inst::BoolToNum { dst, .. }
        | Inst::StrLen { dst, .. }
        | Inst::StrUpper { dst, .. }
        | Inst::StrIncludes { dst, .. }
        | Inst::StrSlice { dst, .. }
        | Inst::StrReplace { dst, .. }
        | Inst::StrSplitCount { dst, .. }
        | Inst::MathSqrt { dst, .. }
        | Inst::MathAbs { dst, .. }
        | Inst::AllocArray { dst, .. }
        | Inst::ArrayGet { dst, .. }
        | Inst::ArrayLen { dst, .. }
        | Inst::AllocObj { dst, .. }
        | Inst::ObjGetNum { dst, .. }
        | Inst::ObjGetStr { dst, .. }
        | Inst::ObjGetObj { dst, .. }
        | Inst::ConstNull { dst, .. }
        | Inst::NullishCoalesce { dst, .. }
        | Inst::OptionalChain { dst, .. }
        | Inst::Unary { dst, .. }
        | Inst::Binary { dst, .. }
        | Inst::Compare { dst, .. }
        | Inst::GetLocal { dst, .. } => Some(*dst),
        Inst::Call { dst, .. } => *dst,
        Inst::SetLocal { .. }
        | Inst::Print { .. }
        | Inst::ArrayPush { .. }
        | Inst::ArraySet { .. }
        | Inst::ObjSetNum { .. }
        | Inst::ObjSetStr { .. }
        | Inst::ObjSetObj { .. } => None,
    }
}

/// Rewrites every value (definitions and uses) of an instruction through `map`.
fn map_values(inst: &mut Inst, map: &mut impl FnMut(Value) -> Value) {
    match inst {
        Inst::ConstF64 { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstStr { dst, .. }
        | Inst::ConstNull { dst, .. }
        | Inst::GetLocal { dst, .. } => {
            *dst = map(*dst);
        }
        Inst::ConcatStr { dst, lhs, rhs } => {
            *dst = map(*dst);
            *lhs = map(*lhs);
            *rhs = map(*rhs);
        }
        Inst::Unary { dst, src, .. }
        | Inst::NumToStr { dst, src }
        | Inst::BoolToStr { dst, src }
        | Inst::StrToNum { dst, src }
        | Inst::BoolToNum { dst, src }
        | Inst::StrLen { dst, src }
        | Inst::StrUpper { dst, src }
        | Inst::MathSqrt { dst, src }
        | Inst::MathAbs { dst, src } => {
            *dst = map(*dst);
            *src = map(*src);
        }
        Inst::StrIncludes { dst, src, needle } => {
            *dst = map(*dst);
            *src = map(*src);
            *needle = map(*needle);
        }
        Inst::StrSplitCount { dst, src, sep } => {
            *dst = map(*dst);
            *src = map(*src);
            *sep = map(*sep);
        }
        Inst::StrSlice { dst, src, start, end } => {
            *dst = map(*dst);
            *src = map(*src);
            *start = map(*start);
            *end = map(*end);
        }
        Inst::StrReplace { dst, src, from, to } => {
            *dst = map(*dst);
            *src = map(*src);
            *from = map(*from);
            *to = map(*to);
        }
        Inst::Binary { dst, lhs, rhs, .. } | Inst::Compare { dst, lhs, rhs, .. } => {
            *dst = map(*dst);
            *lhs = map(*lhs);
            *rhs = map(*rhs);
        }
        Inst::SetLocal { src, .. } => *src = map(*src),
        Inst::Call { dst, args, .. } => {
            if let Some(d) = dst {
                *d = map(*d);
            }
            for a in args.iter_mut() {
                *a = map(*a);
            }
        }
        Inst::Print { value, .. } => *value = map(*value),
        Inst::AllocArray { dst, .. } | Inst::AllocObj { dst } => {
            *dst = map(*dst);
        }
        Inst::ArrayPush { arr, val } => {
            *arr = map(*arr);
            *val = map(*val);
        }
        Inst::ArrayGet { dst, arr, index } => {
            *dst = map(*dst);
            *arr = map(*arr);
            *index = map(*index);
        }
        Inst::ArraySet { arr, index, val } => {
            *arr = map(*arr);
            *index = map(*index);
            *val = map(*val);
        }
        Inst::ArrayLen { dst, arr } => {
            *dst = map(*dst);
            *arr = map(*arr);
        }
        Inst::ObjSetNum { obj, key, val } | Inst::ObjSetStr { obj, key, val } | Inst::ObjSetObj { obj, key, val } => {
            *obj = map(*obj);
            *key = map(*key);
            *val = map(*val);
        }
        Inst::ObjGetNum { dst, obj, key } | Inst::ObjGetStr { dst, obj, key } | Inst::ObjGetObj { dst, obj, key } => {
            *dst = map(*dst);
            *obj = map(*obj);
            *key = map(*key);
        }
        Inst::NullishCoalesce { dst, lhs, rhs } => {
            *dst = map(*dst);
            *lhs = map(*lhs);
            *rhs = map(*rhs);
        }
        Inst::OptionalChain { dst, obj, key } => {
            *dst = map(*dst);
            *obj = map(*obj);
            *key = map(*key);
        }
    }
}

/// Renumbers values densely in definition order and drops the type entries of
/// values that no longer exist, so every declared value has a definition again
/// after instructions or blocks were removed.
pub fn compact_values(f: &mut Function) {
    let mut new_id: Vec<Option<u32>> = vec![None; f.value_types.len()];
    let mut new_types = Vec::new();
    for block in &f.blocks {
        for inst in &block.insts {
            if let Some(d) = defined_value(inst) {
                if let Some(slot) = new_id.get_mut(d.0 as usize) {
                    *slot = Some(new_types.len() as u32);
                    new_types.push(f.value_types[d.0 as usize]);
                }
            }
        }
    }
    // A use of a value without a definition maps out of range, which the
    // verifier then reports.
    let mut map = |v: Value| Value(new_id.get(v.0 as usize).copied().flatten().unwrap_or(u32::MAX));
    for block in &mut f.blocks {
        for inst in &mut block.insts {
            map_values(inst, &mut map);
        }
        match &mut block.terminator {
            Some(Terminator::Branch { cond, .. }) => *cond = map(*cond),
            Some(Terminator::Return(Some(v))) => *v = map(*v),
            _ => {}
        }
    }
    f.value_types = new_types;
}

fn const_inst(dst: Value, c: Const) -> Inst {
    match c {
        Const::F64(value) => Inst::ConstF64 { dst, value },
        Const::Bool(value) => Inst::ConstBool { dst, value },
    }
}

fn fold_binary(op: BinOp, a: f64, b: f64) -> f64 {
    match op {
        BinOp::AddF64 => a + b,
        BinOp::SubF64 => a - b,
        BinOp::MulF64 => a * b,
        BinOp::DivF64 => a / b,
        // Rust's `%` on f64 is C `fmod`, which is what ECMAScript `%` specifies.
        BinOp::RemF64 => a % b,
    }
}

fn fold_compare(op: CmpOp, a: Const, b: Const) -> Option<bool> {
    match (a, b) {
        (Const::F64(a), Const::F64(b)) => Some(match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }),
        (Const::Bool(a), Const::Bool(b)) => match op {
            CmpOp::Eq => Some(a == b),
            CmpOp::Ne => Some(a != b),
            _ => None,
        },
        _ => None,
    }
}

pub fn fold_constants(f: &mut Function) {
    for block in &mut f.blocks {
        let mut known: HashMap<Value, Const> = HashMap::new();
        let mut local_known: HashMap<Local, Const> = HashMap::new();

        for inst in &mut block.insts {
            let replacement: Option<Inst> = match &*inst {
                Inst::ConstF64 { dst, value } => {
                    known.insert(*dst, Const::F64(*value));
                    None
                }
                Inst::ConstBool { dst, value } => {
                    known.insert(*dst, Const::Bool(*value));
                    None
                }
                Inst::Unary { dst, op, src } => match (op, known.get(src)) {
                    (UnOp::NegF64, Some(Const::F64(x))) => Some(const_inst(*dst, Const::F64(-*x))),
                    (UnOp::NotBool, Some(Const::Bool(x))) => Some(const_inst(*dst, Const::Bool(!*x))),
                    _ => None,
                },
                Inst::MathSqrt { dst, src } => match known.get(src) {
                    Some(Const::F64(x)) => Some(const_inst(*dst, Const::F64(x.sqrt()))),
                    _ => None,
                },
                Inst::MathAbs { dst, src } => match known.get(src) {
                    Some(Const::F64(x)) => Some(const_inst(*dst, Const::F64(x.abs()))),
                    _ => None,
                },
                Inst::Binary { dst, op, lhs, rhs } => match (known.get(lhs), known.get(rhs)) {
                    (Some(Const::F64(a)), Some(Const::F64(b))) => {
                        Some(const_inst(*dst, Const::F64(fold_binary(*op, *a, *b))))
                    }
                    _ => None,
                },
                Inst::Compare { dst, op, lhs, rhs, .. } => match (known.get(lhs), known.get(rhs)) {
                    (Some(a), Some(b)) => {
                        fold_compare(*op, *a, *b).map(|r| const_inst(*dst, Const::Bool(r)))
                    }
                    _ => None,
                },
                Inst::GetLocal { dst, local } => {
                    local_known.get(local).map(|c| const_inst(*dst, *c))
                }
                Inst::SetLocal { local, src } => {
                    match known.get(src) {
                        Some(c) => {
                            local_known.insert(*local, *c);
                        }
                        None => {
                            local_known.remove(local);
                        }
                    }
                    None
                }
                Inst::ConstStr { .. } | Inst::ConcatStr { .. } | Inst::NumToStr { .. } | Inst::BoolToStr { .. } | Inst::StrToNum { .. } | Inst::BoolToNum { .. } | Inst::StrLen { .. } | Inst::StrUpper { .. } | Inst::StrIncludes { .. } | Inst::StrSlice { .. } | Inst::StrReplace { .. } | Inst::StrSplitCount { .. } | Inst::AllocArray { .. } | Inst::ArrayPush { .. } | Inst::ArrayGet { .. } | Inst::ArraySet { .. } | Inst::ArrayLen { .. } | Inst::AllocObj { .. } | Inst::ObjSetNum { .. } | Inst::ObjGetNum { .. } | Inst::ObjSetStr { .. } | Inst::ObjGetStr { .. } | Inst::ObjSetObj { .. } | Inst::ObjGetObj { .. } | Inst::ConstNull { .. } | Inst::NullishCoalesce { .. } | Inst::OptionalChain { .. } => None,
                Inst::Call { .. } | Inst::Print { .. } => None,
            };
            if let Some(new_inst) = replacement {
                // Record the folded constant so later instructions can fold too.
                match &new_inst {
                    Inst::ConstF64 { dst, value } => {
                        known.insert(*dst, Const::F64(*value));
                    }
                    Inst::ConstBool { dst, value } => {
                        known.insert(*dst, Const::Bool(*value));
                    }
                    _ => {}
                }
                *inst = new_inst;
            }
        }

        if let Some(Terminator::Branch { cond, then_block, else_block }) = &block.terminator {
            if let Some(Const::Bool(c)) = known.get(cond) {
                let target = if *c { *then_block } else { *else_block };
                block.terminator = Some(Terminator::Jump(target));
            }
        }
    }
}

fn uses(inst: &Inst, mut visit: impl FnMut(Value)) {
    match inst {
        Inst::ConstF64 { .. } | Inst::ConstBool { .. } | Inst::ConstStr { .. } | Inst::ObjSetObj { .. } | Inst::ObjGetObj { .. } | Inst::ConstNull { .. } | Inst::GetLocal { .. } => {}
        Inst::ConcatStr { lhs, rhs, .. } => {
            visit(*lhs);
            visit(*rhs);
        }
        Inst::Unary { src, .. }
        | Inst::NumToStr { src, .. }
        | Inst::BoolToStr { src, .. }
        | Inst::StrToNum { src, .. }
        | Inst::BoolToNum { src, .. }
        | Inst::StrLen { src, .. }
        | Inst::StrUpper { src, .. }
        | Inst::MathSqrt { src, .. }
        | Inst::MathAbs { src, .. }
        | Inst::SetLocal { src, .. } => visit(*src),
        Inst::StrIncludes { src, needle, .. } => {
            visit(*src);
            visit(*needle);
        }
        Inst::StrSplitCount { src, sep, .. } => {
            visit(*src);
            visit(*sep);
        }
        Inst::StrSlice { src, start, end, .. } => {
            visit(*src);
            visit(*start);
            visit(*end);
        }
        Inst::StrReplace { src, from, to, .. } => {
            visit(*src);
            visit(*from);
            visit(*to);
        }
        Inst::Binary { lhs, rhs, .. } | Inst::Compare { lhs, rhs, .. } => {
            visit(*lhs);
            visit(*rhs);
        }
        Inst::Call { args, .. } => args.iter().for_each(|a| visit(*a)),
        Inst::Print { value, .. } => visit(*value),
        Inst::AllocArray { .. } | Inst::AllocObj { .. } => {}
        Inst::ArrayPush { arr, val } => {
            visit(*arr);
            visit(*val);
        }
        Inst::ArrayGet { arr, index, .. } => {
            visit(*arr);
            visit(*index);
        }
        Inst::ArraySet { arr, index, val } => {
            visit(*arr);
            visit(*index);
            visit(*val);
        }
        Inst::ArrayLen { arr, .. } => {
            visit(*arr);
        }
        Inst::ObjSetNum { obj, key, val } | Inst::ObjSetStr { obj, key, val } | Inst::ObjSetObj { obj, key, val } => {
            visit(*obj);
            visit(*key);
            visit(*val);
        }
        Inst::ObjGetNum { obj, key, .. } | Inst::ObjGetStr { obj, key, .. } | Inst::ObjGetObj { obj, key, .. } => {
            visit(*obj);
            visit(*key);
        }
        Inst::NullishCoalesce { lhs, rhs, .. } => {
            visit(*lhs);
            visit(*rhs);
        }
        Inst::OptionalChain { obj, key, .. } => {
            visit(*obj);
            visit(*key);
        }
    }
}

/// The value an instruction defines if the instruction is pure (no side effects
/// and cannot trap), i.e. removable when that value is never used.
fn pure_def(inst: &Inst) -> Option<Value> {
    match inst {
        Inst::ConstF64 { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstStr { dst, .. }
        | Inst::ConcatStr { dst, .. }
        | Inst::NumToStr { dst, .. }
        | Inst::BoolToStr { dst, .. }
        | Inst::StrToNum { dst, .. }
        | Inst::BoolToNum { dst, .. }
        | Inst::StrLen { dst, .. }
        | Inst::StrUpper { dst, .. }
        | Inst::StrIncludes { dst, .. }
        | Inst::StrSlice { dst, .. }
        | Inst::StrReplace { dst, .. }
        | Inst::StrSplitCount { dst, .. }
        | Inst::MathSqrt { dst, .. }
        | Inst::MathAbs { dst, .. }
        | Inst::AllocArray { dst, .. }
        | Inst::ArrayGet { dst, .. }
        | Inst::ArrayLen { dst, .. }
        | Inst::AllocObj { dst, .. }
        | Inst::ObjGetNum { dst, .. }
        | Inst::ObjGetStr { dst, .. }
        | Inst::ObjGetObj { dst, .. }
        | Inst::ConstNull { dst, .. }
        | Inst::NullishCoalesce { dst, .. }
        | Inst::OptionalChain { dst, .. }
        | Inst::Unary { dst, .. }
        | Inst::Binary { dst, .. }
        | Inst::Compare { dst, .. }
        | Inst::GetLocal { dst, .. } => Some(*dst),
        Inst::SetLocal { .. }
        | Inst::Call { .. }
        | Inst::Print { .. }
        | Inst::ArrayPush { .. }
        | Inst::ArraySet { .. }
        | Inst::ObjSetNum { .. }
        | Inst::ObjSetStr { .. }
        | Inst::ObjSetObj { .. } => None,
    }
}

pub fn eliminate_dead_code(f: &mut Function) {
    loop {
        let mut use_count = vec![0u32; f.value_types.len()];
        for block in &f.blocks {
            for inst in &block.insts {
                uses(inst, |v| use_count[v.0 as usize] += 1);
            }
            match &block.terminator {
                Some(Terminator::Branch { cond, .. }) => use_count[cond.0 as usize] += 1,
                Some(Terminator::Return(Some(v))) => use_count[v.0 as usize] += 1,
                _ => {}
            }
        }
        let mut removed = false;
        for block in &mut f.blocks {
            let before = block.insts.len();
            block.insts.retain(|inst| match pure_def(inst) {
                Some(dst) => use_count[dst.0 as usize] > 0,
                None => true,
            });
            removed |= block.insts.len() != before;
        }
        if !removed {
            return;
        }
    }
}

pub fn remove_unreachable_blocks(f: &mut Function) {
    let n = f.blocks.len();
    let mut reachable = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(b) = stack.pop() {
        if reachable[b] {
            continue;
        }
        reachable[b] = true;
        match &f.blocks[b].terminator {
            Some(Terminator::Jump(t)) => stack.push(t.0 as usize),
            Some(Terminator::Branch { then_block, else_block, .. }) => {
                stack.push(then_block.0 as usize);
                stack.push(else_block.0 as usize);
            }
            _ => {}
        }
    }
    if reachable.iter().all(|r| *r) {
        return;
    }

    let mut remap: Vec<Option<u32>> = vec![None; n];
    let mut next = 0u32;
    for (i, r) in reachable.iter().enumerate() {
        if *r {
            remap[i] = Some(next);
            next += 1;
        }
    }
    let fix = |b: Block| Block(remap[b.0 as usize].expect("edge to a reachable block"));

    let old = std::mem::take(&mut f.blocks);
    for (i, mut data) in old.into_iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        data.terminator = data.terminator.map(|t| match t {
            Terminator::Jump(b) => Terminator::Jump(fix(b)),
            Terminator::Branch { cond, then_block, else_block } => Terminator::Branch {
                cond,
                then_block: fix(then_block),
                else_block: fix(else_block),
            },
            ret @ Terminator::Return(_) => ret,
        });
        f.blocks.push(BlockData { insts: data.insts, terminator: data.terminator });
    }
}
