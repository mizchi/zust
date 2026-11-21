//! zustscript's syntax tree. Every expression carries an id: the checker records what it resolved (callee kind,
//! types) in side tables keyed by it, and the JS emitter reads those tables. (A small stand-in for the typed IR the
//! design calls for: zust's acg has no such layer, it type-checks and records low-level instructions in one pass.)

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

pub type Id = u32;

/// A type as written
#[derive(Clone, Debug)]
pub enum TyS {
    /// `Int`, `Array<T>`, `Todo`, a generic parameter
    Name(String, Vec<TyS>, Pos),
    Tuple(Vec<TyS>),
    Fn(Vec<TyS>, Box<TyS>),
    Unit,
}

#[derive(Clone, Debug)]
pub enum Lit {
    Int(i64),
    /// (the source text)
    Float(String),
    Str(String),
    Bool(bool),
    Unit,
}

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Bind(String),
    Lit(Lit),
    Tuple(Vec<Pat>),
    /// (enum named by a path `Shape::Circle`, variant, sub-patterns)
    Variant(Option<String>, String, Vec<Pat>, Pos),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: Id,
    pub pos: Pos,
    pub kind: EK,
}

#[derive(Clone, Debug)]
pub enum EK {
    Lit(Lit),
    Var(String),
    /// `Shape::Circle`
    Path(String, String),
    Call(Box<Expr>, Vec<Expr>),
    Method(Box<Expr>, String, Vec<Expr>),
    Field(Box<Expr>, String),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Un(&'static str, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Match(Box<Expr>, Vec<(Pat, Expr)>),
    Block(Vec<Stmt>, Option<Box<Expr>>),
    Closure(Vec<(String, Option<TyS>)>, Box<Expr>),
    /// `Todo { done: true, ..t }`
    Record(String, Vec<(String, Expr)>, Option<Box<Expr>>),
    Array(Vec<Expr>),
    Tuple(Vec<Expr>),
    Jsx(Box<Jsx>),
    /// `x = e` (`x += e` is `x = x + e`): only a local declared `let mut`
    Assign(String, Box<Expr>),
    /// `while c { .. }`
    While(Box<Expr>, Box<Expr>),
    /// `for x in xs { .. }` / `for i in range(a, b) { .. }`
    For(String, Box<Expr>, Box<Expr>),
    Break,
    Continue,
    /// `xs[i]` (i32 index; out of range is an error, in JS as in wasm)
    Index(Box<Expr>, Box<Expr>),
    /// `x as f64` (numeric conversions only: there is no implicit one)
    Cast(Box<Expr>, TyS),
    /// `perform Log("hi")`: an operation of an effect (an enum whose variants are `Op(Args) -> Result`)
    Perform(Option<String>, String, Vec<Expr>),
    /// `handle body { Log(s), k => .. }`: a deep handler; `k` resumes the body once
    Handle(Box<Expr>, Vec<HArm>),
}

#[derive(Clone, Debug)]
pub struct HArm {
    pub variant: String,
    pub pats: Vec<Pat>,
    /// the continuation's name
    pub k: String,
    pub body: Expr,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct Jsx {
    /// "" for a fragment `<>..</>`
    pub tag: String,
    pub attrs: Vec<(String, Expr)>,
    pub children: Vec<JsxChild>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum JsxChild {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let(Pat, Option<TyS>, Expr),
    /// `let mut x = e;`: a local that can be assigned. Mutation stays local: records and arrays are immutable,
    /// and a closure cannot capture a mutable local, so a function stays pure from the outside
    LetMut(String, Option<TyS>, Expr),
    Expr(Expr),
}

/// A type that can cross the JS / wasm boundary (the signature of a `wasm fn`, in Rust's spelling)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WTy {
    I32,
    U32,
    I64,
    F64,
    Bool,
    Unit,
    /// `&[f64]` / `&[i32]` / `&[u32]` / `&[i64]` (argument: copied into linear memory for the call)
    Slice(Elem),
    /// `&str` (argument: UTF-16 → UTF-8 into linear memory for the call)
    Str,
    /// `String` / `Vec<f64>` / ... (result: copied out, then freed by the glue)
    String,
    Vec(Elem),
    /// A record or an enum of a `@wasm fn`'s signature: crosses as its scalar slots (no linear memory for an
    /// argument; a result is written to the glue's out area, 8 bytes a slot)
    Value(Box<VTy>),
    /// A JS function handed to wasm. `&dyn Fn(A) -> R` (false): borrowed for the call, the glue releases it when the
    /// call returns. `Box<dyn Fn(A) -> R>` (true): owned by the wasm side, released when Rust drops it.
    /// Arguments and result are scalars (i32 u32 f64 bool ()).
    Callback(Vec<WTy>, Box<WTy>, bool),
}

/// The shape of a value that crosses the boundary as slots
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VTy {
    /// i32 u32 i64 f64 bool
    Scalar(WTy),
    /// (Rust type, fields)
    Record(String, Vec<(String, VTy)>),
    /// (Rust type, emitted as JS strings, variants with their fields): one i32 slot for the tag, then the slots of
    /// every variant's fields
    Enum(String, bool, Vec<(String, Vec<VTy>)>),
}

impl VTy {
    pub fn slots(&self) -> usize {
        match self {
            VTy::Scalar(_) => 1,
            VTy::Record(_, fs) => fs.iter().map(|f| f.1.slots()).sum(),
            VTy::Enum(_, _, vs) => 1 + vs.iter().flat_map(|v| v.1.iter()).map(|f| f.slots()).sum::<usize>(),
        }
    }
}

/// An element type of an array that crosses the boundary
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem {
    I32,
    U32,
    I64,
    F64,
}

impl Elem {
    pub fn parse(s: &str) -> Option<Elem> {
        Some(match s {
            "i32" => Elem::I32,
            "u32" => Elem::U32,
            "i64" => Elem::I64,
            "f64" => Elem::F64,
            _ => return None,
        })
    }
    /// (Rust type, JS typed array, size in bytes)
    pub fn info(self) -> (&'static str, &'static str, usize) {
        match self {
            Elem::I32 => ("i32", "Int32Array", 4),
            Elem::U32 => ("u32", "Uint32Array", 4),
            Elem::I64 => ("i64", "BigInt64Array", 8),
            Elem::F64 => ("f64", "Float64Array", 8),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    Record { name: String, params: Vec<String>, fields: Vec<(String, TyS)>, pos: Pos },
    /// (a variant with `-> R` is an effect operation)
    Enum { name: String, params: Vec<String>, variants: Vec<(String, Vec<TyS>, Option<TyS>)>, pos: Pos },
    /// (bounds: `T: Num` / `T: Integer` / `T: Float`; a function with one is monomorphized per numeric type)
    /// (`wasm`: written `@wasm fn`: lowered to Rust and compiled to wasm, see lower.rs)
    /// (`simd`: written `@simd fn`: an array kernel vectorized with v128, see simd.rs)
    Fn { name: String, public: bool, wasm: bool, simd: bool, generics: Vec<String>, bounds: Vec<(String, String)>, args: Vec<(String, TyS)>, ret: Option<TyS>, body: Expr, pos: Pos },
    /// `extern "react" fn useState<T>(init: T) -> (T, fn(T) -> ()) = "useState";`
    Extern { name: String, module: Option<String>, generics: Vec<String>, args: Vec<TyS>, ret: TyS, js: String, pos: Pos },
    /// `wasm fn` (the text is the whole Rust function, handed to zustc)
    WasmFn { name: String, public: bool, args: Vec<(String, WTy)>, ret: WTy, text: String, pos: Pos },
    /// `wasm { .. }`: Rust items for the wasm side only (helpers, structs)
    WasmRaw(String),
}
