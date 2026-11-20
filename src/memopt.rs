// Loop-invariant hoisting of list-header loads (`LoadH`).
//
// A list/string lives in memory as {ptr, len, cap}; every `get/set/len/push` reads those words. This pass moves
// a `LoadH [a+off]` out of a natural loop into a preheader when nothing in the loop can write that word, so that
// ptr/len of a list that is only read in the loop stay in registers.
//
// What may write a header word (everything else cannot):
//   * Store / Copy / ListPush / ListFree: by address. Same base vreg: offsets decide. Slot-derived bases: slots
//     are disjoint frame objects. A slot is never aliased by a pointer that is not derived from it (frame
//     addresses are never stored to memory or returned: checked below, otherwise the pass is skipped).
//   * Call: any call may write memory reachable through its arguments. Slot-based words are only clobbered when a
//     frame pointer into that slot (or an unclassifiable one) is passed; every other word is always clobbered.
//   * StoreX / Store8 / MemCpy / Free / Alloc / Print*: element, byte and bulk writes. They never write a header
//     word (a header word is only ever written by a word Store of the list lowering, Copy, or a call).
// Only bases that are always valid pointers are hoisted from: a frame slot or a never-redefined parameter. Hoisting
// executes the load even when the loop body would not, so it must not be able to fault.
use crate::ir::*;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cls {
    Slot(usize, i64), // exactly &frame_slot[k] + byte offset
    Frame,            // derived from some frame address, not tracked precisely
    Other,            // parameters, heap pointers
}

fn defs_of(i: &Inst) -> Option<V> {
    match i {
        Inst::Const(d, _) | Inst::Text(d, _) | Inst::FnAddr(d, _) | Inst::VTableAddr(d, _) | Inst::Mov(d, _) | Inst::Bin(d, ..) | Inst::Neg(d, _) | Inst::Not(d, _) => Some(*d),
        Inst::Slot(d, _) | Inst::Load(d, ..) | Inst::LoadH(d, ..) | Inst::Lea(d, ..) | Inst::ListGet(d, ..) => Some(*d),
        Inst::Alloc(d, _) | Inst::Load8(d, ..) | Inst::LoadX(d, ..) | Inst::LeaX(d, ..) => Some(*d),
        Inst::N(_, d, ..) => Some(*d),
        Inst::Thread(_, d, _) | Inst::Atomic(_, d, _) | Inst::Syscall(d, _) | Inst::Global(d, _) => Some(*d),
        Inst::Call(d, ..) | Inst::CallInd(d, ..) => *d,
        _ => None,
    }
}

fn succs(t: &Term) -> Vec<usize> {
    match t {
        Term::Jmp(x) => vec![*x],
        Term::Br(_, a, b) => vec![*a, *b],
        _ => vec![],
    }
}

fn rpo(f: &IrFunc) -> Vec<usize> {
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

fn operands(i: &Inst) -> Vec<V> {
    match i {
        Inst::Const(..) | Inst::Text(..) | Inst::VTableAddr(..) | Inst::Slot(..) => vec![],
        Inst::Mov(_, s) | Inst::Not(_, s) | Inst::Lea(_, s, _) => vec![*s],
        Inst::Bin(_, _, a, b) | Inst::LeaX(_, a, b, _) => vec![*a, *b],
        _ => vec![],
    }
}

/// Instructions that can be executed speculatively and early: no memory access, no panic, no trap.
fn hoistable_pure(i: &Inst) -> bool {
    match i {
        Inst::Const(..) | Inst::Text(..) | Inst::Slot(..) | Inst::Mov(..) | Inst::Not(..) | Inst::Lea(..) | Inst::LeaX(..) => true,
        Inst::Bin(_, op, ..) => matches!(op, Op::AddU | Op::SubU | Op::MulU | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne),
        _ => false,
    }
}

struct Info {
    cls: Vec<Cls>,
    ndefs: Vec<usize>,
}

/// Classify every vreg; None when the frame-address invariant cannot be established (skip the pass).
fn analyse(f: &IrFunc) -> Option<Info> {
    let nv = f.nvregs;
    let mut ndefs = vec![0usize; nv];
    let mut def_inst: Vec<Option<&Inst>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = defs_of(i) {
                ndefs[d] += 1;
                def_inst[d] = Some(i);
            }
        }
    }
    // a parameter is implicitly defined on entry: an instruction assigning it is a second definition
    for p in 0..f.nparams.min(nv) {
        ndefs[p] += 1;
    }
    // frame-derived: fixpoint over every kind of definition that can forward a pointer
    let mut derived = vec![false; nv];
    loop {
        let mut ch = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, src): (V, bool) = match i {
                    Inst::Slot(d, _) => (*d, true),
                    Inst::Mov(d, s) | Inst::Lea(d, s, _) => (*d, derived[*s]),
                    Inst::Bin(d, _, a, b2) | Inst::LeaX(d, a, b2, _) => (*d, derived[*a] || derived[*b2]),
                    _ => continue,
                };
                if src && !derived[d] {
                    derived[d] = true;
                    ch = true;
                }
            }
        }
        if !ch {
            break;
        }
    }
    // a frame address must never be stored to memory or returned
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Store(_, _, v) | Inst::Store8(_, _, v) | Inst::StoreX(_, _, _, v) | Inst::ListPush(_, v) if derived[*v] => return None,
                _ => {}
            }
        }
        if let Term::Ret(Some(v)) = b.term {
            if derived[v] {
                return None;
            }
        }
    }
    let mut cls = vec![Cls::Other; nv];
    for v in 0..nv {
        if derived[v] {
            cls[v] = Cls::Frame;
        }
    }
    // exact slot addresses: single-definition chains Slot / Lea / Mov (definitions are acyclic in practice;
    // iterate a few times to propagate through the chains)
    for _ in 0..8 {
        for v in 0..nv {
            if !derived[v] || ndefs[v] != 1 {
                continue;
            }
            cls[v] = match def_inst[v] {
                Some(Inst::Slot(_, k)) => Cls::Slot(*k, 0),
                Some(Inst::Lea(_, s, o)) => match cls[*s] {
                    Cls::Slot(k, b) if ndefs[*s] == 1 => Cls::Slot(k, b + *o as i64),
                    _ => Cls::Frame,
                },
                Some(Inst::Mov(_, s)) => match cls[*s] {
                    Cls::Slot(k, b) if ndefs[*s] == 1 => Cls::Slot(k, b),
                    _ => Cls::Frame,
                },
                _ => Cls::Frame,
            };
        }
    }
    Some(Info { cls, ndefs })
}

fn overlap(o1: i64, n1: i64, o2: i64, n2: i64) -> bool {
    o1 < o2 + n2 && o2 < o1 + n1
}

/// May a write of `n` bytes at [b+o] touch the 8 bytes at [a+off]?
fn write_alias(inf: &Info, b: V, o: i64, n: i64, a: V, off: i64) -> bool {
    if b == a {
        return overlap(o, n, off, 8);
    }
    match (inf.cls[b], inf.cls[a]) {
        (Cls::Slot(k1, b1), Cls::Slot(k2, b2)) => k1 == k2 && overlap(b1 + o, n, b2 + off, 8),
        (Cls::Other, Cls::Other) => true,
        (Cls::Other, _) | (_, Cls::Other) => false, // heap/parameter memory is never frame memory
        _ => true,                                  // Frame vs Slot/Frame: unknown
    }
}

fn clobbers(inf: &Info, i: &Inst, a: V, off: i64) -> bool {
    match i {
        Inst::Store(b, o, _) => write_alias(inf, *b, *o as i64, 8, a, off),
        Inst::Copy(d, _, n) => write_alias(inf, *d, 0, 8 * *n as i64, a, off),
        Inst::ListPush(l, _) => write_alias(inf, *l, 0, 24, a, off) || inf.cls[*l] == Cls::Other,
        Inst::ListFree(l) => write_alias(inf, *l, 0, 24, a, off),
        Inst::ListGet(..) => false,
        Inst::Thread(..) | Inst::Atomic(..) | Inst::Syscall(..) | Inst::ExternCall(..) => true,
        Inst::Call(_, _, args) | Inst::CallInd(_, _, args) => match inf.cls[a] {
            Cls::Other => true,
            Cls::Slot(k, _) => args.iter().any(|x| match inf.cls[*x] {
                Cls::Slot(k2, _) => k2 == k,
                Cls::Frame => true,
                Cls::Other => false,
            }),
            Cls::Frame => args.iter().any(|x| inf.cls[*x] != Cls::Other),
        },
        _ => false,
    }
}

struct Loop {
    header: usize,
    body: Vec<bool>,
}

fn find_loops(f: &IrFunc) -> Vec<Loop> {
    let n = f.blocks.len();
    let order = rpo(f);
    let mut pos = vec![usize::MAX; n];
    for (k, &b) in order.iter().enumerate() {
        pos[b] = k;
    }
    let mut preds: Vec<Vec<usize>> = vec![vec![]; n];
    for &b in &order {
        for s in succs(&f.blocks[b].term) {
            preds[s].push(b);
        }
    }
    let mut loops: Vec<Loop> = vec![];
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
                    work.extend(preds[x].iter().cloned().filter(|&p| pos[p] != usize::MAX));
                }
            }
            if let Some(l) = loops.iter_mut().find(|l| l.header == h) {
                for (i, b) in body.iter().enumerate() {
                    l.body[i] |= *b;
                }
            } else {
                loops.push(Loop { header: h, body });
            }
        }
    }
    loops.sort_by_key(|l| l.body.iter().filter(|b| **b).count());
    loops
}

/// Insert a block at index `at`, shifting every block id >= at.
fn insert_block(f: &mut IrFunc, at: usize, blk: Block) {
    let m = |x: usize| if x >= at { x + 1 } else { x };
    for b in &mut f.blocks {
        b.term = match b.term {
            Term::Jmp(t) => Term::Jmp(m(t)),
            Term::Br(c, t, e) => Term::Br(c, m(t), m(e)),
            ref t => t.clone(),
        };
    }
    f.blocks.insert(at, blk);
}

/// Hoist from one loop. Returns true when the function changed.
fn hoist_loop(f: &mut IrFunc, inf: &Info, lp: &Loop) -> bool {
    let h = lp.header;
    if h == 0 {
        return false;
    }
    let n = f.blocks.len();
    // vregs defined inside the loop
    let mut def_in = vec![false; f.nvregs];
    for (b, inb) in lp.body.iter().enumerate() {
        if *inb {
            for i in &f.blocks[b].insts {
                if let Some(d) = defs_of(i) {
                    def_in[d] = true;
                }
            }
        }
    }
    // candidate locations, in order of first appearance
    let mut cands: Vec<(V, i32)> = vec![];
    for (b, inb) in lp.body.iter().enumerate() {
        if !*inb {
            continue;
        }
        for i in &f.blocks[b].insts {
            if let Inst::LoadH(_, a, off) = i {
                let a = *a;
                let safe_base = match inf.cls[a] {
                    Cls::Slot(..) => true,
                    Cls::Other => a < f.nparams && inf.ndefs[a] == 1,
                    Cls::Frame => false,
                };
                if !safe_base || def_in[a] || cands.contains(&(a, *off)) {
                    continue;
                }
                let killed = lp.body.iter().enumerate().any(|(b2, inb2)| {
                    *inb2 && f.blocks[b2].insts.iter().any(|i2| clobbers(inf, i2, a, *off as i64))
                });
                if !killed {
                    cands.push((a, *off));
                }
            }
        }
    }
    // pure single-definition instructions whose operands are invariant (chains are found iteratively)
    let mut inv_def = def_in.clone();
    let mut pure_hoist: Vec<(usize, usize)> = vec![]; // (block, inst index), in discovery order
    let mut taken: HashSet<(usize, usize)> = HashSet::new();
    loop {
        let mut grew = false;
        for (b, inb) in lp.body.iter().enumerate() {
            if !*inb {
                continue;
            }
            for (ii, i) in f.blocks[b].insts.iter().enumerate() {
                if taken.contains(&(b, ii)) {
                    continue;
                }
                let Some(d) = defs_of(i) else { continue };
                if inf.ndefs[d] != 1 || !hoistable_pure(i) {
                    continue;
                }
                if operands(i).iter().all(|o| !inv_def[*o]) {
                    inv_def[d] = false;
                    taken.insert((b, ii));
                    pure_hoist.push((b, ii));
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let trivial = |&(b, ii): &(usize, usize)| matches!(f.blocks[b].insts[ii], Inst::Const(..) | Inst::Text(..) | Inst::Slot(..));
    if cands.is_empty() && pure_hoist.iter().all(trivial) {
        return false; // constants are rematerialised anyway
    }
    let outside: Vec<usize> = (0..n)
        .filter(|&p| !lp.body[p] && succs(&f.blocks[p].term).contains(&h))
        .collect();
    if outside.is_empty() {
        return false;
    }
    let mut pre_insts = vec![];
    for &(b, ii) in &pure_hoist {
        pre_insts.push(f.blocks[b].insts[ii].clone());
    }
    for &(b, ii) in &pure_hoist {
        // the instruction moves to the preheader: leave a no-op behind that dce removes
        f.blocks[b].insts[ii] = Inst::Mov(0, 0);
    }
    let mut hv = vec![];
    for &(a, off) in &cands {
        let v = f.nvregs;
        f.nvregs += 1;
        pre_insts.push(Inst::LoadH(v, a, off));
        hv.push(v);
    }
    // replace the loads inside the loop (block ids are still the original ones here)
    for b in 0..n {
        if !lp.body[b] {
            continue;
        }
        for i in f.blocks[b].insts.iter_mut() {
            if let Inst::LoadH(d, a, off) = i {
                if let Some(k) = cands.iter().position(|c| *c == (*a, *off)) {
                    *i = Inst::Mov(*d, hv[k]);
                }
            }
        }
    }
    // preheader: the single outside predecessor when it just jumps here, else a fresh block before the header
    if outside.len() == 1 && matches!(f.blocks[outside[0]].term, Term::Jmp(_)) {
        f.blocks[outside[0]].insts.extend(pre_insts);
    } else {
        insert_block(f, h, Block { insts: pre_insts, term: Term::Jmp(h + 1) });
        for &p in &outside {
            let p = if p >= h { p + 1 } else { p };
            let redir = |t: usize| if t == h + 1 { h } else { t };
            f.blocks[p].term = match f.blocks[p].term {
                Term::Jmp(t) => Term::Jmp(redir(t)),
                Term::Br(c, t, e) => Term::Br(c, redir(t), redir(e)),
                ref t => t.clone(),
            };
        }
    }
    true
}

pub fn run(f: &mut IrFunc) -> bool {
    let mut changed = false;
    for _ in 0..64 {
        let Some(inf) = analyse(f) else { return changed };
        let loops = find_loops(f);
        let mut did = false;
        for lp in &loops {
            if hoist_loop(f, &inf, lp) {
                did = true;
                break;
            }
        }
        if !did {
            break;
        }
        changed = true;
    }
    changed
}
