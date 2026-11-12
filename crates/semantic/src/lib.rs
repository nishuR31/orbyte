//! Semantic analysis: scope resolution, local slot assignment and a strict
//! static check of the milestone-1 subset.
//!
//! The rule of this stage is *reject, never miscompile*: anything whose
//! JavaScript semantics we cannot yet reproduce exactly (implicit conversions,
//! `undefined`, closures, hoisting of `var`) is a compile error.
//!
//! Binding rules enforced here:
//!
//! * `let`/`const` are block-scoped and live in the temporal dead zone from the
//!   start of their block until their declaration has been evaluated. Any
//!   access in that window (including `let x = x + 1`, or reading an outer `x`
//!   that an inner `let x` shadows later in the same block) is a compile error,
//!   because JavaScript would throw a `ReferenceError` there.
//! * `var` is function-scoped, but it is not hoisted as `undefined`. A `var`
//!   declared in a nested block may only be read inside that block, because
//!   outside it the declaration might not have run and JavaScript would
//!   produce `undefined`. Reading it there is a compile error.

pub mod hir;

use std::collections::{HashMap, HashSet};

use ast::{BinaryOp, Expr, ExprKind, FunctionDecl, Ident, Program, Stmt, StmtKind, UnaryOp, VarKind};
use diagnostics::{Diagnostic, Result, Span};
use hir::{FuncId, LocalId, Type};

/// Name of the synthetic function holding top-level statements.
pub const MAIN_NAME: &str = "__main";

/// A resolved name.
#[derive(Clone, Copy)]
struct Binding {
    id: LocalId,
    is_const: bool,
    /// Id of the scope whose body the declaration ran in. The binding may only
    /// be read while that scope is still active (see the module docs on `var`).
    owner: u32,
}

#[derive(Default)]
struct Scope {
    id: u32,
    names: HashMap<String, Binding>,
    /// `let`/`const` names declared in this scope whose declaration has not been
    /// evaluated yet (the temporal dead zone).
    tdz: HashSet<String>,
}

struct Env<'a> {
    rets: &'a [Option<Type>],
    param_counts: &'a [usize],
    param_types: &'a [Vec<Type>],
    names: &'a HashMap<String, FuncId>,
    str_props: std::cell::RefCell<HashSet<String>>,
}

fn param_is_obj(param: &str, stmts: &[Stmt]) -> bool {
    fn expr_has_obj_use(param: &str, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Member { object, .. } => {
                if matches!(&object.kind, ExprKind::Ident(id) if id == param) {
                    return true;
                }
                expr_has_obj_use(param, object)
            }
            ExprKind::Binary { lhs, rhs, .. } => expr_has_obj_use(param, lhs) || expr_has_obj_use(param, rhs),
            ExprKind::Unary { operand, .. } => expr_has_obj_use(param, operand),
            ExprKind::Call { callee, args } => expr_has_obj_use(param, callee) || args.iter().any(|a| expr_has_obj_use(param, a)),
            _ => false,
        }
    }
    fn stmt_has_obj_use(param: &str, s: &Stmt) -> bool {
        match &s.kind {
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => expr_has_obj_use(param, e),
            StmtKind::Var { init: Some(e), .. } => expr_has_obj_use(param, e),
            StmtKind::If { cond, then, otherwise } => {
                expr_has_obj_use(param, cond) || stmt_has_obj_use(param, then) || otherwise.as_ref().map_or(false, |o| stmt_has_obj_use(param, o))
            }
            StmtKind::While { cond, body } => expr_has_obj_use(param, cond) || stmt_has_obj_use(param, body),
            StmtKind::Block(stmts) => stmts.iter().any(|st| stmt_has_obj_use(param, st)),
            _ => false,
        }
    }
    stmts.iter().any(|s| stmt_has_obj_use(param, s))
}

struct FnCx<'a> {
    env: &'a Env<'a>,
    locals: Vec<hir::Local>,
    scopes: Vec<Scope>,
    ret_seen: Option<Option<Type>>,
    is_main: bool,
    loop_depth: u32,
    next_scope_id: u32,
}

pub fn analyze(program: &Program) -> Result<hir::Module> {
    let mut decls: Vec<&FunctionDecl> = Vec::new();
    let mut main_body: Vec<&Stmt> = Vec::new();
    let mut names: HashMap<String, FuncId> = HashMap::new();

    // Function declarations are hoisted: collect them all before checking bodies.
    for stmt in &program.body {
        match &stmt.kind {
            StmtKind::Function(f) => {
                if names.insert(f.name.name.clone(), decls.len() as FuncId).is_some() {
                    return Err(Diagnostic::new(
                        format!("function '{}' is declared more than once", f.name.name),
                        f.name.span,
                    ));
                }
                decls.push(f);
            }
            _ => main_body.push(stmt),
        }
    }

    let param_counts: Vec<usize> = decls.iter().map(|f| f.params.len()).collect();
    let param_types: Vec<Vec<Type>> = decls
        .iter()
        .map(|f| {
            f.params
                .iter()
                .map(|p| if param_is_obj(&p.name, &f.body) { Type::Obj } else { Type::Number })
                .collect()
        })
        .collect();
    let n = decls.len();

    // Return types are inferred by iterating to a fixed point. The starting
    // guess comes from a syntactic scan of each function's first `return`; the
    // iteration then re-checks every body against the current guesses until
    // they agree. A function that fails to check under a stale guess keeps its
    // previous guess for the next round (the error may disappear once its
    // callees settle); errors are only reported once the guesses stop changing.
    let mut rets: Vec<Option<Type>> = decls.iter().map(|f| seed_return(&f.body)).collect();
    let str_props = std::cell::RefCell::new(HashSet::new());
    for _ in 0..=(n + 1) {
        let env = Env {
            rets: &rets,
            param_counts: &param_counts,
            param_types: &param_types,
            names: &names,
            str_props: str_props.clone(),
        };
        let mut funcs = Vec::with_capacity(n + 1);
        let mut new_rets = Vec::with_capacity(n);
        let mut first_error: Option<Diagnostic> = None;
        for (i, f) in decls.iter().enumerate() {
            match check_function(f, &env, &param_types[i]) {
                Ok(func) => {
                    new_rets.push(func.ret);
                    funcs.push(func);
                }
                Err(e) => {
                    new_rets.push(rets[i]);
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if new_rets == rets {
            if let Some(e) = first_error {
                return Err(e);
            }
            funcs.push(check_main(&main_body, &env)?);
            return Ok(hir::Module { main: n as FuncId, functions: funcs });
        }
        rets = new_rets;
    }

    Err(Diagnostic::new(
        "could not infer function return types (unsupported recursion pattern)",
        decls.first().map(|f| f.name.span).unwrap_or_default(),
    ))
}

/// Initial return-type guess: the shape of the first `return` found in the body.
fn seed_return(stmts: &[Stmt]) -> Option<Type> {
    fn syntactically_type(e: &Expr) -> Type {
        match &e.kind {
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::String,
            ExprKind::Unary { op: UnaryOp::Not, .. } => Type::Bool,
            ExprKind::Binary { op, lhs, .. } => {
                if matches!(op, BinaryOp::Add) && matches!(&lhs.kind, ExprKind::Str(_)) {
                    Type::String
                } else if !matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
                ) {
                    Type::Bool
                } else {
                    Type::Number
                }
            }
            _ => Type::Number,
        }
    }
    // Some(Some(t)) = returns t, Some(None) = bare return, None = no return seen.
    fn scan(stmts: &[Stmt]) -> Option<Option<Type>> {
        for s in stmts {
            let found = match &s.kind {
                StmtKind::Return(Some(e)) => Some(Some(syntactically_type(e))),
                StmtKind::Return(None) => Some(None),
                StmtKind::If { then, otherwise, .. } => scan(std::slice::from_ref(&**then))
                    .or_else(|| otherwise.as_deref().and_then(|o| scan(std::slice::from_ref(o)))),
                StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                    scan(std::slice::from_ref(&**body))
                }
                StmtKind::Block(inner) => scan(inner),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
    scan(stmts).flatten()
}

fn check_function(f: &FunctionDecl, env: &Env<'_>, param_tys: &[Type]) -> Result<hir::Function> {
    let mut cx = FnCx::new(env, false);
    for (i, p) in f.params.iter().enumerate() {
        cx.declare(p, param_tys[i], false, false)?;
    }
    cx.hoist_lexicals(f.body.iter());
    let mut body = Vec::new();
    for s in &f.body {
        cx.stmt(s, &mut body)?;
    }
    let ret = match cx.ret_seen {
        None | Some(None) => None,
        Some(Some(t)) => Some(t),
    };
    if ret.is_some() && !always_returns(&body) {
        return Err(Diagnostic::new(
            format!(
                "function '{}' can reach its end without returning a value (undefined isn't supported yet)",
                f.name.name
            ),
            f.name.span,
        ));
    }
    Ok(hir::Function {
        name: f.name.name.clone(),
        param_count: f.params.len(),
        locals: cx.locals,
        ret,
        body,
    })
}

fn check_main(stmts: &[&Stmt], env: &Env<'_>) -> Result<hir::Function> {
    let mut cx = FnCx::new(env, true);
    cx.hoist_lexicals(stmts.iter().copied());
    let mut body = Vec::new();
    for s in stmts {
        cx.stmt(s, &mut body)?;
    }
    Ok(hir::Function {
        name: MAIN_NAME.to_string(),
        param_count: 0,
        locals: cx.locals,
        ret: None,
        body,
    })
}

fn always_returns(stmts: &[hir::Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        hir::Stmt::Return(_) => true,
        hir::Stmt::If { then, otherwise, .. } => always_returns(then) && always_returns(otherwise),
        _ => false,
    })
}

fn value_type(e: &hir::Expr, span: Span) -> Result<Type> {
    e.ty.ok_or_else(|| Diagnostic::new("this expression has no value", span))
}

fn expect_type(found: Type, wanted: Type, what: &str, span: Span) -> Result<()> {
    if found == wanted {
        Ok(())
    } else {
        Err(Diagnostic::new(
            format!("{what} must be {wanted}, found {found} (implicit conversions aren't supported yet)"),
            span,
        ))
    }
}

impl<'a> FnCx<'a> {
    fn new(env: &'a Env<'a>, is_main: bool) -> Self {
        FnCx {
            env,
            locals: Vec::new(),
            scopes: vec![Scope { id: 0, ..Scope::default() }],
            ret_seen: None,
            is_main,
            loop_depth: 0,
            next_scope_id: 1,
        }
    }

    fn push_scope(&mut self) {
        let id = self.next_scope_id;
        self.next_scope_id += 1;
        self.scopes.push(Scope { id, ..Scope::default() });
    }

    /// Puts the `let`/`const` names declared directly in `stmts` into the
    /// temporal dead zone of the current scope.
    fn hoist_lexicals<'s>(&mut self, stmts: impl IntoIterator<Item = &'s Stmt>) {
        let scope = self.scopes.last_mut().expect("there is always a scope");
        for s in stmts {
            if let StmtKind::Var { kind: VarKind::Let | VarKind::Const, name, .. } = &s.kind {
                scope.tdz.insert(name.name.clone());
            }
        }
    }

    /// Resolves `name`, rejecting accesses JavaScript would not allow to see an
    /// initialised value (temporal dead zone, `var` outside its block).
    fn lookup(&self, name: &str, span: Span) -> Result<Option<Binding>> {
        for scope in self.scopes.iter().rev() {
            if scope.tdz.contains(name) {
                return Err(Diagnostic::new(
                    format!("cannot access '{name}' before its initialization"),
                    span,
                ));
            }
            if let Some(b) = scope.names.get(name) {
                if !self.scopes.iter().any(|s| s.id == b.owner) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{name}' may be uninitialized here: a 'var' declared in a nested block can only be used inside that block for now"
                        ),
                        span,
                    ));
                }
                return Ok(Some(*b));
            }
        }
        Ok(None)
    }

    fn declare(&mut self, name: &Ident, ty: Type, is_const: bool, function_scope: bool) -> Result<LocalId> {
        let current = self.scopes.len() - 1;
        let idx = if function_scope { 0 } else { current };
        let clash = if function_scope {
            // A `var` may not collide with a lexical binding of an enclosing block either.
            self.scopes
                .iter()
                .any(|s| s.names.contains_key(&name.name) || s.tdz.contains(&name.name))
        } else {
            self.scopes[idx].names.contains_key(&name.name)
        };
        if clash {
            return Err(Diagnostic::new(
                format!("'{}' is already declared in this scope", name.name),
                name.span,
            ));
        }
        let id = self.locals.len() as LocalId;
        self.locals.push(hir::Local { name: name.name.clone(), ty });
        let owner = self.scopes[current].id;
        self.scopes[idx].names.insert(name.name.clone(), Binding { id, is_const, owner });
        // The declaration has now been evaluated: the name leaves the dead zone.
        self.scopes[idx].tdz.remove(&name.name);
        Ok(id)
    }

    fn block_of(&mut self, s: &Stmt) -> Result<Vec<hir::Stmt>> {
        let mut out = Vec::new();
        self.push_scope();
        self.stmt(s, &mut out)?;
        self.scopes.pop();
        Ok(out)
    }

    fn loop_body(&mut self, body: &Stmt) -> Result<Vec<hir::Stmt>> {
        self.loop_depth += 1;
        let result = self.block_of(body);
        self.loop_depth -= 1;
        result
    }

    /// Checks an expression whose value is thrown away (an expression
    /// statement or a `for` update). Only here is postfix `x++` meaningful,
    /// because its value (the old one) is never observed.
    fn discarded(&mut self, e: &Expr) -> Result<hir::Expr> {
        match &e.kind {
            ExprKind::Update { target, increment, prefix: false } => {
                let as_prefix = Expr {
                    kind: ExprKind::Update { target: target.clone(), increment: *increment, prefix: true },
                    span: e.span,
                };
                self.expr(&as_prefix)
            }
            _ => self.expr(e),
        }
    }

    fn condition(&mut self, cond: &Expr) -> Result<hir::Expr> {
        let c = self.expr(cond)?;
        let t = value_type(&c, cond.span)?;
        expect_type(t, Type::Bool, "a condition", cond.span)?;
        Ok(c)
    }

    fn stmt(&mut self, s: &Stmt, out: &mut Vec<hir::Stmt>) -> Result<()> {
        match &s.kind {
            StmtKind::Function(f) => Err(Diagnostic::new(
                "nested function declarations aren't supported yet",
                f.span,
            )),
            StmtKind::Empty => Ok(()),
            StmtKind::Expr(e) => {
                let e = self.discarded(e)?;
                out.push(hir::Stmt::Expr(e));
                Ok(())
            }
            StmtKind::Var { kind, name, init } => {
                let Some(init) = init else {
                    return Err(Diagnostic::new(
                        format!("'{}' needs an initializer (undefined isn't supported yet)", name.name),
                        name.span,
                    ));
                };
                // Evaluate the initializer before declaring, so `let x = x + 1` is an error.
                let value = self.expr(init)?;
                let ty = value_type(&value, init.span)?;
                // `var` is function-scoped, but not hoisted: using it before its
                // declaration is a compile error instead of silently `undefined`.
                let id = self.declare(name, ty, *kind == VarKind::Const, *kind == VarKind::Var)?;
                out.push(hir::Stmt::SetLocal(id, value));
                Ok(())
            }
            StmtKind::If { cond, then, otherwise } => {
                let c = self.condition(cond)?;
                let t = self.block_of(then)?;
                let o = match otherwise {
                    Some(o) => self.block_of(o)?,
                    None => Vec::new(),
                };
                out.push(hir::Stmt::If { cond: c, then: t, otherwise: o });
                Ok(())
            }
            StmtKind::While { cond, body } => {
                let c = self.condition(cond)?;
                let b = self.loop_body(body)?;
                out.push(hir::Stmt::While { cond: c, body: b, update: Vec::new() });
                Ok(())
            }
            StmtKind::For { init, cond, update, body } => {
                // The whole loop gets its own scope so `for (let i ...)` does not leak `i`.
                self.push_scope();
                if let Some(init) = init {
                    self.hoist_lexicals(std::iter::once(&**init));
                    self.stmt(init, out)?;
                }
                let c = match cond {
                    Some(c) => self.condition(c)?,
                    None => hir::Expr::new(hir::ExprKind::Bool(true), Some(Type::Bool)),
                };
                let b = self.loop_body(body)?;
                let mut upd = Vec::new();
                if let Some(u) = update {
                    let e = self.discarded(u)?;
                    upd.push(hir::Stmt::Expr(e));
                }
                self.scopes.pop();
                out.push(hir::Stmt::While { cond: c, body: b, update: upd });
                Ok(())
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    let word = if matches!(s.kind, StmtKind::Break) { "break" } else { "continue" };
                    return Err(Diagnostic::new(format!("'{word}' outside of a loop"), s.span));
                }
                out.push(if matches!(s.kind, StmtKind::Break) {
                    hir::Stmt::Break
                } else {
                    hir::Stmt::Continue
                });
                Ok(())
            }
            StmtKind::Block(stmts) => {
                self.push_scope();
                self.hoist_lexicals(stmts.iter());
                for s in stmts {
                    self.stmt(s, out)?;
                }
                self.scopes.pop();
                Ok(())
            }
            StmtKind::Return(value) => {
                if self.is_main {
                    return Err(Diagnostic::new("'return' outside of a function", s.span));
                }
                let (v, ty) = match value {
                    Some(e) => {
                        let v = self.expr(e)?;
                        let t = value_type(&v, e.span)?;
                        (Some(v), Some(t))
                    }
                    None => (None, None),
                };
                let seen = self.ret_seen;
                match seen {
                    None => self.ret_seen = Some(ty),
                    Some(prev) if prev != ty => {
                        return Err(Diagnostic::new(
                            "inconsistent return types in this function (all returns must produce the same type)",
                            s.span,
                        ));
                    }
                    Some(_) => {}
                }
                out.push(hir::Stmt::Return(v));
                Ok(())
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<hir::Expr> {
        let span = e.span;
        match &e.kind {
            ExprKind::Number(v) => Ok(hir::Expr::new(hir::ExprKind::Number(*v), Some(Type::Number))),
            ExprKind::Str(s) => Ok(hir::Expr::new(hir::ExprKind::Str(s.clone()), Some(Type::String))),
            ExprKind::Bool(b) => Ok(hir::Expr::new(hir::ExprKind::Bool(*b), Some(Type::Bool))),
            ExprKind::Null => Ok(hir::Expr::new(hir::ExprKind::Null, Some(Type::Null))),
            ExprKind::Undefined => Ok(hir::Expr::new(hir::ExprKind::Undefined, Some(Type::Null))),
            ExprKind::Ident(name) => {
                if let Some(b) = self.lookup(name, span)? {
                    let ty = self.locals[b.id as usize].ty;
                    Ok(hir::Expr::new(hir::ExprKind::Local(b.id), Some(ty)))
                } else if self.env.names.contains_key(name) {
                    Err(Diagnostic::new("functions can't be used as values yet", span))
                } else {
                    Err(Diagnostic::new(format!("'{name}' is not defined"), span))
                }
            }
            ExprKind::Unary { op, operand } => {
                if *op == UnaryOp::TypeOf {
                    if let ExprKind::Ident(name) = &operand.kind {
                        if name == "undefined" {
                            return Ok(hir::Expr::new(hir::ExprKind::Str("undefined".into()), Some(Type::String)));
                        }
                        if name == "null" {
                            return Ok(hir::Expr::new(hir::ExprKind::Str("object".into()), Some(Type::String)));
                        }
                        if let Some(b) = self.lookup(name, operand.span).ok().flatten() {
                            let local_ty = self.locals[b.id as usize].ty;
                            let str_val = match local_ty {
                                Type::Number => "number",
                                Type::Bool => "boolean",
                                Type::String => "string",
                                Type::Arr | Type::Obj => "object",
                                Type::Null => "object",
                            };
                            return Ok(hir::Expr::new(hir::ExprKind::Str(str_val.into()), Some(Type::String)));
                        }
                        if self.env.names.contains_key(name) {
                            return Ok(hir::Expr::new(hir::ExprKind::Str("function".into()), Some(Type::String)));
                        }
                        return Ok(hir::Expr::new(hir::ExprKind::Str("undefined".into()), Some(Type::String)));
                    }
                    let v = self.expr(operand)?;
                    let t = value_type(&v, operand.span)?;
                    let str_val = match t {
                        Type::Number => "number",
                        Type::Bool => "boolean",
                        Type::String => "string",
                        Type::Arr | Type::Obj => "object",
                        Type::Null => "object",
                    };
                    return Ok(hir::Expr::new(hir::ExprKind::Str(str_val.into()), Some(Type::String)));
                }
                let v = self.expr(operand)?;
                let t = value_type(&v, operand.span)?;
                let result = match op {
                    UnaryOp::Neg => {
                        expect_type(t, Type::Number, "operand of '-'", operand.span)?;
                        Type::Number
                    }
                    UnaryOp::Not => {
                        expect_type(t, Type::Bool, "operand of '!'", operand.span)?;
                        Type::Bool
                    }
                    UnaryOp::TypeOf => unreachable!(),
                };
                Ok(hir::Expr::new(hir::ExprKind::Unary(*op, Box::new(v)), Some(result)))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs)?;
                let lt = value_type(&l, lhs.span)?;
                let r = self.expr(rhs)?;
                let rt = value_type(&r, rhs.span)?;
                let what = format!("operand of '{}'", op.symbol());
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    expect_type(lt, Type::Bool, &what, lhs.span)?;
                    expect_type(rt, Type::Bool, &what, rhs.span)?;
                    let kind = if *op == BinaryOp::And {
                        hir::ExprKind::And(Box::new(l), Box::new(r))
                    } else {
                        hir::ExprKind::Or(Box::new(l), Box::new(r))
                    };
                    return Ok(hir::Expr::new(kind, Some(Type::Bool)));
                }
                let result = match op {
                    BinaryOp::Add => {
                        if lt == Type::String || rt == Type::String {
                            Type::String
                        } else {
                            expect_type(lt, Type::Number, &what, lhs.span)?;
                            expect_type(rt, Type::Number, &what, rhs.span)?;
                            Type::Number
                        }
                    }
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                        expect_type(lt, Type::Number, &what, lhs.span)?;
                        expect_type(rt, Type::Number, &what, rhs.span)?;
                        Type::Number
                    }
                    BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                        expect_type(lt, Type::Number, &what, lhs.span)?;
                        expect_type(rt, Type::Number, &what, rhs.span)?;
                        Type::Bool
                    }
                    BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
                    BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::StrictEq | BinaryOp::StrictNotEq => {
                        if lt != rt {
                            return Err(Diagnostic::new(
                                format!("cannot compare {lt} with {rt} (implicit conversions aren't supported yet)"),
                                span,
                            ));
                        }
                        Type::Bool
                    }
                    BinaryOp::NullishCoalescing => {
                        // `??`: if LHS is null/undefined, result is RHS type.
                        // If LHS is a known non-null type (Number/Bool/String/Arr/Obj),
                        // it will never be null so the result is always LHS type.
                        if lt == Type::Null {
                            rt // null ?? T → T
                        } else {
                            lt // non-null LHS ?? anything → always LHS
                        }
                    }
                };
                // NullishCoalescing must produce a NullishCoalescing HIR node, not Binary.
                if *op == BinaryOp::NullishCoalescing {
                    return Ok(hir::Expr::new(
                        hir::ExprKind::NullishCoalescing(Box::new(l), Box::new(r)),
                        Some(result),
                    ));
                }
                Ok(hir::Expr::new(
                    hir::ExprKind::Binary(*op, Box::new(l), Box::new(r)),
                    Some(result),
                ))
            }
            ExprKind::Assign { target, value } => {
                let Some(binding) = self.lookup(&target.name, target.span)? else {
                    return Err(Diagnostic::new(format!("'{}' is not defined", target.name), target.span));
                };
                let (id, is_const) = (binding.id, binding.is_const);
                if is_const {
                    return Err(Diagnostic::new(
                        format!("cannot assign to constant '{}'", target.name),
                        target.span,
                    ));
                }
                let v = self.expr(value)?;
                let t = value_type(&v, value.span)?;
                let local_ty = self.locals[id as usize].ty;
                if t != local_ty {
                    return Err(Diagnostic::new(
                        format!(
                            "cannot assign {t} to '{}', which holds {local_ty} (variables are statically typed until the dynamic runtime lands)",
                            target.name
                        ),
                        span,
                    ));
                }
                Ok(hir::Expr::new(hir::ExprKind::Assign(id, Box::new(v)), Some(local_ty)))
            }
            ExprKind::Update { target, increment, prefix } => {
                if !*prefix {
                    return Err(Diagnostic::new(
                        "postfix '++'/'--' is only supported as a statement or a for-loop update (its value isn't supported yet)",
                        span,
                    ));
                }
                // `++x` is `x = x + 1`; the type check of the desugared form
                // rejects non-number targets and constants for free.
                let one = Expr { kind: ExprKind::Number(1.0), span };
                let current = Expr { kind: ExprKind::Ident(target.name.clone()), span: target.span };
                let op = if *increment { BinaryOp::Add } else { BinaryOp::Sub };
                let sum = Expr {
                    kind: ExprKind::Binary { op, lhs: Box::new(current), rhs: Box::new(one) },
                    span,
                };
                let assign = Expr {
                    kind: ExprKind::Assign { target: target.clone(), value: Box::new(sum) },
                    span,
                };
                self.expr(&assign)
            }
            ExprKind::Call { callee, args } => self.call(callee, args, span),
            ExprKind::Member { object, property } => {
                if property.name == "length" {
                    if let ExprKind::Call { callee, args } = &object.kind {
                        if let ExprKind::Member { object: inner_obj, property: inner_prop } = &callee.kind {
                            if inner_prop.name == "split" && args.len() == 1 {
                                let str_obj = self.expr(inner_obj)?;
                                let str_ty = value_type(&str_obj, inner_obj.span)?;
                                expect_type(str_ty, Type::String, "receiver of 'split'", inner_obj.span)?;
                                let sep_arg = self.expr(&args[0])?;
                                let sep_ty = value_type(&sep_arg, args[0].span)?;
                                expect_type(sep_ty, Type::String, "separator argument of 'split'", args[0].span)?;
                                return Ok(hir::Expr::new(hir::ExprKind::StrSplitCount(Box::new(str_obj), Box::new(sep_arg)), Some(Type::Number)));
                            }
                        }
                    }
                    let obj = self.expr(object)?;
                    let ty = value_type(&obj, object.span)?;
                    if ty == Type::String {
                        return Ok(hir::Expr::new(hir::ExprKind::StrLen(Box::new(obj)), Some(Type::Number)));
                    }
                    if ty == Type::Arr {
                        return Ok(hir::Expr::new(hir::ExprKind::ArrayLen(Box::new(obj)), Some(Type::Number)));
                    }
                }
                let obj = self.expr(object)?;
                let ty = value_type(&obj, object.span)?;
                if ty == Type::Obj {
                    let key_str = hir::Expr::new(hir::ExprKind::Str(property.name.clone()), Some(Type::String));
                    if self.env.str_props.borrow().contains(&property.name) {
                        return Ok(hir::Expr::new(hir::ExprKind::ObjGetStr(Box::new(obj), Box::new(key_str)), Some(Type::String)));
                    } else {
                        return Ok(hir::Expr::new(hir::ExprKind::ObjGetNum(Box::new(obj), Box::new(key_str)), Some(Type::Number)));
                    }
                }
                Err(Diagnostic::new("property access isn't supported yet", span))
            }
            ExprKind::Array(elements) => {
                let mut helements = Vec::new();
                for elem in elements {
                    let he = self.expr(elem)?;
                    let ty = value_type(&he, elem.span)?;
                    expect_type(ty, Type::Number, "array element", elem.span)?;
                    helements.push(he);
                }
                Ok(hir::Expr::new(hir::ExprKind::Array(helements), Some(Type::Arr)))
            }
            ExprKind::Index { object, index } => {
                let obj = self.expr(object)?;
                let o_ty = value_type(&obj, object.span)?;
                expect_type(o_ty, Type::Arr, "subscripted object", object.span)?;
                let idx = self.expr(index)?;
                let i_ty = value_type(&idx, index.span)?;
                expect_type(i_ty, Type::Number, "array index", index.span)?;
                Ok(hir::Expr::new(hir::ExprKind::ArrayGet(Box::new(obj), Box::new(idx)), Some(Type::Number)))
            }
            ExprKind::IndexAssign { object, index, value } => {
                let obj = self.expr(object)?;
                let o_ty = value_type(&obj, object.span)?;
                expect_type(o_ty, Type::Arr, "subscripted object", object.span)?;
                let idx = self.expr(index)?;
                let i_ty = value_type(&idx, index.span)?;
                expect_type(i_ty, Type::Number, "array index", index.span)?;
                let val = self.expr(value)?;
                let v_ty = value_type(&val, value.span)?;
                expect_type(v_ty, Type::Number, "assigned value", value.span)?;
                Ok(hir::Expr::new(hir::ExprKind::ArraySet(Box::new(obj), Box::new(idx), Box::new(val)), Some(Type::Number)))
            }
            ExprKind::Object(props) => {
                let mut hprops = Vec::new();
                for (key, val) in props {
                    let hv = self.expr(val)?;
                    let ty = value_type(&hv, val.span)?;
                    if ty == Type::String {
                        self.env.str_props.borrow_mut().insert(key.name.clone());
                    }
                    hprops.push((key.name.clone(), hv));
                }
                Ok(hir::Expr::new(hir::ExprKind::Object(hprops), Some(Type::Obj)))
            }
            ExprKind::MemberAssign { object, property, value } => {
                let obj = self.expr(object)?;
                let o_ty = value_type(&obj, object.span)?;
                expect_type(o_ty, Type::Obj, "target object", object.span)?;
                let val = self.expr(value)?;
                let v_ty = value_type(&val, value.span)?;
                let key_str = hir::Expr::new(hir::ExprKind::Str(property.name.clone()), Some(Type::String));
                if v_ty == Type::String {
                    self.env.str_props.borrow_mut().insert(property.name.clone());
                    Ok(hir::Expr::new(hir::ExprKind::ObjSetStr(Box::new(obj), Box::new(key_str), Box::new(val)), Some(Type::String)))
                } else {
                    Ok(hir::Expr::new(hir::ExprKind::ObjSetNum(Box::new(obj), Box::new(key_str), Box::new(val)), Some(Type::Number)))
                }
            }
            ExprKind::OptionalChain { object, property } => {
                // `obj?.prop`: object may be null; property access proceeds only if non-null.
                let obj = self.expr(object)?;
                let o_ty = value_type(&obj, object.span)?;
                // Allow Obj or Null as the object type.
                if o_ty != Type::Obj && o_ty != Type::Null {
                    return Err(Diagnostic::new(
                        format!("cannot use '?.' on {o_ty} (expected object or null)"),
                        object.span,
                    ));
                }
                // Result type: property type (dynamic for objects). At runtime, null object → null.
                Ok(hir::Expr::new(
                    hir::ExprKind::OptionalChain(Box::new(obj), property.name.clone()),
                    Some(Type::Null), // conservative: result could be null or the property value
                ))
            }
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Result<hir::Expr> {
        match &callee.kind {
            ExprKind::Ident(name) => {
                if self.lookup(name, callee.span)?.is_some() {
                    return Err(Diagnostic::new(format!("'{name}' is not a function"), callee.span));
                }
                let Some(&id) = self.env.names.get(name) else {
                    return Err(Diagnostic::new(format!("'{name}' is not defined"), callee.span));
                };
                let expected = self.env.param_counts[id as usize];
                if args.len() != expected {
                    return Err(Diagnostic::new(
                        format!(
                            "'{name}' expects {expected} argument(s) but got {} (undefined isn't supported yet)",
                            args.len()
                        ),
                        span,
                    ));
                }
                let expected_tys = &self.env.param_types[id as usize];
                let mut hargs = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let v = self.expr(a)?;
                    let t = value_type(&v, a.span)?;
                    expect_type(t, expected_tys[i], "a function argument", a.span)?;
                    hargs.push(v);
                }
                Ok(hir::Expr::new(
                    hir::ExprKind::Call(id, hargs),
                    self.env.rets[id as usize],
                ))
            }
            ExprKind::Member { object, property } => {
                let is_console_log = matches!(&object.kind, ExprKind::Ident(o) if o == "console")
                    && property.name == "log"
                    && self.lookup("console", span)?.is_none();
                if is_console_log {
                    if args.len() != 1 {
                        return Err(Diagnostic::new("console.log takes exactly one argument for now", span));
                    }
                    let v = self.expr(&args[0])?;
                    value_type(&v, args[0].span)?;
                    return Ok(hir::Expr::new(hir::ExprKind::Print(Box::new(v)), None));
                }
                let is_math = matches!(&object.kind, ExprKind::Ident(o) if o == "Math")
                    && self.lookup("Math", span)?.is_none();
                if is_math {
                    match property.name.as_str() {
                        "sqrt" => {
                            if args.len() != 1 {
                                return Err(Diagnostic::new("Math.sqrt takes 1 argument", span));
                            }
                            let v = self.expr(&args[0])?;
                            let ty = value_type(&v, args[0].span)?;
                            expect_type(ty, Type::Number, "argument of 'Math.sqrt'", args[0].span)?;
                            return Ok(hir::Expr::new(hir::ExprKind::MathSqrt(Box::new(v)), Some(Type::Number)));
                        }
                        "abs" => {
                            if args.len() != 1 {
                                return Err(Diagnostic::new("Math.abs takes 1 argument", span));
                            }
                            let v = self.expr(&args[0])?;
                            let ty = value_type(&v, args[0].span)?;
                            expect_type(ty, Type::Number, "argument of 'Math.abs'", args[0].span)?;
                            return Ok(hir::Expr::new(hir::ExprKind::MathAbs(Box::new(v)), Some(Type::Number)));
                        }
                        other => {
                            return Err(Diagnostic::new(format!("Math.{other} isn't supported yet"), span));
                        }
                    }
                }
                let obj = self.expr(object)?;
                let ty = value_type(&obj, object.span)?;
                if ty == Type::String {
                    match property.name.as_str() {
                        "toUpperCase" => {
                            if !args.is_empty() {
                                return Err(Diagnostic::new("toUpperCase takes 0 arguments", span));
                            }
                            return Ok(hir::Expr::new(hir::ExprKind::StrUpper(Box::new(obj)), Some(Type::String)));
                        }
                        "includes" => {
                            if args.len() != 1 {
                                return Err(Diagnostic::new("includes takes 1 argument", span));
                            }
                            let needle = self.expr(&args[0])?;
                            let n_ty = value_type(&needle, args[0].span)?;
                            expect_type(n_ty, Type::String, "argument of 'includes'", args[0].span)?;
                            return Ok(hir::Expr::new(hir::ExprKind::StrIncludes(Box::new(obj), Box::new(needle)), Some(Type::Bool)));
                        }
                        "slice" => {
                            if args.is_empty() || args.len() > 2 {
                                return Err(Diagnostic::new("slice takes 1 or 2 arguments", span));
                            }
                            let start = self.expr(&args[0])?;
                            let s_ty = value_type(&start, args[0].span)?;
                            expect_type(s_ty, Type::Number, "start argument of 'slice'", args[0].span)?;
                            let end = if args.len() == 2 {
                                let e = self.expr(&args[1])?;
                                let e_ty = value_type(&e, args[1].span)?;
                                expect_type(e_ty, Type::Number, "end argument of 'slice'", args[1].span)?;
                                e
                            } else {
                                hir::Expr::new(hir::ExprKind::Number(f64::MAX), Some(Type::Number))
                            };
                            return Ok(hir::Expr::new(hir::ExprKind::StrSlice(Box::new(obj), Box::new(start), Box::new(end)), Some(Type::String)));
                        }
                        "replace" => {
                            if args.len() != 2 {
                                return Err(Diagnostic::new("replace takes 2 arguments", span));
                            }
                            let from = self.expr(&args[0])?;
                            let f_ty = value_type(&from, args[0].span)?;
                            expect_type(f_ty, Type::String, "pattern argument of 'replace'", args[0].span)?;
                            let to = self.expr(&args[1])?;
                            let t_ty = value_type(&to, args[1].span)?;
                            expect_type(t_ty, Type::String, "replacement argument of 'replace'", args[1].span)?;
                            return Ok(hir::Expr::new(hir::ExprKind::StrReplace(Box::new(obj), Box::new(from), Box::new(to)), Some(Type::String)));
                        }
                        other => {
                            return Err(Diagnostic::new(format!("string method '{other}' isn't supported yet"), span));
                        }
                    }
                }
                if ty == Type::Arr && property.name == "push" {
                    if args.len() != 1 {
                        return Err(Diagnostic::new("push takes 1 argument", span));
                    }
                    let val = self.expr(&args[0])?;
                    let v_ty = value_type(&val, args[0].span)?;
                    expect_type(v_ty, Type::Number, "argument of 'push'", args[0].span)?;
                    return Ok(hir::Expr::new(hir::ExprKind::ArrayPush(Box::new(obj), Box::new(val)), None));
                }
                Err(Diagnostic::new("only 'console.log', string methods, and array methods are supported as method calls", span))
            }
            _ => Err(Diagnostic::new(
                "only direct calls to named functions are supported",
                callee.span,
            )),
        }
    }
}
