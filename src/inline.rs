// IR-level function inliner (runs inside `opt::run`, between two rounds of the per-function passes).
//
// Policy (bottom-up over the call graph, so callees already contain their own inlined callees):
//   * never inline runtime helpers (`__*`), recursive functions (anything on a call-graph cycle) or `main`;
//   * inline when the callee is small (<= SMALL), or a call-free leaf up to LEAF_LOOP when the call
//     site sits in a loop (bigger leaves, as they are the typical `step`/accessor kernels);
//   * a caller stops growing at max(MIN_CAP, 3 x its original size);
//   * optionally one level of a small self-recursive function into itself (`RECURSIVE_ONE_LEVEL`).
// The inlined body is a renumbered copy of the callee: vregs are offset, parameters become `Mov`s from the
// arguments, frame slots are appended, `Ret(v)` becomes `Mov(dst, v); Jmp(cont)`. Everything the callee
// did (checks, panics, calls, evaluation order of its body) stays in place, so behaviour is identical.
use crate::ir::*;
use crate::sroa::{remap_term, uses_mut};

const SMALL: usize = 36;
const LEAF_LOOP: usize = 110;
const MIN_CAP: usize = 500;
const RECURSIVE_ONE_LEVEL: usize = 40; // a small self-recursive function is inlined into itself ...
const REC_ROUNDS: usize = 3; // ... this many times (each round inlines the previous result: 2^k levels),
const REC_ROUND_MAX: usize = 70; // ... while the body stays below this size

fn size(f: &IrFunc) -> usize {
    f.blocks.iter().map(|b| b.insts.len() + 1).sum()
}

fn calls_of(f: &IrFunc) -> Vec<usize> {
    let mut v = vec![];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(_, c, _) = i {
                if !v.contains(c) {
                    v.push(*c);
                }
            }
        }
    }
    v
}

fn is_leaf(f: &IrFunc) -> bool {
    !f.blocks.iter().any(|b| {
        b.insts
            .iter()
            .any(|i| matches!(i, Inst::Call(..) | Inst::CallInd(..)))
    })
}

/// Per block: is it inside a loop (a back edge j -> i with i <= j covers blocks i..=j)?
fn in_loop(f: &IrFunc) -> Vec<bool> {
    let n = f.blocks.len();
    let mut r = vec![false; n];
    for (j, b) in f.blocks.iter().enumerate() {
        let ss: Vec<usize> = match &b.term {
            Term::Jmp(x) => vec![*x],
            Term::Br(_, a, c) => vec![*a, *c],
            _ => vec![],
        };
        for s in ss {
            if s <= j {
                for x in &mut r[s..=j] {
                    *x = true;
                }
            }
        }
    }
    r
}

/// Functions that can reach themselves through calls.
fn recursive(p: &IrProgram) -> Vec<bool> {
    let n = p.funcs.len();
    let g: Vec<Vec<usize>> = p.funcs.iter().map(calls_of).collect();
    let mut rec = vec![false; n];
    for s in 0..n {
        let mut seen = vec![false; n];
        let mut st: Vec<usize> = g[s].clone();
        while let Some(x) = st.pop() {
            if x == s {
                rec[s] = true;
                break;
            }
            if !seen[x] {
                seen[x] = true;
                st.extend(g[x].iter().copied());
            }
        }
    }
    rec
}

/// Callee-first order (callees before callers); recursive functions come wherever.
fn order(p: &IrProgram) -> Vec<usize> {
    let n = p.funcs.len();
    let g: Vec<Vec<usize>> = p.funcs.iter().map(calls_of).collect();
    let mut seen = vec![false; n];
    let mut out = vec![];
    for s in 0..n {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut st = vec![(s, 0usize)];
        while let Some((x, i)) = st.pop() {
            if i < g[x].len() {
                st.push((x, i + 1));
                let y = g[x][i];
                if !seen[y] {
                    seen[y] = true;
                    st.push((y, 0));
                }
            } else {
                out.push(x);
            }
        }
    }
    out
}

fn clone_func(f: &IrFunc) -> IrFunc {
    IrFunc {
        name: f.name.clone(),
        nparams: f.nparams,
        vranges: f.vranges.clone(),
        nvregs: f.nvregs,
        slots: f.slots.clone(),
        blocks: f
            .blocks
            .iter()
            .map(|b| Block {
                insts: b.insts.clone(),
                term: b.term.clone(),
            })
            .collect(),
    }
}

/// Inline `callee` at instruction `ci` of block `bi` of `f`.
fn inline_at(f: &mut IrFunc, bi: usize, ci: usize, callee: &IrFunc) {
    let (dst, args) = match &f.blocks[bi].insts[ci] {
        Inst::Call(d, _, a) => (*d, a.clone()),
        _ => unreachable!(),
    };
    let vbase = f.nvregs;
    let sbase = f.slots.len();
    let bbase = f.blocks.len() + 1; // block 0 of the callee lands at bbase; the continuation is f.blocks.len()
    let cont = f.blocks.len();
    f.nvregs += callee.nvregs;
    // the callee's declared intervals carry over (its parameters, range variables and typed results)
    if callee.vranges.iter().any(|r| r.is_some()) {
        if f.vranges.len() < f.nvregs {
            f.vranges.resize(f.nvregs, None);
        }
        for (v, r) in callee.vranges.iter().enumerate() {
            if r.is_some() {
                f.vranges[vbase + v] = *r;
            }
        }
    }
    f.slots.extend(callee.slots.iter().copied());
    // split the call block: continuation gets the tail and the original terminator
    let tail: Vec<Inst> = f.blocks[bi].insts.split_off(ci + 1);
    f.blocks[bi].insts.pop(); // the call itself
    let old_term = std::mem::replace(&mut f.blocks[bi].term, Term::Jmp(bbase));
    for (k, a) in args.iter().enumerate() {
        f.blocks[bi].insts.push(Inst::Mov(vbase + k, *a));
    }
    f.blocks.push(Block {
        insts: tail,
        term: old_term,
    });
    for b in &callee.blocks {
        let mut insts = Vec::with_capacity(b.insts.len() + 1);
        for i in &b.insts {
            let mut i = i.clone();
            match &mut i {
                Inst::Const(d, _)
                | Inst::Text(d, _)
                | Inst::FnAddr(d, _)
                | Inst::VTableAddr(d, _)
                | Inst::Mov(d, _)
                | Inst::Bin(d, ..)
                | Inst::Neg(d, _)
                | Inst::Not(d, _) => *d += vbase,
                Inst::Slot(d, k) => {
                    *d += vbase;
                    *k += sbase;
                }
                Inst::Load(d, ..)
                | Inst::LoadH(d, ..)
                | Inst::LoadX(d, ..)
                | Inst::LeaX(d, ..)
                | Inst::Lea(d, ..)
                | Inst::ListGet(d, ..)
                | Inst::Alloc(d, _)
                | Inst::Load8(d, ..)
                | Inst::N(_, d, ..)
                | Inst::Thread(_, d, _)
                | Inst::Atomic(_, d, _)
                | Inst::Syscall(d, _)
                | Inst::Global(d, _) => *d += vbase,
                Inst::Call(Some(d), ..)
                | Inst::CallInd(Some(d), ..)
                | Inst::ExternCall(Some(d), ..) => *d += vbase,
                _ => {}
            }
            uses_mut(&mut i, &mut |u| *u += vbase);
            insts.push(i);
        }
        let term = match &b.term {
            Term::Ret(v) => {
                if let (Some(d), Some(v)) = (dst, v) {
                    insts.push(Inst::Mov(d, *v + vbase));
                }
                Term::Jmp(cont)
            }
            Term::Br(c, t, e) => Term::Br(*c + vbase, *t + bbase, *e + bbase),
            Term::Jmp(t) => Term::Jmp(*t + bbase),
            Term::Unreachable => Term::Unreachable,
        };
        f.blocks.push(Block { insts, term });
    }
    // layout: callee blocks and the continuation go right after the call block (not at the end), so the
    // inlined body reads straight through and loops keep their blocks contiguous
    let n = f.blocks.len();
    let mut ord: Vec<usize> = (0..=bi).collect();
    ord.extend(bbase..n);
    ord.push(cont);
    ord.extend(bi + 1..cont);
    let mut pos = vec![0usize; n];
    for (new, old) in ord.iter().enumerate() {
        pos[*old] = new;
    }
    let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks)
        .into_iter()
        .map(Some)
        .collect();
    for o in &ord {
        let mut b = old[*o].take().unwrap();
        remap_term(&mut b.term, &|x| pos[x]);
        f.blocks.push(b);
    }
}

/// `optf` re-optimises a function right after it received inlined bodies (so it is cheap to inline further up).
pub fn run(p: &mut IrProgram, optf: &mut dyn FnMut(&mut IrFunc)) {
    let rec = recursive(p);
    let orig: Vec<usize> = p.funcs.iter().map(size).collect();
    let ord = order(p);
    let rounds: usize = std::env::var("YERUX_REC_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(REC_ROUNDS);
    for fi in ord {
        for round in 0..rounds {
            if round > 0
                && !(rec[fi]
                    && orig[fi] <= RECURSIVE_ONE_LEVEL
                    && size(&p.funcs[fi]) <= REC_ROUND_MAX)
            {
                break;
            }
            let cap = MIN_CAP.max(3 * orig[fi]);
            let one_level = rec[fi]
                && orig[fi] <= RECURSIVE_ONE_LEVEL
                && !p.funcs[fi].name.starts_with("__")
                && std::env::var_os("YERUX_NO_REC_INLINE").is_none();
            let snapshot = if one_level {
                Some(clone_func(&p.funcs[fi]))
            } else {
                None
            };
            let mut fresh = vec![false; p.funcs[fi].blocks.len()]; // blocks that came from inlining
            let mut did = false;
            let mut bi = 0;
            while bi < p.funcs[fi].blocks.len() {
                let mut ci = 0;
                while ci < p.funcs[fi].blocks[bi].insts.len() {
                    let (callee_id, nargs) = match &p.funcs[fi].blocks[bi].insts[ci] {
                        Inst::Call(_, c, a) => (*c, a.len()),
                        _ => {
                            ci += 1;
                            continue;
                        }
                    };
                    let f = &p.funcs[fi];
                    let self_call = callee_id == fi;
                    let ok = if self_call {
                        // one level only, and only for call sites of the original body
                        snapshot.is_some() && !fresh[bi] && size(f) < cap
                    } else {
                        let g = &p.funcs[callee_id];
                        let looped = in_loop(f)[bi];
                        !rec[callee_id]
                            && !g.name.starts_with("__")
                            && callee_id != p.main
                            && g.nparams == nargs
                            && size(f) + size(g) <= cap
                            && (size(g) <= SMALL || (looped && is_leaf(g) && size(g) <= LEAF_LOOP))
                    };
                    if !ok {
                        ci += 1;
                        continue;
                    }
                    let callee = if self_call {
                        clone_func(snapshot.as_ref().unwrap())
                    } else {
                        clone_func(&p.funcs[callee_id])
                    };
                    let cn = callee.blocks.len();
                    inline_at(&mut p.funcs[fi], bi, ci, &callee);
                    // same permutation as inline_at: [..=bi] [callee blocks] [continuation] [rest]
                    let tail = fresh.split_off(bi + 1);
                    let inherit = fresh[bi];
                    fresh.extend(std::iter::repeat_n(true, cn));
                    fresh.push(inherit);
                    fresh.extend(tail);
                    did = true;
                    break; // rest of this block moved to the continuation block, which is visited later
                }
                bi += 1;
            }
            if did {
                optf(&mut p.funcs[fi]);
            }
        }
    }
}
