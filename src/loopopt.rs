// Loop optimisations on the (non-SSA) IR: natural-loop discovery and strength reduction of `iv * k`.
//
// Strength reduction. A basic induction variable `v` is a vreg whose only definition inside the loop is
// `v = v + step` (Add/AddU, `step` not assigned in the loop). A `t = MulU v, k` (k not assigned in the loop; MulU
// means the range analysis proved the product cannot overflow) that runs on every iteration is replaced by
// `t = s`, where `s` is a fresh vreg kept equal to `v * k` (modulo 2^64): initialised in a new preheader
// (`s = v *% k`) and bumped by `step *% k` right after v's own increment. Everything is wrapping (WAdd/WMul),
// so `s` equals the true product wherever the MulU was proven exact, and nothing is assumed about `s` elsewhere.
use crate::ir::*;
use crate::opt::{def, rpo, succs, CheckInfo};

pub struct Loop {
    pub header: usize,
    pub body: Vec<bool>,
    pub latches: Vec<usize>,
}

fn preds_of(f: &IrFunc, order: &[usize]) -> Vec<Vec<usize>> {
    let mut preds = vec![vec![]; f.blocks.len()];
    for &b in order {
        for s in succs(&f.blocks[b].term) {
            if !preds[s].contains(&b) {
                preds[s].push(b);
            }
        }
    }
    preds
}

/// Immediate dominators (Cooper/Harvey/Kennedy), indexed by block id; unreachable blocks and the entry get usize::MAX/0.
pub fn idoms(f: &IrFunc, order: &[usize], preds: &[Vec<usize>]) -> Vec<usize> {
    let n = f.blocks.len();
    let mut pos = vec![usize::MAX; n];
    for (k, &b) in order.iter().enumerate() {
        pos[b] = k;
    }
    let mut idom = vec![usize::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().skip(1) {
            let mut new = usize::MAX;
            for &p in &preds[b] {
                if idom[p] == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    p
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while pos[x] > pos[y] {
                            x = idom[x];
                        }
                        while pos[y] > pos[x] {
                            y = idom[y];
                        }
                    }
                    x
                };
            }
            if new != idom[b] {
                idom[b] = new;
                changed = true;
            }
        }
    }
    idom
}

pub fn dominates(idom: &[usize], a: usize, mut b: usize) -> bool {
    loop {
        if a == b {
            return true;
        }
        if b == 0 || idom[b] == usize::MAX {
            return false;
        }
        b = idom[b];
    }
}

/// Natural loops (one per header; back edges to the same header are merged).
pub fn find_loops(f: &IrFunc) -> Vec<Loop> {
    let n = f.blocks.len();
    let order = rpo(f);
    let preds = preds_of(f, &order);
    let idom = idoms(f, &order, &preds);
    let mut loops: Vec<Loop> = vec![];
    for &t in &order {
        for h in succs(&f.blocks[t].term) {
            if !dominates(&idom, h, t) {
                continue;
            }
            let li = match loops.iter().position(|l| l.header == h) {
                Some(i) => i,
                None => {
                    let mut body = vec![false; n];
                    body[h] = true;
                    loops.push(Loop {
                        header: h,
                        body,
                        latches: vec![],
                    });
                    loops.len() - 1
                }
            };
            if !loops[li].latches.contains(&t) {
                loops[li].latches.push(t);
            }
            let body = &mut loops[li].body;
            let mut work = vec![t];
            while let Some(x) = work.pop() {
                if !body[x] {
                    body[x] = true;
                    work.extend(preds[x].iter().cloned());
                }
            }
        }
    }
    loops
}

/// Give the loop a dedicated preheader (a new block jumping to the header, taking over every edge from outside the
/// loop). Returns its id, or None when the header is the entry block.
pub fn make_preheader(f: &mut IrFunc, l: &Loop) -> Option<usize> {
    let h = l.header;
    if h == 0 {
        return None;
    }
    let order = rpo(f);
    let preds = preds_of(f, &order);
    let outside: Vec<usize> = preds[h]
        .iter()
        .cloned()
        .filter(|&p| !l.body.get(p).copied().unwrap_or(false))
        .collect();
    if outside.is_empty() {
        return None;
    }
    let np = f.blocks.len();
    f.blocks.push(Block {
        insts: vec![],
        term: Term::Jmp(h),
    });
    for p in outside {
        match &mut f.blocks[p].term {
            Term::Jmp(x) => {
                if *x == h {
                    *x = np
                }
            }
            Term::Br(_, a, b) => {
                if *a == h {
                    *a = np
                }
                if *b == h {
                    *b = np
                }
            }
            _ => {}
        }
    }
    Some(np)
}

/// Strength-reduce `iv * invariant` in every loop. Returns true when something changed.
pub fn strength(f: &mut IrFunc) -> bool {
    let loops = find_loops(f);
    if loops.is_empty() {
        return false;
    }
    let order = rpo(f);
    let preds = preds_of(f, &order);
    let idom = idoms(f, &order, &preds);
    // innermost loop of every block (smallest body containing it)
    let nb = f.blocks.len();
    let mut inner = vec![usize::MAX; nb];
    let mut inner_sz = vec![usize::MAX; nb];
    for (li, l) in loops.iter().enumerate() {
        let sz = l.body.iter().filter(|&&x| x).count();
        for b in 0..nb {
            if l.body[b] && sz < inner_sz[b] {
                inner[b] = li;
                inner_sz[b] = sz;
            }
        }
    }
    let mut changed = false;
    for (li, l) in loops.iter().enumerate() {
        // definitions inside the loop
        let mut ndefs = vec![0usize; f.nvregs];
        for b in 0..nb {
            if l.body[b] {
                for i in &f.blocks[b].insts {
                    if let Some(d) = def(i) {
                        ndefs[d] += 1;
                    }
                }
            }
        }
        // basic induction variables: v -> (block, inst index, step vreg)
        let mut ivs: Vec<(V, usize, usize, V)> = vec![];
        for b in 0..nb {
            if !l.body[b] {
                continue;
            }
            for (ix, i) in f.blocks[b].insts.iter().enumerate() {
                if let Inst::Bin(d, Op::Add | Op::AddU, x, y) = i {
                    let (v, step) = if d == x {
                        (*d, *y)
                    } else if d == y {
                        (*d, *x)
                    } else {
                        continue;
                    };
                    if ndefs[v] == 1 && ndefs[step] == 0 && step != v {
                        ivs.push((v, b, ix, step));
                    }
                }
            }
        }
        if ivs.is_empty() {
            continue;
        }
        // candidates: MulU in a block whose innermost loop is this one and that runs on every iteration
        let mut cands: Vec<(usize, usize, V, V, V)> = vec![]; // block, index, t, iv, k
        for b in 0..nb {
            if !l.body[b] || inner[b] != li || !l.latches.iter().all(|&t| dominates(&idom, b, t)) {
                continue;
            }
            for (ix, i) in f.blocks[b].insts.iter().enumerate() {
                if let Inst::Bin(t, Op::MulU, x, y) = i {
                    for (a, k) in [(*x, *y), (*y, *x)] {
                        if ndefs[k] == 0 && ivs.iter().any(|iv| iv.0 == a) && a != k {
                            cands.push((b, ix, *t, a, k));
                            break;
                        }
                    }
                }
            }
        }
        if cands.is_empty() {
            continue;
        }
        let Some(np) = make_preheader(f, l) else {
            continue;
        };
        let mut pre: Vec<Inst> = vec![];
        let mut made: Vec<((V, V), V)> = vec![]; // (iv, k) -> s
        let mut bumps: Vec<(usize, usize, Vec<Inst>)> = vec![]; // after (block, index): instructions
        for &(b, ix, t, iv, k) in &cands {
            let s = if let Some(&(_, s)) = made.iter().find(|m| m.0 == (iv, k)) {
                s
            } else {
                let s = f.nvregs;
                let delta = f.nvregs + 1;
                f.nvregs += 2;
                let &(_, ib, iix, step) = ivs.iter().find(|x| x.0 == iv).unwrap();
                pre.push(Inst::N(NOp::WMul, s, iv, k));
                pre.push(Inst::N(NOp::WMul, delta, step, k));
                bumps.push((ib, iix, vec![Inst::N(NOp::WAdd, s, s, delta)]));
                made.push(((iv, k), s));
                s
            };
            f.blocks[b].insts[ix] = Inst::Mov(t, s);
        }
        f.blocks[np].insts = pre;
        // apply bumps back to front per block so indices stay valid
        bumps.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        for (b, ix, ins) in bumps {
            for (o, i) in ins.into_iter().enumerate() {
                f.blocks[b].insts.insert(ix + 1 + o, i);
            }
        }
        changed = true;
        // block ids / dominators changed: one loop per call keeps the bookkeeping trivial
        break;
    }
    changed
}

fn remap_blocks(t: &mut Term, m: &dyn Fn(usize) -> usize) {
    match t {
        Term::Jmp(x) => *x = m(*x),
        Term::Br(_, a, b) => {
            *a = m(*a);
            *b = m(*b);
        }
        _ => {}
    }
}

const VERSION_MAX_BODY: usize = 150;
const VERSION_MAX_FUNC: usize = 6000;

/// Instructions of a block without the location markers (they take no code, so they are not part of any size limit).
fn unmarked(insts: &[Inst]) -> usize {
    insts.iter().filter(|i| !matches!(i, Inst::Loc(_))).count()
}

/// Bounds-check elimination by loop versioning. For an innermost loop whose `Check(idx, len)` has a loop-invariant
/// `len` and an index range [0, hi] known from the range analysis, the loop is duplicated: a guard in front of
/// the loop tests `hi < len` for every such check, and when it holds the check-free copy runs, otherwise the
/// original loop with all its checks (so a panic still fires at exactly the same point, after the same side
/// effects). The guard is a pure comparison on loop-invariant values, evaluated before the first iteration.
pub fn version_checks(f: &mut IrFunc, cands: &[CheckInfo]) -> bool {
    if cands.is_empty() || f.blocks.iter().map(|b| unmarked(&b.insts)).sum::<usize>() > VERSION_MAX_FUNC
    {
        return false;
    }
    let loops = find_loops(f);
    let nb0 = f.blocks.len();
    let mut changed = false;
    for l in &loops {
        // innermost only
        if loops
            .iter()
            .any(|o| o.header != l.header && l.body[o.header])
        {
            continue;
        }
        let size: usize = (0..nb0)
            .filter(|&b| l.body[b])
            .map(|b| unmarked(&f.blocks[b].insts) + 1)
            .sum();
        if size > VERSION_MAX_BODY {
            continue;
        }
        let mut defd = vec![false; f.nvregs];
        for b in 0..nb0 {
            if l.body[b] {
                for i in &f.blocks[b].insts {
                    if let Some(d) = def(i) {
                        defd[d] = true;
                    }
                }
            }
        }
        let mine: Vec<&CheckInfo> = cands
            .iter()
            .filter(|c| c.block < nb0 && l.body[c.block] && !defd[c.len])
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut guards: Vec<(V, i64)> = vec![];
        for c in &mine {
            match guards.iter_mut().find(|g| g.0 == c.len) {
                Some(g) => g.1 = g.1.max(c.hi),
                None => guards.push((c.len, c.hi)),
            }
        }
        let Some(np) = make_preheader(f, l) else {
            continue;
        };
        // clone the body
        let mut map = vec![usize::MAX; nb0];
        let mut order: Vec<usize> = vec![];
        for b in 0..nb0 {
            if l.body[b] {
                map[b] = f.blocks.len() + order.len();
                order.push(b);
            }
        }
        for &b in &order {
            let mut nbk = Block {
                insts: f.blocks[b].insts.clone(),
                term: f.blocks[b].term.clone(),
            };
            remap_blocks(&mut nbk.term, &|x| {
                if x < nb0 && map[x] != usize::MAX {
                    map[x]
                } else {
                    x
                }
            });
            let mut drops: Vec<usize> =
                mine.iter().filter(|c| c.block == b).map(|c| c.at).collect();
            drops.sort();
            for &at in drops.iter().rev() {
                nbk.insts.remove(at);
            }
            f.blocks.push(nbk);
        }
        let fast = map[l.header];
        // guard chain: np -> g1 -> g2 ... -> fast, any failing guard goes to the original header
        let slow = l.header;
        let mut cur = np;
        for (gi, &(len, hi)) in guards.iter().enumerate() {
            let c = f.nvregs;
            let g = f.nvregs + 1;
            f.nvregs += 2;
            let target = if gi + 1 == guards.len() {
                fast
            } else {
                f.blocks.push(Block {
                    insts: vec![],
                    term: Term::Jmp(slow),
                });
                f.blocks.len() - 1
            };
            f.blocks[cur].insts = vec![Inst::Const(c, hi), Inst::Bin(g, Op::Lt, c, len)];
            f.blocks[cur].term = Term::Br(g, target, slow);
            cur = target;
        }
        changed = true;
    }
    changed
}

/// An instruction that cannot write memory (so a header word read before the loop still holds inside it).
fn reads_only(i: &Inst) -> bool {
    matches!(
        i,
        Inst::N(..)
            | Inst::Const(..)
            | Inst::Text(..)
            | Inst::Mov(..)
            | Inst::Bin(..)
            | Inst::Neg(..)
            | Inst::Not(..)
            | Inst::FnAddr(..)
            | Inst::VTableAddr(..)
            | Inst::Slot(..)
            | Inst::Load(..)
            | Inst::LoadH(..)
            | Inst::Lea(..)
            | Inst::Load8(..)
            | Inst::LoadX(..)
            | Inst::LeaX(..)
            | Inst::Check(..)
            | Inst::CheckNonEmpty(..)
            | Inst::Assert(..)
            | Inst::Loc(..)
            | Inst::Global(..)
    )
}

/// Loop-invariant list/string header loads: in a loop that never writes memory (no store, call, allocation...), a
/// `LoadH` through an address defined outside the loop reads the same word every iteration, so it is read once in
/// the preheader. This keeps `len`/`ptr` of the lists in registers and lets the range analysis see one length vreg
/// for the whole loop. Returns true when something moved.
pub fn hoist_header_loads(f: &mut IrFunc) -> bool {
    let mut changed = false;
    let mut done: Vec<usize> = vec![];
    loop {
        let loops = find_loops(f);
        let Some(l) = loops.into_iter().find(|l| !done.contains(&l.header)) else { break };
        done.push(l.header);
        let nb = f.blocks.len();
        let inb = |b: usize| l.body.get(b).copied().unwrap_or(false);
        if (0..nb).any(|b| inb(b) && f.blocks[b].insts.iter().any(|i| !reads_only(i))) {
            continue;
        }
        let mut ndefs = vec![0usize; f.nvregs];
        for b in 0..nb {
            if inb(b) {
                for i in &f.blocks[b].insts {
                    if let Some(d) = def(i) {
                        ndefs[d] += 1;
                    }
                }
            }
        }
        let cand = |i: &Inst| matches!(i, Inst::LoadH(d, a, _) if ndefs[*a] == 0 && ndefs[*d] == 1);
        if !(0..nb).any(|b| inb(b) && f.blocks[b].insts.iter().any(|i| cand(i))) {
            continue;
        }
        let Some(pre) = make_preheader(f, &l) else { continue };
        let live = crate::opt::liveness(f);
        let mut hoisted = vec![];
        for b in 0..nb {
            if !inb(b) {
                continue;
            }
            let insts = std::mem::take(&mut f.blocks[b].insts);
            for i in insts {
                if cand(&i) && matches!(&i, Inst::LoadH(d, ..) if !live[pre][*d]) {
                    hoisted.push(i);
                } else {
                    f.blocks[b].insts.push(i);
                }
            }
        }
        changed |= !hoisted.is_empty();
        f.blocks[pre].insts.extend(hoisted);
    }
    changed
}
