// Scalar replacement of aggregates (SROA) for frame slots.
//
// A frame slot whose address never escapes is split into one vreg per 8-byte word. "Never escapes" means
// every vreg derived from `Slot(v, k)` (through single-definition `Mov` / `Lea`) is only used as
//   * the address operand of Load / Store (constant word-aligned offset inside the slot),
//   * either side of a `Copy` whose other side is memory or another (different) slot,
//   * the source of another such derived `Mov` / `Lea`.
// Anything else (call argument, stored as a value, list helper, Load8/Store8, branch, return, ...)
// keeps the slot in memory. Copies from/to memory are expanded into word loads/stores, copies between
// two scalarised slots into word moves. The word vregs are zero-initialised at function entry so they
// are never live-in at the start (garbage words of partially written enums stay well defined).
//
// Runs inside the `opt` fixpoint; the inliner exposes the slots of inlined callees to it.
use crate::ir::*;
use crate::regalloc::inst_def;

const MAX_WORDS: usize = 16;

pub(crate) fn uses_mut(i: &mut Inst, f: &mut dyn FnMut(&mut V)) {
    match i {
        Inst::Const(..) | Inst::Text(..) | Inst::Slot(..) | Inst::FnAddr(..) | Inst::VTableAddr(..) | Inst::Global(..) => {}
        Inst::Thread(_, _, args) | Inst::Atomic(_, _, args) | Inst::Syscall(_, args) => args.iter_mut().for_each(f),
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
        Inst::Load(_, a, _) | Inst::LoadH(_, a, _) | Inst::Lea(_, a, _) | Inst::ListFree(a) => f(a),
        Inst::LoadX(_, b, i, _) | Inst::LeaX(_, b, i, _) => {
            f(b);
            f(i)
        }
        Inst::StoreX(b, i, _, v) => {
            f(b);
            f(i);
            f(v)
        }
        Inst::Store(a, _, v) | Inst::Copy(a, v, _) | Inst::ListPush(a, v) | Inst::ListGet(_, a, v) => {
            f(a);
            f(v)
        }
        Inst::Alloc(_, n) | Inst::Free(n) | Inst::Load8(_, n, _) | Inst::CheckNonEmpty(n) | Inst::Assert(n) => f(n),
        Inst::Store8(a, _, v) | Inst::PrintBytes(a, v) | Inst::Panic(a, v) | Inst::Check(a, v) => {
            f(a);
            f(v)
        }
        Inst::MemCpy(d, s, n) => {
            f(d);
            f(s);
            f(n)
        }
        Inst::Realloc(_, old, n, used) => {
            f(old);
            f(n);
            f(used)
        }
    }
}

pub(crate) fn remap_term(t: &mut Term, m: &dyn Fn(usize) -> usize) {
    match t {
        Term::Jmp(x) => *x = m(*x),
        Term::Br(_, a, b) => {
            *a = m(*a);
            *b = m(*b)
        }
        _ => {}
    }
}

fn succs(t: &Term) -> Vec<usize> {
    match t {
        Term::Jmp(x) => vec![*x],
        Term::Br(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

fn term_use(t: &Term) -> Option<V> {
    match t {
        Term::Br(c, ..) => Some(*c),
        Term::Ret(Some(v)) => Some(*v),
        _ => None,
    }
}

/// Where a derived address points: (slot, byte offset).
type Ptr = Option<(usize, i32)>;

/// Returns true when something was scalarised.
pub fn run(f: &mut IrFunc) -> bool {
    let nv = f.nvregs;
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    // definition counts (parameters count as one)
    let mut defs = vec![0usize; nv];
    for p in 0..f.nparams.min(nv) {
        defs[p] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defs[d] += 1;
            }
        }
    }
    // derived address map (fixpoint: a Lea may appear before its base in block order)
    let mut ptr: Vec<Ptr> = vec![None; nv];
    let mut escaped = vec![false; ns];
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, src) = match i {
                    Inst::Slot(d, k) => {
                        if defs[*d] == 1 && ptr[*d].is_none() {
                            ptr[*d] = Some((*k, 0));
                            changed = true;
                        }
                        continue;
                    }
                    Inst::Lea(d, a, o) => (*d, ptr[*a].map(|(k, x)| (k, x + *o))),
                    Inst::Mov(d, a) => (*d, ptr[*a]),
                    _ => continue,
                };
                if defs[d] == 1 && ptr[d].is_none() {
                    if let Some(p) = src {
                        ptr[d] = Some(p);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    // a Slot vreg with several definitions is an escape
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Slot(d, k) = i {
                if ptr[*d] != Some((*k, 0)) {
                    escaped[*k] = true;
                }
            }
        }
    }
    for k in 0..ns {
        if f.slots[k] > MAX_WORDS {
            escaped[k] = true;
        }
    }
    let size = |k: usize| f.slots[k] as i32 * 8;
    let ok_word = |p: (usize, i32), extra: i32| p.1 >= 0 && p.1 % 8 == 0 && p.1 + extra <= size(p.0);
    for b in &f.blocks {
        for i in &b.insts {
            // classify every use of a tracked vreg
            let esc = |v: V, escaped: &mut Vec<bool>| {
                if let Some((k, _)) = ptr[v] {
                    escaped[k] = true;
                }
            };
            match i {
                Inst::Slot(..) => {}
                Inst::Mov(d, a) | Inst::Lea(d, a, _) => {
                    // derived chain is fine only when the destination is tracked too
                    if ptr[*a].is_some() && ptr[*d].is_none() {
                        esc(*a, &mut escaped);
                    }
                }
                Inst::Load(_, a, o) | Inst::LoadH(_, a, o) => {
                    if let Some((k, x)) = ptr[*a] {
                        if !ok_word((k, x + *o), 8) {
                            escaped[k] = true;
                        }
                    }
                }
                Inst::Store(a, o, v) => {
                    esc(*v, &mut escaped);
                    if let Some((k, x)) = ptr[*a] {
                        if !ok_word((k, x + *o), 8) {
                            escaped[k] = true;
                        }
                    }
                }
                Inst::Copy(d, s, n) => {
                    let (pd, ps) = (ptr[*d], ptr[*s]);
                    let len = *n as i32 * 8;
                    if let Some(p) = pd {
                        if !ok_word(p, len) {
                            escaped[p.0] = true;
                        }
                    }
                    if let Some(p) = ps {
                        if !ok_word(p, len) {
                            escaped[p.0] = true;
                        }
                    }
                    if let (Some(a), Some(b)) = (pd, ps) {
                        if a.0 == b.0 {
                            escaped[a.0] = true;
                        }
                    }
                }
                other => {
                    let mut o = other.clone();
                    uses_mut(&mut o, &mut |v| esc(*v, &mut escaped));
                }
            }
        }
        if let Some(v) = term_use(&b.term) {
            if let Some((k, _)) = ptr[v] {
                escaped[k] = true;
            }
        }
    }
    // a derived vreg whose own slot escaped is irrelevant; one whose slot survives must have all uses classified
    // above (Mov/Lea destinations are tracked, so their uses were classified when seen as operands).
    if escaped.iter().all(|e| *e) {
        return false;
    }
    // ── rewrite ──
    let mut base = vec![usize::MAX; ns];
    let mut nvn = f.nvregs;
    let mut init: Vec<Inst> = vec![];
    for k in 0..ns {
        if !escaped[k] {
            base[k] = nvn;
            for w in 0..f.slots[k] {
                init.push(Inst::Const(nvn + w, 0));
            }
            nvn += f.slots[k];
        }
    }
    let word = |p: (usize, i32)| base[p.0] + (p.1 / 8) as usize;
    let live = |v: V| -> Ptr { ptr[v].filter(|(k, _)| !escaped[*k]) };
    for b in &mut f.blocks {
        let old = std::mem::take(&mut b.insts);
        for i in old {
            match i {
                Inst::Slot(d, _) | Inst::Mov(d, _) | Inst::Lea(d, ..) if live(d).is_some() => {}
                Inst::Load(d, a, o) | Inst::LoadH(d, a, o) => match live(a) {
                    Some((k, x)) => b.insts.push(Inst::Mov(d, word((k, x + o)))),
                    None => b.insts.push(if matches!(i, Inst::LoadH(..)) { Inst::LoadH(d, a, o) } else { Inst::Load(d, a, o) }),
                },
                Inst::Store(a, o, v) => match live(a) {
                    Some((k, x)) => b.insts.push(Inst::Mov(word((k, x + o)), v)),
                    None => b.insts.push(Inst::Store(a, o, v)),
                },
                Inst::Copy(d, s, n) => match (live(d), live(s)) {
                    (None, None) => b.insts.push(Inst::Copy(d, s, n)),
                    (Some(pd), Some(ps)) => {
                        for w in 0..n {
                            b.insts.push(Inst::Mov(word((pd.0, pd.1 + 8 * w as i32)), word((ps.0, ps.1 + 8 * w as i32))));
                        }
                    }
                    (Some(pd), None) => {
                        for w in 0..n {
                            b.insts.push(Inst::Load(word((pd.0, pd.1 + 8 * w as i32)), s, 8 * w as i32));
                        }
                    }
                    (None, Some(ps)) => {
                        for w in 0..n {
                            b.insts.push(Inst::Store(d, 8 * w as i32, word((ps.0, ps.1 + 8 * w as i32))));
                        }
                    }
                },
                other => b.insts.push(other),
            }
        }
    }
    f.nvregs = nvn;
    // compact the slot table
    let mut newk = vec![usize::MAX; ns];
    let mut slots = vec![];
    for k in 0..ns {
        if escaped[k] {
            newk[k] = slots.len();
            slots.push(f.slots[k]);
        }
    }
    f.slots = slots;
    for b in &mut f.blocks {
        for i in &mut b.insts {
            if let Inst::Slot(_, k) = i {
                *k = newk[*k];
            }
        }
    }
    // zero-initialise at entry (a fresh entry block when block 0 is a branch target)
    let entry_has_pred = f.blocks.iter().any(|b| succs(&b.term).contains(&0));
    if entry_has_pred {
        for b in &mut f.blocks {
            remap_term(&mut b.term, &|x| x + 1);
        }
        f.blocks.insert(0, Block { insts: init, term: Term::Jmp(1) });
    } else {
        let e = &mut f.blocks[0].insts;
        let rest = std::mem::take(e);
        *e = init;
        e.extend(rest);
    }
    true
}
