//! The types of zustc's instruction sequence (IR): types, values, blocks, variables, stack slots, fn references, relocations, and the recorded instructions (Op).
//! acg (type checking and code generation) only records instructions into a Recorder; turning them into machine code is done by the backend (minclif / Cranelift),
//! which replays the record. check (LSP) discards the record without a backend. Depends on no external crates (zustc itself can compile it)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Type {
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// A SIMD vector of a fixed width. Its lanes are not part of the type: each SIMD instruction says which
    /// [`Shape`] it reads it as (see [`VBits`] for the widths and why)
    Vec(VBits),
}

impl Type {
    pub fn bytes(self) -> u32 {
        match self {
            Type::I8 => 1,
            Type::I16 => 2,
            Type::I32 => 4,
            Type::I64 | Type::F64 => 8,
            Type::I128 => 16,
            Type::Vec(w) => w.bytes(),
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

// --- SIMD (issue #30) ---------------------------------------------------------------------------------------
//
// A vector type is only its width ([`VBits`]); an instruction reads it through a [`Shape`] (lane type x width), as
// wasm's `i32x4.add` reads a `v128`. The instructions are written per lane, not per width, so one [`VOp`] covers
// 128-bit SIMD (wasm simd128, SSE, NEON), 256-bit (AVX2) and 512-bit (AVX-512) vectors: a backend says which widths it
// emits (wasm: 128 only; the others reject a wider vector, they do not split it silently).
//
// Two kinds of instruction do not scale with the width and are 128-bit only: `Shuffle` / `Swizzle` (a byte index
// table; AVX's shuffles work within 128-bit halves) and `Vconst` of more than 16 bytes (a wider constant is a splat).
// `Half` / `Concat` change the width (a 256-bit vector's 128-bit halves); with them an instruction that works per
// 128-bit lane on x86 (`_mm256_packs_epi32`) is written from 128-bit ones. A backend with narrower registers than a
// vector holds it in parts: Cranelift (no 256-bit registers on x86-64 yet) as 128-bit values, wasm not at all.
//
// Wasm's flexible-vectors proposal (phase 3: `vec.i32` and the like, whose length the engine sets: a runtime
// constant, read by `vec.i32.length`) fits the same instructions with a width that is not known until the program
// runs. Its lane-wise operations are these ones (add .. max, compares to masks, `narrow`, `widen_low/high` as
// `Extend`, splat, lane access by immediate index within the low 128 bits), and it has no byte shuffle. It would be
// a width `VBits::Flex` whose size is the target's: such vectors could be locals and parameters but not fields of a
// laid-out struct, as Cranelift's dynamic vector types (`i32x4*gv`) and Rust's scalable vectors are; plus an
// instruction for its lane count and the lane shifts / half concatenations. Not added until a backend has it.

/// The width of a vector
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum VBits {
    B128,
    B256,
    B512,
}

impl VBits {
    pub fn bits(self) -> u32 {
        match self {
            VBits::B128 => 128,
            VBits::B256 => 256,
            VBits::B512 => 512,
        }
    }
    pub fn bytes(self) -> u32 {
        self.bits() / 8
    }
    /// the width of a half (none below 128 bits)
    pub fn half(self) -> Option<VBits> {
        match self {
            VBits::B128 => None,
            VBits::B256 => Some(VBits::B128),
            VBits::B512 => Some(VBits::B256),
        }
    }
    pub fn from_bits(b: u32) -> Option<VBits> {
        Some(match b {
            128 => VBits::B128,
            256 => VBits::B256,
            512 => VBits::B512,
            _ => return None,
        })
    }
}

/// The type of one lane
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lane {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl Lane {
    /// the scalar type of one lane (what splat takes and extract_lane gives)
    pub fn ty(self) -> Type {
        match self {
            Lane::I8 => Type::I8,
            Lane::I16 => Type::I16,
            Lane::I32 => Type::I32,
            Lane::I64 => Type::I64,
            Lane::F32 => Type::F32,
            Lane::F64 => Type::F64,
        }
    }
    pub fn bits(self) -> u32 {
        self.ty().bits()
    }
    pub fn is_float(self) -> bool {
        matches!(self, Lane::F32 | Lane::F64)
    }
}

/// How an instruction reads a vector: its lane type and width (the lane count follows)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Shape {
    pub lane: Lane,
    pub bits: VBits,
}

impl Shape {
    pub const I8X16: Shape = Shape { lane: Lane::I8, bits: VBits::B128 };
    pub const I16X8: Shape = Shape { lane: Lane::I16, bits: VBits::B128 };
    pub const I32X4: Shape = Shape { lane: Lane::I32, bits: VBits::B128 };
    pub const I64X2: Shape = Shape { lane: Lane::I64, bits: VBits::B128 };
    pub const F32X4: Shape = Shape { lane: Lane::F32, bits: VBits::B128 };
    pub const F64X2: Shape = Shape { lane: Lane::F64, bits: VBits::B128 };
    pub fn new(lane: Lane, bits: VBits) -> Shape {
        Shape { lane, bits }
    }
    pub fn count(self) -> u32 {
        self.bits.bits() / self.lane.bits()
    }
    /// the vector type
    pub fn ty(self) -> Type {
        Type::Vec(self.bits)
    }
    pub fn is_float(self) -> bool {
        self.lane.is_float()
    }
}

/// A SIMD instruction (issue #30), in [`Op::Simd`] with the [`Shape`] it reads its vector operands as. The operands
/// are vectors of that width unless noted; the result is one unless [`VOp::result`] says otherwise. For a
/// conversion the shape is the result's (`Narrow`: i8 lanes from two vectors of i16 lanes). "Half" is half the lanes
/// of the width. The set is what wasm SIMD and SSE4.1 / AVX2 / NEON have in common, and the relaxed instructions
/// separately (their results may differ between engines and CPUs: a program opts into them by name)
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
    Splat,
    /// one lane (result: the lane type; i8 / i16 lanes as their bits)
    Extract(u8),
    /// (vector, scalar of the lane type)
    Replace(u8),
    /// (128-bit only) bytes of (a, b) picked by 16 indices < 32 (byte i of the u128 is the index of result byte i)
    Shuffle(u128),
    /// (128-bit only) bytes of a picked by the bytes of b (an index >= 16 gives 0)
    Swizzle,
    Add,
    Sub,
    /// (not for i8 lanes)
    Mul,
    /// (floats)
    Div,
    Neg,
    Abs,
    /// (floats)
    Sqrt,
    Ceil,
    Floor,
    Trunc,
    Nearest,
    /// (unsigned): integers; floats propagate NaN, -0 < +0
    Min(bool),
    Max(bool),
    /// pseudo-min / max of floats: b < a ? b : a / a < b ? b : a
    Pmin,
    Pmax,
    /// saturating, i8 / i16 lanes (unsigned)
    AddSat(bool),
    SubSat(bool),
    /// unsigned rounding average, i8 / i16 lanes
    Avgr,
    /// lanewise compare to a mask of all ones / zeros
    Icmp(IntCC),
    Fcmp(FloatCC),
    /// (vector, I32 amount mod the lane width)
    Shl,
    /// (unsigned)
    Shr(bool),
    /// every lane non-zero (result I8: 0 / 1)
    AllTrue,
    /// the top bit of each lane (result I32: up to 32 lanes; a wider count needs I64, not used yet)
    Bitmask,
    /// i8 lanes
    Popcnt,
    /// (unsigned): two vectors of twice as wide lanes narrowed with saturation (to i8 / i16 lanes)
    Narrow(bool),
    /// (high half, unsigned): half the lanes widened (to i16 / i32 / i64 lanes)
    Extend(bool, bool),
    /// (high half, unsigned): the widened product of half the lanes
    ExtMul(bool, bool),
    /// (unsigned): sums of adjacent pairs, widened (to i16 / i32 lanes)
    ExtAddPairwise(bool),
    /// i32 lanes from the products of adjacent i16 pairs, summed
    Dot,
    /// i16 lanes, q15 rounding saturating multiply
    Q15MulrSat,
    /// (unsigned): to f32 lanes from as many i32 lanes / to f64 lanes from the low half of i32 lanes
    FcvtFromInt(bool),
    /// (from f64, unsigned): to i32 lanes, saturating, from f32 lanes / from f64 lanes into the low half (the rest 0)
    FcvtToIntSat(bool, bool),
    /// f64 lanes to the low half of f32 lanes (the rest 0)
    Demote,
    /// the low half of f32 lanes to f64 lanes
    Promote,
    /// a * b + c rounded once (floats; x86's FMA). Not in wasm (only the relaxed form)
    Fma,
    /// the low / high half of a vector of shape s: a vector of half the width (x86's `_mm256_extractf128_ps`)
    Half(bool),
    /// (low, high): two vectors of half the width of shape s, as one
    Concat,
    // relaxed SIMD (results may differ between engines): wasm's relaxed-simd proposal
    /// a * b + c, fused or not (floats)
    RelaxedMadd,
    /// -(a * b) + c, fused or not
    RelaxedNmadd,
    RelaxedMin,
    RelaxedMax,
    /// (a, b, mask): bitselect, or by the top bit of each lane
    RelaxedLaneselect,
    /// (128-bit only)
    RelaxedSwizzle,
    /// (from f64, unsigned): as FcvtToIntSat, out-of-range lanes as the engine likes
    RelaxedTrunc(bool, bool),
    RelaxedQ15Mulr,
    /// i16 lanes from i8 x i7 lanes
    RelaxedDot,
    /// i32 lanes from i8 x i7 lanes, plus i32 lanes
    RelaxedDotAdd,
}

impl VOp {
    /// the type of the result, read as `s`
    pub fn result(self, s: Shape) -> Type {
        match self {
            VOp::AnyTrue | VOp::AllTrue => Type::I8,
            VOp::Bitmask => Type::I32,
            VOp::Extract(_) => s.lane.ty(),
            VOp::Half(_) => Type::Vec(s.bits.half().unwrap_or(s.bits)),
            _ => s.ty(),
        }
    }
    /// how many operands it takes
    pub fn arity(self) -> usize {
        match self {
            VOp::Not
            | VOp::AnyTrue
            | VOp::Splat
            | VOp::Extract(_)
            | VOp::Neg
            | VOp::Abs
            | VOp::Sqrt
            | VOp::Ceil
            | VOp::Floor
            | VOp::Trunc
            | VOp::Nearest
            | VOp::AllTrue
            | VOp::Bitmask
            | VOp::Popcnt
            | VOp::Extend(..)
            | VOp::ExtAddPairwise(_)
            | VOp::FcvtFromInt(_)
            | VOp::FcvtToIntSat(..)
            | VOp::Demote
            | VOp::Promote
            | VOp::Half(_)
            | VOp::RelaxedTrunc(..) => 1,
            VOp::Bitselect | VOp::Fma | VOp::RelaxedMadd | VOp::RelaxedNmadd | VOp::RelaxedLaneselect | VOp::RelaxedDotAdd => 3,
            _ => 2,
        }
    }
    /// whether it is defined only on 128-bit vectors (a byte index table)
    pub fn only_128(self) -> bool {
        matches!(self, VOp::Shuffle(_) | VOp::Swizzle | VOp::RelaxedSwizzle)
    }
    /// whether it is a relaxed instruction
    pub fn relaxed(self) -> bool {
        matches!(
            self,
            VOp::RelaxedMadd
                | VOp::RelaxedNmadd
                | VOp::RelaxedMin
                | VOp::RelaxedMax
                | VOp::RelaxedLaneselect
                | VOp::RelaxedSwizzle
                | VOp::RelaxedTrunc(..)
                | VOp::RelaxedQ15Mulr
                | VOp::RelaxedDot
                | VOp::RelaxedDotAdd
        )
    }
}

/// Floating-point comparisons
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatCC {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    /// Either is NaN
    Unordered,
}

/// Integer comparisons
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
    /// The 2nd byte of setcc
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
/// A fn to call (an external name)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct FuncRef(pub u32);

/// A relocation in the code (a call's rel32 etc.)
#[derive(Clone, Debug, PartialEq)]
pub struct Reloc {
    pub offset: u32,
    /// The name referred to (the name of the FuncRef's number)
    pub name: String,
    /// R_X86_64_PLT32 (4) / R_X86_64_PC32 (2)
    pub kind: u32,
    pub addend: i64,
}

/// A FunctionBuilder instruction (recorded in call order, and called again on the real Emitter once the allocation is decided)
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
    /// a 128-bit vector constant (byte i is bits 8i..8i+8; a wider constant is a splat)
    Vconst(u128),
    /// a SIMD instruction, reading its vectors as the shape
    Simd(Shape, VOp, Vec<Value>),
    /// Atomic memory operations, all sequentially consistent (std's orderings are all as strong or weaker): a load of
    /// the type from the address, a store of the value (value, address), a read-modify-write giving the old value
    /// (op, address, operand), a compare-and-swap giving the old value (address, expected, new), and a full fence.
    /// Integers of 1, 2, 4 or 8 bytes only
    AtomicLoad(Type, Value),
    AtomicStore(Value, Value),
    AtomicRmw(AtomicOp, Value, Value),
    AtomicCas(Value, Value, Value),
    Fence,
}

/// The operation of [`Op::AtomicRmw`] (the others std has are compare-and-swap loops in the prelude)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicOp {
    Xchg,
    Add,
}

impl Op {
    /// whether it is a SIMD instruction (the native backends reject a function that has one, or a vector value)
    pub fn is_simd(&self) -> bool {
        matches!(self, Op::Vconst(_) | Op::Simd(..))
    }
}

// --- Recorder: a builder that only records instructions (used by acg) -----------------------------------------------

/// A builder that only records instructions (used by acg). Numbers the values, blocks, variables, slots and fn references, and remembers the types of values.
/// It does not produce machine code: the backend (minclif / Cranelift) replays ops in the same order to build it (the numbers are reassigned by the replaying side).
/// The types of values are determined the same way as in minclif's FunctionBuilder (checked on replay with codegen's ZUSTC_IR_VERIFY=1)

pub struct Recorder {
    pub params: Vec<Type>,
    pub rets: Vec<Type>,
    /// The recorded instructions, and the results of each instruction (the numbers of values / blocks / variables / slots / fn references)
    pub ops: Vec<Op>,
    pub res: Vec<Vec<u32>>,
    /// The slots zeroed at entry
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
    /// v is the address of a stack slot (`stack_addr`), found among the last ops (false where not found there)
    pub fn is_stack_addr(&self, v: Value) -> bool {
        for (op, res) in self.ops.iter().zip(&self.res).rev().take(256) {
            if res.contains(&v.0) {
                return matches!(op, Op::StackAddr(..));
            }
        }
        false
    }
    /// Remembers the slots to zero at entry
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
    pub fn atomic_load(&mut self, t: Type, addr: Value) -> Value {
        self.ops.push(Op::AtomicLoad(t, addr));
        let r = self.val(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn atomic_store(&mut self, v: Value, addr: Value) {
        self.ops.push(Op::AtomicStore(v, addr));
        self.res.push(vec![]);
    }
    pub fn atomic_rmw(&mut self, op: AtomicOp, addr: Value, x: Value) -> Value {
        self.ops.push(Op::AtomicRmw(op, addr, x));
        let r = self.val(self.vt[x.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn atomic_cas(&mut self, addr: Value, expected: Value, new: Value) -> Value {
        self.ops.push(Op::AtomicCas(addr, expected, new));
        let r = self.val(self.vt[new.0 as usize]);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fence(&mut self) {
        self.ops.push(Op::Fence);
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
        let r = self.val(Type::Vec(VBits::B128));
        self.res.push(vec![r.0]);
        r
    }
    pub fn simd(&mut self, s: Shape, op: VOp, args: &[Value]) -> Value {
        self.ops.push(Op::Simd(s, op, args.to_vec()));
        let r = self.val(op.result(s));
        self.res.push(vec![r.0]);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(Shape::F32X4.count(), 4);
        assert_eq!(Shape::I8X16.count(), 16);
        assert_eq!(Shape::new(Lane::F32, VBits::B256).count(), 8);
        assert_eq!(Shape::new(Lane::I16, VBits::B512).count(), 32);
        assert_eq!(Type::Vec(VBits::B256).bytes(), 32);
        assert_eq!(VOp::Add.result(Shape::new(Lane::I32, VBits::B512)), Type::Vec(VBits::B512));
        assert_eq!(VOp::Extract(3).result(Shape::F64X2), Type::F64);
        assert_eq!(VBits::from_bits(256), Some(VBits::B256));
        assert_eq!(VBits::from_bits(192), None);
    }
}
