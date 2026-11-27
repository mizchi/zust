//! A minimal backend in place of Cranelift (x86-64 / System V / ELF). The groundwork for replacing Cranelift for self-hosting.
//!
//! The API follows Cranelift's FunctionBuilder (blocks, block params, variables, stack slots, instructions).
//! Generation is simple: every value lives in an 8-byte slot of the frame, and each instruction loads into rax / rcx, computes and writes back.
//! Integer values are kept in their slots zero-extended to the type's width (sign-extended only for signed comparisons / extensions).
//! It depends on no external crates, so zustc itself can compile it too. tests/minclif.rs links it with cc and runs it.


/// Fast hash (FxHash: for the tables looked up by number; std's SipHash took several % of compilation)
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    h: u64,
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.h = (self.h.rotate_left(5) ^ b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.h = (self.h.rotate_left(5) ^ i as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u32(&mut self, i: u32) {
        self.h = (self.h.rotate_left(5) ^ i as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u64(&mut self, i: u64) {
        self.h = (self.h.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn finish(&self) -> u64 {
        self.h
    }
}

pub type FMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;
pub type FSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<FxHasher>>;

/// Value types
pub use zust_ir::{AtomicOp, Block, FloatCC, FuncRef, IntCC, Lane, Op, Reloc, Shape, StackSlot, Type, VBits, VOp, Value, Variable};

/// Assembling one fn's machine code (FunctionBuilder uses it twice: analysis (track), and the real run using the allocation)
pub struct Emitter {
    params: Vec<Type>,
    rets: Vec<Type>,
    /// Value slots (negative offsets from rbp) and types
    values: Vec<(i32, Type)>,
    vars: Vec<(i32, Type)>,
    slots: Vec<i32>,
    frame: i32,
    blocks: Vec<BlockData>,
    cur: Option<Block>,
    funcs: Vec<String>,
    code: Vec<u8>,
    relocs: Vec<Reloc>,
    /// Forward-reference jumps: (where to write the rel32, target)
    fixups: Vec<(usize, Block)>,
    entry_params: Vec<Value>,
    /// Trap locations (ud2 offsets) and codes (corresponding to Cranelift's TrapCode::user)
    traps: Vec<(u32, u8)>,
    entry_jump: usize,
    /// Whether the current block ended with a terminator (jump / brif / return / trap)
    terminated: bool,
    /// If the previous instruction is "write rax to value v's slot", (v, the code position after it).
    /// A load of v into rax right after that can be skipped (cleared at the start of a block: other places jump there)
    rax_holds: Option<(u32, usize)>,
    /// The number of the current instruction (in the order FunctionBuilder recorded)
    pub op: u32,
    /// The record during analysis
    track: Option<Box<Track>>,
    /// The allocation (in the real run)
    plan: Plan,
    /// Where the allocated registers are saved
    save_slots: [i32; 5],
    /// Per value, whether it is a slot address (slot, offset)
    paddr: Vec<Option<(u32, i32)>>,
    /// Per variable, whether its definitions so far are all the same slot address (None: no definition yet, Some(None): different).
    /// The value of use_var is also treated as that address (an inlined `&mut self`, etc.). If a different definition comes later, sroa drops it
    var_paddr: Vec<Option<Option<(u32, i32)>>>,
    /// Per value, whether it is an iconst constant
    kval: Vec<Option<i64>>,
}

/// Registers to hold values / variables: rbx, r12, r13, r14, r15 (callee-saved, so they survive across calls.
/// They do not clash with rax / rcx / rdx / the argument registers / r11 used in assembling instructions)
/// The last 5 (rsi, rdi, r8, r9, r10) are not callee-saved, so they are used only for live ranges that contain no call
/// (the argument registers are written in order within a call, but the range of a value passed as an argument contains that call)
pub const ALLOC_REGS: [u8; 10] = [3, 12, 13, 14, 15, 6, 7, 8, 9, 10];
/// Not kept in a register (slot)
pub const MEM: u8 = 255;
/// Keeps variable numbers apart from value numbers
const VAR_BASE: u32 = 1 << 28;
/// The numbers of the fields of stack slots kept in registers
const FIELD_BASE: u32 = 1 << 29;

/// Analysis record: reads / writes of values / variables (vregs), and segments (ranges where a block was written contiguously) and their successors
#[derive(Default)]
#[doc(hidden)]
pub struct Track {
    /// (vreg, instruction number, is a write)
    pub ev: Vec<(u32, u32, bool)>,
    /// Whether it is used directly as a slot (address / reads and writes of a different width) or is not an integer: cannot be kept in a register
    mem_only: Vec<bool>,
    var_mem_only: Vec<bool>,
    /// Whether it is floating point (kept in xmm)
    vfloat: Vec<bool>,
    varfloat: Vec<bool>,
    pub segs: Vec<Seg>,
    /// The numbers of the instructions that call a fn
    calls: Vec<u32>,
    /// Per value, how many times it was used as a slot address for load / store / iadd_imm
    addr_uses: Vec<u32>,
    /// Loads / stores from slot addresses: (instruction, slot, offset, type, is a store)
    pub acc: Vec<(u32, u32, i32, Type, bool)>,
    /// Variable definitions: (variable, the slot address if the value is one)
    var_defs: Vec<(u32, Option<(u32, i32)>)>,
}

#[derive(Clone, Default)]
pub struct Seg {
    pub block: u32,
    pub start: u32,
    end: u32,
    /// The target blocks of jump / brif
    succ: Vec<u32>,
    /// Switched to another block without a terminator (continues to the next segment of the same block)
    falls: bool,
}

/// The result of allocation: the register per value / variable (a slot if MEM), the registers used (bits of ALLOC_REGS)
#[derive(Clone, Default)]
pub struct Plan {
    pub vloc: Vec<u8>,
    pub varloc: Vec<u8>,
    /// Bits of the ALLOC_REGS[..5] used (the callee-saved ones)
    pub used: u8,
    /// Per slot, whether its fields were made variables (the address is used only by load / store)
    pub prom: Vec<bool>,
    /// (slot, offset) → field number, the field's register (that place in the slot if MEM)
    pub fields: FMap<(u32, i32), u32>,
    pub floc: Vec<u8>,
}

/// mov dst, src (64 bit; register numbers are 0 to 15)
fn mov_rr(dst: u8, src: u8) -> [u8; 3] {
    [0x48 | ((src >> 3) << 2) | (dst >> 3), 0x89, 0xC0 | ((src & 7) << 3) | (dst & 7)]
}

/// movaps xmm_dst, xmm_src
fn movaps(dst: u8, src: u8) -> Vec<u8> {
    let mut o = vec![];
    if dst >= 8 || src >= 8 {
        o.push(0x40 | ((dst >> 3) << 2) | (src >> 3));
    }
    o.extend_from_slice(&[0x0F, 0x28, 0xC0 | ((dst & 7) << 3) | (src & 7)]);
    o
}

/// mov [rbp+d], r / mov r, [rbp+d]
fn mov_mr(r: u8, d: i32, load: bool) -> [u8; 7] {
    let db = d.to_le_bytes();
    [0x48 | ((r >> 3) << 2), if load { 0x8B } else { 0x89 }, 0x85 | ((r & 7) << 3), db[0], db[1], db[2], db[3]]
}

struct BlockData {
    params: Vec<Value>,
    /// The code position (when switched to)
    offset: Option<usize>,
    /// When switched to another block before the terminator, the position of the rel32 of the jmp to the continuation (pointed at the current position on coming back)
    cont: Option<usize>,
}

impl Emitter {
    pub fn new(params: Vec<Type>, rets: Vec<Type>, plan: Option<Plan>) -> Emitter {
        let track = if plan.is_none() {
            // (The segment before the entry: continues to the first block)
            let mut t = Track::default();
            t.segs.push(Seg { block: u32::MAX, start: 0, end: 0, succ: vec![], falls: true });
            Some(Box::new(t))
        } else {
            None
        };
        let plan = plan.unwrap_or_default();
        let mut b = Emitter {
            params: params.clone(),
            rets,
            values: vec![],
            vars: vec![],
            slots: vec![],
            frame: 0,
            blocks: vec![],
            cur: None,
            funcs: vec![],
            code: vec![],
            relocs: vec![],
            fixups: vec![],
            entry_params: vec![],
            traps: vec![],
            entry_jump: 0,
            terminated: false,
            rax_holds: None,
            op: 0,
            track,
            plan,
            save_slots: [0; 5],
            paddr: vec![],
            var_paddr: vec![],
            kval: vec![],
        };
        // Prologue: push rbp; mov rbp, rsp; sub rsp, <frame> (frame is written at the end)
        b.code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC, 0, 0, 0, 0]);
        // (Saving the allocated registers)
        for i in 0..5 {
            b.save_slots[i] = b.alloc(8, 8);
            if b.plan.used & (1 << i) != 0 {
                let d = b.save_slots[i];
                b.code.extend_from_slice(&mov_mr(ALLOC_REGS[i], d, false));
            }
        }
        // Arguments into slots
        const ARG_STORE: [[u8; 3]; 6] = [[0x48, 0x89, 0xBD], [0x48, 0x89, 0xB5], [0x48, 0x89, 0x95], [0x48, 0x89, 0x8D], [0x4C, 0x89, 0x85], [0x4C, 0x89, 0x8D]];
        let (mut ni, mut nf) = (0usize, 0u8);
        let mut ns = 0i32;
        for (_, &t) in params.iter().enumerate() {
            let v = b.new_value(t);
            let off = b.values[v.0 as usize].0;
            // (Floating-point arguments are in xmm0 to 7)
            if t.is_float() && nf < 8 {
                let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
                b.code.extend_from_slice(&[p, 0x0F, 0x11, 0x85 | (nf << 3)]);
                b.code.extend_from_slice(&off.to_le_bytes());
                nf += 1;
                b.entry_params.push(v);
                b.force_mem(v);
                continue;
            }
            // (An i128 takes 2 registers (low, high). If 2 are not left, the whole goes on the stack)
            let parts = if t.bytes() == 16 { 2 } else { 1 };
            if ni + parts <= 6 {
                for k in 0..parts {
                    b.code.extend_from_slice(&ARG_STORE[ni + k]);
                    b.code.extend_from_slice(&(off + 8 * k as i32).to_le_bytes());
                }
                ni += parts;
            } else {
                ni = 6;
                for k in 0..parts {
                    // Passed on the stack: [rbp + 16 + 8*n]
                    let src = 16 + 8 * ns;
                    ns += 1;
                    b.code.extend_from_slice(&[0x48, 0x8B, 0x85]);
                    b.code.extend_from_slice(&src.to_le_bytes());
                    b.store_rax_at(off + 8 * k as i32);
                }
            }
            b.entry_params.push(v);
            // (Integer arguments can be kept in registers: treated as written at the entry (instruction 0), and in the real run loaded into the register below)
            if t.bytes() == 16 {
                b.force_mem(v);
            } else {
                b.ev_def(v.0);
            }
        }
        // (If there are slots to zero at the entry, finish_zeroing jumps from here: normally this just jumps to the next instruction)
        for k in 0..b.entry_params.len() {
            let v = b.entry_params[k];
            let r = b.loc(v);
            if r != MEM {
                let o = b.values[v.0 as usize].0;
                b.code.extend_from_slice(&mov_mr(r, o, true));
            }
        }
        b.entry_jump = b.code.len();
        b.code.extend_from_slice(&[0xE9, 0, 0, 0, 0]);
        b
    }

    /// The values of the fn's arguments
    pub fn func_params(&self) -> Vec<Value> {
        self.entry_params.clone()
    }

    fn alloc(&mut self, size: u32, align: u32) -> i32 {
        let a = align.max(1) as i32;
        self.frame = (self.frame + size as i32 + a - 1) / a * a;
        -self.frame
    }

    fn new_value(&mut self, t: Type) -> Value {
        let off = if t.bytes() >= 16 { self.alloc(t.bytes(), 16) } else { self.alloc(8, 8) };
        self.values.push((off, t));
        self.paddr.push(None);
        self.kval.push(None);
        if let Some(tr) = &mut self.track {
            tr.addr_uses.push(0);
            tr.mem_only.push(t.bytes() == 16);
            tr.vfloat.push(t.is_float());
        }
        Value(self.values.len() as u32 - 1)
    }

    pub fn value_type(&self, v: Value) -> Type {
        self.values[v.0 as usize].1
    }

    pub fn create_block(&mut self) -> Block {
        self.blocks.push(BlockData { params: vec![], offset: None, cont: None });
        Block(self.blocks.len() as u32 - 1)
    }

    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        let v = self.new_value(t);
        self.blocks[b.0 as usize].params.push(v);
        v
    }

    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.blocks[b.0 as usize].params.clone()
    }

    /// Switches to a block. When leaving a block midway (no terminator yet), puts a jmp to the continuation, and
    /// when coming back to that block later (the continuation can be written, as in Cranelift), points that jmp at the current position
    pub fn switch_to_block(&mut self, b: Block) {
        let falls = self.cur.is_some_and(|c| c != b) && !self.terminated;
        if self.cur != Some(b) {
            self.seg_switch(b, falls);
        }
        if let Some(c) = self.cur {
            if !self.terminated && c != b {
                self.emit(&[0xE9]);
                let at = self.code.len();
                self.emit(&[0, 0, 0, 0]);
                self.blocks[c.0 as usize].cont = Some(at);
            }
        }
        let here = self.code.len();
        self.rax_holds = None;
        let bd = &mut self.blocks[b.0 as usize];
        if let Some(at) = bd.cont.take() {
            let rel = here as i32 - (at as i32 + 4);
            self.code[at..at + 4].copy_from_slice(&rel.to_le_bytes());
        } else if bd.offset.is_none() {
            bd.offset = Some(here);
        }
        self.cur = Some(b);
        self.terminated = false;
    }

    pub fn current_block(&self) -> Option<Block> {
        self.cur
    }

    pub fn declare_var(&mut self, t: Type) -> Variable {
        let off = if t.bytes() >= 16 { self.alloc(t.bytes(), 16) } else { self.alloc(8, 8) };
        self.vars.push((off, t));
        self.var_paddr.push(None);
        if let Some(tr) = &mut self.track {
            tr.var_mem_only.push(t.bytes() == 16);
            tr.varfloat.push(t.is_float());
        }
        Variable(self.vars.len() as u32 - 1)
    }

    pub fn def_var(&mut self, var: Variable, v: Value) {
        let pa = self.paddr[v.0 as usize];
        let st = self.var_paddr[var.0 as usize];
        self.var_paddr[var.0 as usize] = match (st, pa) {
            (None, Some(a)) => Some(Some(a)),
            (Some(Some(b)), Some(a)) if a == b => Some(Some(a)),
            _ => Some(None),
        };
        if let Some(tr) = &mut self.track {
            tr.var_defs.push((var.0, pa));
        }
        if pa.is_some() {
            self.addr_use(v);
        }
        self.def_var_inner(var, v);
    }

    fn def_var_inner(&mut self, var: Variable, v: Value) {
        let (off, t) = self.vars[var.0 as usize];
        // (If both are registers, copy directly)
        let (lv, lr) = (self.loc(v), self.var_loc(var));
        if lv != MEM && lr != MEM && (lv >= 16) == (lr >= 16) {
            self.ev_use(v.0);
            self.ev_def(VAR_BASE + var.0);
            if lv >= 16 {
                self.emit(&movaps(lr - 16, lv - 16));
            } else {
                self.emit(&mov_rr(lr, lv));
            }
            self.rax_holds = None;
            return;
        }
        self.load_rax(v);
        self.ev_def(VAR_BASE + var.0);
        let l = self.var_loc(var);
        if l != MEM && l >= 16 {
            // movq xmm, rax
            let k = l - 16;
            self.emit(&[0x66, 0x48 | ((k >> 3) << 2), 0x0F, 0x6E, 0xC0 | ((k & 7) << 3)]);
        } else if l != MEM {
            self.emit(&mov_rr(l, 0));
        } else {
            self.store_rax_at(off);
        }
        if t.bytes() == 16 {
            let o = self.off(v);
            self.load_rax_at(o + 8);
            self.store_rax_at(off + 8);
        }
    }

    pub fn use_var(&mut self, var: Variable) -> Value {
        let r = self.use_var_inner(var);
        if let Some(Some(a)) = self.var_paddr[var.0 as usize] {
            self.paddr[r.0 as usize] = Some(a);
        }
        r
    }

    fn use_var_inner(&mut self, var: Variable) -> Value {
        let (off, t) = self.vars[var.0 as usize];
        let (lr, ld) = (self.var_loc(var), self.next_loc());
        if lr != MEM && ld != MEM && (lr >= 16) == (ld >= 16) {
            self.ev_use(VAR_BASE + var.0);
            if lr >= 16 {
                self.emit(&movaps(ld - 16, lr - 16));
            } else {
                self.emit(&mov_rr(ld, lr));
            }
            return self.result_in_reg(t);
        }
        let v = self.new_value(t);
        self.ev_use(VAR_BASE + var.0);
        let l = self.var_loc(var);
        if l != MEM && l >= 16 {
            // movq rax, xmm
            let k = l - 16;
            self.emit(&[0x66, 0x48 | ((k >> 3) << 2), 0x0F, 0x7E, 0xC0 | ((k & 7) << 3)]);
        } else if l != MEM {
            self.emit(&mov_rr(0, l));
        } else {
            self.load_rax_at(off);
        }
        if t.bytes() == 16 {
            self.force_mem(v);
            let o = self.values[v.0 as usize].0;
            self.store_rax_at(o);
            self.load_rax_at(off + 8);
            self.store_rax_at(o + 8);
        } else {
            self.store_rax(v);
        }
        v
    }

    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        let off = self.alloc(size.max(1), align.max(1).min(16));
        self.slots.push(off);
        StackSlot(self.slots.len() as u32 - 1)
    }

    /// Registers the name of a fn to call
    pub fn import_function(&mut self, name: &str) -> FuncRef {
        if let Some(i) = self.funcs.iter().position(|f| f == name) {
            return FuncRef(i as u32);
        }
        self.funcs.push(name.to_string());
        FuncRef(self.funcs.len() as u32 - 1)
    }

    // --- Instruction parts ---------------------------------------------------

    fn emit(&mut self, b: &[u8]) {
        self.code.extend_from_slice(b);
    }

    fn emit_disp(&mut self, op: &[u8], d: i32) {
        self.code.extend_from_slice(op);
        self.code.extend_from_slice(&d.to_le_bytes());
    }

    fn load_rax_at(&mut self, off: i32) {
        self.emit_disp(&[0x48, 0x8B, 0x85], off);
    }

    fn store_rax_at(&mut self, off: i32) {
        self.emit_disp(&[0x48, 0x89, 0x85], off);
    }

    /// Whether rax holds v now (just written)
    fn rax_is(&self, v: Value) -> bool {
        self.rax_holds == Some((v.0, self.code.len()))
    }

    /// Loads value v into integer register r
    fn load_reg(&mut self, r: u8, v: Value) {
        self.ev_use(v.0);
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            // movq r64, xmm (for f32, movd r32, xmm: the upper half is 0)
            let k = l - 16;
            let w = if self.values[v.0 as usize].1 == Type::F64 { 0x48 } else { 0x40 };
            self.emit(&[0x66, w | ((k >> 3) << 2) | (r >> 3), 0x0F, 0x7E, 0xC0 | ((k & 7) << 3) | (r & 7)]);
            return;
        }
        if l != MEM {
            if l != r {
                self.emit(&mov_rr(r, l));
            }
            return;
        }
        let off = self.values[v.0 as usize].0;
        self.emit(&mov_mr(r, off, true));
    }

    fn load_rax(&mut self, v: Value) {
        if self.rax_is(v) {
            self.ev_use(v.0);
            return;
        }
        self.load_reg(0, v);
    }

    fn load_rcx(&mut self, v: Value) {
        if self.rax_is(v) {
            self.ev_use(v.0);
            self.emit(&[0x48, 0x89, 0xC1]); // mov rcx, rax
            return;
        }
        self.load_reg(1, v);
    }

    fn store_rax(&mut self, v: Value) {
        self.ev_def(v.0);
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            // movq xmm, rax (for f32, movd xmm, eax)
            let k = l - 16;
            let w = if self.values[v.0 as usize].1 == Type::F64 { 0x48 } else { 0x40 };
            self.emit(&[0x66, w | ((k >> 3) << 2), 0x0F, 0x6E, 0xC0 | ((k & 7) << 3)]);
            self.rax_holds = Some((v.0, self.code.len()));
            return;
        }
        if l != MEM {
            self.emit(&mov_rr(l, 0));
        } else {
            let off = self.values[v.0 as usize].0;
            self.store_rax_at(off);
        }
        if self.values[v.0 as usize].1.bytes() != 16 {
            self.rax_holds = Some((v.0, self.code.len()));
        }
    }

    /// Zero-extends rax from the type's width (nothing for I64)
    fn zext_rax(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xB6, 0xC0]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xB7, 0xC0]),
            Type::I32 => self.emit(&[0x89, 0xC0]),
            _ => {}
        }
    }

    /// Sign-extends rax from the type's width
    fn sext_rax(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC0]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC0]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC0]),
            _ => {}
        }
    }

    fn sext_rcx(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC9]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC9]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC9]),
            _ => {}
        }
    }

    fn result(&mut self, t: Type) -> Value {
        self.zext_rax(t);
        let v = self.new_value(t);
        self.store_rax(v);
        v
    }

    // --- Instructions ----------------------------------------------------------

    pub fn iconst(&mut self, t: Type, x: i64) -> Value {
        let v = self.iconst_inner(t, x);
        self.kval[v.0 as usize] = Some(x);
        v
    }

    fn iconst_inner(&mut self, t: Type, x: i64) -> Value {
        // (If the result is kept in a register, write it directly: the value zero-extended to the type's width)
        let d = self.next_loc();
        if d != MEM && t.bytes() != 16 && !t.is_float() {
            let xv: u64 = match t {
                Type::I8 => x as u64 & 0xFF,
                Type::I16 => x as u64 & 0xFFFF,
                Type::I32 => x as u64 & 0xFFFF_FFFF,
                _ => x as u64,
            };
            let b = if d >= 8 { 1u8 } else { 0 };
            if xv <= 0xFFFF_FFFF {
                // mov r32, imm32 (the upper half becomes 0)
                if b != 0 {
                    self.emit(&[0x41]);
                }
                self.emit(&[0xB8 + (d & 7)]);
                self.emit(&(xv as u32).to_le_bytes());
            } else if (xv as i64) >= i32::MIN as i64 && (xv as i64) < 0 {
                // mov r64, imm32 (sign-extended)
                self.emit(&[0x48 | b, 0xC7, 0xC0 | (d & 7)]);
                self.emit(&(xv as i64 as i32).to_le_bytes());
            } else {
                self.emit(&[0x48 | b, 0xB8 + (d & 7)]);
                self.emit(&xv.to_le_bytes());
            }
            return self.result_in_reg(t);
        }
        self.emit(&[0x48, 0xB8]);
        self.emit(&x.to_le_bytes());
        self.result(t)
    }

    /// If 64 / 32 bit a and b are in registers, writes cmp a, b and returns true (a 32-bit comparison is correct for signed ones too)
    fn cmp_regs(&mut self, t: Type, a: Value, b: Value) -> bool {
        if t != Type::I64 && t != Type::I32 {
            return false;
        }
        let (Some(ra), Some(rb)) = (self.rloc(a), self.rloc(b)) else { return false };
        let w = if t == Type::I64 { 0x48 } else { 0x40 };
        // cmp ra, rb: 39 /r (reg = rb, rm = ra)
        self.emit(&[w | ((rb >> 3) << 2) | (ra >> 3), 0x39, 0xC0 | ((rb & 7) << 3) | (ra & 7)]);
        true
    }

    /// modrm (+ SIB) + disp32: [base + off], with r in the reg field
    fn mem_operand(&mut self, r: u8, base: u8, off: i32) {
        self.emit(&[0x80 | ((r & 7) << 3) | (base & 7)]);
        if base & 7 == 4 {
            self.emit(&[0x24]);
        }
        self.emit(&off.to_le_bytes());
    }

    /// The register of the next value to create (a slot if MEM)
    fn next_loc(&self) -> u8 {
        self.plan.vloc.get(self.values.len()).copied().unwrap_or(MEM)
    }

    /// After writing the next value directly into a register: creates the value (rax does not hold it)
    fn result_in_reg(&mut self, t: Type) -> Value {
        let v = self.new_value(t);
        self.ev_def(v.0);
        self.rax_holds = None;
        v
    }

    /// The value's register (None if a slot)
    fn rloc(&self, v: Value) -> Option<u8> {
        let l = self.loc(v);
        if l == MEM { None } else { Some(l) }
    }

    fn binop(&mut self, a: Value, b: Value, op: &[u8]) -> Value {
        let t = self.value_type(a);
        // (For 64 / 32 bit, if both operands and the result are registers, the 2-operand form: mov d, a; op d, b.
        //  Live ranges are closed, so d differs from the registers of a / b. 32-bit instructions zero the upper half)
        if t == Type::I64 || t == Type::I32 {
            let d = self.next_loc();
            if let (Some(ra), Some(rb), true) = (self.rloc(a), self.rloc(b), d != MEM) {
                // (add / sub / and / or / xor rax, rcx and imul rax, rcx)
                let code: Option<u8> = if op.len() == 3 && op[2] == 0xC8 && matches!(op[1], 0x01 | 0x29 | 0x21 | 0x09 | 0x31) {
                    Some(op[1])
                } else if op.len() == 4 && op[1] == 0x0F && op[2] == 0xAF {
                    Some(0xAF)
                } else {
                    None
                };
                if let Some(c) = code {
                    let w = if t == Type::I64 { 0x48 } else { 0x40 };
                    self.emit(&mov_rr(d, ra));
                    if c == 0xAF {
                        // imul d, b: 0F AF /r (reg = d)
                        self.emit(&[w | ((d >> 3) << 2) | (rb >> 3), 0x0F, 0xAF, 0xC0 | ((d & 7) << 3) | (rb & 7)]);
                    } else {
                        // op d, b: op /r (reg = b, rm = d)
                        self.emit(&[w | ((rb >> 3) << 2) | (d >> 3), c, 0xC0 | ((rb & 7) << 3) | (d & 7)]);
                    }
                    return self.result_in_reg(t);
                }
            }
        }
        // (If rax holds b, move it to rcx first)
        if self.rax_is(b) && a != b {
            self.load_rcx(b);
            self.load_rax(a);
        } else {
            self.load_rax(a);
            self.load_rcx(b);
        }
        self.emit(op);
        self.result(t)
    }

    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        // (Slot address + constant: stays an address of that slot. For a slot whose fields were made variables, only create the value's number)
        let (pa, pb) = (self.paddr[a.0 as usize], self.paddr[b.0 as usize]);
        let derived = match (pa, self.kval[b.0 as usize], pb, self.kval[a.0 as usize]) {
            (Some((sl, o)), Some(x), _, _) if (x as i32) as i64 == x => Some((a, sl, o.wrapping_add(x as i32))),
            (_, _, Some((sl, o)), Some(x)) if (x as i32) as i64 == x => Some((b, sl, o.wrapping_add(x as i32))),
            _ => None,
        };
        if let Some((base, sl, no)) = derived {
            if self.promoted(sl) {
                let r = self.new_value(Type::I64);
                self.paddr[r.0 as usize] = Some((sl, no));
                return r;
            }
            self.addr_use(base);
            let r = self.binop(a, b, &[0x48, 0x01, 0xC8]);
            self.paddr[r.0 as usize] = Some((sl, no));
            return r;
        }
        self.binop(a, b, &[0x48, 0x01, 0xC8])
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x29, 0xC8])
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x0F, 0xAF, 0xC1])
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x21, 0xC8])
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x09, 0xC8])
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x31, 0xC8])
    }

    pub fn iadd_imm(&mut self, a: Value, x: i64) -> Value {
        let t = self.value_type(a);
        let k = self.iconst(t, x);
        self.iadd(a, k)
    }

    /// Unsigned / signed division and remainder (the check for division by 0 is up to the caller)
    fn divop(&mut self, a: Value, b: Value, signed: bool, rem: bool) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        if signed {
            self.sext_rax(t);
            self.sext_rcx(t);
            self.emit(&[0x48, 0x99]); // cqo
            self.emit(&[0x48, 0xF7, 0xF9]); // idiv rcx
        } else {
            self.emit(&[0x31, 0xD2]); // xor edx, edx
            self.emit(&[0x48, 0xF7, 0xF1]); // div rcx
        }
        if rem {
            self.emit(&[0x48, 0x89, 0xD0]); // mov rax, rdx
        }
        self.result(t)
    }

    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, false, false)
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, true, false)
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, false, true)
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, true, true)
    }

    /// Shift (the amount is the low 8 bits of rcx)
    fn shift(&mut self, a: Value, b: Value, op: u8, arith: bool) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        if arith {
            self.sext_rax(t);
        }
        self.load_rcx(b);
        self.emit(&[0x48, 0xD3, op]);
        self.result(t)
    }

    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xE0, false)
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xE8, false)
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xF8, true)
    }

    pub fn ineg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0xF7, 0xD8]);
        self.result(t)
    }

    pub fn bnot(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0xF7, 0xD0]);
        self.result(t)
    }

    /// Comparison (the result is an I8 0 / 1)
    pub fn icmp(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        if self.cmp_regs(t, a, b) {
            self.emit(&[0x0F, cc.setcc(), 0xC0]); // setcc al
            return self.result(Type::I8);
        }
        self.load_rax(a);
        self.load_rcx(b);
        if cc.signed() {
            self.sext_rax(t);
            self.sext_rcx(t);
        }
        self.emit(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        self.emit(&[0x0F, cc.setcc(), 0xC0]); // setcc al
        // (movzx rax, al is done by result)
        self.result(Type::I8)
    }

    /// For `brif(icmp(cc, a, b), ..)` when the icmp's value is used nowhere else: branches with cmp and jcc without creating the value.
    /// (The icmp value's number is kept the same: it is only created, not written)
    pub fn cmp_only(&mut self, cc: IntCC, a: Value, b: Value) {
        let t = self.value_type(a);
        if self.cmp_regs(t, a, b) {
            let _ = self.new_value(Type::I8);
            self.rax_holds = None;
            return;
        }
        self.load_rax(a);
        self.load_rcx(b);
        if cc.signed() {
            self.sext_rax(t);
            self.sext_rcx(t);
        }
        self.emit(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        let _ = self.new_value(Type::I8);
        self.rax_holds = None;
    }

    /// After cmp_only: to else if it does not hold (jcc on the opposite condition of setcc)
    pub fn brif_cc(&mut self, cc: IntCC, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.branch_on((cc.setcc() ^ 1) - 0x10, then_b, then_args, else_b, else_args);
    }

    pub fn icmp_imm(&mut self, cc: IntCC, a: Value, x: i64) -> Value {
        let t = self.value_type(a);
        let k = self.iconst(t, x);
        self.icmp(cc, a, k)
    }

    /// c ? a : b
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.load_rcx(a);
        self.load_rax(b);
        let off = self.off(c);
        // cmp byte [rbp+off], 0; cmovne rax, rcx
        self.emit_disp(&[0x80, 0xBD], off);
        self.emit(&[0x00]);
        self.emit(&[0x48, 0x0F, 0x45, 0xC1]);
        self.result(t)
    }

    pub fn uextend(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        self.result(t)
    }

    pub fn sextend(&mut self, t: Type, a: Value) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        self.sext_rax(st);
        self.result(t)
    }

    pub fn ireduce(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        self.result(t)
    }

    /// Loads [addr + off]
    pub fn load(&mut self, t: Type, addr: Value, off: i32) -> Value {
        if let Some((s, o)) = self.paddr[addr.0 as usize] {
            if self.promoted(s) {
                return self.field_load(t, s, o + off);
            }
            self.addr_use(addr);
            self.acc(s, o + off, t, false);
        }
        // (If the address and the result are registers, mov d, [base + off] / movzx)
        let d = self.next_loc();
        if let (Some(base), true) = (self.rloc(addr), d != MEM && matches!(t, Type::I64 | Type::I32 | Type::I16 | Type::I8)) {
            let rx = ((d >> 3) << 2) | (base >> 3);
            match t {
                Type::I64 => self.emit(&[0x48 | rx, 0x8B]),
                Type::I32 => self.emit(&[0x40 | rx, 0x8B]),
                Type::I16 => self.emit(&[0x48 | rx, 0x0F, 0xB7]),
                _ => self.emit(&[0x48 | rx, 0x0F, 0xB6]),
            }
            self.mem_operand(d, base, off);
            return self.result_in_reg(t);
        }
        self.load_rax(addr);
        match t {
            Type::I64 | Type::F64 => self.emit_disp(&[0x48, 0x8B, 0x80], off),
            Type::I32 | Type::F32 => self.emit_disp(&[0x8B, 0x80], off),
            Type::I16 => self.emit_disp(&[0x48, 0x0F, 0xB7, 0x80], off),
            Type::I8 => self.emit_disp(&[0x48, 0x0F, 0xB6, 0x80], off),
            Type::I128 | Type::Vec(_) => {
                // Load the low half into rcx and the high half into rax, and write
                self.emit_disp(&[0x48, 0x8B, 0x88], off);
                self.emit_disp(&[0x48, 0x8B, 0x80], off + 8);
                let v = self.new_value(t);
                let o = self.off(v);
                self.emit_disp(&[0x48, 0x89, 0x8D], o);
                self.store_rax_at(o + 8);
                return v;
            }
        }
        self.result(t)
    }

    /// Writes v to [addr + off]
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        let t = self.value_type(v);
        if let Some((s, o)) = self.paddr[addr.0 as usize] {
            if self.promoted(s) {
                self.field_store(v, s, o + off);
                return;
            }
            self.addr_use(addr);
            self.acc(s, o + off, t, true);
        }
        // (If the value and the address are registers, mov [base + off], r)
        if let (Some(r), Some(base)) = (self.rloc(v), self.rloc(addr)) {
            if matches!(t, Type::I64 | Type::I32 | Type::I16 | Type::I8) {
                let rx = ((r >> 3) << 2) | (base >> 3);
                match t {
                    Type::I64 => self.emit(&[0x48 | rx, 0x89]),
                    Type::I32 => self.emit(&[0x40 | rx, 0x89]),
                    Type::I16 => self.emit(&[0x66, 0x40 | rx, 0x89]),
                    // (With a REX prefix this is the low 8 bits like sil / dil)
                    _ => self.emit(&[0x40 | rx, 0x88]),
                }
                self.mem_operand(r, base, off);
                return;
            }
        }
        if self.rax_is(v) && v != addr {
            self.load_rcx(v);
            self.load_rax(addr);
        } else {
            self.load_rax(addr);
            self.load_rcx(v);
        }
        match t {
            Type::I64 | Type::F64 => self.emit_disp(&[0x48, 0x89, 0x88], off),
            Type::I32 | Type::F32 => self.emit_disp(&[0x89, 0x88], off),
            Type::I16 => self.emit_disp(&[0x66, 0x89, 0x88], off),
            Type::I8 => self.emit_disp(&[0x88, 0x88], off),
            Type::I128 | Type::Vec(_) => {
                self.emit_disp(&[0x48, 0x89, 0x88], off);
                let o = self.off(v);
                self.emit_disp(&[0x48, 0x8B, 0x8D], o + 8);
                self.emit_disp(&[0x48, 0x89, 0x88], off + 8);
            }
        }
    }

    /// The address of a stack slot
    pub fn stack_addr(&mut self, s: StackSlot, off: i32) -> Value {
        // (A slot whose fields were made variables: no address is created)
        if self.promoted(s.0) {
            let v = self.new_value(Type::I64);
            self.paddr[v.0 as usize] = Some((s.0, off));
            return v;
        }
        let base = self.slots[s.0 as usize];
        self.emit_disp(&[0x48, 0x8D, 0x85], base + off);
        let v = self.result(Type::I64);
        self.paddr[v.0 as usize] = Some((s.0, off));
        v
    }

    fn promoted(&self, s: u32) -> bool {
        self.plan.prom.get(s as usize).copied().unwrap_or(false)
    }

    /// A slot address value was used for load / store / iadd_imm (during analysis)
    fn addr_use(&mut self, a: Value) {
        if let Some(tr) = &mut self.track {
            tr.addr_uses[a.0 as usize] += 1;
        }
    }

    fn acc(&mut self, s: u32, off: i32, t: Type, store: bool) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.acc.push((op, s, off, t, store));
        }
    }

    /// Reads a field made into a variable (creates the result value)
    fn field_load(&mut self, t: Type, s: u32, off: i32) -> Value {
        let f = self.plan.fields[&(s, off)] as usize;
        let l = self.plan.floc[f];
        let d = self.slots[s as usize] + off;
        if t.is_float() {
            let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
            if l != MEM {
                let dl = self.next_loc();
                if dl != MEM && dl >= 16 {
                    self.emit(&movaps(dl - 16, l - 16));
                    return self.result_in_reg(t);
                }
                self.emit(&movaps(0, l - 16));
            } else {
                self.emit_disp(&[p, 0x0F, 0x10, 0x85], d);
            }
            return self.xmm0_result(t);
        }
        if l != MEM {
            let dl = self.next_loc();
            if dl != MEM && dl < 16 {
                self.emit(&mov_rr(dl, l));
                return self.result_in_reg(t);
            }
            self.emit(&mov_rr(0, l));
            return self.result(t);
        }
        match t {
            Type::I64 => self.emit_disp(&[0x48, 0x8B, 0x85], d),
            Type::I32 => self.emit_disp(&[0x8B, 0x85], d),
            Type::I16 => self.emit_disp(&[0x48, 0x0F, 0xB7, 0x85], d),
            _ => self.emit_disp(&[0x48, 0x0F, 0xB6, 0x85], d),
        }
        self.result(t)
    }

    /// Writes v to a field made into a variable
    fn field_store(&mut self, v: Value, s: u32, off: i32) {
        let t = self.value_type(v);
        let f = self.plan.fields[&(s, off)] as usize;
        let l = self.plan.floc[f];
        let d = self.slots[s as usize] + off;
        if t.is_float() {
            self.xmm_load(0, v);
            if l != MEM {
                self.emit(&movaps(l - 16, 0));
            } else {
                let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
                self.emit_disp(&[p, 0x0F, 0x11, 0x85], d);
            }
        } else if l != MEM {
            self.load_reg(l, v);
        } else {
            self.load_rax(v);
            match t {
                Type::I64 => self.emit_disp(&[0x48, 0x89, 0x85], d),
                Type::I32 => self.emit_disp(&[0x89, 0x85], d),
                Type::I16 => self.emit_disp(&[0x66, 0x89, 0x85], d),
                _ => self.emit_disp(&[0x88, 0x85], d),
            }
        }
        self.rax_holds = None;
    }

    fn rel32_to(&mut self, b: Block) {
        let at = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.fixups.push((at, b));
    }

    /// Passes block arguments (writes to the target's param slots)
    fn pass_args(&mut self, b: Block, args: &[Value]) {
        let ps = self.blocks[b.0 as usize].params.clone();
        // (With only one, no swapping happens: write it directly)
        if args.len() == 1 && ps.len() >= 1 && self.value_type(args[0]).bytes() != 16 {
            self.load_rax(args[0]);
            self.store_rax(ps[0]);
            return;
        }
        // (In case the arguments swap with one another, read them all before writing: push them on the stack)
        for &a in args {
            self.load_rax(a);
            self.emit(&[0x50]); // push rax
            if self.value_type(a).bytes() == 16 {
                let o = self.off(a);
                self.load_rax_at(o + 8);
                self.emit(&[0x50]);
            }
        }
        for &p in ps.iter().rev().take(args.len()) {
            if self.value_type(p).bytes() == 16 {
                let o = self.off(p);
                self.emit(&[0x58]);
                self.store_rax_at(o + 8);
            }
            self.emit(&[0x58]); // pop rax
            self.store_rax(p);
        }
    }

    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.seg_succ(b);
        self.pass_args(b, args);
        self.emit(&[0xE9]);
        self.rel32_to(b);
        self.terminated = true;
    }

    /// To then_b if c is not 0, to else_b if it is 0
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.branch_on(0x84, then_b, then_args, else_b, else_args);
    }

    /// On the result of the previous comparison, to else if condition jcc (the byte after 0x0F) holds, to then if it does not
    fn branch_on(&mut self, jcc: u8, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        if else_args.is_empty() {
            // (If nothing is passed to else, jump directly)
            self.seg_succ(else_b);
            self.emit(&[0x0F, jcc]);
            self.rel32_to(else_b);
            self.jump(then_b, then_args);
            return;
        }
        self.emit(&[0x0F, jcc]);
        let jz_at = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.jump(then_b, then_args);
        let here = self.code.len() as i32;
        let rel = here - (jz_at as i32 + 4);
        self.code[jz_at..jz_at + 4].copy_from_slice(&rel.to_le_bytes());
        self.rax_holds = None;
        self.jump(else_b, else_args);
    }

    pub fn return_(&mut self, vals: &[Value]) {
        if let Some(&v) = vals.first() {
            if self.value_type(v).is_float() {
                self.xmm_load(0, v);
            } else {
                self.load_rax(v);
            }
            if self.value_type(v).bytes() == 16 {
                // (The high half is rdx)
                let off = self.off(v);
                self.emit_disp(&[0x48, 0x8B, 0x95], off + 8);
            }
        }
        if let Some(&v) = vals.get(1) {
            // The second is rdx
            let off = self.off(v);
            self.emit_disp(&[0x48, 0x8B, 0x95], off);
        }
        // (Restore the allocated registers)
        for i in 0..5 {
            if self.plan.used & (1 << i) != 0 {
                let d = self.save_slots[i];
                self.emit(&mov_mr(ALLOC_REGS[i], d, true));
            }
        }
        // leave; ret
        self.emit(&[0xC9, 0xC3]);
        self.terminated = true;
    }

    pub fn trap(&mut self) {
        self.emit(&[0x0F, 0x0B]); // ud2
        self.terminated = true;
    }

    /// Calls a fn (up to 6 arguments in registers, the rest on the stack). 0 to 2 return values (rax, rdx)
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.calls.push(op);
        }
        const ARG_LOAD: [[u8; 3]; 6] = [[0x48, 0x8B, 0xBD], [0x48, 0x8B, 0xB5], [0x48, 0x8B, 0x95], [0x48, 0x8B, 0x8D], [0x4C, 0x8B, 0x85], [0x4C, 0x8B, 0x8D]];
        let fargs: Vec<Value> = args.iter().copied().filter(|&a| self.value_type(a).is_float()).collect();
        let args: Vec<Value> = args.iter().copied().filter(|&a| !self.value_type(a).is_float()).collect();
        // Split the 8-byte parts (slot offsets) between registers and the stack (an i128 is 2; if 2 are not left, the whole goes on the stack)
        // (The parts passed in registers are (value, is the high half). If there is a copy, read from it)
        let mut regs: Vec<(Value, bool)> = vec![];
        let mut stack_parts: Vec<i32> = vec![];
        for &a in &args {
            let wide = self.value_type(a).bytes() == 16;
            let n = if wide { 2 } else { 1 };
            if regs.len() + n <= 6 && stack_parts.is_empty() {
                regs.push((a, false));
                if wide {
                    regs.push((a, true));
                }
            } else {
                let off = self.off(a);
                stack_parts.push(off);
                if wide {
                    stack_parts.push(off + 8);
                }
            }
        }
        // (Arguments passed on the stack: align rsp to 16 bytes, then push in reverse order)
        let pad = stack_parts.len() % 2 == 1;
        if pad {
            self.emit(&[0x48, 0x83, 0xEC, 0x08]); // sub rsp, 8
        }
        for &o in stack_parts.iter().rev() {
            self.load_rax_at(o);
            self.emit(&[0x50]);
        }
        const ARG_REG: [u8; 6] = [7, 6, 2, 1, 8, 9];
        // (If an argument's value is in an argument register, writing them in order breaks it with an earlier write: then go through the stack)
        let clash = regs.iter().any(|&(v, hi)| !hi && self.value_type(v).bytes() != 16 && ARG_REG.contains(&self.loc(v)));
        if clash {
            for &(v, hi) in regs.iter() {
                if hi || self.value_type(v).bytes() == 16 {
                    let off = self.off(v) + if hi { 8 } else { 0 };
                    self.load_rax_at(off);
                } else {
                    self.load_reg(0, v);
                }
                self.emit(&[0x50]); // push rax
            }
            for i in (0..regs.len()).rev() {
                let r = ARG_REG[i];
                // pop r
                if r >= 8 {
                    self.emit(&[0x41, 0x58 + (r - 8)]);
                } else {
                    self.emit(&[0x58 + r]);
                }
            }
            self.rax_holds = None;
        } else {
            for (i, &(v, hi)) in regs.iter().enumerate() {
                if hi || self.value_type(v).bytes() == 16 {
                    let off = self.off(v) + if hi { 8 } else { 0 };
                    self.emit_disp(&ARG_LOAD[i], off);
                } else {
                    self.load_reg(ARG_REG[i], v);
                }
            }
        }
        for (k, &a) in fargs.iter().enumerate().take(8) {
            self.xmm_load(k as u8, a);
        }
        // (al = the number passed in xmm, for variadic fns. Our own fns (`_x..`) are not variadic, so it is not needed)
        if !self.funcs[f.0 as usize].starts_with("_x") {
            self.emit(&[0xB8]);
            self.emit(&(fargs.len().min(8) as u32).to_le_bytes());
        }
        self.emit(&[0xE8]);
        let at = self.code.len() as u32;
        self.emit(&[0, 0, 0, 0]);
        self.relocs.push(Reloc { offset: at, name: self.funcs[f.0 as usize].clone(), kind: 4, addend: -4 });
        let n = stack_parts.len() as i32 + pad as i32;
        if n > 0 {
            self.emit_disp(&[0x48, 0x81, 0xC4], 8 * n); // add rsp, 8n
        }
        let mut out = vec![];
        // (two results (a C struct returned in registers): the second integer is in rdx and the second float in xmm1.
        // Each result is read through rax / xmm0, so move them out of the way first: (f, i) keeps the int in rdx,
        // (f, f) keeps xmm1 in rdx and moves it to xmm0 before reading it)
        if let [a, b] = rets {
            match (a.is_float(), b.is_float()) {
                (true, false) => self.emit(&[0x48, 0x89, 0xC2]),       // mov rdx, rax
                (true, true) => self.emit(&[0x66, 0x48, 0x0F, 0x7E, 0xCA]), // movq rdx, xmm1
                _ => {}
            }
        }
        for (i, &t) in rets.iter().enumerate() {
            if t.is_float() {
                if i == 1 && rets[0].is_float() {
                    self.emit(&[0x66, 0x48, 0x0F, 0x6E, 0xC2]); // movq xmm0, rdx
                }
                out.push(self.xmm0_result(t));
                continue;
            }
            if t.bytes() == 16 {
                // rax (low) and rdx (high)
                let v = self.new_value(t);
                let o = self.off(v);
                self.store_rax_at(o);
                self.emit_disp(&[0x48, 0x89, 0x95], o + 8); // mov [rbp+o+8], rdx
                out.push(v);
                continue;
            }
            if i == 1 {
                self.emit(&[0x48, 0x89, 0xD0]); // mov rax, rdx
            }
            out.push(self.result(t));
        }
        out
    }

    /// Finishes assembling: (machine code, relocations). Zeroes the first byte of the zero slots at the entry
    pub fn finish_zeroing(mut self, zero: &[StackSlot]) -> (Vec<u8>, Vec<Reloc>) {
        if !zero.is_empty() {
            let stub = self.code.len();
            for s in zero {
                let off = self.slots[s.0 as usize];
                self.emit_disp(&[0xC6, 0x85], off); // mov byte [rbp+off], 0
                self.emit(&[0]);
            }
            let back = self.entry_jump + 5;
            let at = self.code.len();
            self.emit(&[0xE9]);
            let rel = back as i32 - (at as i32 + 5);
            self.emit(&rel.to_le_bytes());
            let rel = stub as i32 - (self.entry_jump as i32 + 5);
            let j = self.entry_jump;
            self.code[j + 1..j + 5].copy_from_slice(&rel.to_le_bytes());
        }
        self.finish()
    }

    /// Finishes assembling: (machine code, relocations)
    pub fn finish(mut self) -> (Vec<u8>, Vec<Reloc>) {
        // Frame size (16-byte boundary)
        let frame = (self.frame + 15) / 16 * 16;
        self.code[7..11].copy_from_slice(&frame.to_le_bytes());
        for &(at, b) in &self.fixups {
            let target = self.blocks[b.0 as usize].offset.expect("jump to a block that was never switched to") as i32;
            let rel = target - (at as i32 + 4);
            self.code[at..at + 4].copy_from_slice(&rel.to_le_bytes());
            // (A jmp to the very next instruction becomes a 5-byte nop)
            if rel == 0 && at >= 1 && self.code[at - 1] == 0xE9 {
                self.code[at - 1..at + 4].copy_from_slice(&[0x0F, 0x1F, 0x44, 0x00, 0x00]);
            }
        }
        let _ = (&self.params, &self.rets);
        (self.code, self.relocs)
    }
}


// --- Floating point / i128 / overflow / bit operations / indirect calls / addresses ---------------
impl Emitter {
    /// The offset of the value's slot (read from memory / used as an address: writes to that slot cannot be elided)
    fn off(&mut self, v: Value) -> i32 {
        self.ev_use(v.0);
        self.force_mem(v);
        // (A value kept in a register is not in the slot: write it first just in case. Analysis made it mem_only, so this does not normally happen)
        let r = self.loc(v);
        let o = self.values[v.0 as usize].0;
        if r != MEM && r >= 16 {
            // movsd [rbp+o], xmm
            let k = r - 16;
            self.emit(&[0xF2, 0x40 | ((k >> 3) << 2), 0x0F, 0x11, 0x85 | ((k & 7) << 3)]);
            self.emit(&o.to_le_bytes());
        } else if r != MEM {
            self.emit(&mov_mr(r, o, false));
        }
        o
    }

    /// The value's register (a slot if MEM)
    fn loc(&self, v: Value) -> u8 {
        self.plan.vloc.get(v.0 as usize).copied().unwrap_or(MEM)
    }
    fn var_loc(&self, var: Variable) -> u8 {
        self.plan.varloc.get(var.0 as usize).copied().unwrap_or(MEM)
    }
    fn force_mem(&mut self, v: Value) {
        if let Some(tr) = &mut self.track {
            tr.mem_only[v.0 as usize] = true;
        }
    }
    fn ev_use(&mut self, id: u32) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.ev.push((id, op, false));
        }
    }
    fn ev_def(&mut self, id: u32) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.ev.push((id, op, true));
        }
    }
    /// Closes the segment and starts the next (during analysis)
    fn seg_switch(&mut self, b: Block, falls: bool) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.end = op;
                last.falls = falls;
            }
            tr.segs.push(Seg { block: b.0, start: op, end: op, succ: vec![], falls: false });
        }
    }
    fn seg_succ(&mut self, b: Block) {
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.succ.push(b.0);
            }
        }
    }
    pub fn take_track(&mut self) -> Option<Box<Track>> {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.end = op;
            }
        }
        self.track.take()
    }


    /// Loads into xmm0 (reg 0) / xmm1 (reg 1)
    fn xmm_load(&mut self, reg: u8, v: Value) {
        let p = if self.value_type(v) == Type::F32 { 0xF3 } else { 0xF2 };
        // (A value kept in xmm: movaps xmm_reg, xmm_k)
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            self.ev_use(v.0);
            let k = l - 16;
            self.emit(&movaps(reg, k));
            return;
        }
        // (From the slot: rather than off, which keeps it out of registers, only record that it was read)
        self.ev_use(v.0);
        let o = self.values[v.0 as usize].0;
        self.emit_disp(&[p, 0x0F, 0x10, 0x85 | (reg << 3)], o);
    }

    fn xmm0_result(&mut self, t: Type) -> Value {
        let d = self.next_loc();
        if d != MEM && d >= 16 {
            self.emit(&movaps(d - 16, 0));
            return self.result_in_reg(t);
        }
        let v = self.new_value(t);
        self.ev_def(v.0);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        let o = self.values[v.0 as usize].0;
        // (Zero the upper half of the slot before writing: so that an f32 can also be compared in an 8-byte slot)
        self.emit(&[0x31, 0xC0]);
        self.store_rax_at(o);
        self.emit_disp(&[p, 0x0F, 0x11, 0x85], o);
        self.rax_holds = None;
        v
    }

    pub fn f64const(&mut self, x: f64) -> Value {
        self.emit(&[0x48, 0xB8]);
        self.emit(&x.to_bits().to_le_bytes());
        let v = self.new_value(Type::F64);
        self.store_rax(v);
        v
    }

    pub fn f32const(&mut self, x: f32) -> Value {
        self.emit(&[0x48, 0xB8]);
        self.emit(&(x.to_bits() as u64).to_le_bytes());
        let v = self.new_value(Type::F32);
        self.store_rax(v);
        v
    }

    fn fop(&mut self, a: Value, b: Value, opc: u8) -> Value {
        let t = self.value_type(a);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        let l = self.loc(b);
        if l != MEM && l >= 16 {
            // op xmm0, xmm_k
            let k = l - 16;
            self.ev_use(b.0);
            if k >= 8 {
                self.emit(&[p, 0x41, 0x0F, opc, 0xC0 | (k & 7)]);
            } else {
                self.emit(&[p, 0x0F, opc, 0xC0 | k]);
            }
        } else {
            self.ev_use(b.0);
            let o = self.values[b.0 as usize].0;
            self.emit_disp(&[p, 0x0F, opc, 0x85], o);
        }
        self.xmm0_result(t)
    }

    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x58)
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x5C)
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x59)
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x5E)
    }

    pub fn sqrt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        self.emit(&[p, 0x0F, 0x51, 0xC0]);
        self.xmm0_result(t)
    }

    /// Floor / ceil / round toward 0 (roundsd immediates 9 / 10 / 11)
    fn round(&mut self, a: Value, mode: u8) -> Value {
        let t = self.value_type(a);
        self.xmm_load(0, a);
        let op = if t == Type::F32 { 0x0A } else { 0x0B };
        self.emit(&[0x66, 0x0F, 0x3A, op, 0xC0, mode]);
        self.xmm0_result(t)
    }
    pub fn floor(&mut self, a: Value) -> Value {
        self.round(a, 9)
    }
    pub fn ceil(&mut self, a: Value) -> Value {
        self.round(a, 10)
    }
    pub fn trunc(&mut self, a: Value) -> Value {
        self.round(a, 11)
    }

    fn sign_mask(t: Type) -> i64 {
        if t == Type::F32 { 1 << 31 } else { i64::MIN }
    }

    /// Treats the bits of a floating-point value as an integer as they are (I32 / I64)
    fn bits_of(&mut self, a: Value) -> Value {
        let t = if self.value_type(a) == Type::F32 { Type::I32 } else { Type::I64 };
        self.bitcast(t, a)
    }

    pub fn fneg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let x = self.bits_of(a);
        let m = self.iconst(self.value_type(x), Self::sign_mask(t));
        let r = self.bxor(x, m);
        self.bitcast(t, r)
    }

    pub fn fabs(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let x = self.bits_of(a);
        let m = self.iconst(self.value_type(x), !Self::sign_mask(t));
        let r = self.band(x, m);
        self.bitcast(t, r)
    }

    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        let (x, y) = (self.bits_of(a), self.bits_of(b));
        let it = self.value_type(x);
        let m = self.iconst(it, Self::sign_mask(t));
        let nm = self.iconst(it, !Self::sign_mask(t));
        let x2 = self.band(x, nm);
        let y2 = self.band(y, m);
        let r = self.bor(x2, y2);
        self.bitcast(t, r)
    }

    /// a * b + c (rounded twice: not fused)
    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        let m = self.fmul(a, b);
        self.fadd(m, c)
    }

    /// Changes only the type (the bits stay as they are)
    pub fn bitcast(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        if !t.is_float() {
            // (When making it an integer, zero-extend by the width: the upper 4 bytes of an f32's slot are undefined)
            return self.result(t);
        }
        let v = self.new_value(t);
        self.store_rax(v);
        v
    }

    /// Comparison (with a NaN, ordering comparisons other than Equal are false, NotEqual is true)
    pub fn fcmp(&mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        let (x, y) = match cc {
            FloatCC::LessThan | FloatCC::LessThanOrEqual => (b, a),
            _ => (a, b),
        };
        self.xmm_load(0, x);
        let o = self.off(y);
        if t == Type::F32 {
            self.emit_disp(&[0x0F, 0x2E, 0x85], o);
        } else {
            self.emit_disp(&[0x66, 0x0F, 0x2E, 0x85], o);
        }
        match cc {
            FloatCC::GreaterThan | FloatCC::LessThan => self.emit(&[0x0F, 0x97, 0xC0]),
            FloatCC::GreaterThanOrEqual | FloatCC::LessThanOrEqual => self.emit(&[0x0F, 0x93, 0xC0]),
            // sete al; setnp cl; and al, cl
            FloatCC::Equal => self.emit(&[0x0F, 0x94, 0xC0, 0x0F, 0x9B, 0xC1, 0x20, 0xC8]),
            // setne al; setp cl; or al, cl
            FloatCC::NotEqual => self.emit(&[0x0F, 0x95, 0xC0, 0x0F, 0x9A, 0xC1, 0x08, 0xC8]),
            // setp al
            FloatCC::Unordered => self.emit(&[0x0F, 0x9A, 0xC0]),
        }
        self.emit(&[0x48, 0x0F, 0xB6, 0xC0]);
        self.result(Type::I8)
    }

    pub fn fpromote(&mut self, a: Value) -> Value {
        self.xmm_load(0, a);
        self.emit(&[0xF3, 0x0F, 0x5A, 0xC0]);
        self.xmm0_result(Type::F64)
    }

    pub fn fdemote(&mut self, a: Value) -> Value {
        self.xmm_load(0, a);
        self.emit(&[0xF2, 0x0F, 0x5A, 0xC0]);
        self.xmm0_result(Type::F32)
    }

    /// Signed integer → floating point
    pub fn fcvt_from_sint(&mut self, t: Type, a: Value) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        self.sext_rax(st);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC0]);
        self.xmm0_result(t)
    }

    /// Unsigned integer → floating point (for 64 bit with the top bit set, halve and then double)
    pub fn fcvt_from_uint(&mut self, t: Type, a: Value) -> Value {
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        let add = if t == Type::F32 { 0x58 } else { 0x58 };
        self.load_rax(a);
        // test rax, rax; js big
        self.emit(&[0x48, 0x85, 0xC0, 0x78, 0x07]);
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC0]); // 5 byte
        self.emit(&[0xEB, 0x15]); // jmp done (2 byte)
        // big: mov rcx, rax; shr rcx, 1; and eax, 1; or rcx, rax; cvtsi2sd xmm0, rcx; addsd xmm0, xmm0
        self.emit(&[0x48, 0x89, 0xC1, 0x48, 0xD1, 0xE9, 0x83, 0xE0, 0x01, 0x48, 0x09, 0xC1]); // 12
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC1]); // 5
        self.emit(&[p, 0x0F, add, 0xC0]); // 4
        self.xmm0_result(t)
    }

    /// Floating point → I64 (cvttsd2si: out of range and NaN give i64::MIN)
    fn cvt_raw(&mut self, a: Value) -> Value {
        let p = if self.value_type(a) == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        self.emit(&[p, 0x48, 0x0F, 0x2C, 0xC0]);
        let v = self.new_value(Type::I64);
        self.store_rax(v);
        v
    }

    fn fconst_like(&mut self, a: Value, x: f64) -> Value {
        if self.value_type(a) == Type::F32 { self.f32const(x as f32) } else { self.f64const(x) }
    }

    /// Floating point → signed integer (Rust's `as`: saturates, NaN is 0)
    pub fn fcvt_to_sint_sat(&mut self, t: Type, a: Value) -> Value {
        let n = t.bits() as i32;
        let lo = -(2f64.powi(n - 1));
        let hi = 2f64.powi(n - 1);
        let raw = self.cvt_raw(a);
        let flo = self.fconst_like(a, lo);
        let fhi = self.fconst_like(a, hi);
        let too_low = self.fcmp(FloatCC::LessThan, a, flo);
        let too_high = self.fcmp(FloatCC::GreaterThanOrEqual, a, fhi);
        let nan = self.fcmp(FloatCC::NotEqual, a, a);
        let ilo = self.iconst(Type::I64, if n == 64 { i64::MIN } else { -(1i64 << (n - 1)) });
        let ihi = self.iconst(Type::I64, if n == 64 { i64::MAX } else { (1i64 << (n - 1)) - 1 });
        let z = self.iconst(Type::I64, 0);
        let r1 = self.select(too_high, ihi, raw);
        let r2 = self.select(too_low, ilo, r1);
        let r3 = self.select(nan, z, r2);
        self.ireduce(t, r3)
    }

    /// Floating point → unsigned integer (saturates, NaN and negatives are 0)
    pub fn fcvt_to_uint_sat(&mut self, t: Type, a: Value) -> Value {
        let n = t.bits() as i32;
        let hi = 2f64.powi(n);
        let fz = self.fconst_like(a, 0.0);
        let fhi = self.fconst_like(a, hi);
        let f63 = self.fconst_like(a, 2f64.powi(63));
        // (For 2^63 and above, subtract 2^63, convert, and set the top bit)
        let raw_small = self.cvt_raw(a);
        let a2 = self.fsub(a, f63);
        let raw_big0 = self.cvt_raw(a2);
        let top = self.iconst(Type::I64, i64::MIN);
        let raw_big = self.bxor(raw_big0, top);
        let big = self.fcmp(FloatCC::GreaterThanOrEqual, a, f63);
        let raw = self.select(big, raw_big, raw_small);
        let too_low = self.fcmp(FloatCC::LessThan, a, fz);
        let too_high = self.fcmp(FloatCC::GreaterThanOrEqual, a, fhi);
        let nan = self.fcmp(FloatCC::NotEqual, a, a);
        let ihi = self.iconst(Type::I64, if n == 64 { -1 } else { ((1u64 << n) - 1) as i64 });
        let z = self.iconst(Type::I64, 0);
        let r1 = self.select(too_high, ihi, raw);
        let r2 = self.select(too_low, z, r1);
        let r3 = self.select(nan, z, r2);
        self.ireduce(t, r3)
    }

    // --- i128 ---------------------------------------------------------

    /// i128 from (low, high)
    pub fn iconcat(&mut self, lo: Value, hi: Value) -> Value {
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.load_rax(lo);
        self.store_rax_at(o);
        self.load_rax(hi);
        self.store_rax_at(o + 8);
        v
    }

    /// (low, high) of an i128
    pub fn isplit(&mut self, v: Value) -> (Value, Value) {
        let o = self.off(v);
        self.load_rax_at(o);
        let lo = self.new_value(Type::I64);
        self.store_rax(lo);
        self.load_rax_at(o + 8);
        let hi = self.new_value(Type::I64);
        self.store_rax(hi);
        (lo, hi)
    }

    /// Two-word operation: rax = a.lo OP1 b.lo, rcx = a.hi OP2 b.hi (OP is a 2-byte opcode of the form that takes [rbp + d])
    fn wide2(&mut self, a: Value, b: Value, op_lo: [u8; 2], op_hi: [u8; 2]) -> Value {
        let (oa, ob) = (self.off(a), self.off(b));
        self.load_rax_at(oa);
        self.emit_disp(&[op_lo[0], op_lo[1], 0x85], ob);
        // (adc / sbb use the carry of the operation above, and mov does not change the flags)
        self.emit_disp(&[0x48, 0x8B, 0x8D], oa + 8);
        self.emit_disp(&[op_hi[0], op_hi[1], 0x8D], ob + 8);
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        self.emit_disp(&[0x48, 0x89, 0x8D], o + 8);
        v
    }

    pub fn iadd128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x03], [0x48, 0x13])
    }
    pub fn isub128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x2B], [0x48, 0x1B])
    }
    pub fn band128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x23], [0x48, 0x23])
    }
    pub fn bor128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x0B], [0x48, 0x0B])
    }
    pub fn bxor128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x33], [0x48, 0x33])
    }

    /// Binary i128 operations with libgcc fns (`__multi3`, etc.)
    fn wide_libcall(&mut self, name: &str, a: Value, b: Value) -> Value {
        let (al, ah) = self.isplit(a);
        let (bl, bh) = self.isplit(b);
        let f = self.import_function(name);
        let r = self.call(f, &[al, ah, bl, bh], &[Type::I64, Type::I64]);
        self.iconcat(r[0], r[1])
    }
    pub fn imul128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__multi3", a, b)
    }
    pub fn udiv128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__udivti3", a, b)
    }
    pub fn sdiv128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__divti3", a, b)
    }
    pub fn urem128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__umodti3", a, b)
    }
    pub fn srem128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__modti3", a, b)
    }

    /// i128 comparison
    pub fn icmp128(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let (oa, ob) = (self.off(a), self.off(b));
        match cc {
            IntCC::Equal | IntCC::NotEqual => {
                self.load_rax_at(oa);
                self.emit_disp(&[0x48, 0x33, 0x85], ob); // xor rax, [b]
                self.emit_disp(&[0x48, 0x8B, 0x8D], oa + 8);
                self.emit_disp(&[0x48, 0x33, 0x8D], ob + 8); // xor rcx, [b+8]
                self.emit(&[0x48, 0x09, 0xC8]); // or rax, rcx
                let set = if cc == IntCC::Equal { 0x94 } else { 0x95 };
                self.emit(&[0x0F, set, 0xC0, 0x48, 0x0F, 0xB6, 0xC0]);
                self.result(Type::I8)
            }
            _ => {
                // x < y with the flags of sub / sbb: (a < b), b < a, !(a < b), !(b < a)
                let (x, y, neg) = match cc {
                    IntCC::SignedLessThan | IntCC::UnsignedLessThan => (oa, ob, false),
                    IntCC::SignedGreaterThan | IntCC::UnsignedGreaterThan => (ob, oa, false),
                    IntCC::SignedGreaterThanOrEqual | IntCC::UnsignedGreaterThanOrEqual => (oa, ob, true),
                    _ => (ob, oa, true),
                };
                self.load_rax_at(x);
                self.emit_disp(&[0x48, 0x2B, 0x85], y); // sub rax, [y]
                self.load_rax_at(x + 8);
                self.emit_disp(&[0x48, 0x1B, 0x85], y + 8); // sbb rax, [y+8]
                let set = if cc.signed() { if neg { 0x9D } else { 0x9C } } else if neg { 0x93 } else { 0x92 };
                self.emit(&[0x0F, set, 0xC0, 0x48, 0x0F, 0xB6, 0xC0]);
                self.result(Type::I8)
            }
        }
    }

    /// i128 shift (the amount is the low 7 bits)
    fn shift128(&mut self, a: Value, b: Value, kind: u8) -> Value {
        let oa = self.off(a);
        self.load_rax_at(oa);
        self.emit_disp(&[0x48, 0x8B, 0x95], oa + 8); // mov rdx, [a+8]
        self.load_rcx(b);
        match kind {
            // shld rdx, rax, cl; shl rax, cl; test cl, 64; jz +5; mov rdx, rax; xor eax, eax
            0 => self.emit(&[0x48, 0x0F, 0xA5, 0xC2, 0x48, 0xD3, 0xE0, 0xF6, 0xC1, 0x40, 0x74, 0x05, 0x48, 0x89, 0xC2, 0x31, 0xC0]),
            // shrd rax, rdx, cl; shr rdx, cl; test cl, 64; jz +5; mov rax, rdx; xor edx, edx
            1 => self.emit(&[0x48, 0x0F, 0xAD, 0xD0, 0x48, 0xD3, 0xEA, 0xF6, 0xC1, 0x40, 0x74, 0x05, 0x48, 0x89, 0xD0, 0x31, 0xD2]),
            // shrd rax, rdx, cl; sar rdx, cl; test cl, 64; jz +7; mov rax, rdx; sar rdx, 63
            _ => self.emit(&[0x48, 0x0F, 0xAD, 0xD0, 0x48, 0xD3, 0xFA, 0xF6, 0xC1, 0x40, 0x74, 0x07, 0x48, 0x89, 0xD0, 0x48, 0xC1, 0xFA, 0x3F]),
        }
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        self.emit_disp(&[0x48, 0x89, 0x95], o + 8); // mov [v+8], rdx
        v
    }
    pub fn ishl128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 0)
    }
    pub fn ushr128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 1)
    }
    pub fn sshr128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 2)
    }

    /// Integer → i128 (zero-extended / sign-extended)
    pub fn extend128(&mut self, a: Value, signed: bool) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        if signed {
            self.sext_rax(st);
        }
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        if signed {
            self.emit(&[0x48, 0xC1, 0xF8, 0x3F]); // sar rax, 63
        } else {
            self.emit(&[0x31, 0xC0]);
        }
        self.store_rax_at(o + 8);
        v
    }

    /// Truncates the low half of an i128 to the type's width
    pub fn reduce128(&mut self, t: Type, a: Value) -> Value {
        let o = self.off(a);
        self.load_rax_at(o);
        self.result(t)
    }

    // --- Operations with overflow: (result, whether it overflowed (I8)) -------------------------

    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.iadd(a, b);
        let of = self.icmp(IntCC::UnsignedLessThan, v, a);
        (v, of)
    }

    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.isub(a, b);
        let of = self.icmp(IntCC::UnsignedLessThan, a, b);
        (v, of)
    }

    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.iadd(a, b);
        let x = self.bxor(a, v);
        let y = self.bxor(b, v);
        let m = self.band(x, y);
        let of = self.icmp_imm(IntCC::SignedLessThan, m, 0);
        (v, of)
    }

    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.isub(a, b);
        let x = self.bxor(a, b);
        let y = self.bxor(a, v);
        let m = self.band(x, y);
        let of = self.icmp_imm(IntCC::SignedLessThan, m, 0);
        (v, of)
    }

    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        if t == Type::I64 {
            self.load_rax(a);
            self.load_rcx(b);
            self.emit(&[0x48, 0xF7, 0xE1]); // mul rcx (CF / OF = the upper half is not 0)
            self.emit(&[0x0F, 0x92, 0xC1]); // setc cl
            let v = self.new_value(Type::I64);
            self.store_rax(v);
            self.emit(&[0x48, 0x0F, 0xB6, 0xC1]); // movzx rax, cl
            let of = self.result(Type::I8);
            return (v, of);
        }
        // Narrow types: multiply in 64 bit and see whether higher digits remain
        let (xa, xb) = (self.uextend(Type::I64, a), self.uextend(Type::I64, b));
        let p = self.imul(xa, xb);
        let sh = self.iconst(Type::I64, t.bits() as i64);
        let hi = self.ushr(p, sh);
        let of = self.icmp_imm(IntCC::NotEqual, hi, 0);
        let v = self.ireduce(t, p);
        (v, of)
    }

    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        if t == Type::I64 {
            self.load_rax(a);
            self.load_rcx(b);
            self.emit(&[0x48, 0x0F, 0xAF, 0xC1]); // imul rax, rcx
            self.emit(&[0x0F, 0x90, 0xC1]); // seto cl
            let v = self.new_value(Type::I64);
            self.store_rax(v);
            self.emit(&[0x48, 0x0F, 0xB6, 0xC1]);
            let of = self.result(Type::I8);
            return (v, of);
        }
        let (xa, xb) = (self.sextend(Type::I64, a), self.sextend(Type::I64, b));
        let p = self.imul(xa, xb);
        let v = self.ireduce(t, p);
        let back = self.sextend(Type::I64, v);
        let of = self.icmp(IntCC::NotEqual, back, p);
        (v, of)
    }

    /// The upper half of an unsigned 64-bit multiplication
    pub fn umulhi(&mut self, a: Value, b: Value) -> Value {
        self.load_rax(a);
        self.load_rcx(b);
        self.emit(&[0x48, 0xF7, 0xE1, 0x48, 0x89, 0xD0]); // mul rcx; mov rax, rdx
        self.result(Type::I64)
    }

    // --- Bit operations / min and max -----------------------------------------------

    /// Number of leading zeros (the width if 0)
    pub fn clz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0x0F, 0xBD, 0xC0]); // bsr rax, rax (ZF if 0)
        // jnz +7; mov rax, -1 (7 byte)
        self.emit(&[0x75, 0x07, 0x48, 0xC7, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF]);
        // rax = (bits - 1) - rax
        self.emit(&[0x48, 0xF7, 0xD8]); // neg rax
        self.emit(&[0x48, 0x83, 0xC0, (t.bits() - 1) as u8]); // add rax, bits - 1
        self.result(t)
    }

    /// Number of trailing zeros (the width if 0)
    pub fn ctz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0x0F, 0xBC, 0xC0]); // bsf rax, rax
        // jnz +7; mov rax, bits
        self.emit(&[0x75, 0x07, 0x48, 0xC7, 0xC0]);
        self.emit(&(t.bits()).to_le_bytes());
        self.result(t)
    }

    pub fn popcnt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0xF3, 0x48, 0x0F, 0xB8, 0xC0]);
        self.result(t)
    }

    pub fn bswap(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        // (i128: the low half of the result is the high half's bytes reversed, the high half the low half's)
        if t == Type::I128 {
            let (lo, hi) = self.isplit(a);
            let (l, h) = (self.bswap(hi), self.bswap(lo));
            return self.iconcat(l, h);
        }
        self.load_rax(a);
        match t {
            Type::I16 => self.emit(&[0x66, 0xC1, 0xC0, 0x08]), // rol ax, 8
            Type::I32 => self.emit(&[0x0F, 0xC8]),
            _ => self.emit(&[0x48, 0x0F, 0xC8]),
        }
        self.result(t)
    }

    // --- Atomics (all sequentially consistent: x86-64 loads are acquire, and xchg / lock-prefixed instructions are
    // full barriers) -------------------------------------------------------------------------------------------------

    /// The operand-size prefix and REX.W of an integer of the type (the 8-bit forms are separate opcodes)
    fn atomic_width(t: Type) -> (&'static [u8], &'static [u8]) {
        match t {
            Type::I64 => (&[], &[0x48]),
            Type::I16 => (&[0x66], &[]),
            _ => (&[], &[]),
        }
    }

    /// Loads [addr] (mov: an aligned load is atomic)
    pub fn atomic_load(&mut self, t: Type, addr: Value) -> Value {
        self.load_rax(addr);
        match t {
            Type::I64 => self.emit(&[0x48, 0x8B, 0x00]),
            Type::I32 => self.emit(&[0x8B, 0x00]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xB7, 0x00]),
            _ => self.emit(&[0x48, 0x0F, 0xB6, 0x00]),
        }
        self.result(t)
    }

    /// rax = addr, rcx = x (in an order that keeps a value rax holds)
    fn atomic_args(&mut self, addr: Value, x: Value) {
        if self.rax_is(x) && x != addr {
            self.load_rcx(x);
            self.load_rax(addr);
        } else {
            self.load_rax(addr);
            self.load_rcx(x);
        }
    }

    /// xchg [rax], rcx (implicitly locked)
    fn xchg_rax_rcx(&mut self, t: Type) {
        let (p, w) = Self::atomic_width(t);
        self.emit(p);
        self.emit(w);
        self.emit(if t == Type::I8 { &[0x86, 0x08] } else { &[0x87, 0x08] });
    }

    /// Writes v to [addr] (xchg, a store that is also a full barrier)
    pub fn atomic_store(&mut self, v: Value, addr: Value) {
        let t = self.value_type(v);
        self.atomic_args(addr, v);
        self.xchg_rax_rcx(t);
        self.rax_holds = None;
    }

    /// The old value of [addr], replaced by `op(old, x)`
    pub fn atomic_rmw(&mut self, op: AtomicOp, addr: Value, x: Value) -> Value {
        let t = self.value_type(x);
        self.atomic_args(addr, x);
        match op {
            AtomicOp::Xchg => self.xchg_rax_rcx(t),
            AtomicOp::Add => {
                // lock xadd [rax], rcx
                let (p, w) = Self::atomic_width(t);
                self.emit(p);
                self.emit(&[0xF0]);
                self.emit(w);
                self.emit(if t == Type::I8 { &[0x0F, 0xC0, 0x08] } else { &[0x0F, 0xC1, 0x08] });
            }
        }
        self.emit(&[0x48, 0x89, 0xC8]); // mov rax, rcx
        self.result(t)
    }

    /// The old value of [addr], which is replaced by new when it is expected
    pub fn atomic_cas(&mut self, addr: Value, expected: Value, new: Value) -> Value {
        let t = self.value_type(new);
        // (rdx = addr, rcx = new, rax = expected: each from its own place, which loading another does not touch)
        self.load_reg(2, addr);
        self.load_reg(1, new);
        self.load_reg(0, expected);
        // lock cmpxchg [rdx], rcx
        let (p, w) = Self::atomic_width(t);
        self.emit(p);
        self.emit(&[0xF0]);
        self.emit(w);
        self.emit(if t == Type::I8 { &[0x0F, 0xB0, 0x0A] } else { &[0x0F, 0xB1, 0x0A] });
        self.result(t)
    }

    pub fn fence(&mut self) {
        self.emit(&[0x0F, 0xAE, 0xF0]); // mfence
    }

    fn rot(&mut self, a: Value, b: Value, ext: u8) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        match t {
            Type::I8 => self.emit(&[0xD2, 0xC0 | ext]),
            Type::I16 => self.emit(&[0x66, 0xD3, 0xC0 | ext]),
            Type::I32 => self.emit(&[0xD3, 0xC0 | ext]),
            _ => self.emit(&[0x48, 0xD3, 0xC0 | ext]),
        }
        self.result(t)
    }
    pub fn rotl(&mut self, a: Value, b: Value) -> Value {
        self.rot(a, b, 0x00)
    }
    pub fn rotr(&mut self, a: Value, b: Value) -> Value {
        self.rot(a, b, 0x08)
    }

    fn minmax(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let c = self.icmp(cc, a, b);
        self.select(c, a, b)
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::SignedLessThan, a, b)
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::UnsignedLessThan, a, b)
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::SignedGreaterThan, a, b)
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::UnsignedGreaterThan, a, b)
    }

    pub fn iabs(&mut self, a: Value) -> Value {
        let n = self.ineg(a);
        let c = self.icmp_imm(IntCC::SignedLessThan, a, 0);
        self.select(c, n, a)
    }

    /// Stops if c is 0 (trapz) / not 0 (trapnz)
    fn trap_if(&mut self, c: Value, on_zero: bool) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]);
        self.emit(&[if on_zero { 0x75 } else { 0x74 }, 0x02, 0x0F, 0x0B]); // jnz/jz +2; ud2
    }
    /// A trap with a code (records the location in traps())
    pub fn trap_code(&mut self, code: u8) {
        self.traps.push((self.code.len() as u32, code));
        self.trap();
    }
    fn trap_if_code(&mut self, c: Value, on_zero: bool, code: u8) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]);
        self.emit(&[if on_zero { 0x75 } else { 0x74 }, 0x02]);
        self.traps.push((self.code.len() as u32, code));
        self.emit(&[0x0F, 0x0B]);
    }
    pub fn trapz_code(&mut self, c: Value, code: u8) {
        self.trap_if_code(c, true, code)
    }
    pub fn trapnz_code(&mut self, c: Value, code: u8) {
        self.trap_if_code(c, false, code)
    }
    /// (offset, code) of the traps so far
    pub fn traps(&self) -> Vec<(u32, u8)> {
        self.traps.clone()
    }
    pub fn trapz(&mut self, c: Value) {
        self.trap_if(c, true)
    }
    pub fn trapnz(&mut self, c: Value) {
        self.trap_if(c, false)
    }

    // --- Indirect calls / addresses --------------------------------------------------

    /// Calls fn pointer p
    pub fn call_indirect(&mut self, p: Value, args: &[Value], rets: &[Type]) -> Vec<Value> {
        // (Load it into r11 beforehand and call it at the point of the call: the argument registers get filled by the work in call())
        let f = self.import_function("\0indirect");
        let n = self.relocs.len();
        let o = self.off(p);
        self.emit_disp(&[0x4C, 0x8B, 0x9D], o); // mov r11, [p]
        let out = self.call(f, args, rets);
        // Rewrite the `call rel32` (E8 + 4) that call() wrote into `call r11` + nop
        let r = self.relocs.remove(n);
        let at = r.offset as usize;
        self.code[at - 1..at + 4].copy_from_slice(&[0x41, 0xFF, 0xD3, 0x90, 0x90]);
        out
    }

    /// The address of a name (fn or data) (via the GOT: works for external names and in PIE too)
    pub fn symbol_addr(&mut self, name: &str) -> Value {
        self.emit(&[0x48, 0x8B, 0x05]); // mov rax, [rip + rel32]
        let at = self.code.len() as u32;
        self.emit(&[0, 0, 0, 0]);
        self.relocs.push(Reloc { offset: at, name: name.to_string(), kind: 9, addend: -4 });
        let v = self.new_value(Type::I64);
        self.store_rax(v);
        v
    }

    // --- SIMD (issue #30) ---------------------------------------------------
    // A 128-bit vector is 16 bytes in its slot, moved like an i128 (so loads, stores, variables and block parameters
    // are right). minclif has no vector instructions yet: a SIMD op is `ud2`, and codegen's lower rejects a
    // function with a vector before it is emitted natively (the wasm backend and Cranelift read the recording).

    /// a 128-bit vector constant
    pub fn vconst(&mut self, x: u128) -> Value {
        let v = self.new_value(Type::Vec(VBits::B128));
        let o = self.off(v);
        for k in 0..2 {
            let w = (x >> (64 * k)) as u64;
            self.emit(&[0x48, 0xB8]);
            self.emit(&w.to_le_bytes());
            self.store_rax_at(o + 8 * k);
        }
        v
    }

    /// a SIMD op: not supported here (`ud2`)
    pub fn simd(&mut self, s: Shape, op: VOp, args: &[Value]) -> Value {
        for &a in args {
            self.load_rax(a);
        }
        self.emit(&[0x0F, 0x0B]);
        let t = op.result(s);
        let v = self.new_value(t);
        if t.bytes() <= 8 {
            self.store_rax(v);
        }
        v
    }
}

// --- ELF64 relocatable objects ------------------------------------------

/// Data (name, contents, relocations (offset, name pointed to, addend), visible from outside). Pointers in it are R_X86_64_64
pub type DataDef = (String, Vec<u8>, Vec<(u32, String, i64)>, bool);

/// An ELF .o with fns (name, machine code, relocations, visible from outside) laid out in .text
pub fn write_object(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)]) -> Vec<u8> {
    write_object_full(funcs, &[])
}

/// An ELF .o with fns in .text and data in .data
pub fn write_object_full(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)], datas: &[DataDef]) -> Vec<u8> {
    let pad = |v: &mut Vec<u8>, a: usize, fill: u8| {
        while v.len() % a != 0 {
            v.push(fill);
        }
    };
    let mut text: Vec<u8> = vec![];
    let mut offs = vec![];
    for (_, code, _, _) in funcs {
        pad(&mut text, 16, 0xCC);
        offs.push(text.len());
        text.extend_from_slice(code);
    }
    let mut data: Vec<u8> = vec![];
    let mut doffs = vec![];
    for (_, bytes, _, _) in datas {
        pad(&mut data, 16, 0);
        doffs.push(data.len());
        data.extend_from_slice(bytes);
    }
    // Symbols: 0 = null, 1 = .text, 2 = .data section symbols, locals, globals, undefined
    let mut strtab: Vec<u8> = vec![0];
    let add_str = |s: &str, t: &mut Vec<u8>| -> u32 {
        let o = t.len() as u32;
        t.extend_from_slice(s.as_bytes());
        t.push(0);
        o
    };
    // (name, info, shndx, value, size)
    let mut syms: Vec<(u32, u8, u16, u64, u64)> = vec![(0, 0, 0, 0, 0), (0, 3, 1, 0, 0), (0, 3, 2, 0, 0)];
    let mut index: FMap<String, u32> = FMap::default();
    for pass in 0..2 {
        let global = pass == 1;
        for (i, (name, code, _, g)) in funcs.iter().enumerate() {
            if *g == global {
                let n = add_str(name, &mut strtab);
                syms.push((n, ((global as u8) << 4) | 2, 1, offs[i] as u64, code.len() as u64));
                index.insert(name.clone(), syms.len() as u32 - 1);
            }
        }
        for (i, (name, bytes, _, g)) in datas.iter().enumerate() {
            if *g == global {
                let n = add_str(name, &mut strtab);
                syms.push((n, ((global as u8) << 4) | 1, 2, doffs[i] as u64, bytes.len() as u64));
                index.insert(name.clone(), syms.len() as u32 - 1);
            }
        }
    }
    let first_global = syms.iter().position(|s| s.1 >> 4 == 1).unwrap_or(syms.len()) as u32;
    let undefined = |name: &str, syms: &mut Vec<(u32, u8, u16, u64, u64)>, index: &mut FMap<String, u32>, strtab: &mut Vec<u8>| {
        if !index.contains_key(name) {
            let n = add_str(name, strtab);
            syms.push((n, 1 << 4, 0, 0, 0));
            index.insert(name.to_string(), syms.len() as u32 - 1);
        }
    };
    for (_, _, rs, _) in funcs {
        for r in rs {
            undefined(&r.name, &mut syms, &mut index, &mut strtab);
        }
    }
    for (_, _, rs, _) in datas {
        for r in rs {
            undefined(&r.1, &mut syms, &mut index, &mut strtab);
        }
    }
    let first_global = first_global.min(syms.len() as u32);
    let mut symtab: Vec<u8> = vec![];
    for &(name, info, shndx, value, size) in &syms {
        symtab.extend_from_slice(&name.to_le_bytes());
        symtab.push(info);
        symtab.push(0);
        symtab.extend_from_slice(&shndx.to_le_bytes());
        symtab.extend_from_slice(&value.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
    }
    let sym_of = |name: &str| -> u64 { index[name] as u64 };
    let rela_entry = |out: &mut Vec<u8>, off: u64, sym: u64, kind: u64, addend: i64| {
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&((sym << 32) | kind).to_le_bytes());
        out.extend_from_slice(&addend.to_le_bytes());
    };
    let mut rela_text: Vec<u8> = vec![];
    for (i, (_, _, rs, _)) in funcs.iter().enumerate() {
        for r in rs {
            rela_entry(&mut rela_text, offs[i] as u64 + r.offset as u64, sym_of(&r.name), r.kind as u64, r.addend);
        }
    }
    let mut rela_data: Vec<u8> = vec![];
    for (i, (_, _, rs, _)) in datas.iter().enumerate() {
        for (o, name, addend) in rs {
            rela_entry(&mut rela_data, doffs[i] as u64 + *o as u64, sym_of(name), 1, *addend);
        }
    }
    // (Name positions: .text 1, .data 7, .symtab 13, .strtab 21, .rela.text 29, .rela.data 40, .shstrtab 51, .note.GNU-stack 61)
    let shstr = b"\0.text\0.data\0.symtab\0.strtab\0.rela.text\0.rela.data\0.shstrtab\0.note.GNU-stack\0".to_vec();
    let mut out = vec![0u8; 64];
    pad(&mut out, 16, 0);
    let text_off = out.len();
    out.extend_from_slice(&text);
    pad(&mut out, 16, 0);
    let data_off = out.len();
    out.extend_from_slice(&data);
    pad(&mut out, 8, 0);
    let sym_off = out.len();
    out.extend_from_slice(&symtab);
    let str_off = out.len();
    out.extend_from_slice(&strtab);
    pad(&mut out, 8, 0);
    let rt_off = out.len();
    out.extend_from_slice(&rela_text);
    let rd_off = out.len();
    out.extend_from_slice(&rela_data);
    let shstr_off = out.len();
    out.extend_from_slice(&shstr);
    pad(&mut out, 8, 0);
    let sh_off = out.len();
    let sh = |o: &mut Vec<u8>, name: u32, ty: u32, flags: u64, off: usize, size: usize, link: u32, info: u32, al: u64, ent: u64| {
        for x in [name, ty] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [flags, 0, off as u64, size as u64] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [link, info] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [al, ent] {
            o.extend_from_slice(&x.to_le_bytes());
        }
    };
    // 0 null, 1 .text, 2 .data, 3 .symtab, 4 .strtab, 5 .rela.text, 6 .rela.data, 7 .shstrtab, 8 .note.GNU-stack
    sh(&mut out, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    sh(&mut out, 1, 1, 6, text_off, text.len(), 0, 0, 16, 0);
    sh(&mut out, 7, 1, 3, data_off, data.len(), 0, 0, 16, 0);
    sh(&mut out, 13, 2, 0, sym_off, symtab.len(), 4, first_global, 8, 24);
    sh(&mut out, 21, 3, 0, str_off, strtab.len(), 0, 0, 1, 0);
    sh(&mut out, 29, 4, 0x40, rt_off, rela_text.len(), 3, 1, 8, 24);
    sh(&mut out, 40, 4, 0x40, rd_off, rela_data.len(), 3, 2, 8, 24);
    sh(&mut out, 51, 3, 0, shstr_off, shstr.len(), 0, 0, 1, 0);
    sh(&mut out, 61, 1, 0, shstr_off, 0, 0, 0, 1, 0);
    let mut h: Vec<u8> = vec![0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    h.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    h.extend_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    h.extend_from_slice(&1u32.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(&(sh_off as u64).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    for x in [64u16, 0, 0, 64, 9, 7] {
        h.extend_from_slice(&x.to_le_bytes());
    }
    out[..64].copy_from_slice(&h);
    out
}

// --- Register allocation -------------------------------------------------------------

/// Allocates values / variables to registers from the analysis record (linear scan).
/// A live range is a range of instruction numbers [first, last]: a value written and read only within one segment (a range where a block was written contiguously) stays as is,
/// and one that crosses segments is checked for being live at segment entries / exits (backward dataflow) and extended to the ends of the segments where it is live
/// paddr: the slot address per value, nslots: the number of slots, zero: slots zeroed at the entry (not made variables)
/// Deciding which slot fields to make variables (SROA): (per slot, whether to make it variables, (slot, offset) → field number, field types).
/// Slots whose address is used only by load / store / iadd_imm, whose reads and writes do not overlap, and whose types match. clif_backend uses it too
pub fn sroa(tr: &Track, nvals: usize, paddr: &[Option<(u32, i32)>], nslots: usize, zero: &[u32]) -> (Vec<bool>, FMap<(u32, i32), u32>, Vec<Type>) {
    let mut prom = vec![std::env::var_os("MINCLIF_NO_SROA").is_none(); nslots];
    // (Variables holding a slot address: unless all their definitions are the same address, that slot is not made variables)
    {
        let mut st: FMap<u32, Option<(u32, i32)>> = FMap::default();
        let mut bad: FSet<u32> = FSet::default();
        for &(var, pa) in &tr.var_defs {
            match (st.get(&var), pa) {
                (_, None) => {
                    bad.insert(var);
                }
                (None, Some(a)) => {
                    st.insert(var, Some(a));
                }
                (Some(&Some(b)), Some(a)) if a == b => {}
                _ => {
                    bad.insert(var);
                }
            }
        }
        for &(var, pa) in &tr.var_defs {
            if let (true, Some((sl, _))) = (bad.contains(&var), pa) {
                if (sl as usize) < nslots {
                    prom[sl as usize] = false;
                }
            }
        }
    }
    for &z in zero {
        if (z as usize) < nslots {
            prom[z as usize] = false;
        }
    }
    let mut reads = vec![0u32; nvals];
    for &(id, _, def) in &tr.ev {
        if !def && id < VAR_BASE && (id as usize) < nvals {
            reads[id as usize] += 1;
        }
    }
    for v in 0..nvals.min(paddr.len()) {
        if let Some((sl, _)) = paddr[v] {
            if reads[v] != tr.addr_uses.get(v).copied().unwrap_or(0) {
                prom[sl as usize] = false;
            }
        }
    }
    let mut ftype: FMap<(u32, i32), Type> = FMap::default();
    for &(_, sl, off, t, _) in &tr.acc {
        let su = sl as usize;
        if !prom[su] {
            continue;
        }
        // (an i128 or a vector stays in memory)
        if t.bytes() >= 16 || off < 0 {
            prom[su] = false;
            continue;
        }
        match ftype.get(&(sl, off)) {
            Some(&t0) if t0 != t => prom[su] = false,
            Some(_) => {}
            None => {
                ftype.insert((sl, off), t);
            }
        }
    }
    // (Overlaps: per slot, look at them sorted by offset)
    let mut keys: Vec<(u32, i32)> = ftype.keys().copied().filter(|k| prom[k.0 as usize]).collect();
    keys.sort();
    for w in 1..keys.len() {
        let (a, b) = (keys[w - 1], keys[w]);
        if a.0 == b.0 && a.1 + ftype[&a].bytes() as i32 > b.1 {
            prom[a.0 as usize] = false;
        }
    }
    let mut fields: FMap<(u32, i32), u32> = FMap::default();
    let mut ftys: Vec<Type> = vec![];
    for k in &keys {
        if prom[k.0 as usize] {
            fields.insert(*k, ftys.len() as u32);
            ftys.push(ftype[k]);
        }
    }
    (prom, fields, ftys)
}

pub fn plan(tr: &Track, nvals: usize, nvars: usize, paddr: &[Option<(u32, i32)>], nslots: usize, zero: &[u32]) -> Plan {
    let (prom, fields, ftys) = sroa(tr, nvals, paddr, nslots, zero);
    let nf = ftys.len();
    // (Add the field reads / writes to the read / write record: keeping instruction order)
    let mut ev: Vec<(u32, u32, bool)> = Vec::with_capacity(tr.ev.len() + tr.acc.len());
    {
        let mut k = 0;
        for &(op, sl, off, _, st) in &tr.acc {
            let Some(&f) = fields.get(&(sl, off)) else { continue };
            while k < tr.ev.len() && tr.ev[k].1 <= op {
                ev.push(tr.ev[k]);
                k += 1;
            }
            ev.push((FIELD_BASE + f, op, st));
        }
        while k < tr.ev.len() {
            ev.push(tr.ev[k]);
            k += 1;
        }
    }
    let n = nvals + nvars + nf;
    let idx = |id: u32| -> usize {
        if id >= FIELD_BASE {
            nvals + nvars + (id - FIELD_BASE) as usize
        } else if id >= VAR_BASE {
            nvals + (id - VAR_BASE) as usize
        } else {
            id as usize
        }
    };
    let cand = |i: usize| -> bool {
        if i < nvals {
            // (Not needed, since the addresses of slots made into variables are not created)
            !tr.mem_only.get(i).copied().unwrap_or(true) && !paddr.get(i).copied().flatten().is_some_and(|(sl, _)| prom[sl as usize])
        } else if i < nvals + nvars {
            !tr.var_mem_only.get(i - nvals).copied().unwrap_or(true)
        } else {
            true
        }
    };
    let segs = &tr.segs;
    // Instruction number → segment (segments are in order of start)
    let seg_of = |op: u32| -> usize {
        let mut lo = 0usize;
        let mut hi = segs.len();
        while lo + 1 < hi {
            let m = (lo + hi) / 2;
            if segs[m].start <= op {
                lo = m;
            } else {
                hi = m;
            }
        }
        lo
    };
    const NONE: u32 = u32::MAX;
    let mut first = vec![NONE; n];
    let mut last = vec![0u32; n];
    let mut first_def = vec![false; n];
    let mut seg1 = vec![usize::MAX; n];
    let mut multi = vec![false; n];
    for &(id, op, def) in &ev {
        let i = idx(id);
        if i >= n {
            continue;
        }
        let s = seg_of(op);
        if first[i] == NONE {
            first[i] = op;
            first_def[i] = def;
            seg1[i] = s;
        } else if seg1[i] != s {
            multi[i] = true;
        }
        if op > last[i] {
            last[i] = op;
        }
    }
    // Things that cross segments (and things read before being written within a segment): look at them with dataflow
    let mut gid = vec![usize::MAX; n];
    let mut globals: Vec<usize> = vec![];
    for i in 0..n {
        if first[i] != NONE && cand(i) && (multi[i] || !first_def[i]) {
            gid[i] = globals.len();
            globals.push(i);
        }
    }
    let mut lo: Vec<u32> = first.clone();
    let mut hi: Vec<u32> = last.clone();
    if !globals.is_empty() && !segs.is_empty() {
        let g = globals.len();
        let words = (g + 63) / 64;
        let ns = segs.len();
        let mut gens = vec![0u64; ns * words];
        let mut kill = vec![0u64; ns * words];
        for &(id, op, def) in &ev {
            let i = idx(id);
            if i >= n || gid[i] == usize::MAX {
                continue;
            }
            let s = seg_of(op);
            let (w, bit) = (gid[i] / 64, 1u64 << (gid[i] % 64));
            if def {
                kill[s * words + w] |= bit;
            } else if kill[s * words + w] & bit == 0 {
                gens[s * words + w] |= bit;
            }
        }
        // Successors of a segment: the first segment of the target block, the continuation (the next segment of the same block / for the segment before the entry, the next one)
        let mut first_seg: FMap<u32, usize> = FMap::default();
        for (k, sg) in segs.iter().enumerate() {
            first_seg.entry(sg.block).or_insert(k);
        }
        let mut succ: Vec<Vec<usize>> = vec![vec![]; ns];
        for (k, sg) in segs.iter().enumerate() {
            for b in &sg.succ {
                if let Some(&t) = first_seg.get(b) {
                    succ[k].push(t);
                }
            }
            if sg.block == u32::MAX && k + 1 < ns {
                succ[k].push(k + 1);
            } else if sg.falls {
                if let Some(t) = (k + 1..ns).find(|&t| segs[t].block == sg.block) {
                    succ[k].push(t);
                }
            }
        }
        let mut lin = vec![0u64; ns * words];
        let mut lout = vec![0u64; ns * words];
        loop {
            let mut changed = false;
            for k in (0..ns).rev() {
                for w in 0..words {
                    let mut o = 0u64;
                    for &t in &succ[k] {
                        o |= lin[t * words + w];
                    }
                    let iv = gens[k * words + w] | (o & !kill[k * words + w]);
                    if o != lout[k * words + w] || iv != lin[k * words + w] {
                        lout[k * words + w] = o;
                        lin[k * words + w] = iv;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (gi, &i) in globals.iter().enumerate() {
            let (w, bit) = (gi / 64, 1u64 << (gi % 64));
            for k in 0..ns {
                if lin[k * words + w] & bit != 0 && segs[k].start < lo[i] {
                    lo[i] = segs[k].start;
                }
                if lout[k * words + w] & bit != 0 && segs[k].end > hi[i] {
                    hi[i] = segs[k].end;
                }
            }
        }
    }
    // Linear scan: in order of start, into a free register. A range containing a call gets only callee-saved registers.
    // If there is none, the one with the smallest "uses / length" (the current one, or one holding a register of the same kind) goes to a slot
    let mut uses = vec![0u32; n];
    for &(id, _, _) in &ev {
        let i = idx(id);
        if i < n {
            uses[i] += 1;
        }
    }
    let mut calls = tr.calls.clone();
    calls.sort_unstable();
    let has_call = |a: u32, b: u32| -> bool {
        // (The first call at or after a: binary search)
        let (mut l, mut h) = (0usize, calls.len());
        while l < h {
            let m = (l + h) / 2;
            if calls[m] < a {
                l = m + 1;
            } else {
                h = m;
            }
        }
        l < calls.len() && calls[l] <= b
    };
    let weight = |i: usize| -> f64 { uses[i] as f64 / (hi[i] - lo[i] + 1) as f64 };
    let mut order: Vec<usize> = (0..n).filter(|&i| first[i] != NONE && cand(i)).collect();
    order.sort_by_key(|&i| (lo[i], i));
    let mut loc = vec![MEM; n];
    let mut active: Vec<usize> = vec![];
    let mut free_saved: Vec<u8> = vec![4, 3, 2, 1, 0];
    let mut free_temp: Vec<u8> = vec![9, 8, 7, 6, 5];
    // (Floating point: xmm8 to 15. All are clobbered by calls, so only for ranges with no call. The number is 16 + the xmm number)
    let mut free_xmm: Vec<u8> = vec![31, 30, 29, 28, 27, 26, 25, 24];
    let is_float = |i: usize| -> bool {
        if i < nvals {
            tr.vfloat.get(i).copied().unwrap_or(false)
        } else if i < nvals + nvars {
            tr.varfloat.get(i - nvals).copied().unwrap_or(false)
        } else {
            ftys[i - nvals - nvars].is_float()
        }
    };
    let mut used = 0u8;
    for &i in &order {
        // (Drop the ones that ended)
        let mut k = 0;
        while k < active.len() {
            let a = active[k];
            if hi[a] < lo[i] {
                if loc[a] >= 16 {
                    free_xmm.push(loc[a]);
                } else if loc[a] < 5 {
                    free_saved.push(loc[a]);
                } else {
                    free_temp.push(loc[a]);
                }
                active.remove(k);
            } else {
                k += 1;
            }
        }
        let crosses = has_call(lo[i], hi[i]);
        if is_float(i) {
            if !crosses {
                if let Some(r) = free_xmm.pop() {
                    loc[i] = r;
                    active.push(i);
                }
            }
            continue;
        }
        let got = if crosses { free_saved.pop() } else { free_temp.pop().or_else(|| free_saved.pop()) };
        if let Some(r) = got {
            loc[i] = r;
            if r < 5 {
                used |= 1 << r;
            }
            active.push(i);
            continue;
        }
        // (Evict: if it crosses a call, from the ones holding callee-saved registers)
        let mut worst: Option<usize> = None;
        for k in 0..active.len() {
            let a = active[k];
            if (crosses && loc[a] >= 5) || loc[a] >= 16 {
                continue;
            }
            let better = match worst {
                None => true,
                Some(w) => weight(a) < weight(active[w]),
            };
            if better {
                worst = Some(k);
            }
        }
        if let Some(w) = worst {
            let a = active[w];
            if weight(a) < weight(i) {
                loc[i] = loc[a];
                loc[a] = MEM;
                active[w] = i;
            }
        }
    }
    // (Number → register number)
    let reg = |l: u8| -> u8 {
        if l == MEM {
            MEM
        } else if l >= 16 {
            l
        } else {
            ALLOC_REGS[l as usize]
        }
    };
    Plan {
        vloc: (0..nvals).map(|i| reg(loc[i])).collect(),
        varloc: (nvals..nvals + nvars).map(|i| reg(loc[i])).collect(),
        used,
        prom,
        fields,
        floc: (nvals + nvars..n).map(|i| reg(loc[i])).collect(),
    }
}

// --- FunctionBuilder that records and assembles twice ---------------------------------------------


/// Assembling one fn. Instructions are assembled with an Emitter for analysis while being recorded (fixing value numbers and types),
/// and finish allocates values / variables to registers, then assembles the recorded instructions once more in the same order with the real Emitter
pub struct FunctionBuilder {
    pub params: Vec<Type>,
    pub rets: Vec<Type>,
    dry: Emitter,
    pub ops: Vec<Op>,
    real: Option<Emitter>,
    /// Slots zeroed at the entry (what is passed to finish_zeroing; not made variables)
    pub zero: Vec<u32>,
    /// Return values per instruction (numbers of values / blocks / variables / slots / fns; used by clif_backend)
    pub res: Vec<Vec<u32>>,
}

impl FunctionBuilder {
    pub fn new(params: Vec<Type>, rets: Vec<Type>) -> FunctionBuilder {
        let dry = Emitter::new(params.clone(), rets.clone(), None);
        FunctionBuilder { params, rets, dry, ops: vec![], real: None, zero: vec![], res: vec![] }
    }
    pub fn func_params(&self) -> Vec<Value> {
        self.dry.func_params()
    }
    pub fn value_type(&self, v: Value) -> Type {
        self.dry.value_type(v)
    }
    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.dry.block_params(b)
    }
    pub fn current_block(&self) -> Option<Block> {
        self.dry.current_block()
    }
    /// The analysis record (used by clif_backend)
    /// The slot address per value (passed to sroa), and the numbers of values / slots
    pub fn paddr(&self) -> &[Option<(u32, i32)>] {
        &self.dry.paddr
    }
    pub fn n_values(&self) -> usize {
        self.dry.values.len()
    }
    pub fn n_slots(&self) -> usize {
        self.dry.slots.len()
    }

    pub fn track(&self) -> Option<&Track> {
        match &self.dry.track {
            Some(t) => Some(&**t),
            None => None,
        }
    }

    /// Tells beforehand the slots finish_zeroing zeroes (before traps / finish)
    pub fn set_zero(&mut self, z: &[StackSlot]) {
        self.zero = z.iter().map(|s| s.0).collect();
    }

    /// Decides the allocation and assembles the real run (only once)
    fn realize(&mut self) {
        if self.real.is_some() {
            return;
        }
        let mut tr = self.dry.take_track().unwrap_or_default();
        // (A call followed right away by a trap (panic) does not return: it is not counted as crossing a call)
        let ops = &self.ops;
        tr.calls.retain(|&c| !matches!(ops.get(c as usize + 1), Some(Op::Trap | Op::TrapCode(_))));
        let pl = if std::env::var_os("MINCLIF_NO_REGALLOC").is_some() { Plan::default() } else { plan(&tr, self.dry.values.len(), self.dry.vars.len(), &self.dry.paddr, self.dry.slots.len(), &self.zero) };
        let mut e = Emitter::new(self.params.clone(), self.rets.clone(), Some(pl));
        // (Read count per value: if only a brif reads an icmp's value, combine them into cmp + jcc)
        let mut reads = vec![0u32; self.dry.values.len()];
        for &(id, _, def) in &tr.ev {
            if !def && (id as usize) < reads.len() {
                reads[id as usize] += 1;
            }
        }
        let ops = std::mem::replace(&mut self.ops, vec![]);
        let mut i = 0;
        while i < ops.len() {
            e.op = i as u32;
            if let Op::Icmp(cc, a, b) = &ops[i] {
                // (There may be instructions in between that create neither values nor code (ones creating blocks / variables / slots))
                let mut j = i + 1;
                while j < ops.len() && matches!(ops[j], Op::CreateBlock | Op::DeclareVar(_) | Op::CreateStackSlot(..) | Op::ImportFunction(_)) {
                    j += 1;
                }
                let next = e.values.len() as u32;
                if let Some(Op::Brif(c, tb, ta, eb, ea)) = ops.get(j) {
                    if c.0 == next && reads.get(next as usize) == Some(&1) {
                        e.cmp_only(*cc, *a, *b);
                        for k in i + 1..j {
                            e.op = k as u32;
                            replay(&mut e, &ops[k]);
                        }
                        e.op = j as u32;
                        e.brif_cc(*cc, *tb, ta.as_slice(), *eb, ea.as_slice());
                        i = j + 1;
                        continue;
                    }
                }
            }
            replay(&mut e, &ops[i]);
            i += 1;
        }
        self.real = Some(e);
    }
    pub fn traps(&mut self) -> Vec<(u32, u8)> {
        self.realize();
        self.real.as_ref().unwrap().traps()
    }
    pub fn finish(mut self) -> (Vec<u8>, Vec<Reloc>) {
        self.realize();
        self.real.take().unwrap().finish()
    }
    pub fn finish_zeroing(mut self, zero: &[StackSlot]) -> (Vec<u8>, Vec<Reloc>) {
        if self.real.is_none() {
            self.set_zero(zero);
        }
        self.realize();
        self.real.take().unwrap().finish_zeroing(zero)
    }
    pub fn create_block(&mut self) -> Block {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CreateBlock);
        let r = self.dry.create_block();
        self.res.push(vec![r.0]);
        r
    }
    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AppendBlockParam(b, t));
        let r = self.dry.append_block_param(b, t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn switch_to_block(&mut self, b: Block) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SwitchToBlock(b));
        self.dry.switch_to_block(b);
        self.res.push(vec![]);
    }
    pub fn declare_var(&mut self, t: Type) -> Variable {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::DeclareVar(t));
        let r = self.dry.declare_var(t);
        self.res.push(vec![r.0]);
        r
    }
    pub fn def_var(&mut self, var: Variable, v: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::DefVar(var, v));
        self.dry.def_var(var, v);
        self.res.push(vec![]);
    }
    pub fn use_var(&mut self, var: Variable) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UseVar(var));
        let r = self.dry.use_var(var);
        self.res.push(vec![r.0]);
        r
    }
    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CreateStackSlot(size, align));
        let r = self.dry.create_stack_slot(size, align);
        self.res.push(vec![r.0]);
        r
    }
    pub fn import_function(&mut self, name: &str) -> FuncRef {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::ImportFunction(name.to_string()));
        let r = self.dry.import_function(name);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iconst(&mut self, t: Type, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iconst(t, x));
        let r = self.dry.iconst(t, x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iadd(a, b));
        let r = self.dry.iadd(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isub(a, b));
        let r = self.dry.isub(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Imul(a, b));
        let r = self.dry.imul(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Band(a, b));
        let r = self.dry.band(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bor(a, b));
        let r = self.dry.bor(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bxor(a, b));
        let r = self.dry.bxor(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iadd_imm(&mut self, a: Value, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::IaddImm(a, x));
        let r = self.dry.iadd_imm(a, x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Udiv(a, b));
        let r = self.dry.udiv(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sdiv(a, b));
        let r = self.dry.sdiv(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Urem(a, b));
        let r = self.dry.urem(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Srem(a, b));
        let r = self.dry.srem(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ishl(a, b));
        let r = self.dry.ishl(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ushr(a, b));
        let r = self.dry.ushr(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sshr(a, b));
        let r = self.dry.sshr(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ineg(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ineg(a));
        let r = self.dry.ineg(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bnot(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bnot(a));
        let r = self.dry.bnot(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Icmp(cc, a, b));
        let r = self.dry.icmp(cc, a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp_imm(&mut self, cc: IntCC, a: Value, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::IcmpImm(cc, a, x));
        let r = self.dry.icmp_imm(cc, a, x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Select(c, a, b));
        let r = self.dry.select(c, a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn uextend(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Uextend(t, a));
        let r = self.dry.uextend(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sextend(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sextend(t, a));
        let r = self.dry.sextend(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ireduce(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ireduce(t, a));
        let r = self.dry.ireduce(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn load(&mut self, t: Type, addr: Value, off: i32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Load(t, addr, off));
        let r = self.dry.load(t, addr, off);
        self.res.push(vec![r.0]);
        r
    }
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Store(v, addr, off));
        self.dry.store(v, addr, off);
        self.res.push(vec![]);
    }
    pub fn stack_addr(&mut self, s: StackSlot, off: i32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::StackAddr(s, off));
        let r = self.dry.stack_addr(s, off);
        self.res.push(vec![r.0]);
        r
    }
    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Jump(b, args.to_vec()));
        self.dry.jump(b, args);
        self.res.push(vec![]);
    }
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Brif(c, then_b, then_args.to_vec(), else_b, else_args.to_vec()));
        self.dry.brif(c, then_b, then_args, else_b, else_args);
        self.res.push(vec![]);
    }
    pub fn return_(&mut self, vals: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Return_(vals.to_vec()));
        self.dry.return_(vals);
        self.res.push(vec![]);
    }
    pub fn trap(&mut self) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trap);
        self.dry.trap();
        self.res.push(vec![]);
    }
    pub fn atomic_load(&mut self, t: Type, addr: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AtomicLoad(t, addr));
        let r = self.dry.atomic_load(t, addr);
        self.res.push(vec![r.0]);
        r
    }
    pub fn atomic_store(&mut self, v: Value, addr: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AtomicStore(v, addr));
        self.dry.atomic_store(v, addr);
        self.res.push(vec![]);
    }
    pub fn atomic_rmw(&mut self, op: AtomicOp, addr: Value, x: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AtomicRmw(op, addr, x));
        let r = self.dry.atomic_rmw(op, addr, x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn atomic_cas(&mut self, addr: Value, expected: Value, new: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AtomicCas(addr, expected, new));
        let r = self.dry.atomic_cas(addr, expected, new);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fence(&mut self) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fence);
        self.dry.fence();
        self.res.push(vec![]);
    }
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Call(f, args.to_vec(), rets.to_vec()));
        let r = self.dry.call(f, args, rets);
        self.res.push(r.iter().map(|v| v.0).collect());
        r
    }
    pub fn f64const(&mut self, x: f64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::F64const(x));
        let r = self.dry.f64const(x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn f32const(&mut self, x: f32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::F32const(x));
        let r = self.dry.f32const(x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fadd(a, b));
        let r = self.dry.fadd(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fsub(a, b));
        let r = self.dry.fsub(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fmul(a, b));
        let r = self.dry.fmul(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fdiv(a, b));
        let r = self.dry.fdiv(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sqrt(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sqrt(a));
        let r = self.dry.sqrt(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn floor(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Floor(a));
        let r = self.dry.floor(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ceil(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ceil(a));
        let r = self.dry.ceil(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn trunc(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trunc(a));
        let r = self.dry.trunc(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fneg(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fneg(a));
        let r = self.dry.fneg(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fabs(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fabs(a));
        let r = self.dry.fabs(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fcopysign(a, b));
        let r = self.dry.fcopysign(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fma(a, b, c));
        let r = self.dry.fma(a, b, c);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bitcast(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bitcast(t, a));
        let r = self.dry.bitcast(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcmp(&mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fcmp(cc, a, b));
        let r = self.dry.fcmp(cc, a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fpromote(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fpromote(a));
        let r = self.dry.fpromote(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fdemote(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fdemote(a));
        let r = self.dry.fdemote(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_from_sint(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtFromSint(t, a));
        let r = self.dry.fcvt_from_sint(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_from_uint(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtFromUint(t, a));
        let r = self.dry.fcvt_from_uint(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_to_sint_sat(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtToSintSat(t, a));
        let r = self.dry.fcvt_to_sint_sat(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn fcvt_to_uint_sat(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtToUintSat(t, a));
        let r = self.dry.fcvt_to_uint_sat(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iconcat(&mut self, lo: Value, hi: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iconcat(lo, hi));
        let r = self.dry.iconcat(lo, hi);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isplit(&mut self, v: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isplit(v));
        let r = self.dry.isplit(v);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn iadd128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iadd128(a, b));
        let r = self.dry.iadd128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn isub128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isub128(a, b));
        let r = self.dry.isub128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn band128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Band128(a, b));
        let r = self.dry.band128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bor128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bor128(a, b));
        let r = self.dry.bor128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bxor128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bxor128(a, b));
        let r = self.dry.bxor128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn imul128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Imul128(a, b));
        let r = self.dry.imul128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn udiv128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Udiv128(a, b));
        let r = self.dry.udiv128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sdiv128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sdiv128(a, b));
        let r = self.dry.sdiv128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn urem128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Urem128(a, b));
        let r = self.dry.urem128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn srem128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Srem128(a, b));
        let r = self.dry.srem128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn icmp128(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Icmp128(cc, a, b));
        let r = self.dry.icmp128(cc, a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ishl128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ishl128(a, b));
        let r = self.dry.ishl128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ushr128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ushr128(a, b));
        let r = self.dry.ushr128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn sshr128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sshr128(a, b));
        let r = self.dry.sshr128(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn extend128(&mut self, a: Value, signed: bool) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Extend128(a, signed));
        let r = self.dry.extend128(a, signed);
        self.res.push(vec![r.0]);
        r
    }
    pub fn reduce128(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Reduce128(t, a));
        let r = self.dry.reduce128(t, a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UaddOverflow(a, b));
        let r = self.dry.uadd_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UsubOverflow(a, b));
        let r = self.dry.usub_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SaddOverflow(a, b));
        let r = self.dry.sadd_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SsubOverflow(a, b));
        let r = self.dry.ssub_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UmulOverflow(a, b));
        let r = self.dry.umul_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SmulOverflow(a, b));
        let r = self.dry.smul_overflow(a, b);
        self.res.push(vec![r.0 .0, r.1 .0]);
        r
    }
    pub fn umulhi(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umulhi(a, b));
        let r = self.dry.umulhi(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn clz(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Clz(a));
        let r = self.dry.clz(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn ctz(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ctz(a));
        let r = self.dry.ctz(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn popcnt(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Popcnt(a));
        let r = self.dry.popcnt(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn bswap(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bswap(a));
        let r = self.dry.bswap(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn rotl(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Rotl(a, b));
        let r = self.dry.rotl(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn rotr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Rotr(a, b));
        let r = self.dry.rotr(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Smin(a, b));
        let r = self.dry.smin(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umin(a, b));
        let r = self.dry.umin(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Smax(a, b));
        let r = self.dry.smax(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umax(a, b));
        let r = self.dry.umax(a, b);
        self.res.push(vec![r.0]);
        r
    }
    pub fn iabs(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iabs(a));
        let r = self.dry.iabs(a);
        self.res.push(vec![r.0]);
        r
    }
    pub fn trap_code(&mut self, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapCode(code));
        self.dry.trap_code(code);
        self.res.push(vec![]);
    }
    pub fn trapz_code(&mut self, c: Value, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapzCode(c, code));
        self.dry.trapz_code(c, code);
        self.res.push(vec![]);
    }
    pub fn trapnz_code(&mut self, c: Value, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapnzCode(c, code));
        self.dry.trapnz_code(c, code);
        self.res.push(vec![]);
    }
    pub fn trapz(&mut self, c: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trapz(c));
        self.dry.trapz(c);
        self.res.push(vec![]);
    }
    pub fn trapnz(&mut self, c: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trapnz(c));
        self.dry.trapnz(c);
        self.res.push(vec![]);
    }
    pub fn call_indirect(&mut self, p: Value, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CallIndirect(p, args.to_vec(), rets.to_vec()));
        let r = self.dry.call_indirect(p, args, rets);
        self.res.push(r.iter().map(|v| v.0).collect());
        r
    }
    pub fn symbol_addr(&mut self, name: &str) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SymbolAddr(name.to_string()));
        let r = self.dry.symbol_addr(name);
        self.res.push(vec![r.0]);
        r
    }    pub fn vconst(&mut self, x: u128) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Vconst(x));
        let r = self.dry.vconst(x);
        self.res.push(vec![r.0]);
        r
    }
    pub fn simd(&mut self, s: Shape, op: VOp, args: &[Value]) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Simd(s, op, args.to_vec()));
        let r = self.dry.simd(s, op, args);
        self.res.push(vec![r.0]);
        r
    }
    /// whether it has a SIMD op or a vector value (minclif cannot emit them natively)
    pub fn has_simd(&self) -> bool {
        let vec = |t: Type| matches!(t, Type::Vec(_));
        self.ops.iter().any(|o| o.is_simd())
            || self.params.iter().chain(&self.rets).any(|&t| vec(t))
            || self.dry.values.iter().any(|v| vec(v.1))
            || self.dry.vars.iter().any(|v| vec(v.1))
    }
}

/// Assembles the recorded instructions with the real Emitter (return values get the same numbers as during analysis)
/// Reassembles a recording (zust_ir::Recorder) in the same order. The recording's numbers (values / blocks / variables / slots / fns) are renumbered here
/// (minclif sometimes creates values and slots inside instructions, so the numbers differ). The second is recording slot number → minclif slot number.
/// With ZUSTC_IR_VERIFY=1, checks that the values' types match the recording
pub fn from_recording(rec: &zust_ir::Recorder) -> (FunctionBuilder, Vec<u32>) {
    fn put(m: &mut Vec<u32>, i: u32, x: u32) {
        let i = i as usize;
        if m.len() <= i {
            m.resize(i + 1, u32::MAX);
        }
        m[i] = x;
    }
    fn mv(m: &[u32], v: Value) -> Value {
        Value(m[v.0 as usize])
    }
    let mut fb = FunctionBuilder::new(rec.params.clone(), rec.rets.clone());
    let verify = std::env::var_os("ZUSTC_IR_VERIFY").is_some();
    let mut vm: Vec<u32> = vec![];
    let mut bm: Vec<u32> = vec![];
    let mut varm: Vec<u32> = vec![];
    let mut sm: Vec<u32> = vec![];
    let mut fm: Vec<u32> = vec![];
    let ps = fb.func_params();
    for (i, p) in ps.iter().enumerate() {
        put(&mut vm, i as u32, p.0);
    }
    for (i, op) in rec.ops.iter().enumerate() {
        let out = &rec.res[i];
        match op {
            Op::CreateBlock => {
                let r = fb.create_block();
                put(&mut bm, out[0], r.0);
            }
            Op::AppendBlockParam(f0, f1) => {
                let r = fb.append_block_param(Block(bm[f0.0 as usize]), *f1);
                put(&mut vm, out[0], r.0);
            }
            Op::SwitchToBlock(f0) => {
                fb.switch_to_block(Block(bm[f0.0 as usize]));
            }
            Op::DeclareVar(f0) => {
                let r = fb.declare_var(*f0);
                put(&mut varm, out[0], r.0);
            }
            Op::DefVar(f0, f1) => {
                fb.def_var(Variable(varm[f0.0 as usize]), mv(&vm, *f1));
            }
            Op::UseVar(f0) => {
                let r = fb.use_var(Variable(varm[f0.0 as usize]));
                put(&mut vm, out[0], r.0);
            }
            Op::CreateStackSlot(f0, f1) => {
                let r = fb.create_stack_slot(*f0, *f1);
                put(&mut sm, out[0], r.0);
            }
            Op::ImportFunction(f0) => {
                let r = fb.import_function(f0.as_str());
                put(&mut fm, out[0], r.0);
            }
            Op::Iconst(f0, f1) => {
                let r = fb.iconst(*f0, *f1);
                put(&mut vm, out[0], r.0);
            }
            Op::Iadd(f0, f1) => {
                let r = fb.iadd(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Isub(f0, f1) => {
                let r = fb.isub(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Imul(f0, f1) => {
                let r = fb.imul(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Band(f0, f1) => {
                let r = fb.band(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Bor(f0, f1) => {
                let r = fb.bor(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Bxor(f0, f1) => {
                let r = fb.bxor(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::IaddImm(f0, f1) => {
                let r = fb.iadd_imm(mv(&vm, *f0), *f1);
                put(&mut vm, out[0], r.0);
            }
            Op::Udiv(f0, f1) => {
                let r = fb.udiv(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sdiv(f0, f1) => {
                let r = fb.sdiv(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Urem(f0, f1) => {
                let r = fb.urem(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Srem(f0, f1) => {
                let r = fb.srem(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Ishl(f0, f1) => {
                let r = fb.ishl(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Ushr(f0, f1) => {
                let r = fb.ushr(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sshr(f0, f1) => {
                let r = fb.sshr(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Ineg(f0) => {
                let r = fb.ineg(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Bnot(f0) => {
                let r = fb.bnot(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Icmp(f0, f1, f2) => {
                let r = fb.icmp(*f0, mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::IcmpImm(f0, f1, f2) => {
                let r = fb.icmp_imm(*f0, mv(&vm, *f1), *f2);
                put(&mut vm, out[0], r.0);
            }
            Op::Select(f0, f1, f2) => {
                let r = fb.select(mv(&vm, *f0), mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::Uextend(f0, f1) => {
                let r = fb.uextend(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sextend(f0, f1) => {
                let r = fb.sextend(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Ireduce(f0, f1) => {
                let r = fb.ireduce(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Load(f0, f1, f2) => {
                let r = fb.load(*f0, mv(&vm, *f1), *f2);
                put(&mut vm, out[0], r.0);
            }
            Op::Store(f0, f1, f2) => {
                fb.store(mv(&vm, *f0), mv(&vm, *f1), *f2);
            }
            Op::StackAddr(f0, f1) => {
                let r = fb.stack_addr(StackSlot(sm[f0.0 as usize]), *f1);
                put(&mut vm, out[0], r.0);
            }
            Op::Jump(f0, f1) => {
                fb.jump(Block(bm[f0.0 as usize]), &f1.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>());
            }
            Op::Brif(f0, f1, f2, f3, f4) => {
                fb.brif(mv(&vm, *f0), Block(bm[f1.0 as usize]), &f2.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>(), Block(bm[f3.0 as usize]), &f4.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>());
            }
            Op::Return_(f0) => {
                fb.return_(&f0.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>());
            }
            Op::Trap => {
                fb.trap();
            }
            Op::AtomicLoad(f0, f1) => {
                let r = fb.atomic_load(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::AtomicStore(f0, f1) => {
                fb.atomic_store(mv(&vm, *f0), mv(&vm, *f1));
            }
            Op::AtomicRmw(f0, f1, f2) => {
                let r = fb.atomic_rmw(*f0, mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::AtomicCas(f0, f1, f2) => {
                let r = fb.atomic_cas(mv(&vm, *f0), mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::Fence => {
                fb.fence();
            }
            Op::Call(f0, f1, f2) => {
                let r = fb.call(FuncRef(fm[f0.0 as usize]), &f1.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>(), &f2[..]);
                for (k, x) in r.iter().enumerate() {
                    put(&mut vm, out[k], x.0);
                }
            }
            Op::F64const(f0) => {
                let r = fb.f64const(*f0);
                put(&mut vm, out[0], r.0);
            }
            Op::F32const(f0) => {
                let r = fb.f32const(*f0);
                put(&mut vm, out[0], r.0);
            }
            Op::Fadd(f0, f1) => {
                let r = fb.fadd(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Fsub(f0, f1) => {
                let r = fb.fsub(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Fmul(f0, f1) => {
                let r = fb.fmul(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Fdiv(f0, f1) => {
                let r = fb.fdiv(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sqrt(f0) => {
                let r = fb.sqrt(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Floor(f0) => {
                let r = fb.floor(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Ceil(f0) => {
                let r = fb.ceil(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Trunc(f0) => {
                let r = fb.trunc(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Fneg(f0) => {
                let r = fb.fneg(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Fabs(f0) => {
                let r = fb.fabs(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Fcopysign(f0, f1) => {
                let r = fb.fcopysign(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Fma(f0, f1, f2) => {
                let r = fb.fma(mv(&vm, *f0), mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::Bitcast(f0, f1) => {
                let r = fb.bitcast(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Fcmp(f0, f1, f2) => {
                let r = fb.fcmp(*f0, mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::Fpromote(f0) => {
                let r = fb.fpromote(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Fdemote(f0) => {
                let r = fb.fdemote(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::FcvtFromSint(f0, f1) => {
                let r = fb.fcvt_from_sint(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::FcvtFromUint(f0, f1) => {
                let r = fb.fcvt_from_uint(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::FcvtToSintSat(f0, f1) => {
                let r = fb.fcvt_to_sint_sat(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::FcvtToUintSat(f0, f1) => {
                let r = fb.fcvt_to_uint_sat(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Iconcat(f0, f1) => {
                let r = fb.iconcat(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Isplit(f0) => {
                let r = fb.isplit(mv(&vm, *f0));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::Iadd128(f0, f1) => {
                let r = fb.iadd128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Isub128(f0, f1) => {
                let r = fb.isub128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Band128(f0, f1) => {
                let r = fb.band128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Bor128(f0, f1) => {
                let r = fb.bor128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Bxor128(f0, f1) => {
                let r = fb.bxor128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Imul128(f0, f1) => {
                let r = fb.imul128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Udiv128(f0, f1) => {
                let r = fb.udiv128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sdiv128(f0, f1) => {
                let r = fb.sdiv128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Urem128(f0, f1) => {
                let r = fb.urem128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Srem128(f0, f1) => {
                let r = fb.srem128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Icmp128(f0, f1, f2) => {
                let r = fb.icmp128(*f0, mv(&vm, *f1), mv(&vm, *f2));
                put(&mut vm, out[0], r.0);
            }
            Op::Ishl128(f0, f1) => {
                let r = fb.ishl128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Ushr128(f0, f1) => {
                let r = fb.ushr128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Sshr128(f0, f1) => {
                let r = fb.sshr128(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Extend128(f0, f1) => {
                let r = fb.extend128(mv(&vm, *f0), *f1);
                put(&mut vm, out[0], r.0);
            }
            Op::Reduce128(f0, f1) => {
                let r = fb.reduce128(*f0, mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::UaddOverflow(f0, f1) => {
                let r = fb.uadd_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::UsubOverflow(f0, f1) => {
                let r = fb.usub_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::SaddOverflow(f0, f1) => {
                let r = fb.sadd_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::SsubOverflow(f0, f1) => {
                let r = fb.ssub_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::UmulOverflow(f0, f1) => {
                let r = fb.umul_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::SmulOverflow(f0, f1) => {
                let r = fb.smul_overflow(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0 .0);
                put(&mut vm, out[1], r.1 .0);
            }
            Op::Umulhi(f0, f1) => {
                let r = fb.umulhi(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Clz(f0) => {
                let r = fb.clz(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Ctz(f0) => {
                let r = fb.ctz(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Popcnt(f0) => {
                let r = fb.popcnt(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Bswap(f0) => {
                let r = fb.bswap(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::Rotl(f0, f1) => {
                let r = fb.rotl(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Rotr(f0, f1) => {
                let r = fb.rotr(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Smin(f0, f1) => {
                let r = fb.smin(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Umin(f0, f1) => {
                let r = fb.umin(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Smax(f0, f1) => {
                let r = fb.smax(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Umax(f0, f1) => {
                let r = fb.umax(mv(&vm, *f0), mv(&vm, *f1));
                put(&mut vm, out[0], r.0);
            }
            Op::Iabs(f0) => {
                let r = fb.iabs(mv(&vm, *f0));
                put(&mut vm, out[0], r.0);
            }
            Op::TrapCode(f0) => {
                fb.trap_code(*f0);
            }
            Op::TrapzCode(f0, f1) => {
                fb.trapz_code(mv(&vm, *f0), *f1);
            }
            Op::TrapnzCode(f0, f1) => {
                fb.trapnz_code(mv(&vm, *f0), *f1);
            }
            Op::Trapz(f0) => {
                fb.trapz(mv(&vm, *f0));
            }
            Op::Trapnz(f0) => {
                fb.trapnz(mv(&vm, *f0));
            }
            Op::CallIndirect(f0, f1, f2) => {
                let r = fb.call_indirect(mv(&vm, *f0), &f1.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>(), &f2[..]);
                for (k, x) in r.iter().enumerate() {
                    put(&mut vm, out[k], x.0);
                }
            }
            Op::SymbolAddr(f0) => {
                let r = fb.symbol_addr(f0.as_str());
                put(&mut vm, out[0], r.0);
            }
            Op::Vconst(f0) => {
                let r = fb.vconst(*f0);
                put(&mut vm, out[0], r.0);
            }
            Op::Simd(f0, f1, f2) => {
                let r = fb.simd(*f0, *f1, &f2.iter().map(|x| mv(&vm, *x)).collect::<Vec<Value>>());
                put(&mut vm, out[0], r.0);
            }
        }
        if verify && !matches!(op, Op::CreateBlock | Op::DeclareVar(_) | Op::CreateStackSlot(..) | Op::ImportFunction(_)) {
            for &x in out {
                let (a, b) = (rec.value_type(Value(x)), fb.value_type(Value(vm[x as usize])));
                if a != b {
                    panic!("zust_ir: the type of v{x} is {a:?} in the recording but {b:?} in minclif (op {i})");
                }
            }
        }
    }
    fb.zero = rec.zero.iter().map(|&z| sm[z as usize]).collect();
    (fb, sm)
}

fn replay(e: &mut Emitter, op: &Op) {
    match op {
        Op::CreateBlock => {
            let _ = e.create_block();
        }
        Op::AppendBlockParam(f0, f1) => {
            let _ = e.append_block_param(*f0, *f1);
        }
        Op::SwitchToBlock(f0) => {
            let _ = e.switch_to_block(*f0);
        }
        Op::DeclareVar(f0) => {
            let _ = e.declare_var(*f0);
        }
        Op::DefVar(f0, f1) => {
            let _ = e.def_var(*f0, *f1);
        }
        Op::UseVar(f0) => {
            let _ = e.use_var(*f0);
        }
        Op::CreateStackSlot(f0, f1) => {
            let _ = e.create_stack_slot(*f0, *f1);
        }
        Op::ImportFunction(f0) => {
            let _ = e.import_function(f0.as_str());
        }
        Op::Iconst(f0, f1) => {
            let _ = e.iconst(*f0, *f1);
        }
        Op::Iadd(f0, f1) => {
            let _ = e.iadd(*f0, *f1);
        }
        Op::Isub(f0, f1) => {
            let _ = e.isub(*f0, *f1);
        }
        Op::Imul(f0, f1) => {
            let _ = e.imul(*f0, *f1);
        }
        Op::Band(f0, f1) => {
            let _ = e.band(*f0, *f1);
        }
        Op::Bor(f0, f1) => {
            let _ = e.bor(*f0, *f1);
        }
        Op::Bxor(f0, f1) => {
            let _ = e.bxor(*f0, *f1);
        }
        Op::IaddImm(f0, f1) => {
            let _ = e.iadd_imm(*f0, *f1);
        }
        Op::Udiv(f0, f1) => {
            let _ = e.udiv(*f0, *f1);
        }
        Op::Sdiv(f0, f1) => {
            let _ = e.sdiv(*f0, *f1);
        }
        Op::Urem(f0, f1) => {
            let _ = e.urem(*f0, *f1);
        }
        Op::Srem(f0, f1) => {
            let _ = e.srem(*f0, *f1);
        }
        Op::Ishl(f0, f1) => {
            let _ = e.ishl(*f0, *f1);
        }
        Op::Ushr(f0, f1) => {
            let _ = e.ushr(*f0, *f1);
        }
        Op::Sshr(f0, f1) => {
            let _ = e.sshr(*f0, *f1);
        }
        Op::Ineg(f0) => {
            let _ = e.ineg(*f0);
        }
        Op::Bnot(f0) => {
            let _ = e.bnot(*f0);
        }
        Op::Icmp(f0, f1, f2) => {
            let _ = e.icmp(*f0, *f1, *f2);
        }
        Op::IcmpImm(f0, f1, f2) => {
            let _ = e.icmp_imm(*f0, *f1, *f2);
        }
        Op::Select(f0, f1, f2) => {
            let _ = e.select(*f0, *f1, *f2);
        }
        Op::Uextend(f0, f1) => {
            let _ = e.uextend(*f0, *f1);
        }
        Op::Sextend(f0, f1) => {
            let _ = e.sextend(*f0, *f1);
        }
        Op::Ireduce(f0, f1) => {
            let _ = e.ireduce(*f0, *f1);
        }
        Op::Load(f0, f1, f2) => {
            let _ = e.load(*f0, *f1, *f2);
        }
        Op::Store(f0, f1, f2) => {
            let _ = e.store(*f0, *f1, *f2);
        }
        Op::StackAddr(f0, f1) => {
            let _ = e.stack_addr(*f0, *f1);
        }
        Op::Jump(f0, f1) => {
            let _ = e.jump(*f0, f1.as_slice());
        }
        Op::Brif(f0, f1, f2, f3, f4) => {
            let _ = e.brif(*f0, *f1, f2.as_slice(), *f3, f4.as_slice());
        }
        Op::Return_(f0) => {
            let _ = e.return_(f0.as_slice());
        }
        Op::Trap => {
            let _ = e.trap();
        }
        Op::AtomicLoad(f0, f1) => {
            let _ = e.atomic_load(*f0, *f1);
        }
        Op::AtomicStore(f0, f1) => {
            e.atomic_store(*f0, *f1);
        }
        Op::AtomicRmw(f0, f1, f2) => {
            let _ = e.atomic_rmw(*f0, *f1, *f2);
        }
        Op::AtomicCas(f0, f1, f2) => {
            let _ = e.atomic_cas(*f0, *f1, *f2);
        }
        Op::Fence => {
            e.fence();
        }
        Op::Call(f0, f1, f2) => {
            let _ = e.call(*f0, f1.as_slice(), f2.as_slice());
        }
        Op::F64const(f0) => {
            let _ = e.f64const(*f0);
        }
        Op::F32const(f0) => {
            let _ = e.f32const(*f0);
        }
        Op::Fadd(f0, f1) => {
            let _ = e.fadd(*f0, *f1);
        }
        Op::Fsub(f0, f1) => {
            let _ = e.fsub(*f0, *f1);
        }
        Op::Fmul(f0, f1) => {
            let _ = e.fmul(*f0, *f1);
        }
        Op::Fdiv(f0, f1) => {
            let _ = e.fdiv(*f0, *f1);
        }
        Op::Sqrt(f0) => {
            let _ = e.sqrt(*f0);
        }
        Op::Floor(f0) => {
            let _ = e.floor(*f0);
        }
        Op::Ceil(f0) => {
            let _ = e.ceil(*f0);
        }
        Op::Trunc(f0) => {
            let _ = e.trunc(*f0);
        }
        Op::Fneg(f0) => {
            let _ = e.fneg(*f0);
        }
        Op::Fabs(f0) => {
            let _ = e.fabs(*f0);
        }
        Op::Fcopysign(f0, f1) => {
            let _ = e.fcopysign(*f0, *f1);
        }
        Op::Fma(f0, f1, f2) => {
            let _ = e.fma(*f0, *f1, *f2);
        }
        Op::Bitcast(f0, f1) => {
            let _ = e.bitcast(*f0, *f1);
        }
        Op::Fcmp(f0, f1, f2) => {
            let _ = e.fcmp(*f0, *f1, *f2);
        }
        Op::Fpromote(f0) => {
            let _ = e.fpromote(*f0);
        }
        Op::Fdemote(f0) => {
            let _ = e.fdemote(*f0);
        }
        Op::FcvtFromSint(f0, f1) => {
            let _ = e.fcvt_from_sint(*f0, *f1);
        }
        Op::FcvtFromUint(f0, f1) => {
            let _ = e.fcvt_from_uint(*f0, *f1);
        }
        Op::FcvtToSintSat(f0, f1) => {
            let _ = e.fcvt_to_sint_sat(*f0, *f1);
        }
        Op::FcvtToUintSat(f0, f1) => {
            let _ = e.fcvt_to_uint_sat(*f0, *f1);
        }
        Op::Iconcat(f0, f1) => {
            let _ = e.iconcat(*f0, *f1);
        }
        Op::Isplit(f0) => {
            let _ = e.isplit(*f0);
        }
        Op::Iadd128(f0, f1) => {
            let _ = e.iadd128(*f0, *f1);
        }
        Op::Isub128(f0, f1) => {
            let _ = e.isub128(*f0, *f1);
        }
        Op::Band128(f0, f1) => {
            let _ = e.band128(*f0, *f1);
        }
        Op::Bor128(f0, f1) => {
            let _ = e.bor128(*f0, *f1);
        }
        Op::Bxor128(f0, f1) => {
            let _ = e.bxor128(*f0, *f1);
        }
        Op::Imul128(f0, f1) => {
            let _ = e.imul128(*f0, *f1);
        }
        Op::Udiv128(f0, f1) => {
            let _ = e.udiv128(*f0, *f1);
        }
        Op::Sdiv128(f0, f1) => {
            let _ = e.sdiv128(*f0, *f1);
        }
        Op::Urem128(f0, f1) => {
            let _ = e.urem128(*f0, *f1);
        }
        Op::Srem128(f0, f1) => {
            let _ = e.srem128(*f0, *f1);
        }
        Op::Icmp128(f0, f1, f2) => {
            let _ = e.icmp128(*f0, *f1, *f2);
        }
        Op::Ishl128(f0, f1) => {
            let _ = e.ishl128(*f0, *f1);
        }
        Op::Ushr128(f0, f1) => {
            let _ = e.ushr128(*f0, *f1);
        }
        Op::Sshr128(f0, f1) => {
            let _ = e.sshr128(*f0, *f1);
        }
        Op::Extend128(f0, f1) => {
            let _ = e.extend128(*f0, *f1);
        }
        Op::Reduce128(f0, f1) => {
            let _ = e.reduce128(*f0, *f1);
        }
        Op::UaddOverflow(f0, f1) => {
            let _ = e.uadd_overflow(*f0, *f1);
        }
        Op::UsubOverflow(f0, f1) => {
            let _ = e.usub_overflow(*f0, *f1);
        }
        Op::SaddOverflow(f0, f1) => {
            let _ = e.sadd_overflow(*f0, *f1);
        }
        Op::SsubOverflow(f0, f1) => {
            let _ = e.ssub_overflow(*f0, *f1);
        }
        Op::UmulOverflow(f0, f1) => {
            let _ = e.umul_overflow(*f0, *f1);
        }
        Op::SmulOverflow(f0, f1) => {
            let _ = e.smul_overflow(*f0, *f1);
        }
        Op::Umulhi(f0, f1) => {
            let _ = e.umulhi(*f0, *f1);
        }
        Op::Clz(f0) => {
            let _ = e.clz(*f0);
        }
        Op::Ctz(f0) => {
            let _ = e.ctz(*f0);
        }
        Op::Popcnt(f0) => {
            let _ = e.popcnt(*f0);
        }
        Op::Bswap(f0) => {
            let _ = e.bswap(*f0);
        }
        Op::Rotl(f0, f1) => {
            let _ = e.rotl(*f0, *f1);
        }
        Op::Rotr(f0, f1) => {
            let _ = e.rotr(*f0, *f1);
        }
        Op::Smin(f0, f1) => {
            let _ = e.smin(*f0, *f1);
        }
        Op::Umin(f0, f1) => {
            let _ = e.umin(*f0, *f1);
        }
        Op::Smax(f0, f1) => {
            let _ = e.smax(*f0, *f1);
        }
        Op::Umax(f0, f1) => {
            let _ = e.umax(*f0, *f1);
        }
        Op::Iabs(f0) => {
            let _ = e.iabs(*f0);
        }
        Op::TrapCode(f0) => {
            let _ = e.trap_code(*f0);
        }
        Op::TrapzCode(f0, f1) => {
            let _ = e.trapz_code(*f0, *f1);
        }
        Op::TrapnzCode(f0, f1) => {
            let _ = e.trapnz_code(*f0, *f1);
        }
        Op::Trapz(f0) => {
            let _ = e.trapz(*f0);
        }
        Op::Trapnz(f0) => {
            let _ = e.trapnz(*f0);
        }
        Op::CallIndirect(f0, f1, f2) => {
            let _ = e.call_indirect(*f0, f1.as_slice(), f2.as_slice());
        }
        Op::SymbolAddr(f0) => {
            let _ = e.symbol_addr(f0.as_str());
        }
        Op::Vconst(f0) => {
            let _ = e.vconst(*f0);
        }
        Op::Simd(f0, f1, f2) => {
            let _ = e.simd(*f0, *f1, f2);
        }
    }
}
