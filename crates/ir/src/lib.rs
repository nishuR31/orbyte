//! Architecture-independent intermediate representation.
//!
//! Design notes for milestone 1:
//!
//! * Functions are control-flow graphs of basic blocks; block 0 is the entry.
//! * Every [`Value`] is assigned exactly once and is **block-local**: it is
//!   defined and consumed inside one block. State that crosses blocks lives in
//!   numbered *locals* (`GetLocal`/`SetLocal`). Backends turn locals into SSA
//!   (Cranelift's `Variable` does this), and a later pass can promote them to
//!   explicit phi nodes inside this IR.
//! * Locals `0..params.len()` are initialised from the function parameters on entry.
//! * Types are static: `F64` and `Bool`. Dynamic values arrive with the runtime.

use std::fmt;

mod verify;
pub use verify::verify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    F64,
    Bool,
    Str,
    Arr,
    Obj,
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Block(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncRef(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    NegF64,
    NotBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    AddF64,
    SubF64,
    MulF64,
    DivF64,
    /// IEEE remainder with the sign of the dividend, as ECMAScript `%` requires.
    RemF64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    ConstF64 { dst: Value, value: f64 },
    ConstBool { dst: Value, value: bool },
    ConstStr { dst: Value, value: String },
    ConstNull { dst: Value },
    ConcatStr { dst: Value, lhs: Value, rhs: Value },
    NumToStr { dst: Value, src: Value },
    BoolToStr { dst: Value, src: Value },
    StrToNum { dst: Value, src: Value },
    BoolToNum { dst: Value, src: Value },
    StrLen { dst: Value, src: Value },
    StrUpper { dst: Value, src: Value },
    StrIncludes { dst: Value, src: Value, needle: Value },
    StrSlice { dst: Value, src: Value, start: Value, end: Value },
    StrReplace { dst: Value, src: Value, from: Value, to: Value },
    StrSplitCount { dst: Value, src: Value, sep: Value },
    MathSqrt { dst: Value, src: Value },
    MathAbs { dst: Value, src: Value },
    AllocArray { dst: Value, capacity: usize },
    ArrayPush { arr: Value, val: Value },
    ArrayGet { dst: Value, arr: Value, index: Value },
    ArraySet { arr: Value, index: Value, val: Value },
    ArrayLen { dst: Value, arr: Value },
    AllocObj { dst: Value },
    ObjSetNum { obj: Value, key: Value, val: Value },
    ObjGetNum { dst: Value, obj: Value, key: Value },
    ObjSetStr { obj: Value, key: Value, val: Value },
    ObjGetStr { dst: Value, obj: Value, key: Value },
    ObjSetObj { obj: Value, key: Value, val: Value },
    ObjGetObj { dst: Value, obj: Value, key: Value },
    Unary { dst: Value, op: UnOp, src: Value },
    Binary { dst: Value, op: BinOp, lhs: Value, rhs: Value },
    /// Compares two operands of type `ty`; the result is `Bool`.
    Compare { dst: Value, op: CmpOp, ty: Type, lhs: Value, rhs: Value },
    GetLocal { dst: Value, local: Local },
    SetLocal { local: Local, src: Value },
    Call { dst: Option<Value>, callee: FuncRef, args: Vec<Value> },
    /// `console.log` for a value of the given type.
    Print { ty: Type, value: Value },
    /// Short-circuiting `??`: dst = lhs if lhs is not null, else rhs.
    NullishCoalesce { dst: Value, lhs: Value, rhs: Value },
    /// Optional chaining `obj?.prop`: dst = obj.prop if obj is not null, else null.
    OptionalChain { dst: Value, obj: Value, key: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Jump(Block),
    Branch { cond: Value, then_block: Block, else_block: Block },
    Return(Option<Value>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockData {
    pub insts: Vec<Inst>,
    pub terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Option<Type>,
    /// Types of all locals; the first `params.len()` are the parameters.
    pub locals: Vec<Type>,
    pub blocks: Vec<BlockData>,
    /// Type of every value, indexed by `Value.0`.
    pub value_types: Vec<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
    /// The function executed when the program starts (no parameters, no result).
    pub entry: FuncRef,
}

// ---------- Display (used by `engine inspect`) ----------

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::F64 => write!(f, "f64"),
            Type::Bool => write!(f, "bool"),
            Type::Str => write!(f, "str"),
            Type::Arr => write!(f, "arr"),
            Type::Obj => write!(f, "obj"),
            Type::Null => write!(f, "null"),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

impl fmt::Display for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "l{}", self.0)
    }
}

impl fmt::Display for FuncRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "f{}", self.0)
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Inst::ConstF64 { dst, value } => write!(f, "{dst} = const f64 {value}"),
            Inst::ConstBool { dst, value } => write!(f, "{dst} = const bool {value}"),
            Inst::ConstStr { dst, value } => write!(f, "{dst} = const str {value:?}"),
            Inst::ConstNull { dst } => write!(f, "{dst} = const null"),
            Inst::ConcatStr { dst, lhs, rhs } => write!(f, "{dst} = concat {lhs}, {rhs}"),
            Inst::NumToStr { dst, src } => write!(f, "{dst} = num_to_str {src}"),
            Inst::BoolToStr { dst, src } => write!(f, "{dst} = bool_to_str {src}"),
            Inst::StrToNum { dst, src } => write!(f, "{dst} = str_to_num {src}"),
            Inst::BoolToNum { dst, src } => write!(f, "{dst} = bool_to_num {src}"),
            Inst::StrLen { dst, src } => write!(f, "{dst} = str_len {src}"),
            Inst::StrUpper { dst, src } => write!(f, "{dst} = str_upper {src}"),
            Inst::StrIncludes { dst, src, needle } => write!(f, "{dst} = str_includes {src}, {needle}"),
            Inst::StrSlice { dst, src, start, end } => write!(f, "{dst} = str_slice {src}, {start}, {end}"),
            Inst::StrReplace { dst, src, from, to } => write!(f, "{dst} = str_replace {src}, {from}, {to}"),
            Inst::StrSplitCount { dst, src, sep } => write!(f, "{dst} = str_split_count {src}, {sep}"),
            Inst::MathSqrt { dst, src } => write!(f, "{dst} = math.sqrt {src}"),
            Inst::MathAbs { dst, src } => write!(f, "{dst} = math.abs {src}"),
            Inst::AllocArray { dst, capacity } => write!(f, "{dst} = alloc_array {capacity}"),
            Inst::ArrayPush { arr, val } => write!(f, "array_push {arr}, {val}"),
            Inst::ArrayGet { dst, arr, index } => write!(f, "{dst} = array_get {arr}, {index}"),
            Inst::ArraySet { arr, index, val } => write!(f, "array_set {arr}, {index}, {val}"),
            Inst::ArrayLen { dst, arr } => write!(f, "{dst} = array_len {arr}"),
            Inst::AllocObj { dst } => write!(f, "{dst} = alloc_obj"),
            Inst::ObjSetNum { obj, key, val } => write!(f, "obj_set_num {obj}, {key}, {val}"),
            Inst::ObjGetNum { dst, obj, key } => write!(f, "{dst} = obj_get_num {obj}, {key}"),
            Inst::ObjSetStr { obj, key, val } => write!(f, "obj_set_str {obj}, {key}, {val}"),
            Inst::ObjGetStr { dst, obj, key } => write!(f, "{dst} = obj_get_str {obj}, {key}"),
            Inst::ObjSetObj { obj, key, val } => write!(f, "obj_set_obj {obj}, {key}, {val}"),
            Inst::ObjGetObj { dst, obj, key } => write!(f, "{dst} = obj_get_obj {obj}, {key}"),
            Inst::Unary { dst, op, src } => write!(f, "{dst} = {op:?} {src}"),
            Inst::Binary { dst, op, lhs, rhs } => write!(f, "{dst} = {op:?} {lhs}, {rhs}"),
            Inst::Compare { dst, op, ty, lhs, rhs } => {
                write!(f, "{dst} = cmp.{ty} {op:?} {lhs}, {rhs}")
            }
            Inst::GetLocal { dst, local } => write!(f, "{dst} = get {local}"),
            Inst::SetLocal { local, src } => write!(f, "set {local}, {src}"),
            Inst::Call { dst, callee, args } => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                match dst {
                    Some(d) => write!(f, "{d} = call {callee}({})", args.join(", ")),
                    None => write!(f, "call {callee}({})", args.join(", ")),
                }
            }
            Inst::Print { ty, value } => write!(f, "print.{ty} {value}"),
            Inst::NullishCoalesce { dst, lhs, rhs } => write!(f, "{dst} = nullish_coalesce {lhs}, {rhs}"),
            Inst::OptionalChain { dst, obj, key } => write!(f, "{dst} = optional_chain {obj}, {key}"),
        }
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminator::Jump(b) => write!(f, "jump {b}"),
            Terminator::Branch { cond, then_block, else_block } => {
                write!(f, "branch {cond}, {then_block}, {else_block}")
            }
            Terminator::Return(Some(v)) => write!(f, "return {v}"),
            Terminator::Return(None) => write!(f, "return"),
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params: Vec<String> = self
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("l{i}: {t}"))
            .collect();
        let ret = self.ret.map_or("void".to_string(), |t| t.to_string());
        writeln!(f, "func {}({}) -> {ret} {{", self.name, params.join(", "))?;
        for (i, block) in self.blocks.iter().enumerate() {
            writeln!(f, "  bb{i}:")?;
            for inst in &block.insts {
                writeln!(f, "    {inst}")?;
            }
            match &block.terminator {
                Some(t) => writeln!(f, "    {t}")?,
                None => writeln!(f, "    <unterminated>")?,
            }
        }
        writeln!(f, "}}")
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, func) in self.functions.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{func}")?;
        }
        writeln!(f, "\nentry: {}", self.entry)
    }
}
