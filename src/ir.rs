// Yerux IR: a small three-address code over virtual registers, organised in basic blocks.
// Every backend (x86-64 today, aarch64/wasm later) consumes this and nothing else.
use crate::dropscc::HeapNode;
use crate::syntax::*;
use std::collections::{HashMap, HashSet};

pub type V = usize; // virtual register

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Add, // Add/Sub/Mul/Div/Mod are overflow- and zero-checked (panic on failure)
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    // Unchecked variants, produced only by the range analysis in opt.rs when it has PROVEN the check
    // cannot fire: AddU/SubU/MulU never overflow, DivU/ModU have a divisor that is non-zero and the
    // operation is not MIN / -1. Backends may omit the overflow/zero tests (or lower like the checked op).
    AddU,
    SubU,
    MulU,
    DivU,
    ModU,
}

impl Op {
    pub fn is_unchecked(self) -> bool {
        matches!(self, Op::AddU | Op::SubU | Op::MulU | Op::DivU | Op::ModU)
    }
    /// The checked counterpart (identity for everything else).
    pub fn checked(self) -> Op {
        match self {
            Op::AddU => Op::Add,
            Op::SubU => Op::Sub,
            Op::MulU => Op::Mul,
            Op::DivU => Op::Div,
            Op::ModU => Op::Mod,
            o => o,
        }
    }
    pub fn unchecked(self) -> Op {
        match self {
            Op::Add => Op::AddU,
            Op::Sub => Op::SubU,
            Op::Mul => Op::MulU,
            Op::Div => Op::DivU,
            Op::Mod => Op::ModU,
            o => o,
        }
    }
}

/// Value range tested / extended to by `NOp::Chk` and `NOp::Ext`: the low 8/16/32 bits of a number type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rng {
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    NonNeg,       // 0 ..= i64::MAX (only for Chk)
    Iv(i64, i64), // an arbitrary inclusive interval, the type of an `Int<lo..hi>` (only for Chk)
}

impl Rng {
    pub fn bounds(self) -> (i64, i64) {
        match self {
            Rng::I8 => (-128, 127),
            Rng::I16 => (-32768, 32767),
            Rng::I32 => (i32::MIN as i64, i32::MAX as i64),
            Rng::U8 => (0, 255),
            Rng::U16 => (0, 65535),
            Rng::U32 => (0, u32::MAX as i64),
            Rng::NonNeg => (0, i64::MAX),
            Rng::Iv(lo, hi) => (lo, hi),
        }
    }
    pub fn bits(self) -> u32 {
        match self {
            Rng::I8 | Rng::U8 => 8,
            Rng::I16 | Rng::U16 => 16,
            Rng::I32 | Rng::U32 => 32,
            Rng::NonNeg | Rng::Iv(..) => 64,
        }
    }
    pub fn signed(self) -> bool {
        matches!(self, Rng::I8 | Rng::I16 | Rng::I32)
    }
}

/// `--check-ranges`: re-verify at run time every interval the optimizer is told to trust (a safety net for
/// compiler bugs and for `unsafe` code that breaks a declared range): range-typed parameters, variables and
/// loaded or returned values are checked where they are produced.
pub static CHECK_RANGES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn check_ranges() -> bool {
    CHECK_RANGES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Panic messages of `NOp::Chk` (index into the backends' message tables).
pub const MSG_ADD: u8 = 0;
pub const MSG_SUB: u8 = 1;
pub const MSG_MUL: u8 = 2;
pub const MSG_DIV: u8 = 3;
pub const MSG_NEG: u8 = 4;
pub const MSG_CONV: u8 = 5; // "integer conversion out of range"
pub const MSG_F2I: u8 = 6; // "float is NaN or out of range for the integer type"

/// Number operations beyond `Op`: `d = a op b` (unary ops ignore `b`, which repeats `a`).
/// Floats travel in ordinary vregs as bit patterns: Float = binary64, Float32 = binary32 in the low 32 bits
/// (all Float32 arithmetic is done in binary64 and rounded back, which is exact for + - * / sqrt).
/// Integers narrower than 64 bits are kept sign/zero-extended, UInt64 as its bit pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NOp {
    FAdd,
    FSub,
    FMul,
    FDiv,
    FLt, // IEEE comparisons: 0 when unordered (FNe: 1). `>` and `>=` swap the operands.
    FLe,
    FEq,
    FNe,
    FNeg,
    FSqrt,
    I2F,          // (double) of a signed 64-bit integer
    I2S,          // (float) of a signed 64-bit integer, one rounding
    S2F,          // binary32 -> binary64 (exact)
    F2S,          // binary64 -> binary32 (round to nearest even)
    F2I,          // truncate toward zero; panics (MSG_F2I) on NaN or outside i64
    Chk(Rng, u8), // d = a; panics with message `u8` unless a is in the range
    Ext(Rng),     // d = sign/zero-extension of the low 8/16/32 bits of a
    WAdd,         // wrapping 64-bit arithmetic
    WSub,
    WMul,
    UAdd, // UInt64: checked add/sub/mul/div/mod (panic on overflow / zero divisor)
    USub,
    UMul,
    UDiv,
    UMod,
    ULt, // unsigned 64-bit comparisons (`>` and `>=` swap the operands)
    ULe,
}

impl NOp {
    /// Cannot panic: may be deleted when its result is unused.
    pub fn pure(self) -> bool {
        !matches!(
            self,
            NOp::F2I | NOp::Chk(..) | NOp::UAdd | NOp::USub | NOp::UMul | NOp::UDiv | NOp::UMod
        )
    }
    pub fn unary(self) -> bool {
        matches!(
            self,
            NOp::FNeg
                | NOp::FSqrt
                | NOp::I2F
                | NOp::I2S
                | NOp::S2F
                | NOp::F2S
                | NOp::F2I
                | NOp::Chk(..)
                | NOp::Ext(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThreadOp {
    Start,
    StartRecover,
    Unlock,
    CleanupPush,
    CleanupPop,
    DropEnter,
    DropLeave,
    Wait,
    Park,
    Wake,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AtomicOp {
    Load,
    Store,
    CompareExchange,
    FetchAdd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    N(NOp, V, V, V), // d = a op b, see NOp
    Const(V, i64),
    Text(V, usize), // vreg = address of text literal #n
    Mov(V, V),
    Bin(V, Op, V, V),
    Neg(V, V),
    Not(V, V),
    Call(Option<V>, usize, Vec<V>),
    ExternCall(Option<V>, usize, Vec<V>, Vec<bool>, bool),
    /// Indirect call: result, code address (a `FnAddr` value), arguments (same convention as `Call`).
    CallInd(Option<V>, V, Vec<V>),
    /// vreg = address of function #n (code pointer of a function value)
    FnAddr(V, usize),
    VTableAddr(V, usize),
    PrintInt(V),
    PrintBool(V),
    PrintText(V),
    // ── aggregates: memory is addressed through plain vregs holding pointers ──
    Slot(V, usize),    // vreg = address of frame slot #n (see IrFunc::slots)
    Load(V, V, i32),   // vreg = [addr + off]
    LoadH(V, V, i32), // like Load, but the word is a list/string header field ({ptr,len,cap}): memopt may cache it
    Store(V, i32, V), // [addr + off] = vreg
    Lea(V, V, i32),   // vreg = addr + off
    Copy(V, V, usize), // copy n words: [dst..] = [src..]
    // ── heap: a List is 3 words {ptr, len, cap} at the given address ──
    // Legacy list instructions: no longer produced by the lowering (memopt.rs inlines list access), but
    // every backend still implements them and the passes keep their semantics.
    #[allow(dead_code)]
    ListPush(V, V), // list address, value (amortised growth; may allocate)
    #[allow(dead_code)]
    ListGet(V, V, V), // vreg = list[idx], panics "index out of range"
    ListFree(V), // release the buffer (no-op when empty)
    // ── raw heap / byte primitives (String, generic List<T>): all side-effecting ──
    Alloc(V, V),         // vreg = fresh heap block of n bytes
    Free(V),             // release a heap block (null is ignored)
    Realloc(V, V, V, V), // vreg = block of >= n bytes holding the first `used` bytes of `old` (null: fresh); old is released: dst, old, n, used
    MemCpy(V, V, V),     // copy n bytes forward: dst, src, n
    Load8(V, V, i32),    // vreg = zero-extended byte [addr + off]
    Store8(V, i32, V),   // byte [addr + off] = low byte of vreg
    PrintBytes(V, V),    // write `len` bytes at `ptr`, then a newline
    Check(V, V),         // panic "index out of range" unless 0 <= idx < len
    CheckNonEmpty(V),    // panic "pop from an empty list" when len == 0
    Assert(V),           // panic "assertion failed" when the value is 0 (`assert`, `assert_eq`)
    /// Raw Linux system call (x86-64 ELF only): d = syscall(args[0] = number, args[1..7] = arguments); the
    /// raw kernel result (-errno on failure). Behaves like a call: may read and write any memory.
    Syscall(V, Vec<V>),
    /// Sequentially consistent access to an aligned 64-bit word. A full memory barrier in the optimizer.
    Atomic(AtomicOp, V, Vec<V>),
    Thread(ThreadOp, V, Vec<V>),
    /// d = address of global cell #n (8 zero-initialised bytes that live as long as the program)
    Global(V, usize),
    Panic(V, V), // `panic(msg)`: print "panic: " + `len` bytes at `ptr` + newline to stderr, exit 101 (never returns)
    // ── indexed list-element access: address = base + idx * scale (scale in 1,2,4,8), no overflow check ──
    // LoadX/StoreX are WORD accesses to a list/array element buffer. Contract used by memopt: they never
    // touch a list header word ({ptr,len,cap}), so they cannot invalidate a cached header load.
    LoadX(V, V, V, u8), // vreg = [base + idx*scale]; scale 0 = packed byte element: zero-extended [base + idx]
    StoreX(V, V, u8, V), // [base + idx*scale] = vreg; scale 0 = low byte to [base + idx]
    LeaX(V, V, V, u8),  // vreg = base + idx*scale
    /// Source-location marker (an index into `IrProgram::locs`): the checked instructions and calls that follow
    /// in the same block belong to that `.yx` position. No effect; the backends turn it into the panic site table
    /// (`strip_locs` removes it first, so register allocation and code generation never see it).
    Loc(u32),
}

/// Instructions that can panic (or call something that can) and so need a location marker in front of them.
/// A location marker before every instruction (the native backend's DWARF line table): set by `--dwarf`/`--debug`.
pub static ALL_LOCS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn needs_loc(i: &Inst) -> bool {
    match i {
        Inst::Bin(_, op, ..) => matches!(op, Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod),
        Inst::Neg(..) => true,
        Inst::N(op, ..) => !op.pure(),
        Inst::Check(..)
        | Inst::CheckNonEmpty(_)
        | Inst::Assert(_)
        | Inst::Panic(..)
        | Inst::Call(..)
        | Inst::CallInd(..)
        | Inst::ListGet(..) => true,
        _ => false,
    }
}

/// Take the location markers out of a function: the same instructions without them, and for every instruction
/// the location it belongs to (0 = unknown; markers apply to the rest of their block).
pub fn strip_locs(f: &IrFunc) -> (IrFunc, Vec<Vec<u32>>) {
    let mut locs = Vec::with_capacity(f.blocks.len());
    let blocks = f
        .blocks
        .iter()
        .map(|b| {
            let mut cur = 0;
            let mut ls = Vec::with_capacity(b.insts.len());
            let mut insts = Vec::with_capacity(b.insts.len());
            for i in &b.insts {
                if let Inst::Loc(l) = i {
                    cur = *l;
                } else {
                    insts.push(i.clone());
                    ls.push(cur);
                }
            }
            locs.push(ls);
            Block {
                insts,
                term: b.term.clone(),
            }
        })
        .collect();
    (
        IrFunc {
            name: f.name.clone(),
            lib: f.lib,
            nparams: f.nparams,
            vranges: f.vranges.clone(),
            nvregs: f.nvregs,
            slots: f.slots.clone(),
            blocks,
        },
        locs,
    )
}

/// Interned source positions of the IR program: (file, line, column), index 0 = unknown.
pub struct LocTab {
    pub rows: Vec<(u32, u32, u32)>,
    ix: HashMap<(u32, u32, u32), u32>,
}

impl LocTab {
    fn new() -> LocTab {
        LocTab {
            rows: vec![(0, 0, 0)],
            ix: HashMap::new(),
        }
    }
    fn intern(&mut self, pos: Pos) -> u32 {
        let key = ((pos.line >> 20) as u32, (pos.line & 0xFFFFF) as u32, pos.col as u32);
        if let Some(i) = self.ix.get(&key) {
            return *i;
        }
        let i = self.rows.len() as u32;
        self.rows.push(key);
        self.ix.insert(key, i);
        i
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Jmp(usize),
    Br(V, usize, usize),
    Ret(Option<V>),
    Unreachable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Term,
}

pub struct IrFunc {
    pub name: String,
    /// a function of the built-in library: its panics are reported at the user's call (see `Inst::Loc`)
    pub lib: bool,
    pub nparams: usize,
    /// Declared integer intervals (`Int<lo..hi>` types), indexed by vreg and possibly shorter than `nvregs`:
    /// every definition of such a vreg produces a value inside the interval (parameters, variables of a range
    /// type, call results, list/field loads). Used by `rangeopt` as a cap.
    pub vranges: Vec<Option<(i64, i64)>>,
    pub nvregs: usize,
    pub slots: Vec<usize>, // frame slots, size of each in 8-byte words (aggregates, address-taken scalars)
    pub blocks: Vec<Block>,
}

pub struct IrProgram {
    pub funcs: Vec<IrFunc>,
    pub texts: Vec<Vec<u8>>, // literal bytes
    pub main: usize,
    pub vtables: Vec<Vec<usize>>, // drop glue first, then methods in declaration order
    pub externs: Vec<String>,
    pub links: Vec<String>,
    /// source positions the `Loc` markers refer to, and the names of the source files (set by the driver)
    pub locs: Vec<(u32, u32, u32)>,
    pub files: Vec<String>,
}

// ───────────────────────── Lowering: AST -> IR ─────────────────────────
//
// Value model:
//  * Scalars (Int, Bool, Text) live in vregs.
//  * Aggregates (structs, enums) live in memory, always in whole 8-byte words. An aggregate-typed
//    expression evaluates to a vreg holding the ADDRESS of its memory. Consumers copy out of it
//    (let, assignment, by-value argument, field of a literal), so a shared address never escapes.
//  * Struct layout: fields in declaration order, one after another.
//    Enum layout: word 0 = variant tag, then the payload words of the active variant (overlaid).
//  * `view`/`mut` parameters (and every by-value aggregate parameter) are pointers.
//    Scalars that are passed on as `mut` live in a one-word frame slot so they have an address.
//  * A function returning an aggregate takes a hidden first parameter: the destination address.

#[derive(Clone, Copy)]
enum Loc {
    Val(V), // scalar held in a vreg
    Mem(V), // vreg holds the address of the value
}

struct Sig {
    params: Vec<(Mode, Ty)>,
}

/// An owned local that must be released at scope end if its drop flag is still set.
#[derive(Clone, Copy)]
struct Owned {
    flag: V,
    addr: V,
    ty: Ty,
    cleanup: Option<V>,
}

/// Shared runtime helpers (drop glue, growth, string primitives) generated as ordinary IR functions.
/// Ids are handed out lazily after the user functions; bodies are generated in request order.
struct Helpers {
    base: usize,
    all: Vec<(u8, usize)>,
    ids: HashMap<(u8, usize), usize>,
    cleanup_types: Vec<Ty>,
}

const H_DROP: u8 = 0; // key: list type index; fn(list): drop every element, free the buffer
const H_CLEAR: u8 = 1; // key: list type index; fn(list): drop every element, len = 0
const H_GROW: u8 = 2; // fn(list, elem_bytes): make room for one more element
const H_STR_PUSH: u8 = 3; // fn(string, ptr, len): append bytes
const H_STR_PUSH_INT: u8 = 4; // fn(string, int): append decimal digits
const H_STR_EQ: u8 = 5; // fn(string, ptr, len) -> Bool
const H_STR_PUSH_CHAR: u8 = 6; // fn(string, char): append its UTF-8 bytes
const H_STR_CHAR_SIZE: u8 = 7; // fn(string, i) -> bytes of the character starting at byte i
const H_STR_CHAR_AT: u8 = 8; // fn(string, i) -> the character starting at byte i
const H_DROP_ENV: u8 = 9; // key: struct id of a closure environment; fn(env): drop the captures, free the block
const H_DROP_ENUM: u8 = 10; // key: enum index; fn(enum): drop the owned payloads of the active variant
const H_DROP_DYN_DATA: u8 = 12;
const H_DYN_ADAPTER: u8 = 13;
const H_PANIC_DROP: u8 = 14;
const H_STR_RESERVE: u8 = 15; // fn(string, extra): room for `extra` more bytes
const H_DROP_BOX: u8 = 11; // key: box type index; fn(box): drop the pointee, free the cell

impl Helpers {
    fn cleanup(&mut self, ty: Ty) -> usize {
        let key = self
            .cleanup_types
            .iter()
            .position(|t| *t == ty)
            .unwrap_or_else(|| {
                let key = self.cleanup_types.len();
                self.cleanup_types.push(ty);
                key
            });
        self.get(H_PANIC_DROP, key)
    }

    fn get(&mut self, kind: u8, key: usize) -> usize {
        if let Some(i) = self.ids.get(&(kind, key)) {
            return *i;
        }
        let id = self.base + self.all.len();
        self.all.push((kind, key));
        self.ids.insert((kind, key), id);
        id
    }
}

/// An enclosing loop: where `break` and `continue` jump, and how many scopes are open outside its body.
struct LoopL {
    id: usize,
    depth: usize,
    brk: usize,
    cont: usize,
}

struct L<'a> {
    scopes: Vec<Vec<Owned>>,
    recover_panics: bool,
    guard_destructors: bool,
    cleanup_slots: HashMap<usize, V>,
    capture_flags: HashMap<usize, V>, // variable id -> address of its environment drop flag
    flags: HashMap<usize, V>,         // variable id -> drop flag vreg (1 = still owns its value)
    blocks: Vec<(Vec<Inst>, Option<Term>)>,
    cur: usize,
    nv: usize,
    vr: HashMap<V, (i64, i64)>,
    slots: Vec<usize>,
    vars: HashMap<usize, Loc>,
    memvars: HashSet<usize>,
    ret_ptr: Option<V>,
    loops: Vec<LoopL>,
    types: &'a Types,
    sigs: &'a HashMap<String, Sig>,
    ids: &'a HashMap<String, usize>,
    texts: &'a mut Vec<Vec<u8>>,
    helpers: &'a mut Helpers,
    extern_ids: &'a HashMap<String, usize>,
    locs: &'a mut LocTab,
    /// per function id: is it a library function (calls to those are the ones that need a location)
    lib_fn: &'a [bool],
    /// source position of the expression being lowered (line 0 = none: library code), and per block the
    /// location id its last marker set
    loc: Pos,
    bloc: Vec<u32>,
}

type LR<T> = Result<T, String>;

enum CallTarget<'a> {
    Direct(&'a str),
    Closure(&'a Expr),
    Dyn(usize),
}

enum StrPre {
    Text(V),
    Place(Vec<V>),
}

#[path = "ir_num.rs"]
mod num;

fn is_agg(t: Ty) -> bool {
    matches!(
        t,
        Ty::Struct(_)
            | Ty::Enum(_)
            | Ty::List(_)
            | Ty::String
            | Ty::Box(_)
            | Ty::Fn(_)
            | Ty::Dyn(_)
    )
}

fn is_ref(types: &Types, mode: Mode, ty: Ty) -> bool {
    mode == Mode::Mut || (mode == Mode::View && types.owned(ty))
}

fn supported(t: Ty, what: &str) -> LR<()> {
    match t {
        Ty::Int
        | Ty::Range(..)
        | Ty::Num(_)
        | Ty::Char
        | Ty::Bool
        | Ty::Text
        | Ty::Ptr(_)
        | Ty::Unit
        | Ty::Struct(_)
        | Ty::Enum(_)
        | Ty::List(_)
        | Ty::String
        | Ty::Box(_)
        | Ty::Fn(_)
        | Ty::Dyn(_) => Ok(()),
        _ => Err(format!(
            "native backend: {} of this type are not supported",
            what
        )),
    }
}

// Variables whose address is needed although they are scalars: passed to a `mut` parameter.
fn scan_stmts(b: &[Stmt], sigs: &HashMap<String, Sig>, types: &Types, out: &mut HashSet<usize>) {
    for s in b {
        match &s.k {
            SK::Let(_, _, e, _) | SK::ExprS(e) => scan_expr(e, sigs, types, out),
            SK::Assign(a, b) => {
                scan_expr(a, sigs, types, out);
                scan_expr(b, sigs, types, out);
            }
            SK::If(c, t, el) => {
                scan_expr(c, sigs, types, out);
                scan_stmts(t, sigs, types, out);
                if let Some(e) = el {
                    scan_stmts(e, sigs, types, out);
                }
            }
            SK::While(c, b, meta) => {
                scan_expr(c, sigs, types, out);
                scan_stmts(b, sigs, types, out);
                scan_stmts(&meta.step, sigs, types, out);
            }
            SK::Block(b, _) | SK::Unsafe(b) => scan_stmts(b, sigs, types, out),
            SK::Bind(_, _, e, _) | SK::Unpack(_, e) => scan_expr(e, sigs, types, out),
            SK::Break(..) | SK::Continue(..) | SK::For(..) => {}
            SK::Return(e) => {
                if let Some(e) = e {
                    scan_expr(e, sigs, types, out)
                }
            }
            SK::Match(e, arms) => {
                scan_expr(e, sigs, types, out);
                for a in arms {
                    scan_stmts(&a.body, sigs, types, out);
                }
            }
            SK::Defer(_) => {}
        }
    }
}

fn scan_expr(e: &Expr, sigs: &HashMap<String, Sig>, types: &Types, out: &mut HashSet<usize>) {
    match &e.k {
        EK::Sequence(stmts, result) => {
            scan_stmts(stmts, sigs, types, out);
            scan_expr(result, sigs, types, out);
        }
        EK::Field(b, _) | EK::Conv(b, _) | EK::IsVariant(b, _) | EK::Payload(b, ..) => {
            scan_expr(b, sigs, types, out)
        }
        EK::Un(_, x) => scan_expr(x, sigs, types, out),
        EK::Bin(_, a, b) => {
            scan_expr(a, sigs, types, out);
            scan_expr(b, sigs, types, out);
        }
        EK::Method { recv, args, .. } => {
            scan_expr(recv, sigs, types, out);
            args.iter().for_each(|a| scan_expr(a, sigs, types, out));
        }
        EK::StructLit { fields, .. } => fields
            .iter()
            .for_each(|(_, f)| scan_expr(f, sigs, types, out)),
        EK::ArrayLit(items) => items.iter().for_each(|a| scan_expr(a, sigs, types, out)),
        EK::ArrayFill(b, _) => scan_expr(b, sigs, types, out),
        EK::SliceOf(b, lo, hi) => {
            scan_expr(b, sigs, types, out);
            lo.iter()
                .chain(hi.iter())
                .for_each(|a| scan_expr(a, sigs, types, out));
        }
        EK::Variant { args, .. } => args.iter().for_each(|a| scan_expr(a, sigs, types, out)),
        EK::Static { args, .. } => args.iter().for_each(|a| scan_expr(a, sigs, types, out)),
        EK::DynBox { value, .. } => scan_expr(value, sigs, types, out),
        EK::DynCall { args, sig, .. } => {
            for (a, (mode, _)) in args.iter().zip(&sig.params) {
                if let (Mode::Mut, EK::Var(_, id)) = (mode, &a.k) {
                    out.insert(*id);
                }
                scan_expr(a, sigs, types, out);
            }
        }
        EK::MkClosure { caps, .. } => {
            for c in caps {
                if let EK::AddrOf(v, _) = &c.k {
                    if let EK::Var(_, id) = &v.k {
                        out.insert(*id);
                    }
                }
                scan_expr(c, sigs, types, out);
            }
        }
        EK::CallFn(c, args) => {
            scan_expr(c, sigs, types, out);
            // a `mut` argument of a function value needs an address too
            if let Ty::Fn(fi) = c.ty {
                for (a, (mode, _)) in args.iter().zip(types.fns[fi].params.iter()) {
                    if *mode == Mode::Mut {
                        if let EK::Var(_, id) = &a.k {
                            out.insert(*id);
                        }
                    }
                }
            }
            args.iter().for_each(|a| scan_expr(a, sigs, types, out));
        }
        EK::Call(name, args) => {
            if let Some(sig) = sigs.get(name) {
                for (a, (mode, _)) in args.iter().zip(sig.params.iter()) {
                    if *mode == Mode::Mut {
                        if let EK::Var(_, id) = &a.k {
                            out.insert(*id);
                        }
                    }
                }
            }
            args.iter().for_each(|a| scan_expr(a, sigs, types, out));
        }
        _ => {}
    }
}

impl<'a> L<'a> {
    fn v(&mut self) -> V {
        self.nv += 1;
        self.nv - 1
    }
    fn block(&mut self) -> usize {
        self.blocks.push((vec![], None));
        self.bloc.push(0);
        self.blocks.len() - 1
    }
    fn emit(&mut self, i: Inst) {
        let wants = ALL_LOCS.load(std::sync::atomic::Ordering::Relaxed) || match &i {
            Inst::Call(_, id, _) => self.lib_fn.get(*id).copied().unwrap_or(false),
            Inst::CallInd(..) => false,
            other => needs_loc(other),
        };
        if wants && self.loc.line != 0 {
            let id = self.locs.intern(self.loc);
            if self.bloc[self.cur] != id {
                self.blocks[self.cur].0.push(Inst::Loc(id));
                self.bloc[self.cur] = id;
            }
        }
        self.blocks[self.cur].0.push(i);
    }
    /// Make `pos` the current source location (library code has none); returns the previous one.
    fn at_pos(&mut self, pos: Pos) -> Pos {
        let old = self.loc;
        if pos.line != 0 && pos.line < PRELUDE_LINE {
            self.loc = pos;
        }
        old
    }
    fn term(&mut self, t: Term) {
        if self.blocks[self.cur].1.is_none() {
            self.blocks[self.cur].1 = Some(t);
        }
    }
    fn switch(&mut self, b: usize) {
        self.cur = b;
    }

    // ── layout ──
    fn words(&self, t: Ty) -> usize {
        match t {
            Ty::Int | Ty::Range(..) | Ty::Num(_) | Ty::Char | Ty::Bool | Ty::Text | Ty::Ptr(_) => 1,
            Ty::List(_) | Ty::String => 3,
            Ty::Fn(_) | Ty::Dyn(_) => 2,
            Ty::Box(_) => 1,
            // `[T; N]`: N elements laid out like a List buffer (packed bytes for Bool)
            Ty::Struct(i) if self.types.arrays.contains_key(&i) => {
                let (e, n) = self.types.arrays[&i];
                (self.esz(e) as usize * n).div_ceil(8)
            }
            Ty::Struct(i) => self.types.structs[i]
                .fields
                .iter()
                .map(|(_, f)| self.words(*f))
                .sum(),
            Ty::Enum(i) => {
                1 + self.types.enums[i]
                    .variants
                    .iter()
                    .map(|(_, ps)| ps.iter().map(|p| self.words(*p)).sum::<usize>())
                    .max()
                    .unwrap_or(0)
            }
            _ => 0,
        }
    }
    fn field_off(&self, sid: usize, name: &str) -> (i32, Ty) {
        let mut off = 0;
        for (n, t) in &self.types.structs[sid].fields {
            if n == name {
                return (off as i32 * 8, *t);
            }
            off += self.words(*t);
        }
        unreachable!("checked field")
    }
    fn payload_off(&self, eid: usize, vid: usize, k: usize) -> (i32, Ty) {
        let ps = &self.types.enums[eid].variants[vid].1;
        let off: usize = 1 + ps[..k].iter().map(|p| self.words(*p)).sum::<usize>();
        (off as i32 * 8, ps[k])
    }

    // ── small emit helpers ──
    fn new_slot(&mut self, words: usize) -> V {
        self.slots.push(words.max(1));
        let a = self.v();
        let k = self.slots.len() - 1;
        self.emit(Inst::Slot(a, k));
        a
    }
    fn at(&mut self, base: V, off: i32) -> V {
        if off == 0 {
            return base;
        }
        let d = self.v();
        self.emit(Inst::Lea(d, base, off));
        d
    }
    fn konst(&mut self, n: i64) -> V {
        let d = self.v();
        self.emit(Inst::Const(d, n));
        d
    }
    /// Fresh temporaries (not shared with any variable) can be adopted without copying.
    fn fresh(e: &Expr) -> bool {
        matches!(
            e.k,
            EK::StructLit { .. }
                | EK::ArrayLit(..)
                | EK::ArrayFill(..)
                | EK::Variant { .. }
                | EK::Static { .. }
                | EK::MkClosure { .. }
                | EK::CallFn(..)
                | EK::DynCall { .. }
                | EK::DynBox { .. }
        ) || matches!(&e.k, EK::Call(n, _) if n != "print" && n != "assert")
            || matches!(&e.k, EK::Method { m, .. } if m == "pop" || m == "take")
    }
    /// A private copy of an aggregate in a new frame slot.
    fn copy_temp(&mut self, src: V, ty: Ty) -> V {
        let n = self.words(ty);
        let t = self.new_slot(n);
        self.emit(Inst::Copy(t, src, n));
        t
    }
    /// Write expression `e` into memory at base+off (scalar store or aggregate copy).
    fn store_into(&mut self, base: V, off: i32, e: &Expr) -> LR<()> {
        let v = self.expr(e)?;
        if is_agg(e.ty) {
            let d = self.at(base, off);
            let n = self.words(e.ty);
            self.emit(Inst::Copy(d, v, n));
        } else {
            self.emit(Inst::Store(base, off, v));
        }
        Ok(())
    }

    /// Address (base vreg, byte offset) of a place expression.
    fn place(&mut self, e: &Expr) -> LR<(V, i32)> {
        let pre = self.prep(e)?;
        self.place_at(e, &mut pre.into_iter())
    }

    /// Phase 1 of a place: evaluate every value expression it mentions (list indices, temporaries),
    /// left to right, without forming any pointer into a list yet. Borrowed element addresses are only
    /// computed in phase 2 (`place_at`), after all arguments of the enclosing call are evaluated, so
    /// a borrowed element can never dangle because an argument grew the list.
    fn prep(&mut self, e: &Expr) -> LR<Vec<V>> {
        let saved = self.at_pos(e.pos);
        let r = self.prep_at(e);
        self.loc = saved;
        r
    }

    fn prep_at(&mut self, e: &Expr) -> LR<Vec<V>> {
        Ok(match &e.k {
            EK::Var(..) | EK::Move(..) => vec![],
            EK::Field(b, _) => self.prep(b)?,
            EK::Deref(p) => vec![self.expr(p)?],
            EK::Method { recv, m, args } if m == "at" => {
                let mut v = self.prep(recv)?;
                v.push(self.expr(&args[0])?);
                v
            }
            EK::SliceOf(b, lo, hi) => {
                // by-value bounds first, then the borrowed base (see the evaluation order in SPEC.md)
                let mut v = vec![];
                for x in lo.iter().chain(hi.iter()) {
                    v.push(self.expr(x)?);
                }
                v.extend(self.prep(b)?);
                v
            }
            EK::Method { recv, m, args }
                if matches!(m.as_str(), "get" | "locked_value") && args.is_empty() =>
            {
                self.prep(recv)?
            }
            EK::Payload(b, ..) => self.prep(b)?,
            _ => vec![self.expr(e)?],
        })
    }

    /// Phase 2 of a place: the address, consuming the values `prep` produced (bounds checks happen here).
    fn place_at(&mut self, e: &Expr, it: &mut std::vec::IntoIter<V>) -> LR<(V, i32)> {
        let saved = self.at_pos(e.pos);
        let r = self.place_in(e, it);
        self.loc = saved;
        r
    }

    fn place_in(&mut self, e: &Expr, it: &mut std::vec::IntoIter<V>) -> LR<(V, i32)> {
        match &e.k {
            EK::Var(_, id) | EK::Move(_, id) => match self.vars[id] {
                Loc::Mem(a) => Ok((a, 0)),
                Loc::Val(_) => {
                    Err("native backend: internal error, address of a register variable".into())
                }
            },
            EK::Field(b, f) => {
                let (base, off) = self.place_at(b, it)?;
                let Ty::Struct(sid) = b.ty else {
                    return Err("native backend: field of a non-struct".into());
                };
                let (fo, _) = self.field_off(sid, f);
                Ok((base, off + fo))
            }
            EK::Method { recv, m, .. } if m == "at" => {
                let (lb, lo) = self.place_at(recv, it)?;
                let la = self.at(lb, lo);
                let i = it.next().expect("prepared index");
                if let Some((et, n)) = self.types.array_of(recv.ty) {
                    return Ok((self.arr_elem_addr(la, i, et, n), 0));
                }
                Ok((self.elem_addr(la, i, e.ty), 0))
            }
            EK::SliceOf(b, lo, hi) => {
                // a List-shaped header {ptr, len, len} for the elements [lo, hi) of an array, List or slice
                let lo_v = lo.as_ref().map(|_| it.next().expect("prepared start"));
                let hi_v = hi.as_ref().map(|_| it.next().expect("prepared end"));
                let (bb, bo) = self.place_at(b, it)?;
                let ba = self.at(bb, bo);
                let (ptr, len, et) = match self.types.array_of(b.ty) {
                    Some((et, n)) => (ba, self.konst(n as i64), et),
                    None => {
                        let Ty::List(li) = b.ty else {
                            return Err("native backend: slice of a non-sequence".into());
                        };
                        (self.hload(ba, 0), self.hload(ba, 8), self.types.lists[li])
                    }
                };
                let start = match lo_v {
                    Some(v) => v,
                    None => self.konst(0),
                };
                let end = match hi_v {
                    Some(v) => v,
                    None => len,
                };
                if lo_v.is_some() || hi_v.is_some() {
                    // 0 <= start <= end <= len (the same checks as a List index: `Check(i, n)` is `0 <= i < n`)
                    let len1 = self.bink(Op::AddU, len, 1);
                    self.emit(Inst::Check(end, len1));
                    let end1 = self.bink(Op::AddU, end, 1);
                    self.emit(Inst::Check(start, end1));
                }
                let n = self.bin(Op::SubU, end, start);
                let p = self.elem_ptr(ptr, start, self.esz(et));
                let h = self.new_slot(3);
                self.emit(Inst::Store(h, 0, p));
                self.emit(Inst::Store(h, 8, n));
                self.emit(Inst::Store(h, 16, n));
                Ok((h, 0))
            }
            EK::Deref(_) => Ok((it.next().expect("prepared address"), 0)),
            EK::Method { recv, m, args }
                if matches!(m.as_str(), "get" | "locked_value") && args.is_empty() =>
            {
                // `*b`: the cell the Box points to
                let (bb, bo) = self.place_at(recv, it)?;
                let p = self.load(bb, bo + if m == "locked_value" { 8 } else { 0 });
                if let Ty::Struct(id) = recv.ty {
                    if self.types.runtime_struct(id, "Shared").is_some() {
                        return Ok((p, 8)); // the first word is the reference count
                    }
                }
                Ok((p, 0))
            }
            EK::Payload(b, vid, k) => {
                let (base, off) = self.place_at(b, it)?;
                let Ty::Enum(eid) = b.ty else {
                    return Err("native backend: payload of a non-enum".into());
                };
                let (po, _) = self.payload_off(eid, *vid, *k);
                Ok((base, off + po))
            }
            _ => Ok((it.next().expect("prepared temporary"), 0)), // a temporary aggregate
        }
    }

    /// Bytes per list element (an element always takes at least one word).
    /// `Bool` elements are packed to one byte (`Load8`/`Store8`); everything else is word-sized.
    fn esz(&self, t: Ty) -> i64 {
        if t == Ty::Bool {
            1
        } else {
            8 * self.words(t).max(1) as i64
        }
    }
    /// A place that is one packed byte of a `List<Bool>` (`l.at(i)`): it cannot be read or written with
    /// word loads/stores and cannot be handed out as a pointer.
    fn byte_place(&self, e: &Expr) -> bool {
        matches!(&e.k, EK::Method { recv, m, .. } if m == "at" && self.types.seq_elem(recv.ty) == Some(Ty::Bool))
    }
    /// Bounds-checked address of element `i` of the array `[et; n]` at `base`.
    fn arr_elem_addr(&mut self, base: V, i: V, et: Ty, n: usize) -> V {
        let len = self.konst(n as i64);
        self.emit(Inst::Check(i, len));
        self.elem_ptr(base, i, self.esz(et))
    }
    /// Address of element `idx` (no check) of a buffer at `ptr` with `esz`-byte elements.
    fn elem_ptr(&mut self, ptr: V, idx: V, esz: i64) -> V {
        let d = self.v();
        if esz == 8 || esz == 1 {
            self.emit(Inst::LeaX(d, ptr, idx, esz as u8));
        } else {
            let sz = self.konst(esz);
            let off = self.bin(Op::MulU, idx, sz);
            self.emit(Inst::LeaX(d, ptr, off, 1));
        }
        d
    }
    /// Bounds-checked address of element `i` of the list at `la`.
    fn elem_addr(&mut self, la: V, i: V, et: Ty) -> V {
        let len = self.v();
        self.emit(Inst::LoadH(len, la, 8));
        self.emit(Inst::Check(i, len));
        let ptr = self.v();
        self.emit(Inst::LoadH(ptr, la, 0));
        self.elem_ptr(ptr, i, self.esz(et))
    }
    /// Load of a list/string header word (ptr 0, len 8, cap 16).
    fn hload(&mut self, base: V, off: i32) -> V {
        let d = self.v();
        self.emit(Inst::LoadH(d, base, off));
        d
    }
    fn load(&mut self, base: V, off: i32) -> V {
        let d = self.v();
        self.emit(Inst::Load(d, base, off));
        d
    }
    fn bin(&mut self, op: Op, a: V, b: V) -> V {
        let d = self.v();
        self.emit(Inst::Bin(d, op, a, b));
        d
    }
    fn bink(&mut self, op: Op, a: V, k: i64) -> V {
        let c = self.konst(k);
        self.bin(op, a, c)
    }
    fn assert_message(&mut self, c: V, message: &str) -> LR<()> {
        let zero = self.konst(0);
        let failed = self.bin(Op::Eq, c, zero);
        self.if_then(failed, |l| {
            l.texts.push(message.as_bytes().to_vec());
            let t = l.v();
            l.emit(Inst::Text(t, l.texts.len() - 1));
            let p = l.at(t, 8);
            let n = l.konst(message.len() as i64);
            l.emit(Inst::Panic(p, n));
            l.no_return();
            Ok(())
        })
    }

    /// The code after a `panic` never runs: end the block so the optimizer's facts do not flow past it.
    fn no_return(&mut self) {
        self.term(Term::Unreachable);
        let nb = self.block();
        self.switch(nb);
    }

    fn if_then(&mut self, c: V, f: impl FnOnce(&mut Self) -> LR<()>) -> LR<()> {
        let (t, j) = (self.block(), self.block());
        self.term(Term::Br(c, t, j));
        self.switch(t);
        f(self)?;
        self.term(Term::Jmp(j));
        self.switch(j);
        Ok(())
    }
    fn if_else(
        &mut self,
        c: V,
        f: impl FnOnce(&mut Self) -> LR<()>,
        g: impl FnOnce(&mut Self) -> LR<()>,
    ) -> LR<()> {
        let (t, e, j) = (self.block(), self.block(), self.block());
        self.term(Term::Br(c, t, e));
        self.switch(t);
        f(self)?;
        self.term(Term::Jmp(j));
        self.switch(e);
        g(self)?;
        self.term(Term::Jmp(j));
        self.switch(j);
        Ok(())
    }
    /// `while cond() { body() }`
    fn while_do(
        &mut self,
        cond: impl FnOnce(&mut Self) -> LR<V>,
        body: impl FnOnce(&mut Self) -> LR<()>,
    ) -> LR<()> {
        let (h, b, x) = (self.block(), self.block(), self.block());
        self.term(Term::Jmp(h));
        self.switch(h);
        let c = cond(self)?;
        self.term(Term::Br(c, b, x));
        self.switch(b);
        body(self)?;
        self.term(Term::Jmp(h));
        self.switch(x);
        Ok(())
    }
    fn zero_slot(&mut self, words: usize) -> V {
        let a = self.new_slot(words);
        let z = self.konst(0);
        for k in 0..words.max(1) as i32 {
            self.emit(Inst::Store(a, 8 * k, z));
        }
        a
    }
    /// An aggregate passed or stored by value: fresh temporaries are adopted, anything else is copied
    /// first (the source may live inside a list that is about to change).
    fn value_arg(&mut self, e: &Expr) -> LR<V> {
        let v = self.expr(e)?;
        Ok(if is_agg(e.ty) && !Self::fresh(e) {
            self.copy_temp(v, e.ty)
        } else {
            v
        })
    }
    /// Store/copy value `x` of type `ty` into memory at `addr`.
    fn put(&mut self, addr: V, x: V, ty: Ty) {
        if is_agg(ty) {
            let n = self.words(ty);
            self.emit(Inst::Copy(addr, x, n));
        } else {
            self.emit(Inst::Store(addr, 0, x));
        }
    }

    /// Store `x` as a list element of type `ty` at `addr` (packed byte for `Bool`).
    fn put_elem(&mut self, addr: V, x: V, ty: Ty) {
        if ty == Ty::Bool {
            self.emit(Inst::Store8(addr, 0, x));
        } else {
            self.put(addr, x, ty);
        }
    }
    /// Load a scalar list element of type `ty` from `addr`.
    fn load_elem(&mut self, addr: V, ty: Ty) -> V {
        if ty == Ty::Bool {
            let d = self.v();
            self.emit(Inst::Load8(d, addr, 0));
            d
        } else {
            self.load(addr, 0)
        }
    }

    // ── drop glue and scopes ──
    /// Release the heap memory held by a value of type `ty` at base+off.
    fn drop_value(&mut self, ty: Ty, base: V, off: i32) {
        if let Some((et, n)) = self.types.array_of(ty) {
            // `[T; N]` with owned elements: drop each element (a loop beyond a few)
            if !self.types.owned(et) {
                return;
            }
            let a = self.at(base, off);
            let esz = self.esz(et);
            if n <= 4 {
                for k in 0..n {
                    self.drop_value(et, a, (esz * k as i64) as i32);
                }
                return;
            }
            let i = self.v();
            self.emit(Inst::Const(i, 0));
            let (head, body, done) = (self.block(), self.block(), self.block());
            self.term(Term::Jmp(head));
            self.switch(head);
            let nk = self.konst(n as i64);
            let more = self.bin(Op::Lt, i, nk);
            self.term(Term::Br(more, body, done));
            self.switch(body);
            let addr = self.elem_ptr(a, i, esz);
            self.drop_value(et, addr, 0);
            let i1 = self.bink(Op::AddU, i, 1);
            self.emit(Inst::Mov(i, i1));
            self.term(Term::Jmp(head));
            self.switch(done);
            return;
        }
        match ty {
            Ty::String => {
                let a = self.at(base, off);
                self.emit(Inst::ListFree(a));
            }
            Ty::List(i) => {
                let a = self.at(base, off);
                if self.types.owned(self.types.lists[i]) {
                    let h = self.helpers.get(H_DROP, i);
                    self.emit(Inst::Call(None, h, vec![a]));
                } else {
                    self.emit(Inst::ListFree(a));
                }
            }
            Ty::Struct(sid) => {
                if self.types.is_lock_guard(sid) {
                    let address = self.load(base, off);
                    let unused = self.v();
                    self.emit(Inst::Thread(ThreadOp::Unlock, unused, vec![address]));
                    return;
                }
                if let Some(payload) = self.types.join_payload(sid) {
                    let handle = self.load(base, off);
                    let zero = self.konst(0);
                    let live = self.bin(Op::Ne, handle, zero);
                    let _ = self.if_then(live, |l| {
                        let result = l.v();
                        l.emit(Inst::Thread(ThreadOp::Wait, result, vec![handle]));
                        let has_result = l.bin(Op::Ne, result, zero);
                        l.if_then(has_result, |l| {
                            l.drop_value(payload, result, 0);
                            l.emit(Inst::Free(result));
                            Ok(())
                        })
                    });
                    return;
                }
                if let Some(payload) = self.types.runtime_struct(sid, "Shared") {
                    let cell = self.load(base, off);
                    let decrement = self.konst(-1);
                    let old = self.v();
                    self.emit(Inst::Atomic(AtomicOp::FetchAdd, old, vec![cell, decrement]));
                    let one = self.konst(1);
                    let last = self.bin(Op::Eq, old, one);
                    let _ = self.if_then(last, |l| {
                        l.drop_value(payload, cell, 8);
                        l.emit(Inst::Free(cell));
                        Ok(())
                    });
                    return;
                }
                if let Some(fname) = self.types.user_drop.get(&sid).cloned() {
                    // `impl Drop`: the user's drop runs first, with the value still whole
                    let a = self.at(base, off);
                    let id = self.ids[&fname];
                    let saved = self.guard_destructors.then(|| {
                        let saved = self.v();
                        self.emit(Inst::Thread(ThreadOp::DropEnter, saved, vec![]));
                        saved
                    });
                    self.emit(Inst::Call(None, id, vec![a]));
                    if let Some(saved) = saved {
                        let unused = self.v();
                        self.emit(Inst::Thread(ThreadOp::DropLeave, unused, vec![saved]));
                    }
                }
                let fields = self.types.structs[sid].fields.clone();
                for (n, f) in fields.into_iter().rev() {
                    if self.types.owned(f) {
                        let (fo, _) = self.field_off(sid, &n);
                        if let Some(flag) = self.types.capture_live_field(sid, &n) {
                            let (flag_off, _) = self.field_off(sid, &flag);
                            let live = self.load(base, off + flag_off);
                            let _ = self.if_then(live, |l| {
                                l.drop_value(f, base, off + fo);
                                Ok(())
                            });
                        } else {
                            self.drop_value(f, base, off + fo);
                        }
                    }
                }
            }
            Ty::Dyn(_) => {
                let data = self.load(base, off);
                let table = self.load(base, off + 8);
                let drop = self.table_fn(table, 0);
                self.emit(Inst::CallInd(None, drop, vec![data]));
            }
            Ty::Fn(_) => {
                // a closure with an environment: call the drop function stored in the block's first word
                // Each environment drop frees its block and pushes the environments of the closures it captured on
                // a worklist (a frame slot, heap-grown on first use), so a chain of closures that capture each other
                // is released in a loop instead of recursing.
                let env = self.load(base, off + 8);
                let z = self.konst(0);
                let has = self.bin(Op::Ne, env, z);
                let _ = self.if_then(has, |l| {
                    let la = l.zero_slot(3);
                    let cur = l.v();
                    l.emit(Inst::Mov(cur, env));
                    l.while_do(
                        |l| Ok(l.bin(Op::Ne, cur, z)),
                        |l| {
                            let d = l.load(cur, 0);
                            l.emit(Inst::CallInd(None, d, vec![cur, la]));
                            let len = l.hload(la, 8);
                            let more = l.bin(Op::Ne, len, z);
                            l.if_else(
                                more,
                                |l| {
                                    let n1 = l.bink(Op::SubU, len, 1);
                                    l.emit(Inst::Store(la, 8, n1));
                                    let ptr = l.hload(la, 0);
                                    let e = l.v();
                                    l.emit(Inst::LoadX(e, ptr, n1, 8));
                                    l.emit(Inst::Mov(cur, e));
                                    Ok(())
                                },
                                |l| {
                                    l.emit(Inst::Mov(cur, z));
                                    Ok(())
                                },
                            )
                        },
                    )?;
                    let cap = l.hload(la, 16);
                    let used = l.bin(Op::Ne, cap, z);
                    l.if_then(used, |l| {
                        l.emit(Inst::ListFree(la));
                        Ok(())
                    })
                });
            }
            Ty::Enum(eid) => {
                if self.types.owned(ty) {
                    let a = self.at(base, off);
                    let h = self.helpers.get(H_DROP_ENUM, eid);
                    self.emit(Inst::Call(None, h, vec![a]));
                }
            }
            Ty::Box(bi) => {
                let a = self.at(base, off);
                let h = self.helpers.get(H_DROP_BOX, bi);
                self.emit(Inst::Call(None, h, vec![a]));
            }
            _ => {}
        }
    }
    /// `if flag { drop }`
    fn drop_if_live(&mut self, o: &Owned) {
        let (yes, join) = (self.block(), self.block());
        self.term(Term::Br(o.flag, yes, join));
        self.switch(yes);
        self.emit(Inst::Const(o.flag, 0));
        if let Some(record) = o.cleanup {
            self.emit(Inst::Store(record, 0, o.flag));
        }
        self.drop_value(o.ty, o.addr, 0);
        self.term(Term::Jmp(join));
        self.switch(join);
    }
    fn drop_scope(&mut self, idx: usize) {
        for i in (0..self.scopes[idx].len()).rev() {
            let o = self.scopes[idx][i];
            if let Some(cleanup) = o.cleanup {
                let unused = self.v();
                self.emit(Inst::Thread(ThreadOp::CleanupPop, unused, vec![cleanup]));
            }
            self.drop_if_live(&o);
        }
    }
    /// Release every live owned local of every enclosing scope (before a `return`).
    fn drop_all(&mut self) {
        for i in (0..self.scopes.len()).rev() {
            self.drop_scope(i);
        }
    }
    /// Run a nested block: its owned locals are released at the end.
    fn scoped(&mut self, b: &[Stmt]) -> LR<()> {
        self.scopes.push(vec![]);
        self.stmts(b)?;
        self.drop_scope(self.scopes.len() - 1);
        self.scopes.pop();
        Ok(())
    }
    /// Register an owned variable (drop flag starts set).
    fn track(&mut self, id: usize, addr: V, ty: Ty) {
        let flag = self.konst(1);
        self.flags.insert(id, flag);
        let cleanup = if self.recover_panics {
            // A worker panic can reach this value through a linked record on the live native stack.
            let record = self.new_slot(4);
            let glue = self.helpers.cleanup(ty);
            let drop = self.v();
            let unused = self.v();
            self.emit(Inst::FnAddr(drop, glue));
            self.emit(Inst::Store(record, 0, flag));
            self.emit(Inst::Thread(
                ThreadOp::CleanupPush,
                unused,
                vec![record, addr, drop],
            ));
            self.cleanup_slots.insert(id, record);
            Some(record)
        } else {
            None
        };
        self.scopes.last_mut().unwrap().push(Owned {
            flag,
            addr,
            ty,
            cleanup,
        });
    }

    // ── statements ──
    fn stmts(&mut self, b: &[Stmt]) -> LR<()> {
        for s in b {
            self.stmt(s)?;
        }
        Ok(())
    }

    /// Introduce a variable holding `init`.
    fn declare(&mut self, id: usize, init: &Expr) -> LR<()> {
        supported(init.ty, "variables")?;
        if matches!(init.k, EK::Uninit) {
            // declared now, assigned later: an owned aggregate starts as "moved out" (no drop until assigned)
            if is_agg(init.ty) {
                let n = self.words(init.ty);
                let a = self.new_slot(n);
                self.vars.insert(id, Loc::Mem(a));
                if self.types.owned(init.ty) {
                    self.track(id, a, init.ty);
                    let flag = self.flags[&id];
                    self.emit(Inst::Const(flag, 0));
                }
            } else if self.memvars.contains(&id) {
                let a = self.new_slot(1);
                let zero = self.konst(0);
                self.emit(Inst::Store(a, 0, zero));
                self.vars.insert(id, Loc::Mem(a));
            } else {
                let x = self.v();
                self.emit(Inst::Const(x, 0));
                self.vars.insert(id, Loc::Val(x));
            }
            return Ok(());
        }
        let v = self.expr(init)?;
        if is_agg(init.ty) {
            let a = if Self::fresh(init) {
                v
            } else {
                self.copy_temp(v, init.ty)
            };
            self.vars.insert(id, Loc::Mem(a));
            if self.types.owned(init.ty) {
                self.track(id, a, init.ty);
            }
        } else if self.memvars.contains(&id) {
            let a = self.new_slot(1);
            self.emit(Inst::Store(a, 0, v));
            self.vars.insert(id, Loc::Mem(a));
        } else {
            let x = self.v();
            self.emit(Inst::Mov(x, v));
            if let Ty::Range(lo, hi) = init.ty {
                // `init.ty` is the declared type of the variable: every assignment to it was checked against it
                self.vr.insert(x, (lo, hi));
                if check_ranges() {
                    self.range_check(x, lo, hi);
                }
            }
            self.vars.insert(id, Loc::Val(x));
        }
        Ok(())
    }

    /// Bind every leaf of a tuple pattern to a variable holding a copy of that part of the aggregate at `a`.
    fn unpack(&mut self, pat: &UnpackPat, a: V, off: i32, ty: Ty) -> LR<()> {
        match pat {
            UnpackPat::Bind(_, _, id, _) => {
                supported(ty, "variables")?;
                if is_agg(ty) {
                    let src = self.at(a, off);
                    let m = self.copy_temp(src, ty);
                    self.vars.insert(*id, Loc::Mem(m));
                    if self.types.owned(ty) {
                        self.track(*id, m, ty);
                    }
                } else {
                    let x = self.v();
                    self.emit(Inst::Load(x, a, off));
                    if self.memvars.contains(id) {
                        let m = self.new_slot(1);
                        self.emit(Inst::Store(m, 0, x));
                        self.vars.insert(*id, Loc::Mem(m));
                    } else {
                        self.vars.insert(*id, Loc::Val(x));
                    }
                }
            }
            UnpackPat::Tuple(items, _) => {
                let Ty::Struct(sid) = ty else {
                    return Err("native backend: tuple pattern on a non-tuple".into());
                };
                let fields = self.types.structs[sid].fields.clone();
                for (it, (fname, fty)) in items.iter().zip(fields) {
                    let (fo, _) = self.field_off(sid, &fname);
                    self.unpack(it, a, off + fo, fty)?;
                }
            }
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> LR<()> {
        let saved = self.at_pos(s.pos);
        let r = self.stmt_at(s);
        self.loc = saved;
        r
    }

    fn stmt_at(&mut self, s: &Stmt) -> LR<()> {
        match &s.k {
            SK::Let(_, _, init, id) => self.declare(*id, init)?,
            SK::Assign(target, val) => {
                supported(val.ty, "assignments")?;
                if let EK::Var(_, id) = &target.k {
                    if let Loc::Val(x) = self.vars[id] {
                        let v = self.expr(val)?;
                        self.emit(Inst::Mov(x, v));
                        return Ok(());
                    }
                }
                let v = self.expr(val)?;
                let (base, off) = self.place(target)?;
                if is_agg(val.ty) {
                    let mut guard = None;
                    if self.types.owned(val.ty) {
                        // the old value is released first (only if a local variable still owns it)
                        let flag = match &target.k {
                            EK::Var(_, id) => self.flags.get(id).copied(),
                            _ => None,
                        };
                        let capture_flag = match &target.k {
                            EK::Var(_, id) => self.capture_flags.get(id).copied(),
                            _ => None,
                        };
                        match (flag, capture_flag) {
                            (Some(f), _) => {
                                let o = Owned {
                                    flag: f,
                                    addr: base,
                                    ty: val.ty,
                                    cleanup: match &target.k {
                                        EK::Var(_, id) => self.cleanup_slots.get(id).copied(),
                                        _ => None,
                                    },
                                };
                                self.drop_if_live(&o);
                                guard = Some(f);
                            }
                            (_, Some(addr)) => {
                                let live = self.load(addr, 0);
                                let o = Owned {
                                    flag: live,
                                    addr: base,
                                    ty: val.ty,
                                    cleanup: None,
                                };
                                self.drop_if_live(&o);
                                let one = self.konst(1);
                                self.emit(Inst::Store(addr, 0, one));
                            }
                            _ => self.drop_value(val.ty, base, off),
                        }
                    }
                    if let Some(f) = guard {
                        self.emit(Inst::Const(f, 1));
                        if let EK::Var(_, id) = &target.k {
                            if let Some(&slot) = self.cleanup_slots.get(id) {
                                self.emit(Inst::Store(slot, 0, f));
                            }
                        }
                    }
                    let d = self.at(base, off);
                    let n = self.words(val.ty);
                    self.emit(Inst::Copy(d, v, n));
                } else if self.byte_place(target) {
                    self.emit(Inst::Store8(base, off, v));
                } else {
                    self.emit(Inst::Store(base, off, v));
                }
            }
            SK::ExprS(e) => {
                self.expr(e)?;
            }
            SK::If(c, t, el) => {
                let (tb, eb, jb) = (self.block(), self.block(), self.block());
                self.cond(c, tb, eb)?;
                self.switch(tb);
                self.scoped(t)?;
                self.term(Term::Jmp(jb));
                self.switch(eb);
                if let Some(e) = el {
                    self.scoped(e)?;
                }
                self.term(Term::Jmp(jb));
                self.switch(jb);
            }
            SK::While(c, body, meta) => {
                let (hb, bb, eb) = (self.block(), self.block(), self.block());
                // `continue` goes to the step (when there is one), otherwise straight to the test
                let cb = if meta.step.is_empty() {
                    hb
                } else {
                    self.block()
                };
                self.term(Term::Jmp(hb));
                self.switch(hb);
                self.cond(c, bb, eb)?;
                self.switch(bb);
                self.loops.push(LoopL {
                    id: meta.id,
                    depth: self.scopes.len(),
                    brk: eb,
                    cont: cb,
                });
                self.scoped(body)?;
                self.term(Term::Jmp(cb));
                if cb != hb {
                    self.switch(cb);
                    self.stmts(&meta.step)?;
                    self.term(Term::Jmp(hb));
                }
                self.loops.pop();
                self.switch(eb);
            }
            SK::Break(_, id) | SK::Continue(_, id) => {
                let lp = self
                    .loops
                    .iter()
                    .rev()
                    .find(|l| l.id == *id)
                    .expect("loop of break/continue");
                let (depth, target) = (
                    lp.depth,
                    if matches!(s.k, SK::Break(..)) {
                        lp.brk
                    } else {
                        lp.cont
                    },
                );
                // release everything declared inside the loop body, innermost first
                for i in (depth..self.scopes.len()).rev() {
                    self.drop_scope(i);
                }
                self.term(Term::Jmp(target));
                let nb = self.block(); // anything after the jump is unreachable
                self.switch(nb);
            }
            SK::Block(b, _) | SK::Unsafe(b) => self.scoped(b)?,
            SK::For(..) => unreachable!("`for` is rewritten by the checker"),
            SK::Unpack(pat, init) => {
                let a = self.expr(init)?;
                self.unpack(pat, a, 0, init.ty)?;
            }
            SK::Bind(_, mode, place, id) => {
                let (base, off) = self.place(place)?;
                let a = self.at(base, off);
                self.vars.insert(*id, Loc::Mem(a));
                if *mode == Mode::Own {
                    let EK::Field(env, capture) = &place.k else {
                        unreachable!("worker capture")
                    };
                    let Ty::Struct(sid) = env.ty else {
                        unreachable!("closure environment")
                    };
                    let (flag, _) = self.field_off(sid, &format!("live_{capture}"));
                    let live = self.at(base, flag);
                    self.capture_flags.insert(*id, live);
                }
            }
            SK::Return(opt) => {
                match opt {
                    Some(e) => {
                        supported(e.ty, "return values")?;
                        let v = self.expr(e)?;
                        if is_agg(e.ty) {
                            let n = self.words(e.ty);
                            let rp = self.ret_ptr.expect("hidden return pointer");
                            self.emit(Inst::Copy(rp, v, n));
                            self.drop_all();
                            self.term(Term::Ret(None));
                        } else {
                            self.drop_all();
                            self.term(Term::Ret(Some(v)));
                        }
                    }
                    None => {
                        self.drop_all();
                        self.term(Term::Ret(None));
                    }
                }
                let nb = self.block(); // anything after `return` is unreachable
                self.switch(nb);
            }
            SK::Defer(_) => unreachable!("`defer` is expanded by the checker"),
            SK::Match(scrut, arms) => {
                let Ty::Enum(eid) = scrut.ty else {
                    return Err("native backend: match on a non-enum".into());
                };
                let a = self.expr(scrut)?;
                let tag = self.v();
                self.emit(Inst::Load(tag, a, 0));
                let end = self.block();
                for arm in arms {
                    let (body, next) = (self.block(), self.block());
                    match arm.variant {
                        Some(vi) => {
                            let c = self.konst(vi as i64);
                            let eq = self.v();
                            self.emit(Inst::Bin(eq, Op::Eq, tag, c));
                            self.term(Term::Br(eq, body, next));
                        }
                        None => self.term(Term::Jmp(body)),
                    }
                    self.switch(body);
                    if let Some(vi) = arm.variant {
                        for (k, (_, bid)) in arm.binds.iter().enumerate() {
                            let (off, pty) = self.payload_off(eid, vi, k);
                            let loc = if is_agg(pty) {
                                let src = self.at(a, off);
                                Loc::Mem(self.copy_temp(src, pty))
                            } else if self.memvars.contains(bid) {
                                let x = self.v();
                                self.emit(Inst::Load(x, a, off));
                                let m = self.new_slot(1);
                                self.emit(Inst::Store(m, 0, x));
                                Loc::Mem(m)
                            } else {
                                let x = self.v();
                                self.emit(Inst::Load(x, a, off));
                                Loc::Val(x)
                            };
                            self.vars.insert(*bid, loc);
                        }
                    }
                    self.scoped(&arm.body)?;
                    self.term(Term::Jmp(end));
                    self.switch(next);
                }
                self.term(Term::Jmp(end)); // no arm matched (cannot happen: match is exhaustive)
                self.switch(end);
            }
        }
        Ok(())
    }

    // ── String and List<T> methods ──

    /// Phase 1 of a Text-or-String argument: a Text is evaluated now, a String place is only prepared.
    fn str_prep(&mut self, e: &Expr) -> LR<StrPre> {
        Ok(if e.ty == Ty::String {
            StrPre::Place(self.prep(e)?)
        } else {
            StrPre::Text(self.expr(e)?)
        })
    }
    /// Phase 2: (pointer to the bytes, length).
    fn str_view(&mut self, e: &Expr, pre: StrPre) -> LR<(V, V)> {
        Ok(match pre {
            StrPre::Text(t) => {
                let n = self.load(t, 0);
                let p = self.at(t, 8);
                (p, n)
            }
            StrPre::Place(idx) => {
                let (base, off) = self.place_at(e, &mut idx.into_iter())?;
                let sa = self.at(base, off);
                (self.load(sa, 0), self.load(sa, 8))
            }
        })
    }

    /// Reallocate the string in v0 (len, cap) to hold `need` bytes when `big` (need > cap): capacities are
    /// 2^k - 8 (first 56), the block is a power of two.
    fn str_grow(&mut self, len: V, cap: V, need: V, big: V) -> LR<()> {
        let l = self;
        l.if_then(big, |l| {
                        // capacities are 2^k - 8 (first 56): the block is a power of two
                        let nc = l.v();
                        l.emit(Inst::Mov(nc, cap));
                        let z = l.konst(0);
                        let isz = l.bin(Op::Eq, nc, z);
                        l.if_then(isz, |l| {
                            l.emit(Inst::Const(nc, 56));
                            Ok(())
                        })?;
                        l.while_do(
                            |l| Ok(l.bin(Op::Lt, nc, need)),
                            |l| {
                                let d = l.bink(Op::Mul, nc, 2);
                                let d = l.bink(Op::Add, d, 8);
                                l.emit(Inst::Mov(nc, d));
                                Ok(())
                            },
                        )?;
                        let old = l.hload(0, 0);
                        let nb = l.v();
                        l.emit(Inst::Realloc(nb, old, nc, len));
                        l.emit(Inst::Store(0, 0, nb));
                        l.emit(Inst::Store(0, 16, nc));
                        Ok(())
                    })
    }

    fn string_method(&mut self, recv: &Expr, m: &str, args: &[Expr]) -> LR<V> {
        // by-value arguments are evaluated before the receiver place (SPEC: borrow exception)
        let mut pre_apre = None;
        let mut pre_x = None;
        let mut pre_xs: Vec<V> = vec![];
        match m {
            "push_str" | "eq" => pre_apre = Some(self.str_prep(&args[0])?),
            "push_int" | "byte" | "push_byte" | "push_char" | "char_at" | "char_size" | "reserve" => {
                pre_x = Some(self.expr(&args[0])?)
            }
            "push_slice" => {
                // by-value arguments first (the two offsets), then the borrowed Text/String place
                pre_xs = vec![self.expr(&args[1])?, self.expr(&args[2])?];
                pre_apre = Some(self.str_prep(&args[0])?);
            }
            "push_raw" => pre_xs = vec![self.expr(&args[0])?, self.expr(&args[1])?],
            _ => {}
        }
        let rpre = self.prep(recv)?;
        match m {
            "len" => {
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                Ok(self.hload(b, o + 8))
            }
            "raw_addr" => {
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                Ok(self.hload(b, o))
            }
            "push_raw" => {
                let (p, n) = (pre_xs[0], pre_xs[1]);
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let h = self.helpers.get(H_STR_PUSH, 0);
                self.emit(Inst::Call(None, h, vec![la, p, n]));
                Ok(self.v())
            }
            "clear" => {
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let z = self.konst(0);
                self.emit(Inst::Store(b, o + 8, z));
                Ok(self.v())
            }
            "push_str" | "eq" => {
                let apre = pre_apre.take().unwrap();
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let (p, n) = self.str_view(&args[0], apre)?;
                if m == "push_str" {
                    let h = self.helpers.get(H_STR_PUSH, 0);
                    self.emit(Inst::Call(None, h, vec![la, p, n]));
                    Ok(self.v())
                } else {
                    let h = self.helpers.get(H_STR_EQ, 0);
                    let d = self.v();
                    self.emit(Inst::Call(Some(d), h, vec![la, p, n]));
                    Ok(d)
                }
            }
            "reserve" => {
                let x = pre_x.unwrap();
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let h = self.helpers.get(H_STR_RESERVE, 0);
                self.emit(Inst::Call(None, h, vec![la, x]));
                Ok(self.v())
            }
            "push_int" => {
                let x = pre_x.unwrap();
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let h = self.helpers.get(H_STR_PUSH_INT, 0);
                self.emit(Inst::Call(None, h, vec![la, x]));
                Ok(self.v())
            }
            "push_byte" => {
                let x = pre_x.unwrap();
                let k0 = self.konst(0);
                let ge = self.bin(Op::Ge, x, k0);
                self.assert_message(ge, "byte value out of range (0..255)")?;
                let k255 = self.konst(255);
                let le = self.bin(Op::Le, x, k255);
                self.assert_message(le, "byte value out of range (0..255)")?;
                // a static table with the 256 byte values: the byte to append is one entry of it
                let table: Vec<u8> = (0..=255u8).collect();
                let id = match self.texts.iter().position(|t| *t == table) {
                    Some(i) => i,
                    None => {
                        self.texts.push(table);
                        self.texts.len() - 1
                    }
                };
                let t = self.v();
                self.emit(Inst::Text(t, id));
                let base = self.at(t, 8);
                let p = self.bin(Op::Add, base, x);
                let one = self.konst(1);
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let h = self.helpers.get(H_STR_PUSH, 0);
                self.emit(Inst::Call(None, h, vec![la, p, one]));
                Ok(self.v())
            }
            "push_slice" => {
                let apre = pre_apre.take().unwrap();
                let (from, to) = (pre_xs[0], pre_xs[1]);
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let (p, n) = self.str_view(&args[0], apre)?;
                let n1 = self.bink(Op::Add, n, 1);
                self.emit(Inst::Check(to, n1));
                let to1 = self.bink(Op::Add, to, 1);
                self.emit(Inst::Check(from, to1));
                let p2 = self.bin(Op::Add, p, from);
                let len2 = self.bin(Op::Sub, to, from);
                let h = self.helpers.get(H_STR_PUSH, 0);
                self.emit(Inst::Call(None, h, vec![la, p2, len2]));
                Ok(self.v())
            }
            "push_char" | "char_at" | "char_size" => {
                let x = pre_x.unwrap();
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let h = self.helpers.get(
                    match m {
                        "push_char" => H_STR_PUSH_CHAR,
                        "char_at" => H_STR_CHAR_AT,
                        _ => H_STR_CHAR_SIZE,
                    },
                    0,
                );
                if m == "push_char" {
                    self.emit(Inst::Call(None, h, vec![la, x]));
                    Ok(self.v())
                } else {
                    let d = self.v();
                    self.emit(Inst::Call(Some(d), h, vec![la, x]));
                    Ok(d)
                }
            }
            "byte" => {
                let i = pre_x.unwrap();
                let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
                let la = self.at(b, o);
                let len = self.hload(la, 8);
                self.emit(Inst::Check(i, len));
                let ptr = self.hload(la, 0);
                let addr = self.v();
                self.emit(Inst::LeaX(addr, ptr, i, 1));
                let d = self.v();
                self.emit(Inst::Load8(d, addr, 0));
                Ok(d)
            }
            other => Err(format!(
                "native backend: String method '{}' is not supported yet",
                other
            )),
        }
    }

    /// `len/get/set/swap` of the array `[et; n]` at address `a` (arguments already evaluated).
    fn array_method(&mut self, a: V, et: Ty, n: usize, m: &str, argv: &[V]) -> LR<V> {
        match m {
            "len" => Ok(self.konst(n as i64)),
            "get" => {
                let addr = self.arr_elem_addr(a, argv[0], et, n);
                if is_agg(et) {
                    Ok(addr)
                } else {
                    Ok(self.load_elem(addr, et))
                }
            }
            "set" => {
                let addr = self.arr_elem_addr(a, argv[0], et, n);
                if self.types.owned(et) {
                    self.drop_value(et, addr, 0);
                }
                self.put_elem(addr, argv[1], et);
                Ok(self.v())
            }
            "swap" => {
                let x = self.arr_elem_addr(a, argv[0], et, n);
                let y = self.arr_elem_addr(a, argv[1], et, n);
                if is_agg(et) {
                    let t = self.copy_temp(x, et);
                    let w = self.words(et);
                    self.emit(Inst::Copy(x, y, w));
                    self.emit(Inst::Copy(y, t, w));
                } else {
                    let vx = self.load_elem(x, et);
                    let vy = self.load_elem(y, et);
                    self.put_elem(x, vy, et);
                    self.put_elem(y, vx, et);
                }
                Ok(self.v())
            }
            other => Err(format!(
                "native backend: array method '{}' is not supported",
                other
            )),
        }
    }

    fn list_method(&mut self, recv: &Expr, m: &str, args: &[Expr]) -> LR<V> {
        let li = match recv.ty {
            Ty::List(li) => li,
            t if self.types.array_of(t).is_some() => usize::MAX,
            _ => return Err("native backend: method on a non-list".into()),
        };
        let et = self.types.seq_elem(recv.ty).expect("list or array element");
        // by-value arguments first (left to right), then the list place
        let mut argv = vec![];
        for (k, a) in args.iter().enumerate() {
            argv.push(if m == "push" || (m == "set" && k == 1) {
                self.value_arg(a)?
            } else {
                self.expr(a)?
            });
        }
        let rpre = self.prep(recv)?;
        let (b, o) = self.place_at(recv, &mut rpre.into_iter())?;
        let la = self.at(b, o);
        if let Some((et, n)) = self.types.array_of(recv.ty) {
            return self.array_method(la, et, n, m, &argv);
        }
        match m {
            "len" => Ok(self.hload(la, 8)),
            "push" => {
                let x = argv[0];
                let (len, cap) = (self.hload(la, 8), self.hload(la, 16));
                let full = self.bin(Op::Eq, len, cap);
                let esz = self.esz(et);
                let h = self.helpers.get(H_GROW, 0);
                self.if_then(full, |l| {
                    let k = l.konst(esz);
                    l.emit(Inst::Call(None, h, vec![la, k]));
                    Ok(())
                })?;
                let ptr = self.hload(la, 0);
                if is_agg(et) {
                    let dst = self.elem_ptr(ptr, len, esz);
                    self.put(dst, x, et);
                } else {
                    self.emit(Inst::StoreX(
                        ptr,
                        len,
                        if et == Ty::Bool { 0 } else { 8 },
                        x,
                    ));
                }
                let n1 = self.bink(Op::AddU, len, 1);
                self.emit(Inst::Store(la, 8, n1));
                Ok(self.v())
            }
            "pop" => {
                let len = self.hload(la, 8);
                self.emit(Inst::CheckNonEmpty(len));
                let n1 = self.bink(Op::SubU, len, 1);
                self.emit(Inst::Store(la, 8, n1));
                let ptr = self.hload(la, 0);
                let addr = self.elem_ptr(ptr, n1, self.esz(et));
                if is_agg(et) {
                    Ok(self.copy_temp(addr, et))
                } else {
                    Ok(self.load_elem(addr, et))
                }
            }
            "get" => {
                if !is_agg(et) {
                    let len = self.hload(la, 8);
                    self.emit(Inst::Check(argv[0], len));
                    let ptr = self.hload(la, 0);
                    let d = self.v();
                    self.emit(Inst::LoadX(
                        d,
                        ptr,
                        argv[0],
                        if et == Ty::Bool { 0 } else { 8 },
                    ));
                    Ok(d)
                } else {
                    Ok(self.elem_addr(la, argv[0], et))
                }
            }
            "set" => {
                if !is_agg(et) {
                    let len = self.hload(la, 8);
                    self.emit(Inst::Check(argv[0], len));
                    let ptr = self.hload(la, 0);
                    self.emit(Inst::StoreX(
                        ptr,
                        argv[0],
                        if et == Ty::Bool { 0 } else { 8 },
                        argv[1],
                    ));
                    return Ok(self.v());
                }
                let addr = self.elem_addr(la, argv[0], et);
                if self.types.owned(et) {
                    self.drop_value(et, addr, 0);
                }
                self.put_elem(addr, argv[1], et);
                Ok(self.v())
            }
            "swap" => {
                let a = self.elem_addr(la, argv[0], et);
                let b = self.elem_addr(la, argv[1], et);
                if is_agg(et) {
                    let t = self.copy_temp(a, et);
                    let n = self.words(et);
                    self.emit(Inst::Copy(a, b, n));
                    self.emit(Inst::Copy(b, t, n));
                } else {
                    let x = self.load(a, 0);
                    let y = self.load(b, 0);
                    self.emit(Inst::Store(a, 0, y));
                    self.emit(Inst::Store(b, 0, x));
                }
                Ok(self.v())
            }
            "clear" => {
                if self.types.owned(et) {
                    let h = self.helpers.get(H_CLEAR, li);
                    self.emit(Inst::Call(None, h, vec![la]));
                } else {
                    let z = self.konst(0);
                    self.emit(Inst::Store(la, 8, z));
                }
                Ok(self.v())
            }
            other => Err(format!(
                "native backend: List method '{}' is not supported yet",
                other
            )),
        }
    }

    /// Lower a boolean condition straight into control flow: `&&`, `||` and `!` become
    /// separate branches per operand (left to right, short-circuit), so every comparison
    /// is its own `Br` and `rangeopt` can refine intervals on each edge. Ends the current
    /// block; the caller switches to `t` or `f`.
    fn cond(&mut self, e: &Expr, t: usize, f: usize) -> LR<()> {
        match &e.k {
            EK::Bin(op, a, b) if *op == "&&" || *op == "||" => {
                let mid = self.block();
                if *op == "&&" {
                    self.cond(a, mid, f)?;
                } else {
                    self.cond(a, t, mid)?;
                }
                self.switch(mid);
                self.cond(b, t, f)
            }
            EK::Un("!", x) => self.cond(x, f, t),
            _ => {
                let v = self.expr(e)?;
                self.term(Term::Br(v, t, f));
                Ok(())
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> LR<V> {
        let saved = self.at_pos(e.pos);
        let r = self.expr_at(e);
        self.loc = saved;
        r
    }

    fn expr_at(&mut self, e: &Expr) -> LR<V> {
        let v = self.expr_inner(e)?;
        // --check-ranges also verifies the flow facts the checker used: a read of a variable that has a range type
        // narrower than the variable's own
        if let (true, EK::Var(..), Ty::Range(lo, hi)) = (check_ranges(), &e.k, e.ty) {
            self.range_check(v, lo, hi);
        }
        if let Ty::Range(lo, hi) = e.ty {
            // the value just produced by a call or a load is of the (range) type of the expression
            if matches!(
                e.k,
                EK::Call(..) | EK::CallFn(..) | EK::Method { .. } | EK::Field(..) | EK::Payload(..)
            ) {
                if let Some(
                    Inst::Call(Some(d), ..)
                    | Inst::CallInd(Some(d), ..)
                    | Inst::ListGet(d, ..)
                    | Inst::Load(d, ..)
                    | Inst::LoadX(d, ..)
                    | Inst::Load8(d, ..),
                ) = self.blocks[self.cur].0.last()
                {
                    if *d == v {
                        self.vr.insert(v, (lo, hi));
                        if check_ranges() {
                            self.range_check(v, lo, hi);
                        }
                    }
                }
            }
        }
        Ok(v)
    }

    /// Emit a run-time test that `v` lies in `lo..=hi` (see `CHECK_RANGES`).
    fn range_check(&mut self, v: V, lo: i64, hi: i64) {
        let d = self.v();
        self.emit(Inst::N(NOp::Chk(Rng::Iv(lo, hi), MSG_CONV), d, v, v));
    }

    fn expr_inner(&mut self, e: &Expr) -> LR<V> {
        Ok(match &e.k {
            EK::Ascribe(_) => unreachable!("type ascriptions are erased by the checker"),
            EK::AsElse(..) => unreachable!("`as .. else` is rewritten by the checker"),
            EK::IfExpr(..) | EK::MatchExpr(..) => unreachable!("`if`/`match` values are rewritten by the checker"),
            EK::Uninit => unreachable!("an uninitialised variable only appears as a `let` initialiser"),
            EK::Sequence(stmts, result) => {
                self.scopes.push(vec![]);
                self.stmts(stmts)?;
                let value = self.expr(result)?;
                self.drop_scope(self.scopes.len() - 1);
                self.scopes.pop();
                value
            }
            EK::GenStruct { .. } | EK::GenVariant { .. } => {
                unreachable!("generic literal left by the checker")
            }
            EK::Int(n) => self.konst(*n),
            EK::Char(n) => self.konst(*n),
            EK::Float(..) | EK::Conv(..) => self.num_expr(e)?,
            EK::IsVariant(b, vid) => {
                let a = self.expr(b)?;
                let tag = self.load(a, 0);
                let c = self.konst(*vid as i64);
                self.bin(Op::Eq, tag, c)
            }
            EK::Payload(b, vid, k) => {
                let Ty::Enum(eid) = b.ty else {
                    return Err("native backend: payload of a non-enum".into());
                };
                let a = self.expr(b)?;
                let (off, pty) = self.payload_off(eid, *vid, *k);
                if is_agg(pty) {
                    self.at(a, off)
                } else {
                    self.load(a, off)
                }
            }
            EK::Bool(b) => self.konst(*b as i64),
            EK::Text(s) => {
                self.texts.push(s.as_bytes().to_vec());
                let id = self.texts.len() - 1;
                let d = self.v();
                self.emit(Inst::Text(d, id));
                d
            }
            EK::Var(_, id) | EK::Move(_, id) => {
                supported(e.ty, "values")?;
                if matches!(e.k, EK::Move(..)) && self.types.owned(e.ty) {
                    if let Some(&addr) = self.capture_flags.get(id) {
                        let zero = self.konst(0);
                        self.emit(Inst::Store(addr, 0, zero));
                    } else {
                        let f = self.flags[id];
                        self.emit(Inst::Const(f, 0)); // moved out: the new owner frees it
                        if let Some(&slot) = self.cleanup_slots.get(id) {
                            self.emit(Inst::Store(slot, 0, f));
                        }
                    }
                }
                match self.vars[id] {
                    Loc::Val(x) => x,
                    Loc::Mem(a) if is_agg(e.ty) => a,
                    Loc::Mem(a) => {
                        let d = self.v();
                        self.emit(Inst::Load(d, a, 0));
                        d
                    }
                }
            }
            EK::Field(..) => {
                supported(e.ty, "values")?;
                let (base, off) = self.place(e)?;
                if is_agg(e.ty) {
                    self.at(base, off)
                } else {
                    let d = self.v();
                    self.emit(Inst::Load(d, base, off));
                    d
                }
            }
            EK::StructLit { sid, fields } => {
                let n = self.words(Ty::Struct(*sid));
                let a = self.new_slot(n);
                for (name, fe) in fields {
                    let (off, _) = self.field_off(*sid, name);
                    self.store_into(a, off, fe)?;
                }
                a
            }
            EK::ArrayLit(items) => {
                let (et, _) = self.types.array_of(e.ty).expect("array type");
                let a = self.new_slot(self.words(e.ty));
                let esz = self.esz(et) as i32;
                for (k, it) in items.iter().enumerate() {
                    let v = self.expr(it)?;
                    let off = esz * k as i32;
                    if et == Ty::Bool {
                        self.emit(Inst::Store8(a, off, v));
                    } else if is_agg(et) {
                        let d = self.at(a, off);
                        self.emit(Inst::Copy(d, v, self.words(et)));
                    } else {
                        self.emit(Inst::Store(a, off, v));
                    }
                }
                a
            }
            EK::ArrayFill(x, _) => {
                let (et, n) = self.types.array_of(e.ty).expect("array type");
                let v = self.expr(x)?;
                let a = self.new_slot(self.words(e.ty));
                // for i in 0..n: a[i] = v
                let i = self.v();
                self.emit(Inst::Const(i, 0));
                let (head, body, done) = (self.block(), self.block(), self.block());
                self.term(Term::Jmp(head));
                self.switch(head);
                let nk = self.konst(n as i64);
                let more = self.bin(Op::Lt, i, nk);
                self.term(Term::Br(more, body, done));
                self.switch(body);
                let addr = self.elem_ptr(a, i, self.esz(et));
                self.put_elem(addr, v, et);
                let i1 = self.bink(Op::AddU, i, 1);
                self.emit(Inst::Mov(i, i1));
                self.term(Term::Jmp(head));
                self.switch(done);
                a
            }
            EK::SliceOf(..) => {
                return Err(
                    "native backend: internal error, a slice outside of a call argument".into(),
                )
            }
            EK::Variant { eid, vid, args, .. } => {
                let n = self.words(Ty::Enum(*eid));
                let a = self.new_slot(n);
                let t = self.konst(*vid as i64);
                self.emit(Inst::Store(a, 0, t));
                for (k, ae) in args.iter().enumerate() {
                    let (off, _) = self.payload_off(*eid, *vid, k);
                    self.store_into(a, off, ae)?;
                }
                a
            }
            EK::Method { recv, m, .. } if m == "take" && matches!(recv.ty, Ty::Box(_)) => {
                let Ty::Box(bi) = recv.ty else { unreachable!() };
                let inner = self.types.boxes[bi];
                let a = self.expr(recv)?; // `Move(b)`: the Box variable gives up its cell
                let p = self.load(a, 0);
                let r = if is_agg(inner) {
                    self.copy_temp(p, inner)
                } else {
                    self.load(p, 0)
                };
                self.emit(Inst::Free(p));
                r
            }
            EK::Method { m, args, .. }
                if (m == "at")
                    || (matches!(m.as_str(), "get" | "locked_value") && args.is_empty()) =>
            {
                let (base, off) = self.place(e)?;
                if is_agg(e.ty) {
                    self.at(base, off)
                } else if self.byte_place(e) {
                    let a = self.at(base, off);
                    self.load_elem(a, Ty::Bool)
                } else {
                    self.load(base, off)
                }
            }
            EK::Method { recv, m, args } => match recv.ty {
                Ty::Char => {
                    let c = self.expr(recv)?;
                    if m == "to_int" {
                        c
                    } else {
                        // utf8_len: 1 + (c >= 0x80) + (c >= 0x800) + (c >= 0x10000)
                        let mut n = self.konst(1);
                        for lim in [0x80, 0x800, 0x10000] {
                            let k = self.konst(lim);
                            let ge = self.bin(Op::Ge, c, k);
                            n = self.bin(Op::Add, n, ge);
                        }
                        n
                    }
                }
                Ty::String => self.string_method(recv, m, args)?,
                _ => self.list_method(recv, m, args)?,
            },
            EK::Static {
                ty: Ty::String,
                f,
                args,
            } => match f.as_str() {
                "new" => self.zero_slot(3),
                "from" => {
                    let s = self.zero_slot(3);
                    let pre = self.str_prep(&args[0])?;
                    let (p, n) = self.str_view(&args[0], pre)?;
                    let h = self.helpers.get(H_STR_PUSH, 0);
                    self.emit(Inst::Call(None, h, vec![s, p, n]));
                    s
                }
                "concat" => {
                    let s = self.zero_slot(3);
                    let pa = self.str_prep(&args[0])?;
                    let pb = self.str_prep(&args[1])?;
                    let h = self.helpers.get(H_STR_PUSH, 0);
                    let (p, n) = self.str_view(&args[0], pa)?;
                    self.emit(Inst::Call(None, h, vec![s, p, n]));
                    let (p, n) = self.str_view(&args[1], pb)?;
                    self.emit(Inst::Call(None, h, vec![s, p, n]));
                    s
                }
                other => {
                    return Err(format!(
                        "native backend: String.{} is not supported yet",
                        other
                    ))
                }
            },
            EK::Static {
                ty: Ty::List(_), f, ..
            } if f == "new" => self.zero_slot(3),
            EK::Static {
                ty: Ty::Box(_),
                f,
                args,
            } if f == "from_runtime_word" => {
                let address = self.expr(&args[0])?;
                let b = self.new_slot(1);
                self.emit(Inst::Store(b, 0, address));
                b
            }
            EK::Static {
                ty: Ty::Box(bi),
                f,
                args,
            } if f == "new" => {
                let inner = self.types.boxes[*bi];
                let words = self.words(inner).max(1);
                // the value is evaluated first (it may allocate itself), then the cell is allocated and filled
                let v = self.value_arg(&args[0])?;
                let sz = self.konst(8 * words as i64);
                let p = self.v();
                self.emit(Inst::Alloc(p, sz));
                self.put(p, v, inner);
                let slot = self.new_slot(1);
                self.emit(Inst::Store(slot, 0, p));
                slot
            }
            EK::Static {
                ty: Ty::Char,
                f,
                args,
            } => {
                let i = self.expr(&args[0])?;
                // valid: 0 <= i <= 0x10FFFF and not a surrogate (0xD800..=0xDFFF)
                let (z, hi, s0, s1) = (
                    self.konst(0),
                    self.konst(0x10FFFF),
                    self.konst(0xD800),
                    self.konst(0xDFFF),
                );
                let a = self.bin(Op::Ge, i, z);
                let b = self.bin(Op::Le, i, hi);
                let c = self.bin(Op::Ge, i, s0);
                let d = self.bin(Op::Le, i, s1);
                let cd = self.bin(Op::Mul, c, d);
                let ncd = self.v();
                self.emit(Inst::Not(ncd, cd));
                let ab = self.bin(Op::Mul, a, b);
                let valid = self.bin(Op::Mul, ab, ncd);
                if f == "is_valid" {
                    valid
                } else {
                    self.assert_message(valid, "invalid Unicode scalar value")?;
                    i
                }
            }
            EK::Static { f, .. } => {
                return Err(format!(
                    "native backend: static function '{}' is not supported yet",
                    f
                ))
            }
            EK::Un(op, x) => {
                let xv = self.expr(x)?;
                if *op != "!" && !matches!(x.ty, Ty::Int | Ty::Range(..)) {
                    return Ok(self.num_neg(xv, x.ty));
                }
                let d = self.v();
                self.emit(if *op == "!" {
                    Inst::Not(d, xv)
                } else {
                    Inst::Neg(d, xv)
                });
                d
            }
            EK::Bin(op, a, b) if *op == "&&" || *op == "||" => {
                // short-circuit: r = a; if (r [or !r]) r = b
                let r = self.v();
                let av = self.expr(a)?;
                self.emit(Inst::Mov(r, av));
                let (rhs, end) = (self.block(), self.block());
                self.term(if *op == "&&" {
                    Term::Br(r, rhs, end)
                } else {
                    Term::Br(r, end, rhs)
                });
                self.switch(rhs);
                let bv = self.expr(b)?;
                self.emit(Inst::Mov(r, bv));
                self.term(Term::Jmp(end));
                self.switch(end);
                r
            }
            EK::Bin(op, a, b) => {
                let (av, bv) = (self.expr(a)?, self.expr(b)?);
                if !matches!(a.ty, Ty::Int | Ty::Range(..) | Ty::Bool) {
                    return self.num_bin(op, av, bv, a.ty);
                }
                if matches!(*op, "+%" | "-%" | "*%") {
                    return self.num_bin(op, av, bv, a.ty);
                }
                let o = match *op {
                    "+" => Op::Add,
                    "-" => Op::Sub,
                    "*" => Op::Mul,
                    "/" => Op::Div,
                    "%" => Op::Mod,
                    "<" => Op::Lt,
                    "<=" => Op::Le,
                    ">" => Op::Gt,
                    ">=" => Op::Ge,
                    "==" => Op::Eq,
                    _ => Op::Ne,
                };
                let d = self.v();
                self.emit(Inst::Bin(d, o, av, bv));
                d
            }
            EK::Call(name, args) => match name.as_str() {
                "__sqrt" | "__bits" | "__from_bits" => {
                    let a = self.expr(&args[0])?;
                    if name == "__sqrt" {
                        self.n1(NOp::FSqrt, a)
                    } else {
                        a // reinterpretation: the bit pattern already is the value
                    }
                }
                "assert" => {
                    if matches!(&args[0].k, EK::Bin(op, ..) if *op == "&&" || *op == "||")
                        || matches!(&args[0].k, EK::Un("!", _))
                    {
                        // a compound condition becomes one branch per operand (see `cond`): the optimizer
                        // learns each comparison on the passing path
                        let (ok, bad) = (self.block(), self.block());
                        self.cond(&args[0], ok, bad)?;
                        self.switch(bad);
                        let zero = self.konst(0);
                        self.emit(Inst::Assert(zero));
                        self.term(Term::Unreachable);
                        self.switch(ok);
                    } else {
                        let c = self.expr(&args[0])?;
                        self.emit(Inst::Assert(c));
                    }
                    self.v()
                }
                "panic" => {
                    let (p, n) = if args[0].ty == Ty::String {
                        let pre = self.prep(&args[0])?;
                        let (base, off) = self.place_at(&args[0], &mut pre.into_iter())?;
                        let la = self.at(base, off);
                        (self.hload(la, 0), self.hload(la, 8))
                    } else {
                        let t = self.expr(&args[0])?;
                        (self.at(t, 8), self.load(t, 0))
                    };
                    self.emit(Inst::Panic(p, n));
                    self.no_return();
                    self.v()
                }
                "__atomic_load" | "__atomic_store" | "__atomic_cmpxchg" | "__atomic_fetch_add" => {
                    let op = match name.as_str() {
                        "__atomic_load" => AtomicOp::Load,
                        "__atomic_store" => AtomicOp::Store,
                        "__atomic_cmpxchg" => AtomicOp::CompareExchange,
                        _ => AtomicOp::FetchAdd,
                    };
                    let mut operands = Vec::with_capacity(args.len());
                    for arg in args {
                        operands.push(self.expr(arg)?);
                    }
                    let d = self.v();
                    self.emit(Inst::Atomic(op, d, operands));
                    d
                }
                "__thread_pack" | "__box_address" => {
                    let b = self.expr(&args[0])?;
                    self.load(b, 0)
                }
                "__check_shared_content" => self.v(),
                "__locked_unbox" => {
                    let lock = self.value_arg(&args[0])?;
                    let mutex = self.load(lock, 0);
                    let value = self.load(lock, 8);
                    self.emit(Inst::Free(mutex));
                    let b = self.new_slot(1);
                    self.emit(Inst::Store(b, 0, value));
                    b
                }
                "__parallel_address" => self.expr(&args[0])?,
                "__parallel_call" => {
                    let body = self.expr(&args[0])?;
                    let index = self.expr(&args[1])?;
                    let code = self.load(body, 0);
                    let env = self.load(body, 8);
                    self.emit(Inst::CallInd(None, code, vec![env, index]));
                    self.v()
                }
                "__thread_start" | "__thread_start_recover" | "__thread_wait" => {
                    let arg = self.value_arg(&args[0])?;
                    let (op, operands) = match name.as_str() {
                        "__thread_wait" => (ThreadOp::Wait, vec![arg]),
                        _ => {
                            let op = if name == "__thread_start" {
                                ThreadOp::Start
                            } else {
                                ThreadOp::StartRecover
                            };
                            (op, vec![self.load(arg, 0), self.load(arg, 8)])
                        }
                    };
                    let d = self.v();
                    self.emit(Inst::Thread(op, d, operands));
                    d
                }
                "__futex_park" | "__futex_wake" => {
                    let address = self.expr(&args[0])?;
                    let value = self.expr(&args[1])?;
                    let op = if name == "__futex_park" {
                        ThreadOp::Park
                    } else {
                        ThreadOp::Wake
                    };
                    let d = self.v();
                    self.emit(Inst::Thread(op, d, vec![address, value]));
                    d
                }
                "__syscall" => {
                    let mut vs = vec![];
                    for a in args {
                        vs.push(self.expr(a)?);
                    }
                    let d = self.v();
                    self.emit(Inst::Syscall(d, vs));
                    d
                }
                "__global" => {
                    let n = match args[0].k {
                        EK::Int(n) => n as usize,
                        _ => 0,
                    };
                    let d = self.v();
                    self.emit(Inst::Global(d, n));
                    d
                }
                "__alloc" => {
                    let n = self.expr(&args[0])?;
                    let d = self.v();
                    self.emit(Inst::Alloc(d, n));
                    d
                }
                "__free" => {
                    let p = self.expr(&args[0])?;
                    self.emit(Inst::Free(p));
                    self.v()
                }
                "__ptr_null" => self.konst(0),
                "__ptr_alloc" => {
                    // Ptr<T>.alloc(n): n elements of `size` bytes
                    let n = self.expr(&args[0])?;
                    let size = self.expr(&args[1])?;
                    let bytes = self.bin(Op::Mul, n, size);
                    let d = self.v();
                    self.emit(Inst::Alloc(d, bytes));
                    d
                }
                "__ptr_free" => {
                    // null is a no-op, like C's free
                    let p = self.expr(&args[0])?;
                    let z = self.konst(0);
                    let c = self.bin(Op::Ne, p, z);
                    self.if_then(c, |l| {
                        l.emit(Inst::Free(p));
                        Ok(())
                    })?;
                    self.v()
                }
                "__ptr_is_null" => {
                    let p = self.expr(&args[0])?;
                    let z = self.konst(0);
                    self.bin(Op::Eq, p, z)
                }
                "__load8" | "__load64" => {
                    let p = self.expr(&args[0])?;
                    let d = self.v();
                    self.emit(if name == "__load8" {
                        Inst::Load8(d, p, 0)
                    } else {
                        Inst::Load(d, p, 0)
                    });
                    d
                }
                "__store8" | "__store64" => {
                    let p = self.expr(&args[0])?;
                    let x = self.expr(&args[1])?;
                    self.emit(if name == "__store8" {
                        Inst::Store8(p, 0, x)
                    } else {
                        Inst::Store(p, 0, x)
                    });
                    self.v()
                }
                "print" => {
                    if args[0].ty == Ty::String {
                        let pre = self.prep(&args[0])?;
                        let (base, off) = self.place_at(&args[0], &mut pre.into_iter())?;
                        let la = self.at(base, off);
                        let (p, n) = (self.hload(la, 0), self.hload(la, 8));
                        self.emit(Inst::PrintBytes(p, n));
                        return Ok(self.v());
                    }
                    if args[0].ty == Ty::Char {
                        let c = self.expr(&args[0])?;
                        let s = self.zero_slot(3);
                        let h = self.helpers.get(H_STR_PUSH_CHAR, 0);
                        self.emit(Inst::Call(None, h, vec![s, c]));
                        let (p, n) = (self.load(s, 0), self.load(s, 8));
                        self.emit(Inst::PrintBytes(p, n));
                        self.emit(Inst::ListFree(s));
                        return Ok(self.v());
                    }
                    let a = self.expr(&args[0])?;
                    self.emit(match args[0].ty {
                        Ty::Text => Inst::PrintText(a),
                        Ty::Bool => Inst::PrintBool(a),
                        _ => Inst::PrintInt(a),
                    });
                    self.v()
                }
                _ => {
                    let mut modes: Vec<(Mode, Ty)> = self.sigs[name].params.clone();
                    // extra arguments of a C variadic call are passed by value
                    for a in args.iter().skip(modes.len()) {
                        modes.push((Mode::Own, a.ty));
                    }
                    self.lower_call(CallTarget::Direct(name), args, &modes, e.ty)?
                }
            },
            EK::MkClosure { func, env, caps } => {
                let code = self.v();
                self.emit(Inst::FnAddr(code, self.ids[func]));
                let out = self.new_slot(2);
                self.emit(Inst::Store(out, 0, code));
                match env {
                    None => {
                        let z = self.konst(0);
                        self.emit(Inst::Store(out, 8, z));
                    }
                    Some(sid) => {
                        let bytes = 8 * self.words(Ty::Struct(*sid)) as i64;
                        let n = self.konst(bytes);
                        let p = self.v();
                        self.emit(Inst::Alloc(p, n));
                        let h = self.helpers.get(H_DROP_ENV, *sid);
                        let d = self.v();
                        self.emit(Inst::FnAddr(d, h));
                        self.emit(Inst::Store(p, 0, d));
                        for (k, c) in caps.iter().enumerate() {
                            let field = format!("c{k}");
                            let (off, _) = self.field_off(*sid, &field);
                            self.store_into(p, off, c)?;
                            if let Some(flag) = self.types.capture_live_field(*sid, &field) {
                                let (off, _) = self.field_off(*sid, &flag);
                                let one = self.konst(1);
                                self.emit(Inst::Store(p, off, one));
                            }
                        }
                        self.emit(Inst::Store(out, 8, p));
                    }
                }
                out
            }
            EK::CallFn(callee, args) => {
                let Ty::Fn(fi) = callee.ty else {
                    return Err("native backend: call of a non-function".into());
                };
                let modes = self.types.fns[fi].params.clone();
                self.lower_call(CallTarget::Closure(callee), args, &modes, e.ty)?
            }
            EK::DynBox { table, value } => {
                let v = self.value_arg(value)?;
                let bytes = 8 * (2 + self.words(value.ty).max(1));
                let size = self.konst(bytes as i64);
                let p = self.v();
                self.emit(Inst::Alloc(p, size));
                let data = self.at(p, 16);
                self.put(data, v, value.ty);
                self.emit(Inst::Store(p, 0, data));
                let vt = self.v();
                self.emit(Inst::VTableAddr(vt, *table));
                self.emit(Inst::Store(p, 8, vt));
                let slot = self.new_slot(1);
                self.emit(Inst::Store(slot, 0, p));
                slot
            }
            EK::DynCall { slot, sig, args } => {
                self.lower_call(CallTarget::Dyn(*slot), args, &sig.params, e.ty)?
            }
            EK::AddrOf(p, _) => {
                let (b, o) = self.place(p)?;
                self.at(b, o)
            }
            EK::Closure { .. } | EK::Deref(_) => {
                return Err("native backend: internal error, unexpected closure node".into())
            }
        })
    }

    /// A call: `name` (direct) or the function value `callee` (indirect). Arguments are evaluated in two phases
    /// (by-value ones first, then the addresses of borrowed places), see the evaluation order in SPEC.md.
    fn table_fn(&mut self, table: V, slot: usize) -> V {
        // The image stores table-relative code offsets, independent of the load address.
        let displacement = self.load(table, (8 * slot) as i32);
        self.bin(Op::AddU, table, displacement)
    }

    fn lower_call(
        &mut self,
        target: CallTarget<'_>,
        args: &[Expr],
        modes: &[(Mode, Ty)],
        ty: Ty,
    ) -> LR<V> {
        supported(ty, "return values")?;
        enum Arg {
            Val(V),
            Ref(Vec<V>),
        }
        let cpre = match target {
            CallTarget::Closure(callee) => self.prep(callee)?,
            _ => Vec::new(),
        };
        let mut pre = Vec::new();
        let mut borrowed = Vec::new();
        for (i, (a, (mode, pty))) in args.iter().zip(modes.iter()).enumerate() {
            if is_ref(self.types, *mode, *pty) {
                borrowed.push(i);
                pre.push(Arg::Ref(Vec::new()));
            } else {
                pre.push(Arg::Val(self.value_arg(a)?));
            }
        }
        for i in borrowed {
            pre[i] = Arg::Ref(self.prep(&args[i])?);
        }
        let mut vs = vec![];
        let ret_slot = if is_agg(ty) {
            let n = self.words(ty);
            let s = self.new_slot(n);
            vs.push(s);
            Some(s)
        } else {
            None
        };
        let mut code = None;
        if let CallTarget::Closure(c) = target {
            let (cb, co) = self.place_at(c, &mut cpre.into_iter())?;
            let ca = self.at(cb, co);
            code = Some(self.load(ca, 0));
            vs.push(self.load(ca, 8));
        }
        // phase 2: borrowed element addresses, bounds-checked, right before the call
        let mut writeback = vec![];
        for (i, (a, p)) in args.iter().zip(pre).enumerate() {
            match p {
                Arg::Val(v) => vs.push(v),
                Arg::Ref(idx) => {
                    let (base, off) = self.place_at(a, &mut idx.into_iter())?;
                    if let CallTarget::Dyn(slot) = target {
                        if i == 0 {
                            let data = self.load(base, off);
                            let table = self.load(base, off + 8);
                            code = Some(self.table_fn(table, slot + 1));
                            vs.push(data);
                            continue;
                        }
                    }
                    if self.byte_place(a) {
                        // a packed Bool element cannot be lent as a word: copy in, copy out
                        let tmp = self.new_slot(1);
                        let b = self.at(base, off);
                        let cur = self.load_elem(b, Ty::Bool);
                        self.emit(Inst::Store(tmp, 0, cur));
                        writeback.push((b, tmp));
                        vs.push(tmp);
                    } else {
                        vs.push(self.at(base, off));
                    }
                }
            }
        }
        let dst = if ty == Ty::Unit || ret_slot.is_some() {
            None
        } else {
            Some(self.v())
        };
        match (code, target) {
            (Some(c), _) => self.emit(Inst::CallInd(dst, c, vs)),
            (None, CallTarget::Direct(n)) if self.extern_ids.contains_key(n) => {
                let float_args = args
                    .iter()
                    .map(|a| crate::numeric::is_float(a.ty))
                    .collect();
                self.emit(Inst::ExternCall(
                    dst,
                    self.extern_ids[n],
                    vs,
                    float_args,
                    crate::numeric::is_float(ty),
                ))
            }
            (None, CallTarget::Direct(n)) => self.emit(Inst::Call(dst, self.ids[n], vs)),
            _ => unreachable!("call without a target"),
        }
        for (b, tmp) in writeback {
            let nv = self.load(tmp, 0);
            self.emit(Inst::Store8(b, 0, nv));
        }
        Ok(match (ret_slot, dst) {
            (Some(s), _) => s,
            (_, Some(d)) => d,
            _ => self.v(),
        })
    }
}

impl<'a> L<'a> {
    /// A struct whose drop is the plain field-by-field drop (no guard, handle, shared cell or closure environment).
    fn plain_struct(&self, sid: usize) -> bool {
        !self.types.is_lock_guard(sid)
            && !self.types.user_drop.contains_key(&sid)
            && self.types.join_payload(sid).is_none()
            && self.types.runtime_struct(sid, "Shared").is_none()
            && !self.types.structs[sid].cname.starts_with("Env__")
    }

    /// The most `Box<..>` cells of box type `bi` that dropping one value of `ty` can reach directly (through
    /// struct fields and enum payloads, not through other heap indirections). 0 means the box type is not
    /// self-recursive.
    fn box_fanout(&self, ty: Ty, bi: usize) -> usize {
        match ty {
            Ty::Box(b) => (b == bi) as usize,
            Ty::Struct(sid) if self.plain_struct(sid) => self.types.structs[sid]
                .fields
                .iter()
                .map(|(_, f)| self.box_fanout(*f, bi))
                .sum(),
            Ty::Enum(eid) => self.types.enums[eid]
                .variants
                .iter()
                .map(|(_, ps)| ps.iter().map(|p| self.box_fanout(*p, bi)).sum::<usize>())
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Drop a value of `ty` at base+off like `drop_value`, but instead of recursing into nested cells of box type
    /// `bi` hand them to `defer` (called with the cell pointer; the cell is then dropped and freed by the caller).
    fn drop_deferring(
        &mut self,
        ty: Ty,
        base: V,
        off: i32,
        bi: usize,
        defer: &mut dyn FnMut(&mut Self, V),
    ) {
        if self.box_fanout(ty, bi) == 0 {
            if self.types.owned(ty) {
                self.drop_value(ty, base, off);
            }
            return;
        }
        match ty {
            Ty::Box(_) => {
                let cell = self.load(base, off);
                defer(self, cell);
            }
            Ty::Struct(sid) => {
                let fields = self.types.structs[sid].fields.clone();
                for (n, f) in fields {
                    if self.types.owned(f) {
                        let (fo, _) = self.field_off(sid, &n);
                        self.drop_deferring(f, base, off + fo, bi, defer);
                    }
                }
            }
            Ty::Enum(eid) => {
                let def = self.types.enums[eid].clone();
                let tag = self.load(base, off);
                for (vi, (_, ps)) in def.variants.iter().enumerate() {
                    if !ps.iter().any(|p| self.types.owned(*p)) {
                        continue;
                    }
                    let c = self.konst(vi as i64);
                    let eq = self.bin(Op::Eq, tag, c);
                    let _ = self.if_then(eq, |l| {
                        for (k, p) in ps.iter().enumerate() {
                            if l.types.owned(*p) {
                                let (po, _) = l.payload_off(eid, vi, k);
                                l.drop_deferring(*p, base, off + po, bi, defer);
                            }
                        }
                        Ok(())
                    });
                }
            }
            _ => {}
        }
    }

    /// Drop of a self-recursive `Box` without recursion: nested cells of the same box type are not dropped
    /// by a nested call but visited by a loop. With one possible nested cell per node (lists, linked structures)
    /// the loop just follows the chain; with several (trees) the pending cells wait in a heap worklist.
    /// Either way the stack use is constant, so a chain of any length can be dropped.
    fn drop_box_iterative(&mut self, bi: usize, inner: Ty, fanout: usize) {
        let zero = self.konst(0);
        let cur = self.v();
        let first = self.load(0, 0); // v0 is the address of the box pointer
        self.emit(Inst::Mov(cur, first));
        if fanout == 1 {
            let next = self.v();
            self.emit(Inst::Mov(next, zero));
            let _ = self.while_do(
                |l| Ok(l.bin(Op::Ne, cur, zero)),
                |l| {
                    l.emit(Inst::Mov(next, zero));
                    l.drop_deferring(inner, cur, 0, bi, &mut |l, cell| {
                        l.emit(Inst::Mov(next, cell))
                    });
                    l.emit(Inst::Free(cur));
                    l.emit(Inst::Mov(cur, next));
                    Ok(())
                },
            );
        } else {
            // worklist: a list header {ptr, len, cap} in a frame slot, growing through the list helper
            let la = self.zero_slot(3);
            let grow = self.helpers.get(H_GROW, 0);
            let eight = self.konst(8);
            let push = |l: &mut Self, cell: V| {
                let (len, cap) = (l.hload(la, 8), l.hload(la, 16));
                let full = l.bin(Op::Eq, len, cap);
                let _ = l.if_then(full, |l| {
                    l.emit(Inst::Call(None, grow, vec![la, eight]));
                    Ok(())
                });
                let (len, ptr) = (l.hload(la, 8), l.hload(la, 0));
                l.emit(Inst::StoreX(ptr, len, 8, cell));
                let n1 = l.bink(Op::AddU, len, 1);
                l.emit(Inst::Store(la, 8, n1));
            };
            push(self, cur);
            let _ = self.while_do(
                |l| {
                    let len = l.hload(la, 8);
                    Ok(l.bin(Op::Ne, len, zero))
                },
                |l| {
                    let len = l.hload(la, 8);
                    let n1 = l.bink(Op::SubU, len, 1);
                    l.emit(Inst::Store(la, 8, n1));
                    let ptr = l.hload(la, 0);
                    let cell = l.v();
                    l.emit(Inst::LoadX(cell, ptr, n1, 8));
                    l.drop_deferring(inner, cell, 0, bi, &mut |l, c| push(l, c));
                    l.emit(Inst::Free(cell));
                    Ok(())
                },
            );
            self.emit(Inst::ListFree(la));
        }
    }

    /// Like `drop_deferring`, for a strongly connected component of heap nodes (see dropscc.rs): nested boxes
    /// and lists that belong to `members` are handed to `push(tag, a, b)` (a box cell: `a` = cell; a list
    /// buffer: `a` = buffer, `b` = length), everything else is dropped in place.
    fn drop_deferring_scc(
        &mut self,
        ty: Ty,
        base: V,
        off: i32,
        members: &[HeapNode],
        push: &mut dyn FnMut(&mut Self, i64, V, V),
    ) {
        if !self.types.reaches_node(ty, members) {
            if self.types.owned(ty) {
                self.drop_value(ty, base, off);
            }
            return;
        }
        match ty {
            Ty::Box(b) => {
                let tag = members
                    .iter()
                    .position(|m| *m == HeapNode::Box(b))
                    .expect("box member") as i64;
                let cell = self.load(base, off);
                let zero = self.konst(0);
                push(self, tag, cell, zero);
            }
            Ty::List(l) => {
                let tag = members
                    .iter()
                    .position(|m| *m == HeapNode::List(l))
                    .expect("list member") as i64;
                let a = self.at(base, off);
                let (ptr, len) = (self.hload(a, 0), self.hload(a, 8));
                push(self, tag, ptr, len);
            }
            Ty::Struct(sid) => {
                let fields = self.types.structs[sid].fields.clone();
                for (n, f) in fields {
                    if self.types.owned(f) {
                        let (fo, _) = self.field_off(sid, &n);
                        self.drop_deferring_scc(f, base, off + fo, members, push);
                    }
                }
            }
            Ty::Enum(eid) => {
                let def = self.types.enums[eid].clone();
                let tag = self.load(base, off);
                for (vi, (_, ps)) in def.variants.iter().enumerate() {
                    if !ps.iter().any(|p| self.types.owned(*p)) {
                        continue;
                    }
                    let c = self.konst(vi as i64);
                    let eq = self.bin(Op::Eq, tag, c);
                    let _ = self.if_then(eq, |l| {
                        for (k, p) in ps.iter().enumerate() {
                            if l.types.owned(*p) {
                                let (po, _) = l.payload_off(eid, vi, k);
                                l.drop_deferring_scc(*p, base, off + po, members, push);
                            }
                        }
                        Ok(())
                    });
                }
            }
            _ => {}
        }
    }

    /// Drop the node `(tag, a, b)` and everything below it with a heap worklist of 3-word items
    /// `{tag, a, b}` (tag = index in `members`): constant native stack for any shape of recursion between
    /// the member boxes and lists.
    fn drop_scc_run(&mut self, members: &[HeapNode], tag: i64, a: V, b: V) {
        let la = self.zero_slot(3);
        let grow = self.helpers.get(H_GROW, 0);
        let e24 = self.konst(24);
        let zero = self.konst(0);
        let mut push = |l: &mut Self, tag: i64, a: V, b: V| {
            let (len, cap) = (l.hload(la, 8), l.hload(la, 16));
            let full = l.bin(Op::Eq, len, cap);
            let _ = l.if_then(full, |l| {
                l.emit(Inst::Call(None, grow, vec![la, e24]));
                Ok(())
            });
            let (len, ptr) = (l.hload(la, 8), l.hload(la, 0));
            let i3 = l.bink(Op::MulU, len, 3);
            let tv = l.konst(tag);
            l.emit(Inst::StoreX(ptr, i3, 8, tv));
            let i31 = l.bink(Op::AddU, i3, 1);
            l.emit(Inst::StoreX(ptr, i31, 8, a));
            let i32_ = l.bink(Op::AddU, i3, 2);
            l.emit(Inst::StoreX(ptr, i32_, 8, b));
            let n1 = l.bink(Op::AddU, len, 1);
            l.emit(Inst::Store(la, 8, n1));
        };
        push(self, tag, a, b);
        let _ = self.while_do(
            |l| {
                let len = l.hload(la, 8);
                Ok(l.bin(Op::Ne, len, zero))
            },
            |l| {
                let len = l.hload(la, 8);
                let n1 = l.bink(Op::SubU, len, 1);
                l.emit(Inst::Store(la, 8, n1));
                let ptr = l.hload(la, 0);
                let i3 = l.bink(Op::MulU, n1, 3);
                let (tg, ia, ib) = (l.v(), l.v(), l.v());
                l.emit(Inst::LoadX(tg, ptr, i3, 8));
                let i31 = l.bink(Op::AddU, i3, 1);
                l.emit(Inst::LoadX(ia, ptr, i31, 8));
                let i32_ = l.bink(Op::AddU, i3, 2);
                l.emit(Inst::LoadX(ib, ptr, i32_, 8));
                for (k, node) in members.iter().enumerate() {
                    let kv = l.konst(k as i64);
                    let eq = l.bin(Op::Eq, tg, kv);
                    let node = *node;
                    let _ = l.if_then(eq, |l| {
                        match node {
                            HeapNode::Box(bi) => {
                                let inner = l.types.boxes[bi];
                                l.drop_deferring_scc(inner, ia, 0, members, &mut push);
                                l.emit(Inst::Free(ia));
                            }
                            HeapNode::List(li) => {
                                let et = l.types.lists[li];
                                let esz = l.esz(et);
                                let i = l.konst(0);
                                l.while_do(
                                    |l| Ok(l.bin(Op::Lt, i, ib)),
                                    |l| {
                                        let addr = l.elem_ptr(ia, i, esz);
                                        l.drop_deferring_scc(et, addr, 0, members, &mut push);
                                        let n = l.bink(Op::Add, i, 1);
                                        l.emit(Inst::Mov(i, n));
                                        Ok(())
                                    },
                                )?;
                                l.emit(Inst::Free(ia));
                            }
                        }
                        Ok(())
                    });
                }
                Ok(())
            },
        );
        self.emit(Inst::ListFree(la));
    }

    /// Body of a runtime helper function (see `Helpers`). Parameters are vregs 0..n.
    fn helper_body(&mut self, kind: u8, key: usize) -> LR<Option<V>> {
        match kind {
            H_PANIC_DROP => {
                self.drop_value(self.helpers.cleanup_types[key], 0, 0);
                Ok(None)
            }
            H_DROP_ENV => {
                // v0 = the environment block, v1 = the caller's closure worklist (see `drop_value` for `Fn`):
                // drop what it captured, hand the captured closures' environments to the worklist, free the block
                let fields = self.types.structs[key].fields.clone();
                for (n, f) in fields.iter() {
                    if !self.types.owned(*f) {
                        continue;
                    }
                    let (fo, _) = self.field_off(key, n);
                    let flag = self.types.capture_live_field(key, n);
                    let body = |l: &mut Self| -> LR<()> {
                        if matches!(f, Ty::Fn(_)) {
                            // no worklist (v1 == 0, the worker thread's exit path): drop the closure right here
                            let z = l.konst(0);
                            let wl_given = l.bin(Op::Ne, 1, z);
                            let (push_b, drop_b, join_b) = (l.block(), l.block(), l.block());
                            l.term(Term::Br(wl_given, push_b, drop_b));
                            l.switch(drop_b);
                            l.drop_value(*f, 0, fo);
                            l.term(Term::Jmp(join_b));
                            l.switch(push_b);
                            let e = l.load(0, fo + 8);
                            let has = l.bin(Op::Ne, e, z);
                            l.if_then(has, |l| {
                                let grow = l.helpers.get(H_GROW, 0);
                                let (len, cap) = (l.hload(1, 8), l.hload(1, 16));
                                let full = l.bin(Op::Eq, len, cap);
                                l.if_then(full, |l| {
                                    let e8 = l.konst(8);
                                    l.emit(Inst::Call(None, grow, vec![1, e8]));
                                    Ok(())
                                })?;
                                let (len, ptr) = (l.hload(1, 8), l.hload(1, 0));
                                l.emit(Inst::StoreX(ptr, len, 8, e));
                                let n1 = l.bink(Op::AddU, len, 1);
                                l.emit(Inst::Store(1, 8, n1));
                                Ok(())
                            })?;
                            l.term(Term::Jmp(join_b));
                            l.switch(join_b);
                            Ok(())
                        } else {
                            l.drop_value(*f, 0, fo);
                            Ok(())
                        }
                    };
                    if let Some(flag) = flag {
                        let (flag_off, _) = self.field_off(key, &flag);
                        let live = self.load(0, flag_off);
                        self.if_then(live, body)?;
                    } else {
                        body(self)?;
                    }
                }
                self.emit(Inst::Free(0));
                Ok(None)
            }
            H_DROP | H_CLEAR if self.types.general_scc(HeapNode::List(key)).is_some() => {
                let members = self.types.general_scc(HeapNode::List(key)).unwrap();
                let tag = members
                    .iter()
                    .position(|m| *m == HeapNode::List(key))
                    .unwrap() as i64;
                let (ptr, len) = (self.hload(0, 0), self.hload(0, 8));
                if kind == H_CLEAR {
                    // the buffer is released (the list is left empty without capacity): same worklist as a drop
                    let z = self.konst(0);
                    self.emit(Inst::Store(0, 0, z));
                    self.emit(Inst::Store(0, 8, z));
                    self.emit(Inst::Store(0, 16, z));
                }
                self.drop_scc_run(&members, tag, ptr, len);
                Ok(None)
            }
            H_DROP | H_CLEAR => {
                let et = self.types.lists[key];
                let esz = self.esz(et);
                let (len, ptr) = (self.hload(0, 8), self.hload(0, 0));
                let i = self.konst(0);
                self.while_do(
                    |l| Ok(l.bin(Op::Lt, i, len)),
                    |l| {
                        let addr = l.elem_ptr(ptr, i, esz);
                        l.drop_value(et, addr, 0);
                        let n = l.bink(Op::Add, i, 1);
                        l.emit(Inst::Mov(i, n));
                        Ok(())
                    },
                )?;
                if kind == H_DROP {
                    self.emit(Inst::Free(ptr));
                } else {
                    let z = self.konst(0);
                    self.emit(Inst::Store(0, 8, z));
                }
                Ok(None)
            }
            H_DROP_ENUM => {
                let def = self.types.enums[key].clone();
                let tag = self.load(0, 0);
                for (vi, (_, ps)) in def.variants.iter().enumerate() {
                    if !ps.iter().any(|p| self.types.owned(*p)) {
                        continue;
                    }
                    let c = self.konst(vi as i64);
                    let eq = self.bin(Op::Eq, tag, c);
                    self.if_then(eq, |l| {
                        for (k, p) in ps.iter().enumerate() {
                            if l.types.owned(*p) {
                                let (off, _) = l.payload_off(key, vi, k);
                                l.drop_value(*p, 0, off);
                            }
                        }
                        Ok(())
                    })?;
                }
                Ok(None)
            }
            H_DROP_BOX if self.types.general_scc(HeapNode::Box(key)).is_some() => {
                let members = self.types.general_scc(HeapNode::Box(key)).unwrap();
                let tag = members
                    .iter()
                    .position(|m| *m == HeapNode::Box(key))
                    .unwrap() as i64;
                let cell = self.load(0, 0);
                let zero = self.konst(0);
                self.drop_scc_run(&members, tag, cell, zero);
                Ok(None)
            }
            H_DROP_BOX => {
                let inner = self.types.boxes[key];
                let fanout = self.box_fanout(inner, key);
                if fanout > 0 {
                    self.drop_box_iterative(key, inner, fanout);
                    return Ok(None);
                }
                let p = self.load(0, 0);
                if self.types.owned(inner) {
                    self.drop_value(inner, p, 0);
                }
                self.emit(Inst::Free(p));
                Ok(None)
            }
            H_GROW => {
                // v0 = list, v1 = element bytes. Payload sizes follow 2^k - 8 (blocks are powers of two):
                // 56, 120, 248, ... bytes; the capacity is however many elements that holds.
                let (cap, len, old) = (self.hload(0, 16), self.hload(0, 8), self.hload(0, 0));
                let nb = self.v();
                let zero = self.konst(0);
                let isz = self.bin(Op::Eq, cap, zero);
                self.if_else(
                    isz,
                    |l| {
                        l.emit(Inst::Const(nb, 56));
                        let k = l.konst(56);
                        let big = l.bin(Op::Gt, 1, k);
                        l.if_then(big, |l| {
                            let e8 = l.bink(Op::Mul, 1, 8);
                            l.emit(Inst::Mov(nb, e8));
                            Ok(())
                        })
                    },
                    |l| {
                        let used = l.bin(Op::Mul, cap, 1);
                        let two = l.bink(Op::Mul, used, 2);
                        let n = l.bink(Op::Add, two, 8);
                        l.emit(Inst::Mov(nb, n));
                        Ok(())
                    },
                )?;
                let ncap = self.bin(Op::Div, nb, 1);
                let used = self.bin(Op::Mul, len, 1);
                let nbuf = self.v();
                self.emit(Inst::Realloc(nbuf, old, nb, used));
                self.emit(Inst::Store(0, 0, nbuf));
                self.emit(Inst::Store(0, 16, ncap));
                Ok(None)
            }
            H_STR_PUSH => {
                // v0 = string, v1 = bytes, v2 = n
                let zero = self.konst(0);
                let nz = self.bin(Op::Ne, 2, zero);
                self.if_then(nz, |l| {
                    let (len, cap) = (l.hload(0, 8), l.hload(0, 16));
                    let need = l.bin(Op::Add, len, 2);
                    let big = l.bin(Op::Gt, need, cap);
                    l.str_grow(len, cap, need, big)?;
                    let buf = l.hload(0, 0);
                    let dst = l.v();
                    l.emit(Inst::LeaX(dst, buf, len, 1));
                    l.emit(Inst::MemCpy(dst, 1, 2));
                    l.emit(Inst::Store(0, 8, need));
                    Ok(())
                })?;
                Ok(None)
            }
            H_STR_RESERVE => {
                // v0 = string, v1 = extra bytes
                let zero = self.konst(0);
                let pos = self.bin(Op::Gt, 1, zero);
                self.if_then(pos, |l| {
                    let (len, cap) = (l.hload(0, 8), l.hload(0, 16));
                    let need = l.bin(Op::Add, len, 1);
                    let big = l.bin(Op::Gt, need, cap);
                    l.str_grow(len, cap, need, big)
                })?;
                Ok(None)
            }
            H_STR_PUSH_INT => {
                // v0 = string, v1 = value. Digits are written backwards into a 24-byte frame buffer.
                let buf = self.new_slot(3);
                let pos = self.konst(24);
                let x = self.v();
                self.emit(Inst::Mov(x, 1));
                let zero = self.konst(0);
                let neg = self.bin(Op::Lt, x, zero);
                // Keep the working value nonpositive, including INT64_MIN.
                let positive = self.bin(Op::Gt, x, zero);
                self.if_then(positive, |l| {
                    l.emit(Inst::Neg(x, x));
                    Ok(())
                })?;
                let table = self.v();
                let digits = (0..100)
                    .flat_map(|n| [b'0' + n / 10, b'0' + n % 10])
                    .collect();
                self.emit(Inst::Text(table, self.texts.len()));
                self.texts.push(digits);
                let pairs = self.at(table, 8);
                let (body, exit) = (self.block(), self.block());
                self.term(Term::Jmp(body));
                self.switch(body);
                // With x <= 0, q*100 - x is the next pair's value (0..=99).
                let q = self.bink(Op::Div, x, 100);
                let hundred = self.konst(100);
                let scaled = self.bin(Op::MulU, q, hundred);
                let pair = self.bin(Op::SubU, scaled, x);
                let digits = self.v();
                self.emit(Inst::LeaX(digits, pairs, pair, 2));
                let tens = self.v();
                let ones = self.v();
                self.emit(Inst::Load8(tens, digits, 0));
                self.emit(Inst::Load8(ones, digits, 1));
                // At most ten pairs fit in the 24-byte buffer.
                let p2 = self.bink(Op::SubU, pos, 2);
                self.emit(Inst::Mov(pos, p2));
                let addr = self.v();
                self.emit(Inst::LeaX(addr, buf, pos, 1));
                self.emit(Inst::Store8(addr, 0, tens));
                self.emit(Inst::Store8(addr, 1, ones));
                self.emit(Inst::Mov(x, q));
                let more = self.bin(Op::Ne, x, zero);
                self.term(Term::Br(more, body, exit));
                self.switch(exit);
                // Skip the padding zero of a single-digit leading pair.
                let ten = self.konst(10);
                let single = self.bin(Op::Lt, pair, ten);
                self.if_then(single, |l| {
                    let p1 = l.bink(Op::Add, pos, 1);
                    l.emit(Inst::Mov(pos, p1));
                    Ok(())
                })?;
                self.if_then(neg, |l| {
                    let p1 = l.bink(Op::Sub, pos, 1);
                    l.emit(Inst::Mov(pos, p1));
                    let addr = l.v();
                    l.emit(Inst::LeaX(addr, buf, pos, 1));
                    let minus = l.konst(45);
                    l.emit(Inst::Store8(addr, 0, minus));
                    Ok(())
                })?;
                let start = self.v();
                self.emit(Inst::LeaX(start, buf, pos, 1));
                let total = self.konst(24);
                let n = self.bin(Op::Sub, total, pos);
                let h = self.helpers.get(H_STR_PUSH, 0);
                self.emit(Inst::Call(None, h, vec![0, start, n]));
                Ok(None)
            }
            H_STR_PUSH_CHAR => {
                // v0 = string, v1 = character. UTF-8 bytes are built in a frame buffer, then appended.
                let buf = self.new_slot(1);
                let n = self.v();
                let c = 1;
                let k80 = self.konst(0x80);
                let lt1 = self.bin(Op::Lt, c, k80);
                self.if_else(
                    lt1,
                    |l| {
                        l.emit(Inst::Store8(buf, 0, c));
                        l.emit(Inst::Const(n, 1));
                        Ok(())
                    },
                    |l| {
                        let k800 = l.konst(0x800);
                        let lt2 = l.bin(Op::Lt, c, k800);
                        l.if_else(
                            lt2,
                            |l| {
                                let hi = l.bink(Op::Div, c, 64);
                                let b0 = l.bink(Op::Add, hi, 0xC0);
                                l.emit(Inst::Store8(buf, 0, b0));
                                let lo = l.bink(Op::Mod, c, 64);
                                let b1 = l.bink(Op::Add, lo, 0x80);
                                l.emit(Inst::Store8(buf, 1, b1));
                                l.emit(Inst::Const(n, 2));
                                Ok(())
                            },
                            |l| {
                                let k10000 = l.konst(0x10000);
                                let lt3 = l.bin(Op::Lt, c, k10000);
                                l.if_else(
                                    lt3,
                                    |l| {
                                        let hi = l.bink(Op::Div, c, 4096);
                                        let b0 = l.bink(Op::Add, hi, 0xE0);
                                        l.emit(Inst::Store8(buf, 0, b0));
                                        let m = l.bink(Op::Div, c, 64);
                                        let m = l.bink(Op::Mod, m, 64);
                                        let b1 = l.bink(Op::Add, m, 0x80);
                                        l.emit(Inst::Store8(buf, 1, b1));
                                        let lo = l.bink(Op::Mod, c, 64);
                                        let b2 = l.bink(Op::Add, lo, 0x80);
                                        l.emit(Inst::Store8(buf, 2, b2));
                                        l.emit(Inst::Const(n, 3));
                                        Ok(())
                                    },
                                    |l| {
                                        let hi = l.bink(Op::Div, c, 262144);
                                        let b0 = l.bink(Op::Add, hi, 0xF0);
                                        l.emit(Inst::Store8(buf, 0, b0));
                                        let m1 = l.bink(Op::Div, c, 4096);
                                        let m1 = l.bink(Op::Mod, m1, 64);
                                        let b1 = l.bink(Op::Add, m1, 0x80);
                                        l.emit(Inst::Store8(buf, 1, b1));
                                        let m2 = l.bink(Op::Div, c, 64);
                                        let m2 = l.bink(Op::Mod, m2, 64);
                                        let b2 = l.bink(Op::Add, m2, 0x80);
                                        l.emit(Inst::Store8(buf, 2, b2));
                                        let lo = l.bink(Op::Mod, c, 64);
                                        let b3 = l.bink(Op::Add, lo, 0x80);
                                        l.emit(Inst::Store8(buf, 3, b3));
                                        l.emit(Inst::Const(n, 4));
                                        Ok(())
                                    },
                                )
                            },
                        )
                    },
                )?;
                let h = self.helpers.get(H_STR_PUSH, 0);
                self.emit(Inst::Call(None, h, vec![0, buf, n]));
                Ok(None)
            }
            H_STR_CHAR_SIZE => {
                // v0 = string, v1 = byte index  ->  length in bytes of the character that starts there
                let len = self.load(0, 8);
                self.emit(Inst::Check(1, len));
                let ptr = self.load(0, 0);
                let addr = self.bin(Op::Add, ptr, 1);
                let b = self.v();
                self.emit(Inst::Load8(b, addr, 0));
                let res = self.v();
                let zero = self.konst(0);
                let k80 = self.konst(0x80);
                let lt1 = self.bin(Op::Lt, b, k80);
                self.if_else(
                    lt1,
                    |l| {
                        l.emit(Inst::Const(res, 1));
                        Ok(())
                    },
                    |l| {
                        // 0x80..0xBF is a continuation byte: `i` is not at a character boundary
                        let kc0 = l.konst(0xC0);
                        let ge = l.bin(Op::Ge, b, kc0);
                        l.assert_message(ge, "invalid UTF-8 leading byte")?;
                        let kf8 = l.konst(0xF8);
                        let lt = l.bin(Op::Lt, b, kf8);
                        l.assert_message(lt, "invalid UTF-8 leading byte")?;
                        let _ = zero;
                        let kf0 = l.konst(0xF0);
                        let ge4 = l.bin(Op::Ge, b, kf0);
                        let ke0 = l.konst(0xE0);
                        let ge3 = l.bin(Op::Ge, b, ke0);
                        // 2 + (b >= 0xE0) + (b >= 0xF0)
                        let t = l.bink(Op::Add, ge3, 2);
                        let r = l.bin(Op::Add, t, ge4);
                        l.emit(Inst::Mov(res, r));
                        Ok(())
                    },
                )?;
                Ok(Some(res))
            }
            H_STR_CHAR_AT => {
                // v0 = string, v1 = byte index  ->  the character that starts there
                let h = self.helpers.get(H_STR_CHAR_SIZE, 0);
                let size = self.v();
                self.emit(Inst::Call(Some(size), h, vec![0, 1]));
                let ptr = self.load(0, 0);
                let addr = self.bin(Op::Add, ptr, 1);
                let b0 = self.v();
                self.emit(Inst::Load8(b0, addr, 0));
                let res = self.v();
                let one = self.konst(1);
                let is1 = self.bin(Op::Eq, size, one);
                self.if_else(
                    is1,
                    |l| {
                        l.emit(Inst::Mov(res, b0));
                        Ok(())
                    },
                    |l| {
                        // leading byte minus its marker (0xC0 / 0xE0 / 0xF0), then 6 bits per continuation byte
                        let base = l.v();
                        let two = l.konst(2);
                        let is2 = l.bin(Op::Eq, size, two);
                        l.if_else(
                            is2,
                            |l| {
                                l.emit(Inst::Const(base, 0xC0));
                                Ok(())
                            },
                            |l| {
                                let three = l.konst(3);
                                let is3 = l.bin(Op::Eq, size, three);
                                l.if_else(
                                    is3,
                                    |l| {
                                        l.emit(Inst::Const(base, 0xE0));
                                        Ok(())
                                    },
                                    |l| {
                                        l.emit(Inst::Const(base, 0xF0));
                                        Ok(())
                                    },
                                )
                            },
                        )?;
                        let c = l.v();
                        let d = l.bin(Op::Sub, b0, base);
                        l.emit(Inst::Mov(c, d));
                        let k = l.konst(1);
                        l.while_do(
                            |l| Ok(l.bin(Op::Lt, k, size)),
                            |l| {
                                let pa = l.bin(Op::Add, addr, k);
                                let b = l.v();
                                l.emit(Inst::Load8(b, pa, 0));
                                let c64 = l.bink(Op::Mul, c, 64);
                                let cont = l.bink(Op::Sub, b, 0x80);
                                let nc = l.bin(Op::Add, c64, cont);
                                l.emit(Inst::Mov(c, nc));
                                let k1 = l.bink(Op::Add, k, 1);
                                l.emit(Inst::Mov(k, k1));
                                Ok(())
                            },
                        )?;
                        l.emit(Inst::Mov(res, c));
                        Ok(())
                    },
                )?;
                Ok(Some(res))
            }
            H_STR_EQ => {
                // v0 = string, v1 = bytes, v2 = n  ->  Bool
                let res = self.konst(0);
                let len = self.hload(0, 8);
                let same = self.bin(Op::Eq, len, 2);
                self.if_then(same, |l| {
                    l.emit(Inst::Const(res, 1));
                    let buf = l.hload(0, 0);
                    let i = l.konst(0);
                    l.while_do(
                        |l| Ok(l.bin(Op::Lt, i, 2)),
                        |l| {
                            let (pa, pb) = (l.v(), l.v());
                            l.emit(Inst::LeaX(pa, buf, i, 1));
                            l.emit(Inst::LeaX(pb, 1, i, 1));
                            let (a, b) = (l.v(), l.v());
                            l.emit(Inst::Load8(a, pa, 0));
                            l.emit(Inst::Load8(b, pb, 0));
                            let ne = l.bin(Op::Ne, a, b);
                            l.if_else(
                                ne,
                                |l| {
                                    l.emit(Inst::Const(res, 0));
                                    l.emit(Inst::Mov(i, 2));
                                    Ok(())
                                },
                                |l| {
                                    let n = l.bink(Op::Add, i, 1);
                                    l.emit(Inst::Mov(i, n));
                                    Ok(())
                                },
                            )
                        },
                    )
                })?;
                Ok(Some(res))
            }
            _ => unreachable!(),
        }
    }
}

pub fn lower(prog: &Program) -> Result<IrProgram, String> {
    let ids: HashMap<String, usize> = prog
        .funcs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    let mut sigs: HashMap<String, Sig> = prog
        .funcs
        .iter()
        .map(|f| {
            (
                f.name.clone(),
                Sig {
                    params: f.params.iter().map(|p| (p.mode, p.ty)).collect(),
                },
            )
        })
        .collect();
    for f in &prog.externs {
        sigs.insert(
            f.name.clone(),
            Sig {
                params: f.params.iter().map(|p| (p.mode, p.ty)).collect(),
            },
        );
    }
    let extern_ids: HashMap<String, usize> = prog
        .externs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    let mut texts = vec![];
    let mut funcs = vec![];
    let mut loc_tab = LocTab::new();
    let lib_fn: Vec<bool> = prog.funcs.iter().map(|f| f.pos.line >= PRELUDE_LINE).collect();
    let mut helpers = Helpers {
        base: prog.funcs.len(),
        all: vec![],
        ids: HashMap::new(),
        cleanup_types: vec![],
    };
    let recover_panics = prog.has_recoverable_workers();
    for f in &prog.funcs {
        for p in &f.params {
            supported(p.ty, "parameters")?;
        }
        supported(f.ret, "return values")?;
        let mut memvars = HashSet::new();
        scan_stmts(&f.body, &sigs, &prog.types, &mut memvars);
        let mut l = L {
            scopes: vec![vec![]],
            recover_panics,
            guard_destructors: recover_panics,
            cleanup_slots: HashMap::new(),
            flags: HashMap::new(),
            capture_flags: HashMap::new(),
            blocks: vec![(vec![], None)],
            cur: 0,
            nv: 0,
            vr: HashMap::new(),
            slots: vec![],
            vars: HashMap::new(),
            memvars,
            ret_ptr: None,
            loops: vec![],
            types: &prog.types,
            sigs: &sigs,
            ids: &ids,
            texts: &mut texts,
            helpers: &mut helpers,
            extern_ids: &extern_ids,
            locs: &mut loc_tab,
            lib_fn: &lib_fn,
            loc: Pos::default(),
            bloc: vec![0],
        };
        // parameter vregs: [hidden return pointer], then one per declared parameter
        let hidden = is_agg(f.ret) as usize;
        let nparams = hidden + f.params.len();
        l.nv = nparams;
        if hidden == 1 {
            l.ret_ptr = Some(0);
        }
        for (i, p) in f.params.iter().enumerate() {
            let pv = hidden + i;
            let loc = if is_ref(&prog.types, p.mode, p.ty) || is_agg(p.ty) {
                Loc::Mem(pv) // pointer to the caller's variable, or to a private by-value copy
            } else if l.memvars.contains(&p.id) {
                let m = l.new_slot(1);
                l.emit(Inst::Store(m, 0, pv));
                Loc::Mem(m)
            } else {
                Loc::Val(pv)
            };
            l.vars.insert(p.id, loc);
            if p.mode == Mode::Own && prog.types.owned(p.ty) {
                if let Loc::Mem(a) = loc {
                    l.track(p.id, a, p.ty);
                }
            }
        }
        if check_ranges() {
            for (i, p) in f.params.iter().enumerate() {
                if let (Ty::Range(lo, hi), true) = (p.ty, p.mode != Mode::Mut && !is_agg(p.ty)) {
                    l.range_check(hidden + i, lo, hi);
                }
            }
        }
        l.stmts(&f.body)?;
        l.drop_scope(0);
        l.term(if f.ret == Ty::Unit {
            Term::Ret(None)
        } else {
            Term::Unreachable
        });
        let blocks = l
            .blocks
            .into_iter()
            .map(|(insts, t)| Block {
                insts,
                term: t.unwrap_or(Term::Unreachable),
            })
            .collect();
        let mut vranges = vec![None; l.nv];
        for (v, r) in &l.vr {
            vranges[*v] = Some(*r);
        }
        for (i, p) in f.params.iter().enumerate() {
            // a by-value scalar parameter: its vreg holds a value of the declared type
            if p.mode != Mode::Mut && !is_agg(p.ty) {
                if let Ty::Range(lo, hi) = p.ty {
                    vranges[hidden + i] = Some((lo, hi));
                }
            }
        }
        funcs.push(IrFunc {
            name: f.name.clone(),
            lib: f.pos.line >= PRELUDE_LINE,
            nparams,
            vranges,
            nvregs: l.nv,
            slots: l.slots,
            blocks,
        });
    }
    let vtables = prog
        .types
        .dyn_tables
        .iter()
        .enumerate()
        .map(|(table, entry)| {
            let mut slots = vec![helpers.get(H_DROP_DYN_DATA, table)];
            for name in &entry.methods {
                let target = ids[name];
                let (mode, ty) = sigs[name].params[0];
                let adapter = !is_ref(&prog.types, mode, ty) && !is_agg(ty);
                slots.push(if adapter {
                    helpers.get(H_DYN_ADAPTER, target)
                } else {
                    target
                });
            }
            slots
        })
        .collect();
    // runtime helpers requested while lowering (they may request more helpers themselves)
    let mut k = 0;
    while k < helpers.all.len() {
        let (kind, key) = helpers.all[k];
        let (name, nparams) = match kind {
            H_DROP_DYN_DATA => (format!("__drop_dyn_data{}", key), 1),
            H_DYN_ADAPTER => {
                let target = &funcs[key];
                (format!("__dyn_adapter_{}", target.name), target.nparams)
            }
            H_DROP => (format!("__drop_list{}", key), 1),
            H_CLEAR => (format!("__clear_list{}", key), 1),
            H_GROW => ("__list_grow".to_string(), 2),
            H_STR_PUSH => ("__str_push".to_string(), 3),
            H_STR_PUSH_INT => ("__str_push_int".to_string(), 2),
            H_STR_RESERVE => ("__str_reserve".to_string(), 2),
            H_STR_PUSH_CHAR => ("__str_push_char".to_string(), 2),
            H_STR_CHAR_SIZE => ("__str_char_size".to_string(), 2),
            H_STR_CHAR_AT => ("__str_char_at".to_string(), 2),
            H_PANIC_DROP => (format!("__panic_drop{key}"), 1),
            H_DROP_ENV => (format!("__drop_env{}", key), 2),
            H_DROP_ENUM => (format!("__drop_enum{}", key), 1),
            H_DROP_BOX => (format!("__drop_box{}", key), 1),
            _ => ("__str_eq".to_string(), 3),
        };
        let mut l = L {
            scopes: vec![vec![]],
            recover_panics: false,
            guard_destructors: recover_panics,
            cleanup_slots: HashMap::new(),
            flags: HashMap::new(),
            capture_flags: HashMap::new(),
            blocks: vec![(vec![], None)],
            cur: 0,
            nv: nparams,
            vr: HashMap::new(),
            slots: vec![],
            vars: HashMap::new(),
            memvars: HashSet::new(),
            ret_ptr: None,
            loops: vec![],
            types: &prog.types,
            sigs: &sigs,
            ids: &ids,
            texts: &mut texts,
            helpers: &mut helpers,
            extern_ids: &extern_ids,
            locs: &mut loc_tab,
            lib_fn: &lib_fn,
            loc: Pos::default(),
            bloc: vec![0],
        };
        let r = match kind {
            H_DROP_DYN_DATA => {
                l.drop_value(prog.types.dyn_tables[key].concrete, 0, 0);
                None
            }
            H_DYN_ADAPTER => {
                // A view of a Copy receiver is passed by value to the concrete method.
                let f = &prog.funcs[key];
                let mut args: Vec<V> = (0..nparams).collect();
                let i = is_agg(f.ret) as usize;
                args[i] = l.load(args[i], 0);
                let dst = if f.ret == Ty::Unit || i == 1 {
                    None
                } else {
                    Some(l.v())
                };
                l.emit(Inst::Call(dst, key, args));
                dst
            }
            _ => l.helper_body(kind, key)?,
        };
        l.term(Term::Ret(r));
        let blocks = l
            .blocks
            .into_iter()
            .map(|(insts, t)| Block {
                insts,
                term: t.unwrap_or(Term::Unreachable),
            })
            .collect();
        funcs.push(IrFunc {
            name,
            lib: false,
            nparams,
            vranges: vec![None; nparams],
            nvregs: l.nv,
            slots: l.slots,
            blocks,
        });
        k += 1;
    }
    let main = ids["main"];
    Ok(IrProgram {
        funcs,
        texts,
        main,
        vtables,
        externs: prog.externs.iter().map(|f| f.name.clone()).collect(),
        links: prog.links.clone(),
        locs: loc_tab.rows,
        files: vec![],
    })
}

fn fmt_inst(i: &Inst) -> String {
    match i {
        Inst::Loc(l) => format!("loc#{}", l),
        Inst::Const(d, n) => format!("v{} = {}", d, n),
        Inst::Text(d, n) => format!("v{} = text#{}", d, n),
        Inst::VTableAddr(d, n) => format!("v{} = vtable#{}", d, n),
        Inst::Mov(d, s) => format!("v{} = v{}", d, s),
        Inst::Bin(d, op, a, b) => format!("v{} = {:?} v{}, v{}", d, op, a, b),
        Inst::Neg(d, s) => format!("v{} = neg v{}", d, s),
        Inst::Not(d, s) => format!("v{} = not v{}", d, s),
        Inst::Call(d, f, args) | Inst::ExternCall(d, f, args, ..) => {
            let a: Vec<String> = args.iter().map(|x| format!("v{}", x)).collect();
            match d {
                Some(d) => format!("v{} = call f{}({})", d, f, a.join(", ")),
                None => format!("call f{}({})", f, a.join(", ")),
            }
        }
        Inst::CallInd(d, c, args) => {
            let a: Vec<String> = args.iter().map(|x| format!("v{}", x)).collect();
            match d {
                Some(d) => format!("v{} = call_ind v{}({})", d, c, a.join(", ")),
                None => format!("call_ind v{}({})", c, a.join(", ")),
            }
        }
        Inst::FnAddr(d, f) => format!("v{} = &f{}", d, f),
        Inst::PrintInt(v) => format!("print_int v{}", v),
        Inst::PrintBool(v) => format!("print_bool v{}", v),
        Inst::PrintText(v) => format!("print_text v{}", v),
        Inst::Slot(d, k) => format!("v{} = &slot{}", d, k),
        Inst::Load(d, a, o) => format!("v{} = load [v{}{:+}]", d, a, o),
        Inst::LoadH(d, a, o) => format!("v{} = loadh [v{}{:+}]", d, a, o),
        Inst::Store(a, o, v) => format!("store [v{}{:+}] = v{}", a, o, v),
        Inst::Lea(d, a, o) => format!("v{} = v{} {:+}", d, a, o),
        Inst::Copy(d, s, n) => format!("copy {} words [v{}] <- [v{}]", n, d, s),
        Inst::ListPush(l, v) => format!("list_push [v{}], v{}", l, v),
        Inst::ListGet(d, l, i) => format!("v{} = list_get [v{}], v{}", d, l, i),
        Inst::ListFree(l) => format!("list_free [v{}]", l),
        Inst::Alloc(d, n) => format!("v{} = alloc v{}", d, n),
        Inst::Free(p) => format!("free v{}", p),
        Inst::Realloc(d, o, n, u) => format!("v{} = realloc v{}, v{} bytes, keep v{}", d, o, n, u),
        Inst::MemCpy(d, s, n) => {
            format!("memcpy [v{}] <- [v{}], {} bytes", d, s, format!("v{}", n))
        }
        Inst::Load8(d, a, o) => format!("v{} = load8 [v{}{:+}]", d, a, o),
        Inst::Store8(a, o, v) => format!("store8 [v{}{:+}] = v{}", a, o, v),
        Inst::PrintBytes(p, n) => format!("print_bytes v{}, v{}", p, n),
        Inst::Check(i, n) => format!("check v{} < v{}", i, n),
        Inst::CheckNonEmpty(n) => format!("check_nonempty v{}", n),
        Inst::LoadX(d, b, i, k) => format!("v{} = load [v{}+v{}*{}]", d, b, i, k),
        Inst::StoreX(b, i, k, v) => format!("store [v{}+v{}*{}] = v{}", b, i, k, v),
        Inst::LeaX(d, b, i, k) => format!("v{} = lea [v{}+v{}*{}]", d, b, i, k),
        Inst::Assert(n) => format!("assert v{}", n),
        Inst::Panic(p, n) => format!("panic v{}, v{}", p, n),
        Inst::Syscall(d, a) => format!(
            "v{} = syscall {}",
            d,
            a.iter()
                .map(|x| format!("v{}", x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Inst::Thread(op, d, args) => format!("v{} = thread.{:?} {:?}", d, op, args),
        Inst::Atomic(op, d, args) => format!("v{} = atomic.{:?} {:?}", d, op, args),
        Inst::Global(d, n) => format!("v{} = global #{}", d, n),
        Inst::N(op, d, a, b) => {
            if op.unary() {
                format!("v{} = {:?} v{}", d, op, a)
            } else {
                format!("v{} = {:?} v{}, v{}", d, op, a, b)
            }
        }
    }
}

pub fn dump(p: &IrProgram) -> String {
    let mut s = String::new();
    for (fi, f) in p.funcs.iter().enumerate() {
        s += &format!(
            "fn f{} {}({} params, {} vregs)\n",
            fi, f.name, f.nparams, f.nvregs
        );
        if !f.slots.is_empty() {
            let sl: Vec<String> = f
                .slots
                .iter()
                .enumerate()
                .map(|(i, w)| format!("slot{}={}w", i, w))
                .collect();
            s += &format!("  frame: {}\n", sl.join(" "));
        }
        for (i, b) in f.blocks.iter().enumerate() {
            s += &format!("  b{}:\n", i);
            for ins in b.insts.iter().filter(|i| !matches!(i, Inst::Loc(_))) {
                s += &format!("    {}\n", fmt_inst(ins));
            }
            s += &format!("    {:?}\n", b.term);
        }
    }
    s
}

/// The first function that uses a Linux-only runtime intrinsic, if any. Only the x86-64
/// Linux backend implements them; the other targets report a compile-time error instead.
pub fn os_intrinsics_error(p: &IrProgram) -> Option<String> {
    for f in &p.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Thread(..) | Inst::Atomic(..) => {
                        return Some(format!(
                            "thread runtime intrinsics (used by function '{}')",
                            f.name
                        ))
                    }
                    Inst::Syscall(..) => {
                        return Some(format!(
                            "raw system calls (std.io, used by function '{}')",
                            f.name
                        ))
                    }
                    Inst::Global(..) => {
                        return Some(format!(
                            "raw system calls (std.io, used by function '{}')",
                            f.name
                        ))
                    }
                    _ => {}
                }
            }
        }
    }
    None
}
