//! zustc の命令の列 (IR) の型: 型・値・ブロック・変数・スタックスロット・関数の参照・再配置と、記録した命令 (Op)。
//! acg (型の検査とコード生成) は Recorder に命令を記録するだけで、機械語にするのはバックエンド (minclif / Cranelift) が
//! 記録を読み直して行う。check (LSP) はバックエンドなしで記録を捨てる。外部クレートに依存しない (zustc 自身でもコンパイルできる)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Type {
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// 128-bit SIMD vector. Its lanes are not part of the type: each [`VOp`] says which [`Lanes`] it reads it as
    V128,
}

impl Type {
    pub fn bytes(self) -> u32 {
        match self {
            Type::I8 => 1,
            Type::I16 => 2,
            Type::I32 => 4,
            Type::I64 | Type::F64 => 8,
            Type::I128 | Type::V128 => 16,
            Type::F32 => 4,
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }
    pub fn bits(self) -> u32 {
        self.bytes() * 8
    }
}

/// How a [`Type::V128`] is read by a SIMD instruction: lane type and count
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lanes {
    I8x16,
    I16x8,
    I32x4,
    I64x2,
    F32x4,
    F64x2,
}

impl Lanes {
    /// the scalar type of one lane (what splat takes and extract_lane gives)
    pub fn lane(self) -> Type {
        match self {
            Lanes::I8x16 => Type::I8,
            Lanes::I16x8 => Type::I16,
            Lanes::I32x4 => Type::I32,
            Lanes::I64x2 => Type::I64,
            Lanes::F32x4 => Type::F32,
            Lanes::F64x2 => Type::F64,
        }
    }
    pub fn count(self) -> u32 {
        16 / self.lane().bytes()
    }
    pub fn is_float(self) -> bool {
        matches!(self, Lanes::F32x4 | Lanes::F64x2)
    }
}

/// A SIMD instruction (issue #30). The operands are V128 unless noted; the result is V128 unless [`VOp::result`] says
/// otherwise. The set is what wasm SIMD and SSE4.1 / NEON have in common, and the relaxed instructions separately (their
/// results may differ between engines and CPUs: a program opts into them by name)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VOp {
    // bitwise, whatever the lanes
    Not,
    And,
    /// a & !b
    AndNot,
    Or,
    Xor,
    /// (a & c) | (b & !c)
    Bitselect,
    /// any bit set (result I8: 0 / 1)
    AnyTrue,
    /// (scalar of the lane type) to every lane
    Splat(Lanes),
    /// one lane (result: the lane type; i8 / i16 lanes as their bits)
    Extract(Lanes, u8),
    /// (vector, scalar of the lane type)
    Replace(Lanes, u8),
    /// bytes of (a, b) picked by 16 indices < 32 (byte i of the u128 is the index of result byte i)
    Shuffle(u128),
    /// bytes of a picked by the bytes of b (an index >= 16 gives 0)
    Swizzle,
    Add(Lanes),
    Sub(Lanes),
    /// (not for i8x16)
    Mul(Lanes),
    /// (floats)
    Div(Lanes),
    Neg(Lanes),
    Abs(Lanes),
    /// (floats)
    Sqrt(Lanes),
    Ceil(Lanes),
    Floor(Lanes),
    Trunc(Lanes),
    Nearest(Lanes),
    /// (lanes, unsigned): integers; floats propagate NaN, -0 < +0
    Min(Lanes, bool),
    Max(Lanes, bool),
    /// pseudo-min / max of floats: b < a ? b : a / a < b ? b : a
    Pmin(Lanes),
    Pmax(Lanes),
    /// saturating, i8x16 / i16x8 (lanes, unsigned)
    AddSat(Lanes, bool),
    SubSat(Lanes, bool),
    /// unsigned rounding average, i8x16 / i16x8
    Avgr(Lanes),
    /// lanewise compare to a mask of all ones / zeros
    Icmp(Lanes, IntCC),
    Fcmp(Lanes, FloatCC),
    /// (vector, I32 amount mod the lane width)
    Shl(Lanes),
    /// (lanes, unsigned)
    Shr(Lanes, bool),
    /// every lane non-zero (result I8: 0 / 1)
    AllTrue(Lanes),
    /// the top bit of each lane (result I32)
    Bitmask(Lanes),
    /// i8x16
    Popcnt,
    /// (result lanes, unsigned): two vectors of twice as wide lanes narrowed with saturation (I8x16 / I16x8)
    Narrow(Lanes, bool),
    /// (result lanes, high half, unsigned): half the lanes widened (I16x8 / I32x4 / I64x2)
    Extend(Lanes, bool, bool),
    /// (result lanes, high half, unsigned): the widened product of half the lanes
    ExtMul(Lanes, bool, bool),
    /// (result lanes, unsigned): sums of adjacent pairs, widened (I16x8 / I32x4)
    ExtAddPairwise(Lanes, bool),
    /// i32x4 from the products of adjacent i16 pairs, summed
    Dot,
    /// i16x8 q15 rounding saturating multiply
    Q15MulrSat,
    /// (result F32x4 from I32x4 / F64x2 from the low two I32 lanes, unsigned)
    FcvtFromInt(Lanes, bool),
    /// (source F32x4 / F64x2 (into the low lanes, the rest 0), unsigned): to I32x4, saturating
    FcvtToIntSat(Lanes, bool),
    /// f64x2 to the low lanes of f32x4 (the rest 0)
    Demote,
    /// the low two f32 lanes to f64x2
    Promote,
    // relaxed SIMD (results may differ between engines): wasm's relaxed-simd proposal
    /// a * b + c, fused or not (F32x4 / F64x2)
    RelaxedMadd(Lanes),
    /// -(a * b) + c, fused or not
    RelaxedNmadd(Lanes),
    RelaxedMin(Lanes),
    RelaxedMax(Lanes),
    /// (a, b, mask): bitselect, or by the top bit of each lane
    RelaxedLaneselect(Lanes),
    RelaxedSwizzle,
    /// (source F32x4 / F64x2, unsigned)
    RelaxedTrunc(Lanes, bool),
    RelaxedQ15Mulr,
    /// i16x8 from i8x16 x i7x16
    RelaxedDot,
    /// i32x4 from i8x16 x i7x16, plus an i32x4
    RelaxedDotAdd,
}

impl VOp {
    /// the type of the result
    pub fn result(self) -> Type {
        match self {
            VOp::AnyTrue | VOp::AllTrue(_) => Type::I8,
            VOp::Bitmask(_) => Type::I32,
            VOp::Extract(l, _) => l.lane(),
            _ => Type::V128,
        }
    }
    /// how many operands it takes
    pub fn arity(self) -> usize {
        match self {
            VOp::Not
            | VOp::AnyTrue
            | VOp::Splat(_)
            | VOp::Extract(..)
            | VOp::Neg(_)
            | VOp::Abs(_)
            | VOp::Sqrt(_)
            | VOp::Ceil(_)
            | VOp::Floor(_)
            | VOp::Trunc(_)
            | VOp::Nearest(_)
            | VOp::AllTrue(_)
            | VOp::Bitmask(_)
            | VOp::Popcnt
            | VOp::Extend(..)
            | VOp::ExtAddPairwise(..)
            | VOp::FcvtFromInt(..)
            | VOp::FcvtToIntSat(..)
            | VOp::Demote
            | VOp::Promote
            | VOp::RelaxedTrunc(..) => 1,
            VOp::Bitselect | VOp::RelaxedMadd(_) | VOp::RelaxedNmadd(_) | VOp::RelaxedLaneselect(_) | VOp::RelaxedDotAdd => 3,
            _ => 2,
        }
    }
    /// whether it is a relaxed instruction
    pub fn relaxed(self) -> bool {
        matches!(
            self,
            VOp::RelaxedMadd(_)
                | VOp::RelaxedNmadd(_)
                | VOp::RelaxedMin(_)
                | VOp::RelaxedMax(_)
                | VOp::RelaxedLaneselect(_)
                | VOp::RelaxedSwizzle
                | VOp::RelaxedTrunc(..)
                | VOp::RelaxedQ15Mulr
                | VOp::RelaxedDot
                | VOp::RelaxedDotAdd
        )
    }
}

/// 浮動小数点の比較
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatCC {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    /// どちらかが NaN
    Unordered,
}

/// 整数の比較
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntCC {
    Equal,
    NotEqual,
    SignedLessThan,
    SignedLessThanOrEqual,
    SignedGreaterThan,
    SignedGreaterThanOrEqual,
    UnsignedLessThan,
    UnsignedLessThanOrEqual,
    UnsignedGreaterThan,
    UnsignedGreaterThanOrEqual,
}

impl IntCC {
    pub fn signed(self) -> bool {
        matches!(
            self,
            IntCC::SignedLessThan | IntCC::SignedLessThanOrEqual | IntCC::SignedGreaterThan | IntCC::SignedGreaterThanOrEqual
        )
    }
    /// setcc の 2 byte 目
    pub fn setcc(self) -> u8 {
        match self {
            IntCC::Equal => 0x94,
            IntCC::NotEqual => 0x95,
            IntCC::SignedLessThan => 0x9C,
            IntCC::SignedLessThanOrEqual => 0x9E,
            IntCC::SignedGreaterThan => 0x9F,
            IntCC::SignedGreaterThanOrEqual => 0x9D,
            IntCC::UnsignedLessThan => 0x92,
            IntCC::UnsignedLessThanOrEqual => 0x96,
            IntCC::UnsignedGreaterThan => 0x97,
            IntCC::UnsignedGreaterThanOrEqual => 0x93,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Value(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Block(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Variable(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct StackSlot(pub u32);
/// 呼び出す関数 (外部の名前)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct FuncRef(pub u32);

/// コードの中の再配置 (call の rel32 など)
#[derive(Clone, Debug, PartialEq)]
pub struct Reloc {
    pub offset: u32,
    /// 参照する名前 (FuncRef の番号の名前)
    pub name: String,
    /// R_X86_64_PLT32 (4) / R_X86_64_PC32 (2)
    pub kind: u32,
    pub addend: i64,
}

/// FunctionBuilder の命令 (呼ばれた順に記録し、割り当てを決めてから本番の Emitter でもう一度呼ぶ)
#[derive(Clone, Debug)]
pub enum Op {
    CreateBlock,
    AppendBlockParam(Block, Type),
    SwitchToBlock(Block),
    DeclareVar(Type),
    DefVar(Variable, Value),
    UseVar(Variable),
    CreateStackSlot(u32, u32),
    ImportFunction(String),
    Iconst(Type, i64),
    Iadd(Value, Value),
    Isub(Value, Value),
    Imul(Value, Value),
    Band(Value, Value),
    Bor(Value, Value),
    Bxor(Value, Value),
    IaddImm(Value, i64),
    Udiv(Value, Value),
    Sdiv(Value, Value),
    Urem(Value, Value),
    Srem(Value, Value),
    Ishl(Value, Value),
    Ushr(Value, Value),
    Sshr(Value, Value),
    Ineg(Value),
    Bnot(Value),
    Icmp(IntCC, Value, Value),
    IcmpImm(IntCC, Value, i64),
    Select(Value, Value, Value),
    Uextend(Type, Value),
    Sextend(Type, Value),
    Ireduce(Type, Value),
    Load(Type, Value, i32),
    Store(Value, Value, i32),
    StackAddr(StackSlot, i32),
    Jump(Block, Vec<Value>),
    Brif(Value, Block, Vec<Value>, Block, Vec<Value>),
    Return_(Vec<Value>),
    Trap,
    Call(FuncRef, Vec<Value>, Vec<Type>),
    F64const(f64),
    F32const(f32),
    Fadd(Value, Value),
    Fsub(Value, Value),
    Fmul(Value, Value),
    Fdiv(Value, Value),
    Sqrt(Value),
    Floor(Value),
    Ceil(Value),
    Trunc(Value),
    Fneg(Value),
    Fabs(Value),
    Fcopysign(Value, Value),
    Fma(Value, Value, Value),
    Bitcast(Type, Value),
    Fcmp(FloatCC, Value, Value),
    Fpromote(Value),
    Fdemote(Value),
    FcvtFromSint(Type, Value),
    FcvtFromUint(Type, Value),
    FcvtToSintSat(Type, Value),
    FcvtToUintSat(Type, Value),
    Iconcat(Value, Value),
    Isplit(Value),
    Iadd128(Value, Value),
    Isub128(Value, Value),
    Band128(Value, Value),
    Bor128(Value, Value),
    Bxor128(Value, Value),
    Imul128(Value, Value),
    Udiv128(Value, Value),
    Sdiv128(Value, Value),
    Urem128(Value, Value),
    Srem128(Value, Value),
    Icmp128(IntCC, Value, Value),
    Ishl128(Value, Value),
    Ushr128(Value, Value),
    Sshr128(Value, Value),
    Extend128(Value, bool),
    Reduce128(Type, Value),
    UaddOverflow(Value, Value),
    UsubOverflow(Value, Value),
    SaddOverflow(Value, Value),
    SsubOverflow(Value, Value),
    UmulOverflow(Value, Value),
    SmulOverflow(Value, Value),
    Umulhi(Value, Value),
    Clz(Value),
    Ctz(Value),
    Popcnt(Value),
    Bswap(Value),
    Rotl(Value, Value),
    Rotr(Value, Value),
    Smin(Value, Value),
    Umin(Value, Value),
    Smax(Value, Value),
    Umax(Value, Value),
    Iabs(Value),
    TrapCode(u8),
    TrapzCode(Value, u8),
    TrapnzCode(Value, u8),
    Trapz(Value),
    Trapnz(Value),
    CallIndirect(Value, Vec<Value>, Vec<Type>),
    SymbolAddr(String),
    /// a V128 constant (byte i is bits 8i..8i+8)
    Vconst(u128),
    /// a SIMD instruction
    Simd(VOp, Vec<Value>),
}

impl Op {
    /// whether it is a SIMD instruction (the native backends reject a function that has one, or a V128 value)
    pub fn is_simd(&self) -> bool {
        matches!(self, Op::Vconst(_) | Op::Simd(..))
    }
}

// --- Recorder: 命令を記録するだけの組み立て (acg が使う) -----------------------------------------------

/// 命令を記録するだけの組み立て (acg が使う)。値・ブロック・変数・スロット・関数の参照に番号を振り、値の型を覚える。
/// 機械語にはしない: バックエンド (minclif / Cranelift) が ops を同じ順に読み直して組み立てる (番号は読み直す側で振り直す)。
/// 値の型の決め方は minclif の FunctionBuilder と同じ (codegen の ZUSTC_IR_VERIFY=1 で読み直すときに確かめる)

pub struct Recorder {
    pub params: Vec<Type>,
    pub rets: Vec<Type>,
    /// 記録した命令と、命令ごとの戻り値 (値・ブロック・変数・スロット・関数の参照の番号)
    pub ops: Vec<Op>,
    pub res: Vec<Vec<u32>>,
    /// 入口で 0 にするスロット
    pub zero: Vec<u32>,
    vt: Vec<Type>,
    blocks: Vec<Vec<Value>>,
    vars: Vec<Type>,
    nslots: u32,
    nfuncs: u32,
    cur: Option<Block>,
}

impl Recorder {
    pub fn new(params: Vec<Type>, rets: Vec<Type>) -> Recorder {
        let vt = params.clone();
        Recorder { params, rets, ops: vec![], res: vec![], zero: vec![], vt, blocks: vec![], vars: vec![], nslots: 0, nfuncs: 0, cur: None }
    }
    fn val(&mut self, t: Type) -> Value {
        self.vt.push(t);
        Value(self.vt.len() as u32 - 1)
    }
    pub fn func_params(&self) -> Vec<Value> {
        (0..self.params.len() as u32).map(Value).collect()
    }
    pub fn value_type(&self, v: Value) -> Type {
        self.vt[v.0 as usize]
    }
    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.blocks[b.0 as usize].clone()
    }
    pub fn current_block(&self) -> Option<Block> {
        self.cur
    }
    pub fn n_blocks(&self) -> usize {
        self.blocks.len()
    }
    pub fn n_values(&self) -> usize {
        self.vt.len()
    }
    /// 入口で 0 にするスロットを覚える
    pub fn set_zero(&mut self, z: &[StackSlot]) {
        self.zero = z.iter().map(|s| s.0).collect();
    }
    pub fn create_block(&mut self) -> Block {
        self.ops.push(Op::CreateBlock);
        self.blocks.push(vec![]);
        let r = Block(self.blocks.len() as u32 - 1);
        self.res.push(vec![r.0]);
        r
    }
    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        self.ops.push(Op::AppendBlockParam(b, t));
        let r = self.val(t);
        self.blocks[b.0 as usize].push(r);
        self.res.push(vec![r.0]);
        r
    }
    pub fn switch_to_block(&mut self, b: Block) {
        self.ops.push(Op::SwitchToBlock(b));
        self.cur = Some(b);
        self.res.push(vec![]);
    }
    pub fn declare_var(&mut self, t: Type) -> Variable {
        self.ops.push(Op::DeclareVar(t));
        self.vars.push(t);
        let r = Variable(self.vars.len() as u32 - 1);
        self.res.push(vec![r.0]);
        r
    }
    pub fn def_var(&mut self, var: Variable, v: Value) {
        self.ops.push(Op::DefVar(var, v));
        self.res.push(vec![]);
    }
    pub fn use_var(&mut self, var: Variable) -> Value {
        self.ops.push(Op::UseVar(var));
        let t = self.vars[var.0 as usize];
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        self.ops.push(Op::CreateStackSlot(size, align));
        let r = StackSlot(self.nslots);
        self.nslots += 1;
        self.res.push(vec![r.0]);
        r
    }
    pub fn import_function(&mut self, name: &str) -> FuncRef {
        self.ops.push(Op::ImportFunction(name.to_string()));
        let r = FuncRef(self.nfuncs);
        self.nfuncs += 1;
        self.res.push(vec![r.0]);
        r
    }
    pub fn iconst(&mut self, t: Type, x: i64) -> Value {
        self.ops.push(Op::Iconst(t, x));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Iadd(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Isub(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Imul(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Band(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Bor(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Bxor(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iadd_imm(&mut self, a: Value, x: i64) -> Value {
        self.ops.push(Op::IaddImm(a, x));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Udiv(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Sdiv(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Urem(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Srem(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Ishl(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Ushr(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Sshr(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ineg(&mut self, a: Value) -> Value {
        self.ops.push(Op::Ineg(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bnot(&mut self, a: Value) -> Value {
        self.ops.push(Op::Bnot(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.ops.push(Op::Icmp(cc, a, b));
        let r = self.val(Type::I8);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp_imm(&mut self, cc: IntCC, a: Value, x: i64) -> Value {
        self.ops.push(Op::IcmpImm(cc, a, x));
        let r = self.val(Type::I8);
        self.res.push(vec![r.0]);
        r
    }
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        self.ops.push(Op::Select(c, a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn uextend(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::Uextend(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sextend(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::Sextend(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ireduce(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::Ireduce(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn load(&mut self, t: Type, addr: Value, off: i32) -> Value {
        self.ops.push(Op::Load(t, addr, off));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        self.ops.push(Op::Store(v, addr, off));
        self.res.push(vec![]);
    }
    pub fn stack_addr(&mut self, s: StackSlot, off: i32) -> Value {
        self.ops.push(Op::StackAddr(s, off));
        let r = self.val(Type::I64);
        self.res.push(vec![r.0]);
        r
    }
    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.ops.push(Op::Jump(b, args.to_vec()));
        self.res.push(vec![]);
    }
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.ops.push(Op::Brif(c, then_b, then_args.to_vec(), else_b, else_args.to_vec()));
        self.res.push(vec![]);
    }
    pub fn return_(&mut self, vals: &[Value]) {
        self.ops.push(Op::Return_(vals.to_vec()));
        self.res.push(vec![]);
    }
    pub fn trap(&mut self) {
        self.ops.push(Op::Trap);
        self.res.push(vec![]);
    }
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.ops.push(Op::Call(f, args.to_vec(), rets.to_vec()));
        let r: Vec<Value> = rets.iter().map(|&t| self.val(t)).collect();
        self.res.push(r.iter().map(|v| v.0).collect());
        r
    }
    pub fn f64const(&mut self, x: f64) -> Value {
        self.ops.push(Op::F64const(x));
        let r = self.val(Type::F64);
        self.res.push(vec![r.0]);
        r
    }
    pub fn f32const(&mut self, x: f32) -> Value {
        self.ops.push(Op::F32const(x));
        let r = self.val(Type::F32);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fadd(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fsub(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fmul(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fdiv(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sqrt(&mut self, a: Value) -> Value {
        self.ops.push(Op::Sqrt(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn floor(&mut self, a: Value) -> Value {
        self.ops.push(Op::Floor(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ceil(&mut self, a: Value) -> Value {
        self.ops.push(Op::Ceil(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn trunc(&mut self, a: Value) -> Value {
        self.ops.push(Op::Trunc(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fneg(&mut self, a: Value) -> Value {
        self.ops.push(Op::Fneg(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fabs(&mut self, a: Value) -> Value {
        self.ops.push(Op::Fabs(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fcopysign(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        self.ops.push(Op::Fma(a, b, c));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bitcast(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::Bitcast(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcmp(&mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        self.ops.push(Op::Fcmp(cc, a, b));
        let r = self.val(Type::I8);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fpromote(&mut self, a: Value) -> Value {
        self.ops.push(Op::Fpromote(a));
        let r = self.val(Type::F64);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fdemote(&mut self, a: Value) -> Value {
        self.ops.push(Op::Fdemote(a));
        let r = self.val(Type::F32);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_from_sint(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::FcvtFromSint(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_from_uint(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::FcvtFromUint(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_to_sint_sat(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::FcvtToSintSat(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_to_uint_sat(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::FcvtToUintSat(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iconcat(&mut self, lo: Value, hi: Value) -> Value {
        self.ops.push(Op::Iconcat(lo, hi));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isplit(&mut self, v: Value) -> (Value, Value) {
        self.ops.push(Op::Isplit(v));
        let lo = self.val(Type::I64);
        let hi = self.val(Type::I64);
        self.res.push(vec![lo.0, hi.0]);
        (lo, hi)
    }
    pub fn iadd128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Iadd128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isub128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Isub128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn band128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Band128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bor128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Bor128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bxor128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Bxor128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn imul128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Imul128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn udiv128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Udiv128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sdiv128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Sdiv128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn urem128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Urem128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn srem128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Srem128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp128(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.ops.push(Op::Icmp128(cc, a, b));
        let r = self.val(Type::I8);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ishl128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Ishl128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ushr128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Ushr128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sshr128(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Sshr128(a, b));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn extend128(&mut self, a: Value, signed: bool) -> Value {
        self.ops.push(Op::Extend128(a, signed));
        let r = self.val(Type::I128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn reduce128(&mut self, t: Type, a: Value) -> Value {
        self.ops.push(Op::Reduce128(t, a));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::UaddOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::UsubOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::SaddOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::SsubOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::UmulOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.ops.push(Op::SmulOverflow(a, b));
        let t = self.vt[a.0 as usize];
        let r = self.val(t);
        let o = self.val(Type::I8);
        self.res.push(vec![r.0, o.0]);
        (r, o)
    }
    pub fn umulhi(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Umulhi(a, b));
        let r = self.val(Type::I64);
        self.res.push(vec![r.0]);
        r
    }
    pub fn clz(&mut self, a: Value) -> Value {
        self.ops.push(Op::Clz(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ctz(&mut self, a: Value) -> Value {
        self.ops.push(Op::Ctz(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn popcnt(&mut self, a: Value) -> Value {
        self.ops.push(Op::Popcnt(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bswap(&mut self, a: Value) -> Value {
        self.ops.push(Op::Bswap(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn rotl(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Rotl(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn rotr(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Rotr(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Smin(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Umin(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Smax(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        self.ops.push(Op::Umax(a, b));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iabs(&mut self, a: Value) -> Value {
        self.ops.push(Op::Iabs(a));
        let r = self.val(self.vt[a.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn trap_code(&mut self, code: u8) {
        self.ops.push(Op::TrapCode(code));
        self.res.push(vec![]);
    }
    pub fn trapz_code(&mut self, c: Value, code: u8) {
        self.ops.push(Op::TrapzCode(c, code));
        self.res.push(vec![]);
    }
    pub fn trapnz_code(&mut self, c: Value, code: u8) {
        self.ops.push(Op::TrapnzCode(c, code));
        self.res.push(vec![]);
    }
    pub fn trapz(&mut self, c: Value) {
        self.ops.push(Op::Trapz(c));
        self.res.push(vec![]);
    }
    pub fn trapnz(&mut self, c: Value) {
        self.ops.push(Op::Trapnz(c));
        self.res.push(vec![]);
    }
    pub fn call_indirect(&mut self, p: Value, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.ops.push(Op::CallIndirect(p, args.to_vec(), rets.to_vec()));
        let r: Vec<Value> = rets.iter().map(|&t| self.val(t)).collect();
        self.res.push(r.iter().map(|v| v.0).collect());
        r
    }
    pub fn symbol_addr(&mut self, name: &str) -> Value {
        self.ops.push(Op::SymbolAddr(name.to_string()));
        let r = self.val(Type::I64);
        self.res.push(vec![r.0]);
        r
    }
    pub fn vconst(&mut self, x: u128) -> Value {
        self.ops.push(Op::Vconst(x));
        let r = self.val(Type::V128);
        self.res.push(vec![r.0]);
        r
    }
    pub fn simd(&mut self, op: VOp, args: &[Value]) -> Value {
        self.ops.push(Op::Simd(op, args.to_vec()));
        let r = self.val(op.result());
        self.res.push(vec![r.0]);
        r
    }
}
