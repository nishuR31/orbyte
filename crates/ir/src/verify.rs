//! IR verifier. Run after lowering and after every optimisation pass.

use crate::{Block, BlockData, Function, Inst, Local, Module, Terminator, Type, Value};

pub fn verify(module: &Module) -> Result<(), String> {
    let entry = module
        .functions
        .get(module.entry.0 as usize)
        .ok_or_else(|| "entry function out of range".to_string())?;
    if !entry.params.is_empty() || entry.ret.is_some() {
        return Err("entry function must take no arguments and return nothing".to_string());
    }
    for f in &module.functions {
        verify_function(module, f).map_err(|e| format!("in function '{}': {e}", f.name))?;
    }
    Ok(())
}

struct Checker<'a> {
    module: &'a Module,
    func: &'a Function,
    /// Values defined anywhere so far (each value may be defined once).
    seen: Vec<bool>,
    /// Values defined so far in the current block (values are block-local).
    here: Vec<bool>,
}

/// Verifies one function against its module (call signatures need the module).
///
/// Exposed so the optimiser can re-verify only the function it just changed
/// instead of the whole module after every pass.
pub fn verify_function(module: &Module, func: &Function) -> Result<(), String> {
    if func.blocks.is_empty() {
        return Err("function has no blocks".to_string());
    }
    if func.params.len() > func.locals.len() || func.locals[..func.params.len()] != func.params[..] {
        return Err("parameters must occupy the first locals".to_string());
    }
    let nv = func.value_types.len();
    let mut c = Checker { module, func, seen: vec![false; nv], here: vec![false; nv] };
    for (bi, block) in func.blocks.iter().enumerate() {
        c.here.iter_mut().for_each(|h| *h = false);
        c.block(block).map_err(|e| format!("bb{bi}: {e}"))?;
    }
    // Every declared value must have exactly one definition (uniqueness is
    // checked in `def_val`; this catches declared-but-never-defined values).
    if let Some(i) = c.seen.iter().position(|defined| !*defined) {
        return Err(format!("v{i} is declared but never defined"));
    }
    Ok(())
}

impl Checker<'_> {
    fn block(&mut self, block: &BlockData) -> Result<(), String> {
        for inst in &block.insts {
            self.inst(inst)?;
        }
        let term = block
            .terminator
            .as_ref()
            .ok_or_else(|| "block has no terminator".to_string())?;
        self.term(term)
    }

    fn ty_of(&self, v: Value) -> Result<Type, String> {
        self.func
            .value_types
            .get(v.0 as usize)
            .copied()
            .ok_or_else(|| format!("{v} is out of range"))
    }

    fn use_val(&self, v: Value, want: Type) -> Result<(), String> {
        let t = self.ty_of(v)?;
        if !self.here[v.0 as usize] {
            return Err(format!("{v} used before definition in this block"));
        }
        if t != want {
            return Err(format!("{v} has type {t}, expected {want}"));
        }
        Ok(())
    }

    fn def_val(&mut self, v: Value, want: Type) -> Result<(), String> {
        let t = self.ty_of(v)?;
        if t != want {
            return Err(format!("{v} is declared {t} but defined as {want}"));
        }
        if self.seen[v.0 as usize] {
            return Err(format!("{v} is defined more than once"));
        }
        self.seen[v.0 as usize] = true;
        self.here[v.0 as usize] = true;
        Ok(())
    }

    fn local_ty(&self, l: Local) -> Result<Type, String> {
        self.func
            .locals
            .get(l.0 as usize)
            .copied()
            .ok_or_else(|| format!("{l} is out of range"))
    }

    fn block_ref(&self, b: Block) -> Result<(), String> {
        if (b.0 as usize) < self.func.blocks.len() {
            Ok(())
        } else {
            Err(format!("{b} is out of range"))
        }
    }

    fn inst(&mut self, inst: &Inst) -> Result<(), String> {
        match inst {
            Inst::ConstF64 { dst, .. } => self.def_val(*dst, Type::F64),
            Inst::ConstBool { dst, .. } => self.def_val(*dst, Type::Bool),
            Inst::ConstStr { dst, .. } => self.def_val(*dst, Type::Str),
            Inst::ConstNull { dst, .. } => self.def_val(*dst, Type::Null),
            Inst::ConcatStr { dst, lhs, rhs } => {
                self.use_val(*lhs, Type::Str)?;
                self.use_val(*rhs, Type::Str)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::NullishCoalesce { dst, lhs, rhs } => {
                let ty = self.ty_of(*dst)?;
                let l_ty = self.ty_of(*lhs)?;
                if l_ty != Type::Null && l_ty != ty {
                    return Err(format!("mismatched lhs type in nullish coalesce: {l_ty} vs {ty}"));
                }
                let r_ty = self.ty_of(*rhs)?;
                if r_ty != Type::Null && r_ty != ty {
                    return Err(format!("mismatched rhs type in nullish coalesce: {r_ty} vs {ty}"));
                }
                self.def_val(*dst, ty)
            }
            Inst::OptionalChain { dst, obj, key } => {
                let ty = self.ty_of(*dst)?;
                let o_ty = self.ty_of(*obj)?;
                if o_ty != Type::Obj && o_ty != Type::Null {
                    return Err(format!("expected Obj or Null for optional chain receiver, found {o_ty}"));
                }
                self.use_val(*key, Type::Str)?;
                self.def_val(*dst, ty)
            }
            Inst::NumToStr { dst, src } => {
                self.use_val(*src, Type::F64)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::BoolToStr { dst, src } => {
                self.use_val(*src, Type::Bool)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::StrToNum { dst, src } => {
                self.use_val(*src, Type::Str)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::BoolToNum { dst, src } => {
                self.use_val(*src, Type::Bool)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::StrLen { dst, src } => {
                self.use_val(*src, Type::Str)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::StrUpper { dst, src } => {
                self.use_val(*src, Type::Str)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::StrIncludes { dst, src, needle } => {
                self.use_val(*src, Type::Str)?;
                self.use_val(*needle, Type::Str)?;
                self.def_val(*dst, Type::Bool)
            }
            Inst::StrSlice { dst, src, start, end } => {
                self.use_val(*src, Type::Str)?;
                self.use_val(*start, Type::F64)?;
                self.use_val(*end, Type::F64)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::StrReplace { dst, src, from, to } => {
                self.use_val(*src, Type::Str)?;
                self.use_val(*from, Type::Str)?;
                self.use_val(*to, Type::Str)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::StrSplitCount { dst, src, sep } => {
                self.use_val(*src, Type::Str)?;
                self.use_val(*sep, Type::Str)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::MathSqrt { dst, src } | Inst::MathAbs { dst, src } => {
                self.use_val(*src, Type::F64)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::AllocArray { dst, .. } => self.def_val(*dst, Type::Arr),
            Inst::ArrayPush { arr, val } => {
                self.use_val(*arr, Type::Arr)?;
                self.use_val(*val, Type::F64)
            }
            Inst::ArrayGet { dst, arr, index } => {
                self.use_val(*arr, Type::Arr)?;
                self.use_val(*index, Type::F64)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::ArraySet { arr, index, val } => {
                self.use_val(*arr, Type::Arr)?;
                self.use_val(*index, Type::F64)?;
                self.use_val(*val, Type::F64)
            }
            Inst::ArrayLen { dst, arr } => {
                self.use_val(*arr, Type::Arr)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::AllocObj { dst } => self.def_val(*dst, Type::Obj),
            Inst::ObjSetNum { obj, key, val } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.use_val(*val, Type::F64)
            }
            Inst::ObjGetNum { dst, obj, key } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::ObjSetStr { obj, key, val } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.use_val(*val, Type::Str)
            }
            Inst::ObjGetStr { dst, obj, key } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.def_val(*dst, Type::Str)
            }
            Inst::ObjSetObj { obj, key, val } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.use_val(*val, Type::Obj)
            }
            Inst::ObjGetObj { dst, obj, key } => {
                self.use_val(*obj, Type::Obj)?;
                self.use_val(*key, Type::Str)?;
                self.def_val(*dst, Type::Obj)
            }
            Inst::Unary { dst, op, src } => {
                let t = match op {
                    crate::UnOp::NegF64 => Type::F64,
                    crate::UnOp::NotBool => Type::Bool,
                };
                self.use_val(*src, t)?;
                self.def_val(*dst, t)
            }
            Inst::Binary { dst, lhs, rhs, .. } => {
                self.use_val(*lhs, Type::F64)?;
                self.use_val(*rhs, Type::F64)?;
                self.def_val(*dst, Type::F64)
            }
            Inst::Compare { dst, op, ty, lhs, rhs } => {
                if (*ty == Type::Bool || *ty == Type::Str) && !matches!(op, crate::CmpOp::Eq | crate::CmpOp::Ne) {
                    return Err(format!("only == and != are defined on {ty}"));
                }
                self.use_val(*lhs, *ty)?;
                self.use_val(*rhs, *ty)?;
                self.def_val(*dst, Type::Bool)
            }
            Inst::GetLocal { dst, local } => {
                let t = self.local_ty(*local)?;
                self.def_val(*dst, t)
            }
            Inst::SetLocal { local, src } => {
                let t = self.local_ty(*local)?;
                self.use_val(*src, t)
            }
            Inst::Call { dst, callee, args } => {
                let target = self
                    .module
                    .functions
                    .get(callee.0 as usize)
                    .ok_or_else(|| format!("{callee} is out of range"))?;
                if target.params.len() != args.len() {
                    return Err(format!(
                        "call to '{}' passes {} arguments, expected {}",
                        target.name,
                        args.len(),
                        target.params.len()
                    ));
                }
                for (a, t) in args.iter().zip(&target.params) {
                    self.use_val(*a, *t)?;
                }
                match (dst, target.ret) {
                    (Some(d), Some(t)) => self.def_val(*d, t),
                    (Some(_), None) => Err(format!("'{}' returns no value", target.name)),
                    (None, _) => Ok(()),
                }
            }
            Inst::Print { ty, value } => self.use_val(*value, *ty),
        }
    }

    fn term(&self, term: &Terminator) -> Result<(), String> {
        match term {
            Terminator::Jump(b) => self.block_ref(*b),
            Terminator::Branch { cond, then_block, else_block } => {
                self.use_val(*cond, Type::Bool)?;
                self.block_ref(*then_block)?;
                self.block_ref(*else_block)
            }
            Terminator::Return(v) => match (v, self.func.ret) {
                (Some(v), Some(t)) => self.use_val(*v, t),
                (None, None) => Ok(()),
                (Some(_), None) => Err("returns a value from a void function".to_string()),
                (None, Some(_)) => Err("returns nothing from a value-returning function".to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FuncRef, Terminator};

    fn block(insts: Vec<Inst>, t: Terminator) -> BlockData {
        BlockData { insts, terminator: Some(t) }
    }

    fn void_main(blocks: Vec<BlockData>, value_types: Vec<Type>) -> Function {
        Function {
            name: "main".to_string(),
            params: vec![],
            ret: None,
            locals: vec![],
            blocks,
            value_types,
        }
    }

    fn module_of(functions: Vec<Function>) -> Module {
        Module { functions, entry: FuncRef(0) }
    }

    fn message(m: &Module) -> String {
        verify(m).expect_err("expected the verifier to reject this module")
    }

    #[test]
    fn accepts_a_well_formed_module() {
        let f = void_main(
            vec![block(
                vec![
                    Inst::ConstF64 { dst: Value(0), value: 1.0 },
                    Inst::Print { ty: Type::F64, value: Value(0) },
                ],
                Terminator::Return(None),
            )],
            vec![Type::F64],
        );
        assert_eq!(verify(&module_of(vec![f])), Ok(()));
    }

    #[test]
    fn rejects_a_declared_value_that_is_never_defined() {
        let f = void_main(
            vec![block(vec![Inst::ConstF64 { dst: Value(0), value: 1.0 }], Terminator::Return(None))],
            vec![Type::F64, Type::F64],
        );
        assert!(message(&module_of(vec![f])).contains("never defined"));
    }

    #[test]
    fn rejects_a_value_used_in_another_block() {
        let f = void_main(
            vec![
                block(vec![Inst::ConstF64 { dst: Value(0), value: 1.0 }], Terminator::Jump(Block(1))),
                block(vec![Inst::Print { ty: Type::F64, value: Value(0) }], Terminator::Return(None)),
            ],
            vec![Type::F64],
        );
        assert!(message(&module_of(vec![f])).contains("used before definition"));
    }

    #[test]
    fn rejects_a_type_mismatch() {
        let f = void_main(
            vec![block(
                vec![
                    Inst::ConstBool { dst: Value(0), value: true },
                    Inst::Print { ty: Type::F64, value: Value(0) },
                ],
                Terminator::Return(None),
            )],
            vec![Type::Bool],
        );
        assert!(message(&module_of(vec![f])).contains("expected f64"));
    }

    #[test]
    fn rejects_a_value_defined_twice() {
        let f = void_main(
            vec![block(
                vec![
                    Inst::ConstF64 { dst: Value(0), value: 1.0 },
                    Inst::ConstF64 { dst: Value(0), value: 2.0 },
                ],
                Terminator::Return(None),
            )],
            vec![Type::F64],
        );
        assert!(message(&module_of(vec![f])).contains("more than once"));
    }

    #[test]
    fn rejects_jumps_to_missing_blocks() {
        let f = void_main(vec![block(vec![], Terminator::Jump(Block(7)))], vec![]);
        assert!(message(&module_of(vec![f])).contains("out of range"));
    }

    #[test]
    fn rejects_unterminated_blocks() {
        let f = void_main(vec![BlockData { insts: vec![], terminator: None }], vec![]);
        assert!(message(&module_of(vec![f])).contains("no terminator"));
    }

    #[test]
    fn rejects_calls_with_the_wrong_argument_count() {
        let callee = Function {
            name: "f".to_string(),
            params: vec![Type::F64],
            ret: None,
            locals: vec![Type::F64],
            blocks: vec![block(vec![], Terminator::Return(None))],
            value_types: vec![],
        };
        let main = void_main(
            vec![block(
                vec![Inst::Call { dst: None, callee: FuncRef(1), args: vec![] }],
                Terminator::Return(None),
            )],
            vec![],
        );
        assert!(message(&module_of(vec![main, callee])).contains("expected 1"));
    }

    #[test]
    fn rejects_an_entry_function_with_parameters() {
        let mut f = void_main(vec![block(vec![], Terminator::Return(None))], vec![]);
        f.params = vec![Type::F64];
        f.locals = vec![Type::F64];
        assert!(message(&module_of(vec![f])).contains("entry function"));
    }
}
