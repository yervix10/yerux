// Register allocator for the x86-64 backend: liveness analysis + linear scan over live intervals.
//
// The IR is not SSA (a vreg may be assigned many times), so liveness is a real dataflow problem over
// the CFG. Every vreg gets ONE conservative interval [start, end] in the linear block order that
// covers every position where it is live; linear scan (Poletto/Sarkar) then assigns registers.
//
// Own ABI (all of it enforced here, the runtime is accounted for as "a call"):
//   scratch (never allocated) : rax, rcx, rdx, rsi, rdi   -- fixed uses: div/idiv (rax:rdx), runtime
//                               arguments (rdi, rsi), reloads of spilled operands, return value (rax)
//   caller-saved, allocatable : rbx, r8, r9, r10, r11     -- clobbered by every call: user calls and runtime
//                               calls (the runtime uses rbx and, via syscall, r8-r11 freely)
//   callee-saved, allocatable : r12, r13, r14, r15        -- preserved by the runtime, saved/restored by
//                               every user function that assigns them
//   rbp = frame pointer, rsp = stack pointer. Call arguments: the first four in rdi, rsi, rdx, rcx (scratch
//   registers, so no allocated vreg ever lives in them and the argument moves need no ordering), the rest pushed
//   on the stack (right to left, caller pops); result in rax. A callee moves its register parameters out of
//   rdi.. in the prologue, before anything else can clobber them.
// An interval that is live across a call may only get a callee-saved register (or be spilled).
//
// Besides registers a vreg can end up
//   * spilled      : lives in its own 8-byte frame slot, operands are reloaded into scratch registers;
//   * rematerialised: a vreg with a single definition `Const`/`Slot`/`Text` is never given a location;
//     every use re-creates the value (as an immediate, a `lea rbp+off`, ...). No register pressure.
// Compare-and-branch pairs (`t = Lt a, b ; Br t`) whose `t` has no other use are fused (`cmp; jcc`).
use crate::ir::*;

pub const CALLER_SAVED: u16 = (1 << 3) | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
pub const CALLEE_SAVED: u16 = (1 << 12) | (1 << 13) | (1 << 14) | (1 << 15);
pub const ALLOC: u16 = CALLER_SAVED | CALLEE_SAVED;
/// allocatable xmm registers (xmm0 / xmm1 stay scratch); every one of them is clobbered by calls
pub const XMM_ALLOC: u16 = 0xFFFC;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Loc {
    None, // never defined nor used
    Reg(u8),
    Xmm(u8),      // xmm2..xmm15 (xmm0/xmm1 are scratch); only for vregs that never live across a call
    Spill(usize), // spill slot number
    Const(i64),
    Slot(usize),
    Text(usize),
}

pub struct Alloc {
    pub loc: Vec<Loc>,
    pub nspill: usize,
    pub callee_used: Vec<u8>, // callee-saved registers this function assigns (must be saved)
    pub fused: Vec<bool>,     // per block: last instruction is a compare consumed by the terminator
    /// calls in cold blocks: (block, instruction) -> caller-saved GPRs and xmm registers to save around the call
    pub saves: std::collections::HashMap<(usize, usize), (Vec<u8>, Vec<u8>)>,
    pub param_live: Vec<Vec<bool>>, // per block: which parameters are live on entry (the prologue only moves those)
}

pub fn inst_def(i: &Inst) -> Option<V> {
    match i {
        Inst::Const(d, _) | Inst::Text(d, _) | Inst::FnAddr(d, _) | Inst::VTableAddr(d, _) | Inst::Mov(d, _) | Inst::Bin(d, ..) | Inst::Neg(d, _) | Inst::Not(d, _) => Some(*d),
        Inst::Slot(d, _) | Inst::Load(d, ..) | Inst::LoadH(d, ..) | Inst::Lea(d, ..) | Inst::ListGet(d, ..) => Some(*d),
        Inst::Alloc(d, _) | Inst::Load8(d, ..) | Inst::Realloc(d, ..) | Inst::LoadX(d, ..) | Inst::LeaX(d, ..) => Some(*d),
        Inst::N(_, d, ..) => Some(*d),
        Inst::Thread(_, d, _) | Inst::Atomic(_, d, _) | Inst::Syscall(d, _) | Inst::Global(d, _) => Some(*d),
        Inst::Call(d, ..) | Inst::ExternCall(d, ..) | Inst::CallInd(d, ..) => *d,
        _ => None,
    }
}

pub fn inst_uses(i: &Inst) -> Vec<V> {
    match i {
        Inst::Const(..) | Inst::Text(..) | Inst::Slot(..) | Inst::FnAddr(..) | Inst::VTableAddr(..) | Inst::Global(..) => vec![],
        Inst::Thread(_, _, args) | Inst::Atomic(_, _, args) | Inst::Syscall(_, args) => args.clone(),
        Inst::Mov(_, s) | Inst::Neg(_, s) | Inst::Not(_, s) => vec![*s],
        Inst::Bin(_, _, a, b) | Inst::N(_, _, a, b) => vec![*a, *b],
        Inst::Call(_, _, args) | Inst::ExternCall(_, _, args, ..) => args.clone(),
        Inst::CallInd(_, c, args) => {
            let mut v = vec![*c];
            v.extend(args.iter().copied());
            v
        }
        Inst::PrintInt(v) | Inst::PrintBool(v) | Inst::PrintText(v) => vec![*v],
        Inst::Load(_, a, _) | Inst::LoadH(_, a, _) | Inst::Lea(_, a, _) | Inst::ListFree(a) => vec![*a],
        Inst::Alloc(_, n) | Inst::Free(n) | Inst::CheckNonEmpty(n) | Inst::Assert(n) | Inst::Load8(_, n, _) => vec![*n],
        Inst::MemCpy(a, b, c) => vec![*a, *b, *c],
        Inst::Realloc(_, a, b, c) => vec![*a, *b, *c],
        Inst::Store8(a, _, v) | Inst::PrintBytes(a, v) | Inst::Panic(a, v) | Inst::Check(a, v) => vec![*a, *v],
        Inst::Store(a, _, v) | Inst::Copy(a, v, _) | Inst::ListPush(a, v) | Inst::ListGet(_, a, v) => vec![*a, *v],
        Inst::LoadX(_, b, i, _) | Inst::LeaX(_, b, i, _) => vec![*b, *i],
        Inst::StoreX(b, i, _, v) => vec![*b, *i, *v],
    }
}

pub fn is_call(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Call(..)
            | Inst::ExternCall(..)
            | Inst::CallInd(..)
            | Inst::Thread(..) | Inst::Syscall(..)
            | Inst::ListPush(..)
            | Inst::ListGet(..)
            | Inst::ListFree(..)
            | Inst::PrintInt(..)
            | Inst::PrintBool(..)
            | Inst::PrintText(..)
            | Inst::Alloc(..)
            | Inst::Realloc(..)
            | Inst::Free(..)
            | Inst::PrintBytes(..)
            | Inst::Panic(..)
    )
}

fn term_uses(t: &Term) -> Vec<V> {
    match t {
        Term::Br(c, ..) => vec![*c],
        Term::Ret(Some(v)) => vec![*v],
        _ => vec![],
    }
}

fn succs(t: &Term) -> Vec<usize> {
    match t {
        Term::Jmp(b) => vec![*b],
        Term::Br(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

fn is_cmp(op: Op) -> bool {
    matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne)
}

#[derive(Clone)]
struct Bits(Vec<u64>);
impl Bits {
    fn new(n: usize) -> Bits {
        Bits(vec![0; (n + 63) / 64])
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &x)| (0..64).filter(move |b| x >> b & 1 == 1).map(move |b| w * 64 + b))
    }
}

struct Iv {
    v: V,
    start: usize,
    end: usize,
    weight: u64,
    hint: Option<V>,
}

pub fn allocate(f: &IrFunc, cold: &[bool]) -> Alloc {
    let nv = f.nvregs;
    let nb = f.blocks.len();
    let mut loc = vec![Loc::None; nv];

    // ── definition / use counts, rematerialisable vregs ──
    let mut defs = vec![0usize; nv];
    let mut uses = vec![0usize; nv];
    for p in 0..f.nparams.min(nv) {
        defs[p] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defs[d] += 1;
            }
            for u in inst_uses(i) {
                uses[u] += 1;
            }
        }
        for u in term_uses(&b.term) {
            uses[u] += 1;
        }
    }
    // rematerialisable: every definition is a Const/Slot/Text with the same value (loop rotation copies them)
    let mut remat = vec![false; nv];
    let mut rloc: Vec<Option<Loc>> = vec![None; nv];
    let mut conflict = vec![false; nv];
    for p in 0..f.nparams.min(nv) {
        conflict[p] = true;
    }
    for b in &f.blocks {
        for i in &b.insts {
            let (d, l) = match i {
                Inst::Const(d, n) => (*d, Loc::Const(*n)),
                Inst::Slot(d, k) => (*d, Loc::Slot(*k)),
                Inst::Text(d, t) => (*d, Loc::Text(*t)),
                _ => {
                    if let Some(d) = inst_def(i) {
                        conflict[d] = true;
                    }
                    continue;
                }
            };
            match rloc[d] {
                None => rloc[d] = Some(l),
                Some(o) if o == l => {}
                _ => conflict[d] = true,
            }
        }
    }
    for v in 0..nv {
        if !conflict[v] {
            if let Some(l) = rloc[v] {
                loc[v] = l;
                remat[v] = true;
            }
        }
    }

    // ── compare+branch fusion ──
    // (a compare temp may be defined and consumed in several blocks, loop rotation copies the header)
    let mut fused = vec![false; nb];
    let mut pairs = vec![0usize; nv];
    let fusable = |i: &Inst| -> Option<V> {
        match i {
            Inst::Bin(d, op, ..) if is_cmp(*op) => Some(*d),
            Inst::N(NOp::FLt | NOp::FLe, d, ..) => Some(*d),
            _ => None,
        }
    };
    for b in &f.blocks {
        if let (Some(i), Term::Br(c, ..)) = (b.insts.last(), &b.term) {
            if fusable(i) == Some(*c) {
                pairs[*c] += 1;
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if let (Some(i), Term::Br(c, ..)) = (b.insts.last(), &b.term) {
            if fusable(i) == Some(*c) && uses[*c] == pairs[*c] && defs[*c] == pairs[*c] {
                fused[bi] = true;
            }
        }
    }
    // effective def/uses of instruction `ii` of block `bi`
    let eff_def = |bi: usize, ii: usize, i: &Inst| -> Option<V> {
        if fused[bi] && ii + 1 == f.blocks[bi].insts.len() {
            return None;
        }
        inst_def(i).filter(|d| !remat[*d])
    };
    let eff_uses = |i: &Inst| -> Vec<V> { inst_uses(i).into_iter().filter(|u| !remat[*u]).collect() };

    // ── block-level liveness ──
    let mut gen: Vec<Bits> = vec![Bits::new(nv); nb];
    let mut kill: Vec<Bits> = vec![Bits::new(nv); nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            for u in eff_uses(i) {
                if !kill[bi].get(u) {
                    gen[bi].set(u);
                }
            }
            if let Some(d) = eff_def(bi, ii, i) {
                kill[bi].set(d);
            }
        }
        for u in term_uses(&b.term) {
            if fused[bi] && matches!(b.term, Term::Br(..)) {
                break; // the compare temp is never materialised
            }
            if !remat[u] && !kill[bi].get(u) {
                gen[bi].set(u);
            }
        }
    }
    let mut live_in: Vec<Bits> = vec![Bits::new(nv); nb];
    let mut live_out: Vec<Bits> = vec![Bits::new(nv); nb];
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut out = Bits::new(nv);
            for s in succs(&f.blocks[bi].term) {
                for (o, i) in out.0.iter_mut().zip(live_in[s].0.iter()) {
                    *o |= *i;
                }
            }
            let mut inn = out.clone();
            for (w, (k, g)) in inn.0.iter_mut().zip(kill[bi].0.iter().zip(gen[bi].0.iter())) {
                *w = (*w & !*k) | *g;
            }
            if inn.0 != live_in[bi].0 || out.0 != live_out[bi].0 {
                changed = true;
                live_in[bi] = inn;
                live_out[bi] = out;
            }
        }
        if !changed {
            break;
        }
    }

    // ── loop depth (back edge j -> i with i <= j makes blocks i..=j one level deeper) ──
    let mut depth = vec![0u32; nb];
    for (j, b) in f.blocks.iter().enumerate() {
        if cold[j] {
            continue; // cold blocks sit at the end: their jumps back are not loops
        }
        for s in succs(&b.term) {
            if s <= j {
                for d in &mut depth[s..=j] {
                    *d += 1;
                }
            }
        }
    }

    // ── float class: a vreg prefers an xmm register when it mostly takes part in floating-point operations ──
    // roles: float operand/result of an FP op (+w to fw), anything that needs a GPR (+w to gw); moves, constants,
    // loads and stores of a value are neutral (they work in either class), moves tie classes together.
    let mut fw = vec![0u64; nv];
    let mut gw = vec![0u64; nv];
    let mut parent: Vec<usize> = (0..nv).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let n = p[c];
            p[c] = r;
            c = n;
        }
        r
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        let w = 8u64.pow(depth[bi].min(6));
        for (ii, i) in b.insts.iter().enumerate() {
            let fused_cmp = fused[bi] && ii + 1 == b.insts.len();
            let mut fl: Vec<V> = vec![];
            let mut neutral: Vec<V> = vec![];
            match i {
                Inst::N(op, d, a, bb) => match op {
                    NOp::FAdd | NOp::FSub | NOp::FMul | NOp::FDiv => fl.extend([*d, *a, *bb]),
                    NOp::FSqrt | NOp::FNeg => fl.extend([*d, *a]),
                    NOp::FLt | NOp::FLe | NOp::FEq | NOp::FNe => fl.extend([*a, *bb]),
                    NOp::I2F => fl.push(*d),
                    NOp::F2S | NOp::F2I => fl.push(*a),
                    _ => {}
                },
                Inst::Mov(d, sv) => {
                    neutral.extend([*d, *sv]);
                    let (x, y) = (find(&mut parent, *d), find(&mut parent, *sv));
                    parent[x] = y;
                }
                Inst::ExternCall(_, _, args, floats, ret_float) => {
                    for (v, is_float) in args.iter().zip(floats) {
                        if *is_float { fl.push(*v); } else { gw[*v] += w; }
                    }
                    if *ret_float {
                        if let Some(d) = inst_def(i) { fl.push(d); }
                    }
                }
                Inst::Const(d, _) | Inst::Load(d, ..) | Inst::LoadH(d, ..) | Inst::LoadX(d, ..) => neutral.push(*d),
                Inst::Store(_, _, v) | Inst::StoreX(_, _, _, v) => neutral.push(*v),
                _ => {}
            }
            if fused_cmp {
                // the compare temp has no location
            }
            let mut seen: Vec<V> = vec![];
            for v in fl.iter() {
                if !remat[*v] {
                    fw[*v] += w;
                    seen.push(*v);
                }
            }
            // GPR roles: every def / use that is not float and not neutral
            if let Some(d) = inst_def(i) {
                if !fl.contains(&d) && !neutral.contains(&d) && !remat[d] && !(fused_cmp) {
                    gw[d] += w;
                }
            }
            let nu = {
                // value operands of Store/StoreX are neutral, address operands are not
                let mut u = inst_uses(i);
                if let Inst::Store(_, _, v) | Inst::StoreX(_, _, _, v) = i {
                    if let Some(pos) = u.iter().rposition(|x| x == v) {
                        u.remove(pos);
                    }
                }
                u
            };
            let mut flc = fl.clone();
            for u in nu {
                if remat[u] {
                    continue;
                }
                if let Some(pos) = flc.iter().position(|x| *x == u) {
                    flc.remove(pos);
                    continue;
                }
                if matches!(i, Inst::Mov(..)) {
                    continue;
                }
                gw[u] += w;
            }
            let _ = seen;
        }
        for u in term_uses(&b.term) {
            if !(fused[bi] && matches!(b.term, Term::Br(..))) && !remat[u] {
                gw[u] += w;
            }
        }
    }
    for p in 0..f.nparams.min(nv) {
        gw[p] += 1; // arrives in a GPR
    }
    let mut cf = vec![0u64; nv];
    let mut cg = vec![0u64; nv];
    for v in 0..nv {
        let r = find(&mut parent, v);
        cf[r] += fw[v];
        cg[r] += gw[v];
    }
    let mut prefers_xmm = vec![false; nv];
    for v in 0..nv {
        let r = find(&mut parent, v);
        prefers_xmm[v] = cf[r] > 0 && cf[r] >= cg[r] && !remat[v];
    }

    // ── live ranges with holes (per block, walking backwards) ──
    let mut ranges: Vec<Vec<(usize, usize)>> = vec![vec![]; nv];
    let mut weight = vec![0u64; nv];
    let mut hint: Vec<Option<V>> = vec![None; nv];
    let mut calls: Vec<usize> = vec![];
    let mut soft: Vec<(usize, usize, usize)> = vec![];
    // block positions: header h, instructions h+1.., terminator t = h + n + 1
    let mut hpos = vec![0usize; nb];
    {
        let mut pos = 1usize;
        for (bi, b) in f.blocks.iter().enumerate() {
            hpos[bi] = pos;
            pos += b.insts.len() + 2;
        }
    }
    for p in 0..f.nparams.min(nv) {
        ranges[p].push((0, 1));
    }
    let mut open = vec![usize::MAX; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        let h = hpos[bi];
        let t = h + b.insts.len() + 1;
        let w = 8u64.pow(depth[bi].min(6));
        for v in live_out[bi].iter() {
            open[v] = t;
        }
        // terminator uses
        if !(fused[bi] && matches!(b.term, Term::Br(..))) {
            for u in term_uses(&b.term) {
                if !remat[u] {
                    if open[u] == usize::MAX {
                        open[u] = t;
                    }
                    weight[u] += w;
                }
            }
        }
        for (ii, i) in b.insts.iter().enumerate().rev() {
            let p = h + 1 + ii;
            if is_call(i) {
                if cold[bi] && matches!(i, Inst::Call(..) | Inst::CallInd(..) | Inst::Realloc(..) | Inst::Alloc(..) | Inst::Free(..) | Inst::Thread(..) | Inst::Syscall(..)) {
                    soft.push((bi, ii, p));
                } else {
                    calls.push(p);
                }
            }
            if let Some(d) = eff_def(bi, ii, i) {
                if open[d] != usize::MAX {
                    ranges[d].push((p, open[d]));
                    open[d] = usize::MAX;
                } else {
                    ranges[d].push((p, p));
                }
                weight[d] += w;
                if hint[d].is_none() {
                    hint[d] = match i {
                        Inst::Mov(_, s) | Inst::Neg(_, s) | Inst::Not(_, s) => Some(*s),
                        Inst::Bin(_, _, x, _) => Some(*x),
                        Inst::N(NOp::FAdd | NOp::FSub | NOp::FMul | NOp::FDiv | NOp::FSqrt | NOp::FNeg, _, x, _) => Some(*x),
                        _ => None,
                    };
                }
            }
            for u in eff_uses(i) {
                if open[u] == usize::MAX {
                    open[u] = p;
                }
                weight[u] += w;
            }
        }
        for v in 0..nv {
            if open[v] != usize::MAX {
                ranges[v].push((h, open[v]));
                open[v] = usize::MAX;
            }
        }
    }
    calls.sort_unstable();
    // sort and merge each vreg's ranges
    for r in ranges.iter_mut() {
        r.sort_unstable();
        let mut out: Vec<(usize, usize)> = vec![];
        for &(a, b) in r.iter() {
            if let Some(last) = out.last_mut() {
                if a <= last.1 {
                    last.1 = last.1.max(b);
                    continue;
                }
            }
            out.push((a, b));
        }
        *r = out;
    }

    struct Iv {
        v: V,
        start: usize,
        end: usize,
        weight: u64,
        hint: Option<V>,
    }
    let mut ivs: Vec<Iv> = (0..nv)
        .filter(|&v| !ranges[v].is_empty())
        .map(|v| Iv { v, start: ranges[v][0].0, end: ranges[v].last().unwrap().1, weight: weight[v], hint: hint[v] })
        .collect();
    ivs.sort_by_key(|i| (i.start, i.v));
    if std::env::var_os("YERUX_RA_NOHOLES").is_some() {
        for r in ranges.iter_mut() {
            if !r.is_empty() {
                let (a, b) = (r[0].0, r.last().unwrap().1);
                *r = vec![(a, b)];
            }
        }
    }

    let overlap = |a: &[(usize, usize)], b: &[(usize, usize)]| -> bool {
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if a[i].0 < b[j].1 && b[j].0 < a[i].1 {
                return true;
            }
            if a[i].1 <= b[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        false
    };
    let crosses_call = |r: &[(usize, usize)]| -> bool {
        r.iter().any(|&(a, b)| {
            let k = calls.partition_point(|&p| p <= a);
            k < calls.len() && calls[k] < b
        })
    };

    // ── allocation: linear scan over intervals with holes (no splitting); register ids 0..16 are GPRs, 16..32 xmm ──
    let mut assigned: Vec<Vec<usize>> = vec![vec![]; 32]; // per register: indices into ivs
    let mut reg_of: Vec<Option<u8>> = vec![None; ivs.len()];
    let mut idx_of_v = vec![usize::MAX; nv];
    for (k, iv) in ivs.iter().enumerate() {
        idx_of_v[iv.v] = k;
    }
    for idx in 0..ivs.len() {
        let (cv, cw, ch) = (ivs[idx].v, ivs[idx].weight, ivs[idx].hint);
        let crosses = crosses_call(&ranges[cv]);
        let use_xmm = prefers_xmm[cv] && !crosses;
        let mut classes: Vec<u32> = vec![];
        if use_xmm {
            classes.push((XMM_ALLOC as u32) << 16);
        }
        classes.push((ALLOC & !if crosses { CALLER_SAVED } else { 0 }) as u32);
        let mut done = false;
        for &mask in &classes {
            let mut free: u32 = 0;
            let mut conflict_w = [u64::MAX; 32];
            for r in 0..32u8 {
                if mask >> r & 1 == 0 {
                    continue;
                }
                let mut cwsum = 0u64;
                let mut hit = false;
                for &o in &assigned[r as usize] {
                    if overlap(&ranges[ivs[idx].v], &ranges[ivs[o].v]) {
                        hit = true;
                        cwsum = cwsum.saturating_add(ivs[o].weight);
                    }
                }
                if !hit {
                    free |= 1 << r;
                } else {
                    conflict_w[r as usize] = cwsum;
                }
            }
            let chosen: Option<u8> = if free != 0 {
                let hinted = ch.and_then(|h| match loc[h] {
                    Loc::Reg(r) if free >> r & 1 == 1 => Some(r),
                    Loc::Xmm(x) if free >> (16 + x) & 1 == 1 => Some(16 + x),
                    _ => None,
                });
                Some(match hinted {
                    Some(r) => r,
                    None => {
                        let cs = free & (CALLER_SAVED as u32);
                        if cs != 0 && !crosses {
                            cs.trailing_zeros() as u8
                        } else {
                            free.trailing_zeros() as u8
                        }
                    }
                })
            } else {
                // evict the cheapest set of conflicting intervals if they are cheaper than us
                let mut best: Option<(u64, u8)> = None;
                for r in 0..32u8 {
                    if mask >> r & 1 == 1 && conflict_w[r as usize] != u64::MAX {
                        if best.map_or(true, |(w, _)| conflict_w[r as usize] < w) {
                            best = Some((conflict_w[r as usize], r));
                        }
                    }
                }
                match best {
                    Some((w, r)) if w < cw && !(mask >> 16 != 0 && false) => {
                        let ys = assigned[r as usize].clone();
                        let mut keep = vec![];
                        for o in ys {
                            if overlap(&ranges[ivs[idx].v], &ranges[ivs[o].v]) {
                                reg_of[o] = None;
                                loc[ivs[o].v] = Loc::None;
                            } else {
                                keep.push(o);
                            }
                        }
                        assigned[r as usize] = keep;
                        Some(r)
                    }
                    _ => None,
                }
            };
            if let Some(r) = chosen {
                reg_of[idx] = Some(r);
                loc[cv] = if r >= 16 { Loc::Xmm(r - 16) } else { Loc::Reg(r) };
                assigned[r as usize].push(idx);
                done = true;
                break;
            }
        }
        if !done {
            reg_of[idx] = None;
        }
    }
    let _ = idx_of_v;
    // everything that got no register is spilled; spill slots are shared between intervals with disjoint ranges
    let mut slots: Vec<Vec<usize>> = vec![];
    for idx in 0..ivs.len() {
        if reg_of[idx].is_some() {
            continue;
        }
        let v = ivs[idx].v;
        let mut placed = None;
        for (si, sl) in slots.iter().enumerate() {
            if !sl.iter().any(|&o| overlap(&ranges[v], &ranges[ivs[o].v])) {
                placed = Some(si);
                break;
            }
        }
        let si = match placed {
            Some(si) => si,
            None => {
                slots.push(vec![]);
                slots.len() - 1
            }
        };
        slots[si].push(idx);
        loc[v] = Loc::Spill(si);
    }
    let nspill = slots.len();
    // the exact set of callee-saved registers in the final assignment (evictions change it)
    let mut callee_used: u16 = 0;
    for l in &loc {
        if let Loc::Reg(r) = l {
            if CALLEE_SAVED >> r & 1 == 1 {
                callee_used |= 1 << r;
            }
        }
    }
    let callee_used = (0..16u8).filter(|r| callee_used >> r & 1 == 1).collect();
    let param_live = (0..nb).map(|bi| (0..f.nparams.min(nv)).map(|p| remat[p] || live_in[bi].get(p)).collect()).collect();
    let mut saves = std::collections::HashMap::new();
    for &(bi, ii, p) in &soft {
        let (mut gs, mut xs) = (vec![], vec![]);
        for v in 0..nv {
            let hit = ranges[v].iter().any(|&(a, b)| a < p && p < b);
            if !hit {
                continue;
            }
            match loc[v] {
                Loc::Reg(r) if CALLER_SAVED >> r & 1 == 1 => gs.push(r),
                Loc::Xmm(x) => xs.push(x),
                _ => {}
            }
        }
        if !gs.is_empty() || !xs.is_empty() {
            saves.insert((bi, ii), (gs, xs));
        }
    }
    Alloc { loc, nspill, callee_used, fused, saves, param_live }
}
