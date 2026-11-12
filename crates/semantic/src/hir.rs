//! Resolved, typed program representation produced by semantic analysis.
//!
//! Names are resolved to local slots and function ids, and every expression
//! carries its static type (`None` means the expression produces no value).

use std::fmt;

use ast::{BinaryOp, UnaryOp};

pub type FuncId = u32;
pub type LocalId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Number,
    Bool,
    String,
    Arr,
    Obj,
    Null,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Number => write!(f, "number"),
            Type::Bool => write!(f, "boolean"),
            Type::String => write!(f, "string"),
            Type::Arr => write!(f, "array"),
            Type::Obj => write!(f, "object"),
            Type::Null => write!(f, "null"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Local {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct Module {
    /// Declared functions in source order, followed by the synthetic main function.
    pub functions: Vec<Function>,
    pub main: FuncId,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub param_count: usize,
    /// Parameters come first, then every `let`/`const`/`var` in the function.
    pub locals: Vec<Local>,
    pub ret: Option<Type>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    SetLocal(LocalId, Expr),
    Expr(Expr),
    If { cond: Expr, then: Vec<Stmt>, otherwise: Vec<Stmt> },
    /// `update` runs after the body and after every `continue` (it is how
    /// `for` loops are represented); it is empty for plain `while` loops.
    While { cond: Expr, body: Vec<Stmt>, update: Vec<Stmt> },
    Break,
    Continue,
    Return(Option<Expr>),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Option<Type>,
}

impl Expr {
    pub fn new(kind: ExprKind, ty: Option<Type>) -> Self {
        Expr { kind, ty }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Number(f64),
    Str(String),
    Bool(bool),
    Local(LocalId),
    Null,
    Undefined,
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// Short-circuiting `&&` and `||` over booleans.
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    /// Short-circuiting `??`: returns `lhs` if not null/undefined, else `rhs`.
    NullishCoalescing(Box<Expr>, Box<Expr>),
    /// Optional chaining `obj?.prop`: returns `undefined` if obj is null/undefined.
    OptionalChain(Box<Expr>, String),
    Assign(LocalId, Box<Expr>),
    Call(FuncId, Vec<Expr>),
    /// `console.log(value)`
    Print(Box<Expr>),
    /// `s.length`
    StrLen(Box<Expr>),
    StrUpper(Box<Expr>),
    StrIncludes(Box<Expr>, Box<Expr>),
    StrSlice(Box<Expr>, Box<Expr>, Box<Expr>),
    StrReplace(Box<Expr>, Box<Expr>, Box<Expr>),
    StrSplitCount(Box<Expr>, Box<Expr>),
    MathSqrt(Box<Expr>),
    MathAbs(Box<Expr>),
    Array(Vec<Expr>),
    ArrayGet(Box<Expr>, Box<Expr>),
    ArraySet(Box<Expr>, Box<Expr>, Box<Expr>),
    ArrayLen(Box<Expr>),
    ArrayPush(Box<Expr>, Box<Expr>),
    Object(Vec<(String, Expr)>),
    ObjGetNum(Box<Expr>, Box<Expr>),
    ObjGetStr(Box<Expr>, Box<Expr>),
    ObjSetNum(Box<Expr>, Box<Expr>, Box<Expr>),
    ObjSetStr(Box<Expr>, Box<Expr>, Box<Expr>),
}
