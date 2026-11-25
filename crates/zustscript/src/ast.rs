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
    /// `loop { .. }`: until a `break` (without one, it gives no value: it has any type)
    Loop(Box<Expr>),
    Break,
    Continue,
    /// `return e` / `return`: from the innermost function or closure (its result; `()` without a value)
    Return(Option<Box<Expr>>),
    /// `xs[i]` (i32 index; out of range is an error, in JS as in wasm)
    Index(Box<Expr>, Box<Expr>),
    /// `x as f64` (numeric conversions only: there is no implicit one)
    Cast(Box<Expr>, TyS),
    /// `perform Log("hi")`: an operation of an effect (an enum whose variants are `Op(Args) -> Result`)
    Perform(Option<String>, String, Vec<Expr>),
    /// `handle body { Log(s), k => .. }`: a deep handler; `k` resumes the body once
    Handle(Box<Expr>, Vec<HArm>),
    /// `` `hi ${name}, ${n}` ``: the literal parts (one more than the holes) and the holes (strings, numbers, bools)
    Interp(Vec<String>, Vec<Expr>),
    /// `await e`: the value of the `Promise<T>` `e` (in an `async fn` or an `async` closure)
    Await(Box<Expr>),
    /// `async (x) => e`: the closure inside, which gives a `Promise` of its body's value
    Async(Box<Expr>),
    /// `e?`: the value inside the `Result` / `Option` `e`; an `Err` / `None` is returned from the innermost function
    /// or closure as it is
    Try(Box<Expr>),
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
    F32,
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
    /// a result for a typed-array type (`F64Array` ..): copied out with `.slice()`
    TVec(Elem),
    /// A record or an enum of a `@wasm fn`'s signature: crosses as its scalar slots (no linear memory for an
    /// argument; a result is written to the glue's out area, 8 bytes a slot)
    Value(Box<VTy>),
    /// A JS function handed to wasm. `&dyn Fn(A) -> R` (false): borrowed for the call, the glue releases it when the
    /// call returns. `Box<dyn Fn(A) -> R>` (true): owned by the wasm side, released when Rust drops it.
    /// Arguments and result are scalars (i32 u32 f64 bool ()).
    Callback(Vec<WTy>, Box<WTy>, bool),
    /// A value that crosses as bytes in the format of zustscript-helper / zustscript_helper (a record holding a
    /// string or an array, an array of records, strings or arrays, an enum that does not fit the slots), its owned
    /// Rust type, and whether the Rust side takes it by reference (not Copy)
    Wire(Box<WS>, String, bool),
}

/// The schema of a value that crosses as bytes (helper/rust/src/lib.rs has the format)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WS {
    /// i32 u32 i64 f32 f64 bool
    Scalar(WTy),
    Str,
    Arr(Box<WS>),
    /// (Rust type, fields)
    Rec(String, Vec<(String, WS)>),
    /// An enum: its variant's index as a u32, then that variant's fields. `key`: the JS property holding the tag
    /// (`TAG`, or a TS union's `kind`); None: the JS value is the tag string itself. Each variant: its Rust name, its
    /// JS tag, and its fields by JS name
    Enum { rt: String, key: Option<String>, variants: Vec<(String, String, Vec<(String, WS)>)> },
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
    F32,
    F64,
}

impl Elem {
    pub fn parse(s: &str) -> Option<Elem> {
        Some(match s {
            "i32" => Elem::I32,
            "u32" => Elem::U32,
            "i64" => Elem::I64,
            "f32" => Elem::F32,
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
            Elem::F32 => ("f32", "Float32Array", 4),
            Elem::F64 => ("f64", "Float64Array", 8),
        }
    }
}

/// A property or a method of an `extern type`, and its name in JS
#[derive(Clone, Debug)]
pub enum Member {
    /// `status: i32` / `text: String = "textContent"`: read with `x.status`
    Prop { name: String, ty: TyS, js: String, pos: Pos },
    /// `fn json() -> Promise<Json>;` / `fn set(k: String, v: String) = "setItem";`: called with `x.json()`. A JS name
    /// that ends with `=` (`"textContent="`) assigns that property to the one argument
    Method { name: String, args: Vec<TyS>, ret: TyS, js: String, pos: Pos },
}

/// Where a record read from TS comes from
#[derive(Clone, Debug)]
pub struct TsFrom {
    /// the TS file (canonical)
    pub path: std::path::PathBuf,
    pub exported: bool,
    /// each member as the TS file writes it (`readonly tags: string[]`): a type the file does not export is declared
    /// again in the `.d.ts` with these, so it is the same type to TS
    pub members: Vec<String>,
    /// an enum's union as the file writes it (`"draft" | "published"`), for the same
    pub text: String,
    /// an enum's values in JS: TS's, not zustscript's `{ TAG, _0 }`
    pub repr: Option<TsEnum>,
    /// a type of another zustscript module: that module's file (canonical). `path` is then the specifier of its JS
    /// from the module that uses the type (`./lib/leaf.js`), written again at each import
    pub zsc: Option<std::path::PathBuf>,
}

/// How the values of an enum read from a TS union look in JS (they are TS's own)
#[derive(Clone, Debug, PartialEq)]
pub struct TsEnum {
    /// the tag field (`kind`); None: a union of string literals, each value its tag
    pub key: Option<String>,
    /// each variant's tag (`"circle"`)
    pub tags: Vec<String>,
    /// each variant's fields by name, in order (`r` for `Circle(f64)`)
    pub fields: Vec<Vec<String>>,
}

#[derive(Clone, Debug)]
pub enum Item {
    /// (`from`: read from a TS file by an `import type`, see tsimport.rs)
    Record { name: String, params: Vec<String>, fields: Vec<(String, TyS)>, pos: Pos, from: Option<TsFrom> },
    /// `import type { User, Post } from "./model.ts";` (replaced by the records before checking, see tsimport.rs)
    /// `import { greet, Shape } from "./lib.zsc";` (`values`: functions too; replaced by the types and `extern`s of
    /// the other module before checking, see zscimport.rs)
    ImportType { names: Vec<(String, Pos)>, from: String, values: bool, pos: Pos },
    /// (a variant with `-> R` is an effect operation)
    Enum { name: String, params: Vec<String>, variants: Vec<(String, Vec<TyS>, Option<TyS>)>, pos: Pos, from: Option<TsFrom> },
    /// (bounds: `T: Num` / `T: Integer` / `T: Float`; a function with one is monomorphized per numeric type)
    /// (`wasm`: written `@wasm fn`: lowered to Rust and compiled to wasm, see lower.rs)
    /// (`simd`: written `@simd fn`: an array kernel vectorized with v128, see simd.rs)
    /// (`relaxed`: `@simd(relaxed)`: relaxed SIMD allowed: fused multiply-add, relaxed min / max)
    /// (`is_async`: written `async fn`: its callers get a `Promise<T>` of its result `T`, and its body may `await`)
    Fn { name: String, public: bool, wasm: bool, simd: bool, relaxed: bool, is_async: bool, generics: Vec<String>, bounds: Vec<(String, String)>, args: Vec<(String, TyS)>, ret: Option<TyS>, body: Expr, pos: Pos },
    /// `extern "react" fn useState<T>(init: T) -> (T, fn(T) -> ()) = "useState";`
    /// (`zsc`: an `export fn` of another zustscript module (zscimport.rs): its values are zustscript's, not converted
    /// as JS's are)
    Extern { name: String, module: Option<String>, generics: Vec<String>, args: Vec<TyS>, ret: TyS, js: String, pos: Pos, zsc: bool },
    /// `extern type Response;` / `extern "react" type RefObject<T>;` / `extern type El = "HTMLElement" { .. }`: a JS
    /// value zustscript does not look inside, with the properties and methods declared for it. `ts`: its name in
    /// TS (and in JS, for a module's: the name imported)
    /// (`origin`: the .zsc file that declares it, when it came from another module: one declaration reached through
    /// two imports is the same type)
    ExternType { name: String, module: Option<String>, params: Vec<String>, ts: String, members: Vec<Member>, pos: Pos, origin: Option<std::path::PathBuf> },
    /// `wasm fn` (the text is the whole Rust function, handed to zustc)
    WasmFn { name: String, public: bool, args: Vec<(String, WTy)>, ret: WTy, text: String, pos: Pos },
    /// `wasm { .. }`: Rust items for the wasm side only (helpers, structs)
    WasmRaw(String),
}

/// The prelude's types, which a file uses without declaring them: `enum Option<T> { None, Some(T) }` and
/// `enum Result<T, E> { Ok(T), Err(E) }` (ordinary enums otherwise: the same JS, `.d.ts` and Rust as if declared)
/// (name, type parameters, variants with their fields' types)
pub type PreludeType = (&'static str, &'static [&'static str], &'static [(&'static str, &'static [&'static str])]);
pub const PRELUDE: &[PreludeType] = &[("Option", &["T"], &[("None", &[]), ("Some", &["T"])]), ("Result", &["T", "E"], &[("Ok", &["T"]), ("Err", &["E"])])];

/// A prelude type is at line 0 (no source line is)
pub fn is_prelude(pos: Pos) -> bool {
    pos.line == 0
}

/// Does `variants` declare the prelude type `name` as the prelude does (a file written before the prelude)
pub fn prelude_shaped(name: &str, params: &[String], variants: &[(String, Vec<TyS>, Option<TyS>)]) -> bool {
    let Some((_, ps, vs)) = PRELUDE.iter().find(|p| p.0 == name) else { return false };
    params.len() == ps.len()
        && variants.len() == vs.len()
        && variants.iter().zip(vs.iter()).all(|((v, ts, r), (pv, pts))| {
            v == pv
                && r.is_none()
                && ts.len() == pts.len()
                && ts.iter().zip(pts.iter()).all(|(t, pt)| matches!(t, TyS::Name(n, a, _) if a.is_empty() && Some(n) == ps.iter().position(|p| p == pt).map(|k| &params[k])))
        })
}

/// The items with the prelude's types in front, each unless the file declares a type of its name, or a variant of
/// another enum takes one of its variants' names (variant names are unique in a file: that enum keeps its own)
pub fn with_prelude(items: Vec<Item>) -> Vec<Item> {
    let p0 = Pos { line: 0, col: 0 };
    let mut out = vec![];
    for (name, ps, vs) in PRELUDE {
        let taken = items.iter().any(|it| match it {
            Item::Record { name: n, .. } => n == name,
            Item::Enum { name: n, variants, .. } => n == name || variants.iter().any(|v| vs.iter().any(|pv| pv.0 == v.0)),
            _ => false,
        });
        if taken {
            continue;
        }
        let params: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
        let variants = vs.iter().map(|(v, ts)| (v.to_string(), ts.iter().map(|t| TyS::Name(t.to_string(), vec![], p0)).collect(), None)).collect();
        out.push(Item::Enum { name: name.to_string(), params, variants, pos: p0, from: None });
    }
    out.extend(items);
    out
}
