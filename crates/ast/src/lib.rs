//! Syntax tree for the JavaScript subset supported by milestone 1.

use diagnostics::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Let,
    Const,
    Var,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Function(FunctionDecl),
    Var { kind: VarKind, name: Ident, init: Option<Expr> },
    Expr(Expr),
    If { cond: Expr, then: Box<Stmt>, otherwise: Option<Box<Stmt>> },
    While { cond: Expr, body: Box<Stmt> },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    Break,
    Continue,
    Block(Vec<Stmt>),
    Return(Option<Expr>),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: Ident,
    pub params: Vec<Ident>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    TypeOf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    And,
    Or,
    NullishCoalescing,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::StrictEq => "===",
            BinaryOp::StrictNotEq => "!==",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::NullishCoalescing => "??",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Number(f64),
    Str(String),
    Bool(bool),
    Ident(String),
    Null,
    Undefined,
    Unary { op: UnaryOp, operand: Box<Expr> },
    Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Assign { target: Ident, value: Box<Expr> },
    /// `++x`, `x++`, `--x`, `x--`.
    Update { target: Ident, increment: bool, prefix: bool },
    Call { callee: Box<Expr>, args: Vec<Expr> },
    Member { object: Box<Expr>, property: Ident },
    Array(Vec<Expr>),
    Index { object: Box<Expr>, index: Box<Expr> },
    Object(Vec<(Ident, Expr)>),
    MemberAssign { object: Box<Expr>, property: Ident, value: Box<Expr> },
    IndexAssign { object: Box<Expr>, index: Box<Expr>, value: Box<Expr> },
    /// Optional chaining `obj?.prop`.
    OptionalChain { object: Box<Expr>, property: Ident },
}
