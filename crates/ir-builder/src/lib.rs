//! Lowers the typed HIR from `semantic` into the architecture-independent IR.
//!
//! Every expression is lowered completely inside the current block, so IR
//! values never cross block boundaries. After a `return` the builder switches
//! to a fresh unreachable block so that any following (dead) code still has
//! somewhere to go; the backend discards it.

use ast::{BinaryOp, UnaryOp};
use ir::{BinOp, Block, BlockData, CmpOp, FuncRef, Function, Inst, Local, Terminator, Type, UnOp, Value};
use semantic::hir;

pub fn lower(module: &hir::Module) -> ir::Module {
    let functions = module.functions.iter().map(|f| lower_function(module, f)).collect();
    ir::Module { functions, entry: FuncRef(module.main) }
}

fn lower_type(t: hir::Type) -> Type {
    match t {
        hir::Type::Number => Type::F64,
        hir::Type::Bool => Type::Bool,
        hir::Type::String => Type::Str,
        hir::Type::Arr => Type::Arr,
        hir::Type::Obj => Type::Obj,
        hir::Type::Null => Type::Null,
    }
}

struct Lower<'a> {
    module: &'a hir::Module,
    func: Function,
    current: Block,
    /// Innermost-last stack of `(continue target, break target)` for open loops.
    loops: Vec<(Block, Block)>,
}

/// True if lowering `e` splits the current block (short-circuit operators).
/// IR values are block-local, so operands evaluated before such an expression
/// must be spilled to a local and reloaded afterwards.
fn has_branch(e: &hir::Expr) -> bool {
    match &e.kind {
        hir::ExprKind::And(..) | hir::ExprKind::Or(..) => true,
        hir::ExprKind::Number(_) | hir::ExprKind::Str(_) | hir::ExprKind::Bool(_) | hir::ExprKind::Local(_) | hir::ExprKind::Null | hir::ExprKind::Undefined => false,
        hir::ExprKind::Unary(_, a)
        | hir::ExprKind::Assign(_, a)
        | hir::ExprKind::Print(a)
        | hir::ExprKind::StrLen(a)
        | hir::ExprKind::StrUpper(a)
        | hir::ExprKind::MathSqrt(a)
        | hir::ExprKind::MathAbs(a)
        | hir::ExprKind::ArrayLen(a)
        | hir::ExprKind::NullishCoalescing(a, _)   // rhs is evaluated after lhs, but no block split
        | hir::ExprKind::OptionalChain(a, _) => has_branch(a),
        hir::ExprKind::Binary(_, a, b)
        | hir::ExprKind::StrIncludes(a, b)
        | hir::ExprKind::StrSplitCount(a, b)
        | hir::ExprKind::ArrayGet(a, b)
        | hir::ExprKind::ArrayPush(a, b)
        | hir::ExprKind::ObjGetNum(a, b)
        | hir::ExprKind::ObjGetStr(a, b) => has_branch(a) || has_branch(b),
        hir::ExprKind::StrSlice(a, b, c)
        | hir::ExprKind::StrReplace(a, b, c)
        | hir::ExprKind::ArraySet(a, b, c)
        | hir::ExprKind::ObjSetNum(a, b, c)
        | hir::ExprKind::ObjSetStr(a, b, c) => {
            has_branch(a) || has_branch(b) || has_branch(c)
        }
        hir::ExprKind::Array(elems) => elems.iter().any(has_branch),
        hir::ExprKind::Object(props) => props.iter().any(|(_, v)| has_branch(v)),
        hir::ExprKind::Call(_, args) => args.iter().any(has_branch),
    }
}

fn lower_function(module: &hir::Module, f: &hir::Function) -> Function {
    let func = Function {
        name: f.name.clone(),
        params: f.locals[..f.param_count].iter().map(|l| lower_type(l.ty)).collect(),
        ret: f.ret.map(lower_type),
        locals: f.locals.iter().map(|l| lower_type(l.ty)).collect(),
        blocks: vec![BlockData::default()],
        value_types: Vec::new(),
    };
    let mut lower = Lower { module, func, current: Block(0), loops: Vec::new() };
    for stmt in &f.body {
        lower.stmt(stmt);
    }
    if !lower.is_terminated() {
        // Falling off the end. Semantic analysis guarantees this is only
        // reachable for void functions; for value-returning functions this is
        // dead code that still needs a well-formed terminator.
        let value = f.ret.map(|t| lower.zero(lower_type(t)));
        lower.terminate(Terminator::Return(value));
    }
    lower.func
}

impl Lower<'_> {
    fn new_value(&mut self, ty: Type) -> Value {
        let v = Value(self.func.value_types.len() as u32);
        self.func.value_types.push(ty);
        v
    }

    fn new_block(&mut self) -> Block {
        let b = Block(self.func.blocks.len() as u32);
        self.func.blocks.push(BlockData::default());
        b
    }

    fn new_local(&mut self, ty: Type) -> Local {
        let l = Local(self.func.locals.len() as u32);
        self.func.locals.push(ty);
        l
    }

    /// Lowers `exprs` left to right and returns values that are all usable in
    /// the *current* block, spilling any operand that a later one could outlive.
    fn operands(&mut self, exprs: &[&hir::Expr]) -> Vec<Value> {
        enum Slot {
            Direct(Value),
            Spilled(Local, Type),
        }
        let mut slots = Vec::with_capacity(exprs.len());
        for (i, e) in exprs.iter().enumerate() {
            let v = self.expr(e).expect("operand has a value");
            if exprs[i + 1..].iter().any(|later| has_branch(later)) {
                let ty = self.func.value_types[v.0 as usize];
                let local = self.new_local(ty);
                self.emit(Inst::SetLocal { local, src: v });
                slots.push(Slot::Spilled(local, ty));
            } else {
                slots.push(Slot::Direct(v));
            }
        }
        slots
            .into_iter()
            .map(|slot| match slot {
                Slot::Direct(v) => v,
                Slot::Spilled(local, ty) => {
                    let dst = self.new_value(ty);
                    self.emit(Inst::GetLocal { dst, local });
                    dst
                }
            })
            .collect()
    }

    /// `a && b` (`is_and`) or `a || b`; the result travels through a fresh local.
    fn logical(&mut self, is_and: bool, lhs: &hir::Expr, rhs: &hir::Expr) -> Value {
        let result = self.new_local(Type::Bool);
        let l = self.expr(lhs).expect("lhs has a value");
        self.emit(Inst::SetLocal { local: result, src: l });
        let rhs_block = self.new_block();
        let join = self.new_block();
        let (then_block, else_block) = if is_and { (rhs_block, join) } else { (join, rhs_block) };
        self.terminate(Terminator::Branch { cond: l, then_block, else_block });

        self.current = rhs_block;
        let r = self.expr(rhs).expect("rhs has a value");
        self.emit(Inst::SetLocal { local: result, src: r });
        self.terminate(Terminator::Jump(join));

        self.current = join;
        let dst = self.new_value(Type::Bool);
        self.emit(Inst::GetLocal { dst, local: result });
        dst
    }

    fn emit(&mut self, inst: Inst) {
        self.func.blocks[self.current.0 as usize].insts.push(inst);
    }

    fn is_terminated(&self) -> bool {
        self.func.blocks[self.current.0 as usize].terminator.is_some()
    }

    fn terminate(&mut self, term: Terminator) {
        self.func.blocks[self.current.0 as usize].terminator = Some(term);
    }

    fn zero(&mut self, ty: Type) -> Value {
        let dst = self.new_value(ty);
        match ty {
            Type::F64 => self.emit(Inst::ConstF64 { dst, value: 0.0 }),
            Type::Bool => self.emit(Inst::ConstBool { dst, value: false }),
            Type::Str => self.emit(Inst::ConstStr { dst, value: String::new() }),
            Type::Arr => self.emit(Inst::AllocArray { dst, capacity: 0 }),
            Type::Obj => self.emit(Inst::AllocObj { dst }),
            Type::Null => self.emit(Inst::ConstNull { dst }),
        }
        dst
    }

    fn stmts(&mut self, stmts: &[hir::Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::SetLocal(id, e) => {
                let src = self.expr(e).expect("initializer has a value");
                self.emit(Inst::SetLocal { local: Local(*id), src });
            }
            hir::Stmt::Expr(e) => {
                self.expr(e);
            }
            hir::Stmt::If { cond, then, otherwise } => {
                let c = self.expr(cond).expect("condition has a value");
                let then_block = self.new_block();
                let else_block = self.new_block();
                let join = self.new_block();
                self.terminate(Terminator::Branch { cond: c, then_block, else_block });

                self.current = then_block;
                self.stmts(then);
                if !self.is_terminated() {
                    self.terminate(Terminator::Jump(join));
                }

                self.current = else_block;
                self.stmts(otherwise);
                if !self.is_terminated() {
                    self.terminate(Terminator::Jump(join));
                }

                self.current = join;
            }
            hir::Stmt::While { cond, body, update } => {
                let header = self.new_block();
                let body_block = self.new_block();
                let update_block = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Jump(header));

                self.current = header;
                let c = self.expr(cond).expect("condition has a value");
                self.terminate(Terminator::Branch { cond: c, then_block: body_block, else_block: exit });

                self.current = body_block;
                self.loops.push((update_block, exit));
                self.stmts(body);
                self.loops.pop();
                if !self.is_terminated() {
                    self.terminate(Terminator::Jump(update_block));
                }

                self.current = update_block;
                self.stmts(update);
                if !self.is_terminated() {
                    self.terminate(Terminator::Jump(header));
                }

                self.current = exit;
            }
            hir::Stmt::Break | hir::Stmt::Continue => {
                let &(continue_to, break_to) = self.loops.last().expect("semantic analysis checks loop nesting");
                let target = if matches!(stmt, hir::Stmt::Break) { break_to } else { continue_to };
                self.terminate(Terminator::Jump(target));
                // Anything after a `break`/`continue` is unreachable.
                self.current = self.new_block();
            }
            hir::Stmt::Return(value) => {
                let v = value.as_ref().map(|e| self.expr(e).expect("return value"));
                self.terminate(Terminator::Return(v));
                // Anything after a `return` is unreachable; give it its own block.
                self.current = self.new_block();
            }
        }
    }

    fn expr(&mut self, e: &hir::Expr) -> Option<Value> {
        match &e.kind {
            hir::ExprKind::Number(v) => {
                let dst = self.new_value(Type::F64);
                self.emit(Inst::ConstF64 { dst, value: *v });
                Some(dst)
            }
            hir::ExprKind::Str(s) => {
                let dst = self.new_value(Type::Str);
                self.emit(Inst::ConstStr { dst, value: s.clone() });
                Some(dst)
            }
            hir::ExprKind::Bool(b) => {
                let dst = self.new_value(Type::Bool);
                self.emit(Inst::ConstBool { dst, value: *b });
                Some(dst)
            }
            hir::ExprKind::Null | hir::ExprKind::Undefined => {
                let dst = self.new_value(Type::Null);
                self.emit(Inst::ConstNull { dst });
                Some(dst)
            }
            hir::ExprKind::Local(id) => {
                let ty = self.func.locals[*id as usize];
                let dst = self.new_value(ty);
                self.emit(Inst::GetLocal { dst, local: Local(*id) });
                Some(dst)
            }
            hir::ExprKind::Unary(op, operand) => {
                let src = self.expr(operand).expect("operand has a value");
                let (op, ty) = match op {
                    UnaryOp::Neg => (UnOp::NegF64, Type::F64),
                    UnaryOp::Not => (UnOp::NotBool, Type::Bool),
                    UnaryOp::TypeOf => unreachable!("TypeOf is resolved in semantic analysis"),
                };
                let dst = self.new_value(ty);
                self.emit(Inst::Unary { dst, op, src });
                Some(dst)
            }
            hir::ExprKind::Binary(op, lhs, rhs) => {
                let l_ty = lower_type(lhs.ty.expect("typed operand"));
                let r_ty = lower_type(rhs.ty.expect("typed operand"));
                let ops = self.operands(&[&**lhs, &**rhs]);
                let (mut l, mut r) = (ops[0], ops[1]);
                if *op == BinaryOp::Add && (l_ty == Type::Str || r_ty == Type::Str) {
                    if l_ty != Type::Str {
                        let dst = self.new_value(Type::Str);
                        if l_ty == Type::F64 {
                            self.emit(Inst::NumToStr { dst, src: l });
                        } else {
                            self.emit(Inst::BoolToStr { dst, src: l });
                        }
                        l = dst;
                    }
                    if r_ty != Type::Str {
                        let dst = self.new_value(Type::Str);
                        if r_ty == Type::F64 {
                            self.emit(Inst::NumToStr { dst, src: r });
                        } else {
                            self.emit(Inst::BoolToStr { dst, src: r });
                        }
                        r = dst;
                    }
                    let dst = self.new_value(Type::Str);
                    self.emit(Inst::ConcatStr { dst, lhs: l, rhs: r });
                    return Some(dst);
                }
                let operand_ty = l_ty;
                let (bin, cmp_op): (Option<BinOp>, Option<CmpOp>) = match op {
                    BinaryOp::Add => (Some(BinOp::AddF64), None),
                    BinaryOp::Sub => (Some(BinOp::SubF64), None),
                    BinaryOp::Mul => (Some(BinOp::MulF64), None),
                    BinaryOp::Div => (Some(BinOp::DivF64), None),
                    BinaryOp::Rem => (Some(BinOp::RemF64), None),
                    BinaryOp::Lt => (None, Some(CmpOp::Lt)),
                    BinaryOp::LtEq => (None, Some(CmpOp::Le)),
                    BinaryOp::Gt => (None, Some(CmpOp::Gt)),
                    BinaryOp::GtEq => (None, Some(CmpOp::Ge)),
                    // Both operands have the same type here, so == and === coincide.
                    BinaryOp::Eq | BinaryOp::StrictEq => (None, Some(CmpOp::Eq)),
                    BinaryOp::NotEq | BinaryOp::StrictNotEq => (None, Some(CmpOp::Ne)),
                    BinaryOp::And | BinaryOp::Or | BinaryOp::NullishCoalescing => unreachable!("lowered as hir::ExprKind::And/Or/NullishCoalescing"),
                };
                if let Some(op) = bin {
                    let dst = self.new_value(Type::F64);
                    self.emit(Inst::Binary { dst, op, lhs: l, rhs: r });
                    Some(dst)
                } else {
                    let op = cmp_op.expect("comparison operator");
                    let dst = self.new_value(Type::Bool);
                    self.emit(Inst::Compare { dst, op, ty: operand_ty, lhs: l, rhs: r });
                    Some(dst)
                }
            }
            hir::ExprKind::And(lhs, rhs) => Some(self.logical(true, lhs, rhs)),
            hir::ExprKind::Or(lhs, rhs) => Some(self.logical(false, lhs, rhs)),
            hir::ExprKind::NullishCoalescing(lhs, rhs) => {
                let l = self.expr(lhs).expect("?? lhs");
                let lhs_ty = lhs.ty.expect("typed lhs");
                use hir::Type as HT;
                // If LHS is statically non-null, it can never be null/undefined,
                // so `lhs ?? rhs` always evaluates to `lhs` — just emit lhs.
                if lhs_ty != HT::Null {
                    return Some(l);
                }
                let r = self.expr(rhs).expect("?? rhs");
                let rhs_ty = rhs.ty.expect("typed rhs");
                let result_ty = lower_type(rhs_ty);
                let dst = self.new_value(result_ty);
                self.emit(Inst::NullishCoalesce { dst, lhs: l, rhs: r });
                Some(dst)
            }
            hir::ExprKind::OptionalChain(obj, prop) => {
                let o = self.expr(obj).expect("?. object");
                // Build the key string value for ObjGetStr/ObjGetNum
                let key = self.new_value(Type::Str);
                self.emit(Inst::ConstStr { dst: key, value: prop.clone() });
                let ty = lower_type(obj.ty.expect("typed object"));
                let dst = self.new_value(ty);
                self.emit(Inst::OptionalChain { dst, obj: o, key });
                Some(dst)
            }
            hir::ExprKind::Assign(id, value) => {
                let src = self.expr(value).expect("assigned value");
                self.emit(Inst::SetLocal { local: Local(*id), src });
                Some(src)
            }
            hir::ExprKind::Call(callee, args) => {
                let module = self.module;
                let ret = module.functions[*callee as usize].ret;
                let arg_refs: Vec<&hir::Expr> = args.iter().collect();
                let arg_values = self.operands(&arg_refs);
                let dst = ret.map(|t| self.new_value(lower_type(t)));
                self.emit(Inst::Call { dst, callee: FuncRef(*callee), args: arg_values });
                dst
            }
            hir::ExprKind::Print(arg) => {
                let ty = lower_type(arg.ty.expect("typed argument"));
                let value = self.expr(arg).expect("argument has a value");
                self.emit(Inst::Print { ty, value });
                None
            }
            hir::ExprKind::StrLen(arg) => {
                let src = self.expr(arg).expect("argument has a value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::StrLen { dst, src });
                Some(dst)
            }
            hir::ExprKind::StrUpper(arg) => {
                let src = self.expr(arg).expect("argument has a value");
                let dst = self.new_value(Type::Str);
                self.emit(Inst::StrUpper { dst, src });
                Some(dst)
            }
            hir::ExprKind::StrIncludes(obj, needle) => {
                let ops = self.operands(&[&**obj, &**needle]);
                let dst = self.new_value(Type::Bool);
                self.emit(Inst::StrIncludes { dst, src: ops[0], needle: ops[1] });
                Some(dst)
            }
            hir::ExprKind::StrSlice(obj, start, end) => {
                let ops = self.operands(&[&**obj, &**start, &**end]);
                let dst = self.new_value(Type::Str);
                self.emit(Inst::StrSlice { dst, src: ops[0], start: ops[1], end: ops[2] });
                Some(dst)
            }
            hir::ExprKind::StrReplace(obj, from, to) => {
                let ops = self.operands(&[&**obj, &**from, &**to]);
                let dst = self.new_value(Type::Str);
                self.emit(Inst::StrReplace { dst, src: ops[0], from: ops[1], to: ops[2] });
                Some(dst)
            }
            hir::ExprKind::StrSplitCount(obj, sep) => {
                let ops = self.operands(&[&**obj, &**sep]);
                let dst = self.new_value(Type::F64);
                self.emit(Inst::StrSplitCount { dst, src: ops[0], sep: ops[1] });
                Some(dst)
            }
            hir::ExprKind::MathSqrt(arg) => {
                let src = self.expr(arg).expect("argument has a value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::MathSqrt { dst, src });
                Some(dst)
            }
            hir::ExprKind::MathAbs(arg) => {
                let src = self.expr(arg).expect("argument has a value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::MathAbs { dst, src });
                Some(dst)
            }
            hir::ExprKind::Array(elements) => {
                let dst = self.new_value(Type::Arr);
                self.emit(Inst::AllocArray { dst, capacity: elements.len() });
                for elem in elements {
                    let v = self.expr(elem).expect("array element value");
                    self.emit(Inst::ArrayPush { arr: dst, val: v });
                }
                Some(dst)
            }
            hir::ExprKind::ArrayGet(arr, index) => {
                let a = self.expr(arr).expect("array value");
                let idx = self.expr(index).expect("index value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::ArrayGet { dst, arr: a, index: idx });
                Some(dst)
            }
            hir::ExprKind::ArraySet(arr, index, val) => {
                let a = self.expr(arr).expect("array value");
                let idx = self.expr(index).expect("index value");
                let v = self.expr(val).expect("set value");
                self.emit(Inst::ArraySet { arr: a, index: idx, val: v });
                Some(v)
            }
            hir::ExprKind::ArrayLen(arr) => {
                let a = self.expr(arr).expect("array value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::ArrayLen { dst, arr: a });
                Some(dst)
            }
            hir::ExprKind::ArrayPush(arr, val) => {
                let a = self.expr(arr).expect("array value");
                let v = self.expr(val).expect("push value");
                self.emit(Inst::ArrayPush { arr: a, val: v });
                None
            }
            hir::ExprKind::Object(props) => {
                let dst = self.new_value(Type::Obj);
                self.emit(Inst::AllocObj { dst });
                for (key, val) in props {
                    let k = self.new_value(Type::Str);
                    self.emit(Inst::ConstStr { dst: k, value: key.clone() });
                    let v = self.expr(val).expect("prop value");
                    if val.ty == Some(hir::Type::String) {
                        self.emit(Inst::ObjSetStr { obj: dst, key: k, val: v });
                    } else {
                        self.emit(Inst::ObjSetNum { obj: dst, key: k, val: v });
                    }
                }
                Some(dst)
            }
            hir::ExprKind::ObjGetNum(obj, key) => {
                let o = self.expr(obj).expect("obj value");
                let k = self.expr(key).expect("key value");
                let dst = self.new_value(Type::F64);
                self.emit(Inst::ObjGetNum { dst, obj: o, key: k });
                Some(dst)
            }
            hir::ExprKind::ObjGetStr(obj, key) => {
                let o = self.expr(obj).expect("obj value");
                let k = self.expr(key).expect("key value");
                let dst = self.new_value(Type::Str);
                self.emit(Inst::ObjGetStr { dst, obj: o, key: k });
                Some(dst)
            }
            hir::ExprKind::ObjSetNum(obj, key, val) => {
                let o = self.expr(obj).expect("obj value");
                let k = self.expr(key).expect("key value");
                let v = self.expr(val).expect("val value");
                self.emit(Inst::ObjSetNum { obj: o, key: k, val: v });
                Some(v)
            }
            hir::ExprKind::ObjSetStr(obj, key, val) => {
                let o = self.expr(obj).expect("obj value");
                let k = self.expr(key).expect("key value");
                let v = self.expr(val).expect("val value");
                self.emit(Inst::ObjSetStr { obj: o, key: k, val: v });
                Some(v)
            }
        }
    }
}
