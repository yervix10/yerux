// Yerux IR optimizer: IR -> IR passes. The IR is NOT in SSA form (a vreg may be assigned many
// times, e.g. loop variables), so every pass is a proper dataflow analysis over the CFG.
//
// Passes (iterated to a fixpoint per function):
//   1. constprop  - forward constant propagation/folding (never folds ops that would panic)
//   2. copyprop   - forward available-copies propagation
//   3. coalesce   - `t = op ..; x = t` (t dead afterwards) -> `x = op ..`
//   4. dce        - liveness-based removal of pure instructions with unused results
//   5. cfg        - branch folding, jump threading, unreachable removal, block merging, renumbering
// Finally vregs are compacted (parameters keep ids 0..nparams).
use crate::ir::*;
use std::collections::{HashMap, HashSet};

pub fn run(p: &mut IrProgram) {
    for f in &mut p.funcs {
        opt_func(f);
    }
    if std::env::var_os("YERUX_NO_INLINE").is_none() && !pass_off("inline") {
        crate::inline::run(p, &mut opt_func);
    }
    for f in &mut p.funcs {
        finalize(f);
    }
}

/// Loop transformations that must see the final, fully optimised function and run once: bounds-check
/// elimination by loop versioning (needs the range analysis) first, then strength reduction (its wrapping
/// induction variables hide ranges, so it must come last); then a clean-up round.
fn finalize(f: &mut IrFunc) {
    if f.blocks.len().saturating_mul(f.nvregs) > HUGE_FUNC {
        return;
    }
    let mut did = false;
    if !pass_off("versioning") {
        let cands = rangeopt_collect(f);
        did |= crate::loopopt::version_checks(f, &cands);
    }
    if !pass_off("strength") {
        for _ in 0..8 {
            if !crate::loopopt::strength(f) {
                break;
            }
            did = true;
        }
    }
    if did {
        opt_func(f);
    }
}

fn snapshot(f: &IrFunc) -> String {
    let mut s = String::new();
    for b in &f.blocks {
        s += &format!("{:?}{:?};", b.insts, b.term);
    }
    s
}

/// Debug switch: `YERUX_OPT_OFF=constprop,copyprop,sroa,memopt,rangeopt,coalesce,dce,cfg,inline,versioning,strength` disables the
/// named passes (used by the optimizer differential fuzzer to bisect a miscompile to one pass).
fn pass_off(name: &str) -> bool {
    use std::sync::OnceLock;
    static OFF: OnceLock<Vec<String>> = OnceLock::new();
    OFF.get_or_init(|| {
        std::env::var("YERUX_OPT_OFF")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default()
    })
    .iter()
    .any(|s| s == name)
}

/// Functions this big (blocks x vregs) get only the cheap clean-up: the dataflow passes keep per-block state and
/// are quadratic, so a generated 20000-term expression would take minutes and gigabytes.
const HUGE_FUNC: usize = 6_000_000;

fn opt_func(f: &mut IrFunc) {
    if f.blocks.len().saturating_mul(f.nvregs) > HUGE_FUNC {
        cfg(f);
        compact_vregs(f);
        return;
    }
    for _ in 0..16 {
        let before = snapshot(f);
        if !pass_off("constprop") {
            constprop(f);
        }
        if !pass_off("copyprop") {
            copyprop(f);
        }
        if !pass_off("sroa") {
            crate::sroa::run(f);
        }
        if !pass_off("memopt") {
            crate::memopt::run(f);
        }
        if !pass_off("copyprop") {
            copyprop(f);
        }
        if !pass_off("rangeopt") {
            rangeopt(f);
        }
        if !pass_off("coalesce") {
            coalesce(f);
        }
        if !pass_off("dce") {
            dce(f);
        }
        if !pass_off("cfg") {
            cfg(f);
        }
        if snapshot(f) == before {
            break;
        }
    }
    compact_vregs(f);
}

// ───────────────────────── helpers ─────────────────────────

pub(crate) fn def(i: &Inst) -> Option<V> {
    match i {
        Inst::Const(d, _)
        | Inst::Text(d, _)
        | Inst::FnAddr(d, _)
        | Inst::VTableAddr(d, _)
        | Inst::Mov(d, _)
        | Inst::Bin(d, ..)
        | Inst::Neg(d, _)
        | Inst::Not(d, _) => Some(*d),
        Inst::Slot(d, _)
        | Inst::Load(d, ..)
        | Inst::LoadH(d, ..)
        | Inst::Lea(d, ..)
        | Inst::ListGet(d, ..) => Some(*d),
        Inst::Alloc(d, _)
        | Inst::Load8(d, ..)
        | Inst::Realloc(d, ..)
        | Inst::LoadX(d, ..)
        | Inst::LeaX(d, ..) => Some(*d),
        Inst::N(_, d, ..) => Some(*d),
        Inst::Thread(_, d, _)
        | Inst::Atomic(_, d, _)
        | Inst::Syscall(d, _)
        | Inst::Global(d, _) => Some(*d),
        Inst::Call(d, ..) | Inst::ExternCall(d, ..) | Inst::CallInd(d, ..) => *d,
        _ => None,
    }
}

fn def_mut(i: &mut Inst) -> Option<&mut V> {
    match i {
        Inst::Const(d, _)
        | Inst::Text(d, _)
        | Inst::FnAddr(d, _)
        | Inst::VTableAddr(d, _)
        | Inst::Mov(d, _)
        | Inst::Bin(d, ..)
        | Inst::Neg(d, _)
        | Inst::Not(d, _) => Some(d),
        Inst::Slot(d, _)
        | Inst::Load(d, ..)
        | Inst::LoadH(d, ..)
        | Inst::Lea(d, ..)
        | Inst::ListGet(d, ..) => Some(d),
        Inst::Alloc(d, _)
        | Inst::Load8(d, ..)
        | Inst::Realloc(d, ..)
        | Inst::LoadX(d, ..)
        | Inst::LeaX(d, ..) => Some(d),
        Inst::N(_, d, ..) => Some(d),
        Inst::Thread(_, d, _)
        | Inst::Atomic(_, d, _)
        | Inst::Syscall(d, _)
        | Inst::Global(d, _) => Some(d),
        Inst::Call(d, ..) | Inst::ExternCall(d, ..) | Inst::CallInd(d, ..) => d.as_mut(),
        _ => None,
    }
}

pub(crate) fn uses_mut(i: &mut Inst, f: &mut dyn FnMut(&mut V)) {
    match i {
        Inst::Const(..)
        | Inst::Text(..)
        | Inst::FnAddr(..)
        | Inst::VTableAddr(..)
        | Inst::Global(..) => {}
        Inst::Thread(_, _, args) | Inst::Atomic(_, _, args) | Inst::Syscall(_, args) => {
            args.iter_mut().for_each(f)
        }
        Inst::Mov(_, s) | Inst::Neg(_, s) | Inst::Not(_, s) => f(s),
        Inst::Bin(_, _, a, b) | Inst::N(_, _, a, b) => {
            f(a);
            f(b)
        }
        Inst::Call(_, _, args) | Inst::ExternCall(_, _, args, ..) => args.iter_mut().for_each(f),
        Inst::CallInd(_, c, args) => {
            f(c);
            args.iter_mut().for_each(f)
        }
        Inst::PrintInt(v) | Inst::PrintBool(v) | Inst::PrintText(v) => f(v),
        Inst::Slot(..) => {}
        Inst::Load(_, a, _) | Inst::LoadH(_, a, _) | Inst::Lea(_, a, _) | Inst::ListFree(a) => f(a),
        Inst::Store(a, _, v)
        | Inst::Copy(a, v, _)
        | Inst::ListPush(a, v)
        | Inst::ListGet(_, a, v) => {
            f(a);
            f(v)
        }
        Inst::Alloc(_, n)
        | Inst::Free(n)
        | Inst::Load8(_, n, _)
        | Inst::CheckNonEmpty(n)
        | Inst::Assert(n) => f(n),
        Inst::Store8(a, _, v) | Inst::PrintBytes(a, v) | Inst::Panic(a, v) | Inst::Check(a, v) => {
            f(a);
            f(v)
        }
        Inst::MemCpy(d, s, n) => {
            f(d);
            f(s);
            f(n)
        }
        Inst::Realloc(_, o, n, u) => {
            f(o);
            f(n);
            f(u)
        }
        Inst::LoadX(_, b, i, _) | Inst::LeaX(_, b, i, _) => {
            f(b);
            f(i)
        }
        Inst::StoreX(b, i, _, v) => {
            f(b);
            f(i);
            f(v)
        }
    }
}

pub(crate) fn uses(i: &Inst) -> Vec<V> {
    let mut c = i.clone();
    let mut v = vec![];
    uses_mut(&mut c, &mut |x| v.push(*x));
    v
}

pub(crate) fn term_uses(t: &Term) -> Vec<V> {
    match t {
        Term::Br(c, ..) => vec![*c],
        Term::Ret(Some(v)) => vec![*v],
        _ => vec![],
    }
}

pub(crate) fn succs(t: &Term) -> Vec<usize> {
    match t {
        Term::Jmp(x) => vec![*x],
        Term::Br(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

/// Reverse post-order of reachable blocks.
pub(crate) fn rpo(f: &IrFunc) -> Vec<usize> {
    let n = f.blocks.len();
    let mut seen = vec![false; n];
    let mut post = vec![];
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some((b, i)) = stack.pop() {
        let s = succs(&f.blocks[b].term);
        if i < s.len() {
            stack.push((b, i + 1));
            if !seen[s[i]] {
                seen[s[i]] = true;
                stack.push((s[i], 0));
            }
        } else {
            post.push(b);
        }
    }
    post.reverse();
    post
}

fn fold_bin(op: Op, a: i64, b: i64) -> Option<i64> {
    Some(match op.checked() {
        Op::Add => a.checked_add(b)?,
        Op::Sub => a.checked_sub(b)?,
        Op::Mul => a.checked_mul(b)?,
        Op::Div => a.checked_div(b)?, // None on /0 and MIN / -1: keep the runtime panic
        Op::Mod => a.checked_rem(b)?,
        Op::Lt => (a < b) as i64,
        Op::Le => (a <= b) as i64,
        Op::Gt => (a > b) as i64,
        Op::Ge => (a >= b) as i64,
        Op::Eq => (a == b) as i64,
        Op::Ne => (a != b) as i64,
        _ => unreachable!(),
    })
}

fn is_cmp(op: Op) -> bool {
    !matches!(
        op.checked(),
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod
    )
}

// ───────────────────────── 1. constant propagation ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Lat {
    Top,
    C(i64),
}

fn join_lat(a: Lat, b: Lat) -> Lat {
    if a == b {
        a
    } else {
        Lat::Top
    }
}

type CState = Vec<Lat>;

/// Transfer function; returns the instruction's replacement (if it can be simplified).
fn cp_step(i: &Inst, st: &mut CState) -> Option<Inst> {
    let mut repl = None;
    match i {
        Inst::Const(d, n) => st[*d] = Lat::C(*n),
        Inst::Text(d, _) | Inst::FnAddr(d, _) | Inst::VTableAddr(d, _) => st[*d] = Lat::Top,
        Inst::Mov(d, s) => st[*d] = st[*s],
        Inst::Not(d, s) => {
            if let Lat::C(c) = st[*s] {
                repl = Some(Inst::Const(*d, c ^ 1));
            }
            st[*d] = match st[*s] {
                Lat::C(c) => Lat::C(c ^ 1),
                _ => Lat::Top,
            };
        }
        Inst::Neg(d, s) => {
            let r = match st[*s] {
                Lat::C(c) => c.checked_neg(),
                _ => None,
            };
            if let Some(r) = r {
                repl = Some(Inst::Const(*d, r));
            }
            st[*d] = r.map_or(Lat::Top, Lat::C);
        }
        Inst::Bin(d, op, a, b) => {
            let (la, lb) = (st[*a], st[*b]);
            let folded = match (la, lb) {
                (Lat::C(x), Lat::C(y)) => fold_bin(*op, x, y),
                _ => None,
            };
            if let Some(r) = folded {
                repl = Some(Inst::Const(*d, r));
            } else {
                // algebraic identities that can neither overflow nor trap
                repl = match (&op.checked(), la, lb) {
                    (Op::Add, _, Lat::C(0))
                    | (Op::Sub, _, Lat::C(0))
                    | (Op::Mul, _, Lat::C(1))
                    | (Op::Div, _, Lat::C(1)) => Some(Inst::Mov(*d, *a)),
                    (Op::Add, Lat::C(0), _) | (Op::Mul, Lat::C(1), _) => Some(Inst::Mov(*d, *b)),
                    (Op::Mul, _, Lat::C(0)) | (Op::Mul, Lat::C(0), _) => Some(Inst::Const(*d, 0)),
                    _ => None,
                };
            }
            st[*d] = match &repl {
                Some(Inst::Const(_, n)) => Lat::C(*n),
                Some(Inst::Mov(_, s)) => st[*s],
                _ => Lat::Top,
            };
        }
        Inst::Call(d, ..) | Inst::ExternCall(d, ..) | Inst::CallInd(d, ..) => {
            if let Some(d) = d {
                st[*d] = Lat::Top;
            }
        }
        Inst::Thread(_, d, _)
        | Inst::Atomic(_, d, _)
        | Inst::Syscall(d, _)
        | Inst::Global(d, _) => st[*d] = Lat::Top,
        Inst::PrintInt(_) | Inst::PrintBool(_) | Inst::PrintText(_) => {}
        // memory-derived values are never constants for the folder
        Inst::Slot(d, _)
        | Inst::Load(d, ..)
        | Inst::LoadH(d, ..)
        | Inst::Lea(d, ..)
        | Inst::ListGet(d, ..) => st[*d] = Lat::Top,
        Inst::Alloc(d, _)
        | Inst::Load8(d, ..)
        | Inst::Realloc(d, ..)
        | Inst::LoadX(d, ..)
        | Inst::LeaX(d, ..) => st[*d] = Lat::Top,
        Inst::N(_, d, ..) => st[*d] = Lat::Top,
        Inst::StoreX(..) => {}
        Inst::Store(..) | Inst::Copy(..) | Inst::ListPush(..) | Inst::ListFree(..) => {}
        Inst::Free(..)
        | Inst::MemCpy(..)
        | Inst::Store8(..)
        | Inst::PrintBytes(..)
        | Inst::Panic(..)
        | Inst::Check(..)
        | Inst::CheckNonEmpty(..)
        | Inst::Assert(..) => {}
    }
    repl
}

fn constprop(f: &mut IrFunc) {
    let n = f.blocks.len();
    let order = rpo(f);
    let mut inn: Vec<Option<CState>> = vec![None; n];
    inn[0] = Some(vec![Lat::Top; f.nvregs]);
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let Some(mut st) = inn[b].clone() else {
                continue;
            };
            for i in &f.blocks[b].insts {
                cp_step(i, &mut st);
            }
            let targets: Vec<usize> = match &f.blocks[b].term {
                Term::Br(c, t, e) => match st[*c] {
                    Lat::C(0) => vec![*e],
                    Lat::C(_) => vec![*t],
                    _ => vec![*t, *e],
                },
                t => succs(t),
            };
            for s in targets {
                match &mut inn[s] {
                    None => {
                        inn[s] = Some(st.clone());
                        changed = true;
                    }
                    Some(old) => {
                        for k in 0..old.len() {
                            let j = join_lat(old[k], st[k]);
                            if j != old[k] {
                                old[k] = j;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    for &b in &order {
        let Some(mut st) = inn[b].clone() else {
            continue;
        };
        for i in &mut f.blocks[b].insts {
            if let Some(r) = cp_step(i, &mut st) {
                *i = r;
            }
        }
        if let Term::Br(c, t, e) = f.blocks[b].term {
            f.blocks[b].term = match st[c] {
                Lat::C(0) => Term::Jmp(e),
                Lat::C(_) => Term::Jmp(t),
                _ => Term::Br(c, t, e),
            };
        }
    }
}

// ───────────────────────── 2. copy propagation ─────────────────────────

type Copies = HashMap<V, V>;

fn kill(m: &mut Copies, v: V) {
    m.remove(&v);
    m.retain(|_, s| *s != v);
}

fn cpy_step(i: &mut Inst, m: &mut Copies, rewrite: bool) {
    if rewrite {
        uses_mut(i, &mut |u| {
            if let Some(s) = m.get(u) {
                *u = *s;
            }
        });
    }
    if let Some(d) = def(i) {
        kill(m, d);
        if let Inst::Mov(_, s) = i {
            let s = *s;
            if s != d {
                m.insert(d, s);
            }
        }
    }
}

fn copyprop(f: &mut IrFunc) {
    let n = f.blocks.len();
    let order = rpo(f);
    let mut inn: Vec<Option<Copies>> = vec![None; n];
    inn[0] = Some(HashMap::new());
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let Some(mut m) = inn[b].clone() else {
                continue;
            };
            for i in &f.blocks[b].insts {
                cpy_step(&mut i.clone(), &mut m, false);
            }
            for s in succs(&f.blocks[b].term) {
                match &mut inn[s] {
                    None => {
                        inn[s] = Some(m.clone());
                        changed = true;
                    }
                    Some(old) => {
                        let before = old.len();
                        old.retain(|k, v| m.get(k) == Some(v));
                        if old.len() != before {
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    for &b in &order {
        let Some(mut m) = inn[b].clone() else {
            continue;
        };
        for i in &mut f.blocks[b].insts {
            cpy_step(i, &mut m, true);
        }
        match &mut f.blocks[b].term {
            Term::Br(c, ..) => {
                if let Some(s) = m.get(c) {
                    *c = *s;
                }
            }
            Term::Ret(Some(v)) => {
                if let Some(s) = m.get(v) {
                    *v = *s;
                }
            }
            _ => {}
        }
    }
}

// ───────────────────────── liveness ─────────────────────────

fn liveness(f: &IrFunc) -> Vec<Vec<bool>> {
    // returns live-out per block
    let n = f.blocks.len();
    let nv = f.nvregs;
    let mut live_in = vec![vec![false; nv]; n];
    let mut live_out = vec![vec![false; nv]; n];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            let mut out = vec![false; nv];
            for s in succs(&f.blocks[b].term) {
                for k in 0..nv {
                    out[k] |= live_in[s][k];
                }
            }
            let mut live = out.clone();
            for u in term_uses(&f.blocks[b].term) {
                live[u] = true;
            }
            for i in f.blocks[b].insts.iter().rev() {
                if let Some(d) = def(i) {
                    live[d] = false;
                }
                for u in uses(i) {
                    live[u] = true;
                }
            }
            if live != live_in[b] || out != live_out[b] {
                live_in[b] = live;
                live_out[b] = out;
                changed = true;
            }
        }
    }
    live_out
}

// ───────────────────────── 3. coalescing ─────────────────────────

fn coalesce(f: &mut IrFunc) {
    let live_out = liveness(f);
    for b in 0..f.blocks.len() {
        let mut live = live_out[b].clone();
        for u in term_uses(&f.blocks[b].term) {
            live[u] = true;
        }
        let blk = &mut f.blocks[b];
        // candidates: Mov(x, t) at index i where t is dead after i
        let mut cands = vec![];
        for i in (0..blk.insts.len()).rev() {
            if let Inst::Mov(x, t) = &blk.insts[i] {
                if x != t && !live[*t] {
                    cands.push(i);
                }
            }
            if let Some(d) = def(&blk.insts[i]) {
                live[d] = false;
            }
            for u in uses(&blk.insts[i]) {
                live[u] = true;
            }
        }
        let mut remove = vec![false; blk.insts.len()];
        let mut touched = vec![false; blk.insts.len()];
        for i in cands {
            if i == 0 || touched[i - 1] || remove[i - 1] {
                continue;
            }
            let Inst::Mov(x, t) = blk.insts[i].clone() else {
                continue;
            };
            if def(&blk.insts[i - 1]) == Some(t) {
                *def_mut(&mut blk.insts[i - 1]).unwrap() = x;
                touched[i - 1] = true;
                remove[i] = true;
            }
        }
        let mut k = 0;
        blk.insts.retain(|_| {
            k += 1;
            !remove[k - 1]
        });
    }
}

// ───────────────────────── 4. dead-code elimination ─────────────────────────

fn pure(i: &Inst) -> bool {
    match i {
        Inst::Const(..)
        | Inst::Text(..)
        | Inst::FnAddr(..)
        | Inst::VTableAddr(..)
        | Inst::Mov(..)
        | Inst::Not(..) => true,
        Inst::N(op, ..) => op.pure(),
        Inst::Bin(_, op, ..) => is_cmp(*op) || op.is_unchecked(), // checked arithmetic may panic: keep it
        // address computations and plain reads (bounds are checked by a separate `Check`): dead results can go
        Inst::Slot(..)
        | Inst::Lea(..)
        | Inst::LeaX(..)
        | Inst::Load(..)
        | Inst::LoadH(..)
        | Inst::LoadX(..)
        | Inst::Load8(..) => true,
        _ => false,
    }
}

fn dce(f: &mut IrFunc) {
    let live_out = liveness(f);
    for b in 0..f.blocks.len() {
        let mut live = live_out[b].clone();
        for u in term_uses(&f.blocks[b].term) {
            live[u] = true;
        }
        let blk = &mut f.blocks[b];
        let mut keep = vec![true; blk.insts.len()];
        for i in (0..blk.insts.len()).rev() {
            let ins = &blk.insts[i];
            if let Inst::Mov(d, s) = ins {
                if d == s {
                    keep[i] = false;
                    continue;
                }
            }
            if let Some(d) = def(ins) {
                if pure(ins) && !live[d] {
                    keep[i] = false;
                    continue;
                }
                live[d] = false;
            }
            for u in uses(ins) {
                live[u] = true;
            }
        }
        let mut k = 0;
        blk.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
    }
}

// ───────────────────────── 5. CFG simplification ─────────────────────────

fn remap_term(t: &mut Term, m: &dyn Fn(usize) -> usize) {
    match t {
        Term::Jmp(x) => *x = m(*x),
        Term::Br(_, a, b) => {
            *a = m(*a);
            *b = m(*b)
        }
        _ => {}
    }
}

/// Drop unreachable blocks, keeping the relative order (entry stays at 0).
fn drop_unreachable(f: &mut IrFunc) {
    let order = rpo(f);
    let mut reach = vec![false; f.blocks.len()];
    for b in order {
        reach[b] = true;
    }
    let mut newid = vec![usize::MAX; f.blocks.len()];
    let mut c = 0;
    for b in 0..f.blocks.len() {
        if reach[b] {
            newid[b] = c;
            c += 1;
        }
    }
    let old = std::mem::take(&mut f.blocks);
    for (i, mut b) in old.into_iter().enumerate() {
        if reach[i] {
            remap_term(&mut b.term, &|x| newid[x]);
            f.blocks.push(b);
        }
    }
}

fn cfg(f: &mut IrFunc) {
    loop {
        let mut changed = false;
        // Br with identical targets
        for b in &mut f.blocks {
            if let Term::Br(_, t, e) = b.term {
                if t == e {
                    b.term = Term::Jmp(t);
                    changed = true;
                }
            }
        }
        // jump threading through empty blocks
        let n = f.blocks.len();
        let fwd: Vec<usize> = (0..n)
            .map(|b| {
                let mut cur = b;
                for _ in 0..n {
                    match (&f.blocks[cur].insts.is_empty(), &f.blocks[cur].term) {
                        (true, Term::Jmp(t)) if *t != cur => cur = *t,
                        _ => break,
                    }
                }
                cur
            })
            .collect();
        for b in &mut f.blocks {
            let before = succs(&b.term);
            remap_term(&mut b.term, &|x| fwd[x]);
            if succs(&b.term) != before {
                changed = true;
            }
        }
        let nb = f.blocks.len();
        drop_unreachable(f);
        if f.blocks.len() != nb {
            changed = true;
        }
        // merge B into A when A: Jmp B and B has A as its only predecessor
        let mut preds = vec![0usize; f.blocks.len()];
        for b in &f.blocks {
            for s in succs(&b.term) {
                preds[s] += 1;
            }
        }
        for a in 0..f.blocks.len() {
            if let Term::Jmp(bb) = f.blocks[a].term {
                if bb != 0 && bb != a && preds[bb] == 1 {
                    let moved = std::mem::replace(
                        &mut f.blocks[bb],
                        Block {
                            insts: vec![],
                            term: Term::Unreachable,
                        },
                    );
                    f.blocks[a].insts.extend(moved.insts);
                    f.blocks[a].term = moved.term;
                    preds[bb] = 0; // now unreachable; swept below
                    changed = true;
                }
            }
        }
        drop_unreachable(f);
        if !changed {
            break;
        }
    }
}

// ───────────────────────── vreg compaction ─────────────────────────

fn compact_vregs(f: &mut IrFunc) {
    let mut map: Vec<Option<V>> = vec![None; f.nvregs];
    let mut next = 0;
    for p in 0..f.nparams {
        map[p] = Some(p);
        next += 1;
    }
    let mut get = |v: &mut V, map: &mut Vec<Option<V>>| {
        if map[*v].is_none() {
            map[*v] = Some(next);
            next += 1;
        }
        *v = map[*v].unwrap();
    };
    for b in &mut f.blocks {
        for i in &mut b.insts {
            if let Some(d) = def_mut(i) {
                get(d, &mut map);
            }
            uses_mut(i, &mut |u| get(u, &mut map));
        }
        match &mut b.term {
            Term::Br(c, ..) => get(c, &mut map),
            Term::Ret(Some(v)) => get(v, &mut map),
            _ => {}
        }
    }
    let old = std::mem::take(&mut f.vranges);
    let mut vr = vec![None; next];
    for (v, r) in old.iter().enumerate() {
        if let (Some(r), Some(Some(n))) = (r, map.get(v)) {
            vr[*n] = Some(*r);
        }
    }
    f.vranges = vr;
    f.nvregs = next;
}

// ───────────────────────── 6. range analysis ─────────────────────────
//
// Forward interval analysis over vregs (signed 64-bit), with
//   * widening at loop headers followed by a few narrowing sweeps (so `acc = (acc + i) % m` converges),
//   * branch refinement: `Br` on a comparison narrows both operands on each edge,
//   * "a < b" relation facts (from branches and from dominating `Check`s), killed when a vreg is redefined.
// Checked ops are PANICKING, so the state after one is the state in which it did not panic: results are
// clamped to the i64 range. Consumers:
//   * Add/Sub/Mul whose exact result interval fits in i64  -> AddU/SubU/MulU
//   * Neg whose operand is > i64::MIN                      -> 0 - x with SubU
//   * Div/Mod with divisor in a range excluding 0 and (-1 unless dividend > MIN) -> DivU/ModU
//   * Check(i, n) when 0 <= i < n is proven, CheckNonEmpty(n) when n >= 1 is proven.
// Invariant relied upon (only for facts derived AFTER a surviving Check/CheckNonEmpty): the `len` operand of
// Check is a list/string length, so 0 <= len <= 2^47 (it describes a live heap block). The decision to drop a
// check itself never uses it.

type R = (i64, i64);
const TOP: R = (i64::MIN, i64::MAX);
const LEN_MAX: i128 = 1 << 47;

#[derive(Clone, PartialEq)]
struct RState {
    r: Vec<R>,
    lt: HashSet<(V, V)>,           // (a, b): a < b (signed) holds
    conds: HashMap<V, (Op, V, V)>, // c == (a op b), for branch refinement
    loads: HashMap<(V, i32), V>,   // [addr+off] currently equals this vreg (no store/call since)
}

fn fit(lo: i128, hi: i128) -> (R, bool) {
    let ok = lo >= i64::MIN as i128 && hi <= i64::MAX as i128;
    let l = lo.max(i64::MIN as i128);
    let h = hi.min(i64::MAX as i128);
    if l > h {
        (TOP, false)
    } else {
        ((l as i64, h as i64), ok)
    }
}

/// Result interval of a checked op and whether the check is provably unnecessary.
fn arith_range(op: Op, x: R, y: R) -> (R, bool) {
    let (xl, xh, yl, yh) = (x.0 as i128, x.1 as i128, y.0 as i128, y.1 as i128);
    match op {
        Op::Add => fit(xl + yl, xh + yh),
        Op::Sub => fit(xl - yh, xh - yl),
        Op::Mul => {
            let c = [xl * yl, xl * yh, xh * yl, xh * yh];
            fit(*c.iter().min().unwrap(), *c.iter().max().unwrap())
        }
        Op::Div | Op::Mod => {
            let nonzero = y.0 > 0 || y.1 < 0;
            let no_min_neg1 = !(y.0 <= -1 && -1 <= y.1 && x.0 == i64::MIN);
            let ok = nonzero && no_min_neg1;
            let rng = if op == Op::Div {
                if nonzero {
                    let c = [xl / yl, xl / yh, xh / yl, xh / yh];
                    fit(*c.iter().min().unwrap(), *c.iter().max().unwrap()).0
                } else {
                    let m = xl.abs().max(xh.abs());
                    fit(-m, m).0
                }
            } else {
                let m = yl.abs().max(yh.abs());
                if m == 0 {
                    TOP
                } else {
                    let b = m - 1;
                    let lo = if xl >= 0 { 0 } else { xl.max(-b) };
                    let hi = if xh <= 0 { 0 } else { xh.min(b) };
                    fit(lo, hi).0
                }
            };
            (rng, ok)
        }
        _ => (TOP, false),
    }
}

fn rkill(st: &mut RState, v: V) {
    if !st.lt.is_empty() {
        st.lt.retain(|&(a, b)| a != v && b != v);
    }
    if !st.conds.is_empty() {
        st.conds
            .retain(|k, &mut (_, a, b)| *k != v && a != v && b != v);
    }
    if !st.loads.is_empty() {
        st.loads.retain(|&(a, _), s| a != v && *s != v);
    }
}

fn set(st: &mut RState, d: V, r: R) {
    rkill(st, d);
    st.r[d] = r;
}

/// Intersect the range of `v` with [lo, hi]; false when that leaves nothing (state unchanged then).
fn narrow(st: &mut RState, v: V, lo: i128, hi: i128) -> bool {
    let (l, h) = (lo.max(st.r[v].0 as i128), hi.min(st.r[v].1 as i128));
    if l > h {
        return false;
    }
    st.r[v] = (l as i64, h as i64);
    true
}

fn lt_known(st: &RState, a: V, b: V) -> bool {
    if st.r[a].1 < st.r[b].0 || st.lt.contains(&(a, b)) {
        return true;
    }
    st.lt
        .iter()
        .any(|&(x, m)| x == a && st.lt.contains(&(m, b)))
}

fn cmp_negate(op: Op) -> Op {
    match op {
        Op::Lt => Op::Ge,
        Op::Le => Op::Gt,
        Op::Gt => Op::Le,
        Op::Ge => Op::Lt,
        Op::Eq => Op::Ne,
        _ => Op::Eq,
    }
}

/// Assume `a op b` holds; false when that is impossible.
fn refine(st: &mut RState, op: Op, a: V, b: V) -> bool {
    if a == b {
        return matches!(op, Op::Le | Op::Ge | Op::Eq);
    }
    let (x, y) = (st.r[a], st.r[b]);
    let (xl, xh, yl, yh) = (x.0 as i128, x.1 as i128, y.0 as i128, y.1 as i128);
    match op {
        Op::Lt => {
            if !(narrow(st, a, xl, yh - 1) && narrow(st, b, xl + 1, yh)) {
                return false;
            }
            st.lt.insert((a, b));
            true
        }
        Op::Le => narrow(st, a, xl, yh) && narrow(st, b, xl, yh),
        Op::Gt => refine(st, Op::Lt, b, a),
        Op::Ge => refine(st, Op::Le, b, a),
        Op::Eq => {
            let (l, h) = (xl.max(yl), xh.min(yh));
            narrow(st, a, l, h) && narrow(st, b, l, h)
        }
        _ => {
            // Ne: only a constant at the end of the other interval can be peeled off
            if xl == xh {
                if yl == xl && !narrow(st, b, xl + 1, yh) || yh == xl && !narrow(st, b, yl, xl - 1)
                {
                    return false;
                }
            }
            if yl == yh {
                let (x, c) = (st.r[a], yl);
                if x.0 as i128 == c && !narrow(st, a, c + 1, x.1 as i128)
                    || x.1 as i128 == c && !narrow(st, a, x.0 as i128, c - 1)
                {
                    return false;
                }
            }
            true
        }
    }
}

enum Rew {
    Keep,
    Drop,
    To(Inst),
    NegU(V, V),
}

fn cmp_decide(st: &RState, op: Op, a: V, b: V) -> R {
    let (x, y) = (st.r[a], st.r[b]);
    let t = |c: bool| if c { (1, 1) } else { (0, 0) };
    if a == b {
        return t(matches!(op, Op::Le | Op::Ge | Op::Eq));
    }
    let lt = x.1 < y.0 || lt_known(st, a, b);
    let gt = x.0 > y.1 || lt_known(st, b, a);
    match op {
        Op::Lt if lt => t(true),
        Op::Lt if x.0 >= y.1 => t(false),
        Op::Le if x.1 <= y.0 => t(true),
        Op::Le if gt => t(false),
        Op::Gt if gt => t(true),
        Op::Gt if x.1 <= y.0 => t(false),
        Op::Ge if x.0 >= y.1 => t(true),
        Op::Ge if lt => t(false),
        Op::Eq if lt || gt => t(false),
        Op::Eq if x.0 == x.1 && x == y => t(true),
        Op::Ne if lt || gt => t(true),
        Op::Ne if x.0 == x.1 && x == y => t(false),
        _ => (0, 1),
    }
}

/// Intersect the state with the declared interval of every typed vreg (`IrFunc::vranges`): a vreg of a range
/// type holds a value of that type at every point where it is live.
fn apply_caps(st: &mut RState, caps: &[(V, R)]) {
    for &(v, (lo, hi)) in caps {
        let cur = st.r[v];
        let m = (cur.0.max(lo), cur.1.min(hi));
        if m.0 <= m.1 {
            st.r[v] = m;
        }
    }
}

fn rstep(i: &Inst, st: &mut RState) -> Rew {
    match i {
        Inst::Const(d, n) => set(st, *d, (*n, *n)),
        Inst::Mov(d, s) => {
            if d != s {
                let r = st.r[*s];
                let rels: Vec<(V, V)> = st
                    .lt
                    .iter()
                    .filter(|&&(a, b)| a == *s || b == *s)
                    .map(|&(a, b)| if a == *s { (*d, b) } else { (a, *d) })
                    .collect();
                set(st, *d, r);
                st.lt.extend(rels.into_iter().filter(|&(a, b)| a != b));
            }
        }
        Inst::Bin(d, op, a, b) => {
            let (x, y) = (st.r[*a], st.r[*b]);
            let base = op.checked();
            if is_cmp(base) {
                let r = cmp_decide(st, base, *a, *b);
                set(st, *d, r);
                if d != a && d != b {
                    st.conds.insert(*d, (base, *a, *b));
                }
                return Rew::Keep;
            }
            let (r, ok) = arith_range(base, x, y);
            // moving a value up/down keeps its relations to the others: x < a <= a + k  =>  x < a + k
            let mut carried: Vec<(V, V)> = vec![];
            let mut shift = |st: &RState, a: V, up: bool| {
                for &(p, q) in st.lt.iter() {
                    if up && q == a {
                        carried.push((p, *d));
                    } else if !up && p == a {
                        carried.push((*d, q));
                    }
                }
            };
            match base {
                Op::Add => {
                    if y.0 >= 0 {
                        shift(st, *a, true);
                    }
                    if y.1 <= 0 {
                        shift(st, *a, false);
                    }
                    if x.0 >= 0 {
                        shift(st, *b, true);
                    }
                    if x.1 <= 0 {
                        shift(st, *b, false);
                    }
                }
                Op::Sub => {
                    if y.0 >= 0 {
                        shift(st, *a, false);
                    }
                    if y.1 <= 0 {
                        shift(st, *a, true);
                    }
                }
                _ => {}
            }
            set(st, *d, r);
            st.lt.extend(carried.into_iter().filter(|&(p, q)| p != q));
            if matches!(base, Op::Add | Op::Sub) {
                // d = a + b with b > 0 means a < d, etc. (valid on the non-panicking path)
                let mut rel = vec![];
                if base == Op::Add {
                    if y.0 > 0 {
                        rel.push((*a, *d));
                    }
                    if x.0 > 0 {
                        rel.push((*b, *d));
                    }
                    if y.1 < 0 {
                        rel.push((*d, *a));
                    }
                    if x.1 < 0 {
                        rel.push((*d, *b));
                    }
                } else {
                    if y.0 > 0 {
                        rel.push((*d, *a));
                    }
                    if y.1 < 0 {
                        rel.push((*a, *d));
                    }
                }
                st.lt.extend(rel.into_iter().filter(|&(p, q)| p != q));
            }
            if ok && !op.is_unchecked() {
                return Rew::To(Inst::Bin(*d, op.unchecked(), *a, *b));
            }
        }
        Inst::Neg(d, s) => {
            let x = st.r[*s];
            let (r, ok) = fit(-(x.1 as i128), -(x.0 as i128));
            set(st, *d, r);
            if ok {
                return Rew::NegU(*d, *s);
            }
        }
        Inst::Not(d, s) => {
            let x = st.r[*s];
            let r = if x.0 >= 0 && x.1 <= 1 {
                (1 - x.1, 1 - x.0)
            } else {
                (0, 1)
            };
            set(st, *d, r);
        }
        Inst::Check(idx, n) => {
            let x = st.r[*idx];
            let drop = x.0 >= 0 && lt_known(st, *idx, *n);
            if idx != n {
                // survived: 0 <= idx < n <= 2^47 (n is a length)
                let nh = (st.r[*n].1 as i128).min(LEN_MAX);
                let mut s2 = st.clone();
                if narrow(&mut s2, *idx, 0, nh - 1) && narrow(&mut s2, *n, 1, nh) {
                    s2.lt.insert((*idx, *n));
                    *st = s2;
                }
            }
            if drop {
                return Rew::Drop;
            }
        }
        Inst::Assert(n) => {
            let drop = st.r[*n].0 >= 1;
            narrow(st, *n, 1, i64::MAX as i128);
            // `assert(a < b)`: from here on the comparison holds, like on the true edge of a branch
            if let Some(&(op, a, b)) = st.conds.get(n) {
                let mut s2 = st.clone();
                if refine(&mut s2, op, a, b) {
                    *st = s2;
                }
            }
            if drop {
                return Rew::Drop;
            }
        }
        Inst::CheckNonEmpty(n) => {
            let drop = st.r[*n].0 >= 1;
            narrow(st, *n, 1, LEN_MAX);
            if drop {
                return Rew::Drop;
            }
        }
        Inst::ListGet(d, _, idx) => {
            narrow(st, *idx, 0, i64::MAX as i128);
            set(st, *d, TOP);
        }
        Inst::Load8(d, ..) => set(st, *d, (0, 255)),
        Inst::Load(d, a, off) | Inst::LoadH(d, a, off) => {
            // the same memory word read again (no store in between) is the same value
            if let Some(&s) = st.loads.get(&(*a, *off)) {
                if s != *d {
                    let r = st.r[s];
                    let rels: Vec<(V, V)> = st
                        .lt
                        .iter()
                        .filter(|&&(p, q)| p == s || q == s)
                        .map(|&(p, q)| if p == s { (*d, q) } else { (p, *d) })
                        .collect();
                    set(st, *d, r);
                    st.lt.extend(rels.into_iter().filter(|&(p, q)| p != q));
                    return Rew::Keep;
                }
            }
            set(st, *d, TOP);
            if a != d {
                st.loads.insert((*a, *off), *d);
            }
        }
        Inst::Store(..)
        | Inst::StoreX(..)
        | Inst::Realloc(..)
        | Inst::Copy(..)
        | Inst::ListPush(..)
        | Inst::ListFree(..)
        | Inst::Free(..)
        | Inst::MemCpy(..)
        | Inst::Store8(..)
        | Inst::Call(..)
        | Inst::ExternCall(..)
        | Inst::CallInd(..)
        | Inst::Syscall(..)
        | Inst::Thread(..)
        | Inst::Atomic(..) => {
            st.loads.clear();
            if let Some(d) = def(i) {
                set(st, d, TOP);
            }
        }
        other => {
            if let Some(d) = def(other) {
                set(st, d, TOP);
            }
        }
    }
    Rew::Keep
}

fn state_join(a: &RState, b: &RState) -> RState {
    RState {
        r: a.r
            .iter()
            .zip(&b.r)
            .map(|(x, y)| (x.0.min(y.0), x.1.max(y.1)))
            .collect(),
        lt: a.lt.intersection(&b.lt).cloned().collect(),
        conds: a
            .conds
            .iter()
            .filter(|(k, v)| b.conds.get(k) == Some(v))
            .map(|(k, v)| (*k, *v))
            .collect(),
        loads: a
            .loads
            .iter()
            .filter(|(k, v)| b.loads.get(k) == Some(v))
            .map(|(k, v)| (*k, *v))
            .collect(),
    }
}

/// Join, then widen the vregs that keep growing at this header. A vreg is widened once its range has grown more
/// than `DELAY` times here (so values bounded by a `%` or a clamp settle without widening), and only if it is in
/// `only` (assigned inside the loop; None = all): a vreg the loop never assigns changes only through the outer
/// loops' own widening, and widening it here would throw away precision that narrowing cannot win back.
const DELAY: u8 = 4;
fn state_widen(
    old: &RState,
    new: &RState,
    only: Option<&Vec<bool>>,
    cnt: &mut Vec<u8>,
    th: &[i64],
) -> RState {
    let mut j = state_join(old, new);
    for k in 0..j.r.len() {
        if j.r[k] == old.r[k] {
            continue;
        }
        cnt[k] = cnt[k].saturating_add(1);
        if cnt[k] <= DELAY || only.map_or(false, |o| !o[k]) {
            continue;
        }
        // jump to the next program constant (or the type bound) instead of straight to infinity
        if j.r[k].0 < old.r[k].0 {
            let p = th.partition_point(|&t| t <= j.r[k].0);
            j.r[k].0 = if p == 0 { i64::MIN } else { th[p - 1] };
        }
        if j.r[k].1 > old.r[k].1 {
            let p = th.partition_point(|&t| t < j.r[k].1);
            j.r[k].1 = if p == th.len() { i64::MAX } else { th[p] };
        }
    }
    j
}

/// Edge states leaving block `b` given the state after its instructions (None = infeasible edge).
fn edge_states(t: &Term, st: &RState) -> Vec<(usize, Option<RState>)> {
    match t {
        Term::Jmp(x) => vec![(*x, Some(st.clone()))],
        Term::Br(c, tt, e) => {
            let mk = |truth: bool| -> Option<RState> {
                let mut s = st.clone();
                let r = s.r[*c];
                if truth && r == (0, 0) || !truth && (r.0 > 0 || r.1 < 0) {
                    return None;
                }
                if let Some(&(op, a, b)) = st.conds.get(c) {
                    let op = if truth { op } else { cmp_negate(op) };
                    if !refine(&mut s, op, a, b) {
                        return None;
                    }
                }
                if truth {
                    narrow(&mut s, *c, 1, i64::MAX as i128);
                } else {
                    narrow(&mut s, *c, 0, 0);
                }
                Some(s)
            };
            vec![(*tt, mk(true)), (*e, mk(false))]
        }
        _ => vec![],
    }
}

fn rangeopt(f: &mut IrFunc) {
    rangeopt_impl(f, None);
}

/// A bounds check that survived the range analysis but whose index range is known: `Check(idx, len)` at
/// `blocks[block].insts[at]` with `0 <= idx <= hi` on every execution that reaches it.
pub struct CheckInfo {
    pub block: usize,
    pub at: usize,
    pub len: V,
    pub hi: i64,
}

pub(crate) fn rangeopt_collect(f: &mut IrFunc) -> Vec<CheckInfo> {
    let mut v = vec![];
    rangeopt_impl(f, Some(&mut v));
    v
}

fn rangeopt_impl(f: &mut IrFunc, mut collect: Option<&mut Vec<CheckInfo>>) {
    let n = f.blocks.len();
    let nv = f.nvregs;
    let order = rpo(f);
    let mut pos = vec![usize::MAX; n];
    for (k, &b) in order.iter().enumerate() {
        pos[b] = k;
    }
    let mut header = vec![false; n];
    for &b in &order {
        for s in succs(&f.blocks[b].term) {
            if pos[s] <= pos[b] {
                header[s] = true;
            }
        }
    }
    // widening thresholds: constants of the function and their neighbours (bounds of `%`, loop limits)
    let mut th: Vec<i64> = vec![-1, 0, 1];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const(_, c) = i {
                for d in [-1i64, 0, 1] {
                    th.extend(c.checked_add(d));
                    th.extend(c.checked_neg().and_then(|x| x.checked_add(d)));
                }
            }
        }
    }
    th.sort();
    th.dedup();
    if th.len() > 512 {
        th = vec![-1, 0, 1];
    }
    // vregs assigned inside the natural loop of each header
    let mut preds: Vec<Vec<usize>> = vec![vec![]; n];
    for &b in &order {
        for s in succs(&f.blocks[b].term) {
            preds[s].push(b);
        }
    }
    let mut loop_defs: Vec<Option<Vec<bool>>> = vec![None; n];
    for &t in &order {
        for h in succs(&f.blocks[t].term) {
            if pos[h] > pos[t] {
                continue;
            }
            let mut body = vec![false; n];
            body[h] = true;
            let mut work = vec![t];
            while let Some(x) = work.pop() {
                if !body[x] {
                    body[x] = true;
                    work.extend(preds[x].iter().cloned());
                }
            }
            let defs = loop_defs[h].get_or_insert_with(|| vec![false; nv]);
            for (b, inb) in body.iter().enumerate() {
                if *inb {
                    for i in &f.blocks[b].insts {
                        if let Some(d) = def(i) {
                            defs[d] = true;
                        }
                    }
                }
            }
        }
    }
    let mut entry_ranges = vec![TOP; nv];
    for (v, range) in f.vranges.iter().enumerate().take(nv) {
        if let Some((lo, hi)) = range {
            entry_ranges[v] = (*lo, *hi);
        }
    }
    let caps: Vec<(V, R)> = f
        .vranges
        .iter()
        .enumerate()
        .filter(|(v, _)| *v < nv)
        .filter_map(|(v, r)| r.map(|r| (v, r)))
        .collect();
    let cap_of: Vec<Option<R>> = (0..nv)
        .map(|v| f.vranges.get(v).copied().flatten())
        .collect();
    let mut entry = RState {
        r: entry_ranges,
        lt: HashSet::new(),
        conds: HashMap::new(),
        loads: HashMap::new(),
    };
    apply_caps(&mut entry, &caps);
    let run_block = |b: usize, st: &mut RState| {
        apply_caps(st, &caps);
        for i in &f.blocks[b].insts {
            rstep(i, st);
            if let Some(d) = def(i) {
                if let Some(c) = cap_of.get(d).copied().flatten() {
                    let cur = st.r[d];
                    let m = (cur.0.max(c.0), cur.1.min(c.1));
                    if m.0 <= m.1 {
                        st.r[d] = m;
                    }
                }
            }
        }
    };
    let mut inn: Vec<Option<RState>> = vec![None; n];
    inn[0] = Some(entry.clone());
    let mut visits = vec![0usize; n];
    let mut cnt: Vec<Vec<u8>> = vec![vec![0u8; nv]; n];
    let mut changed = true;
    let mut rounds = 0;
    while changed {
        changed = false;
        rounds += 1;
        if rounds > 200 {
            return; // give up (should not happen): leave the function untouched
        }
        for &b in &order {
            let Some(mut st) = inn[b].clone() else {
                continue;
            };
            run_block(b, &mut st);
            for (s, es) in edge_states(&f.blocks[b].term, &st) {
                let Some(es) = es else { continue };
                let new = match &inn[s] {
                    None => es,
                    Some(old) => {
                        let j = state_join(old, &es);
                        if j == *old {
                            continue;
                        }
                        visits[s] += 1;
                        if visits[s] > 60 {
                            state_widen(old, &es, None, &mut cnt[s], &th)
                        } else if header[s] {
                            state_widen(old, &es, loop_defs[s].as_ref(), &mut cnt[s], &th)
                        } else {
                            j
                        }
                    }
                };
                inn[s] = Some(new);
                changed = true;
            }
        }
    }
    // narrowing: re-derive every in-state from the current ones (still a sound post-fixpoint each time)
    for _ in 0..4 {
        let mut new: Vec<Option<RState>> = vec![None; n];
        new[0] = Some(entry.clone());
        for &b in &order {
            let Some(mut st) = inn[b].clone() else {
                continue;
            };
            run_block(b, &mut st);
            for (s, es) in edge_states(&f.blocks[b].term, &st) {
                let Some(es) = es else { continue };
                new[s] = Some(match &new[s] {
                    None => es,
                    Some(o) => state_join(o, &es),
                });
            }
        }
        inn = new;
    }
    // rewrite
    let mut next_v = f.nvregs;
    for &b in &order {
        let Some(mut st) = inn[b].clone() else {
            continue;
        };
        apply_caps(&mut st, &caps);
        let old = std::mem::take(&mut f.blocks[b].insts);
        let mut out = Vec::with_capacity(old.len());
        for i in old {
            let info = match (&i, collect.is_some()) {
                (Inst::Check(idx, len), true) if idx != len => {
                    let x = st.r[*idx];
                    if x.0 >= 0 && x.1 < i64::MAX {
                        Some((*len, x.1))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let rew = rstep(&i, &mut st);
            if let Some(d) = def(&i) {
                if let Some(c) = cap_of.get(d).copied().flatten() {
                    let cur = st.r[d];
                    let m = (cur.0.max(c.0), cur.1.min(c.1));
                    if m.0 <= m.1 {
                        st.r[d] = m;
                    }
                }
            }
            match rew {
                Rew::Keep => {
                    if let (Some((len, hi)), Some(c)) = (info, collect.as_deref_mut()) {
                        c.push(CheckInfo {
                            block: b,
                            at: out.len(),
                            len,
                            hi,
                        });
                    }
                    out.push(i)
                }
                Rew::Drop => {}
                Rew::To(x) => out.push(x),
                Rew::NegU(d, s) => {
                    let z = next_v;
                    next_v += 1;
                    out.push(Inst::Const(z, 0));
                    out.push(Inst::Bin(d, Op::SubU, z, s));
                }
            }
        }
        f.blocks[b].insts = out;
    }
    f.nvregs = next_v;
}
