// Loop rotation for the native backends: a back edge `p: ...; Jmp(h)` to a small loop header
// `h: <cond insts>; Br(c, a, b)` gets a copy of the header, so the latch tests the condition itself and
// jumps straight back to the body. One conditional branch per iteration instead of `jmp` + `jcc`.
// The IR is not SSA, so copying the header's instructions (same vregs) is always valid.
use crate::ir::*;

const MAX_HEADER_INSTS: usize = 8;

pub fn rotate_loops(f: &IrFunc) -> IrFunc {
    let mut blocks: Vec<Block> = f
        .blocks
        .iter()
        .map(|b| Block {
            insts: b.insts.clone(),
            term: b.term.clone(),
        })
        .collect();
    for p in 0..blocks.len() {
        let h = match blocks[p].term {
            Term::Jmp(h) if h < p => h,
            _ => continue,
        };
        let (t, e) = match f.blocks[h].term {
            Term::Br(_, t, e) => (t, e),
            _ => continue,
        };
        // only worth it when one way out of the copy falls through to the next block
        if f.blocks[h].insts.iter().filter(|i| !matches!(i, Inst::Loc(_))).count() > MAX_HEADER_INSTS
            || (t != p + 1 && e != p + 1)
            || t == h
            || e == h
        {
            continue;
        }
        let copy = f.blocks[h].insts.clone();
        blocks[p].insts.extend(copy);
        blocks[p].term = f.blocks[h].term.clone();
    }
    IrFunc {
        name: f.name.clone(),
        lib: f.lib,
        nparams: f.nparams,
        vranges: f.vranges.clone(),
        nvregs: f.nvregs,
        slots: f.slots.clone(),
        blocks,
    }
}

/// Cold-path layout: blocks that call a cold helper (list growth) move to the end of the function, so the hot path
/// of a loop is straight-line code. Block 0 stays first. Returns the reordered function and a per-block cold flag
/// (the register allocator treats calls in cold blocks as "soft": live registers are saved around them instead of
/// forcing the values into callee-saved registers).
pub fn cold_to_end(f: &IrFunc, cold_fn: &[bool]) -> (IrFunc, Vec<bool>) {
    // the growth helper of strings: the block that reallocates is the slow path
    let grow_helper = f.name == "__str_push";
    let n = f.blocks.len();
    let is_cold: Vec<bool> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(bi, b)| {
            bi != 0
                && b.insts.iter().any(|i| match i {
                    Inst::Call(_, c, _) => cold_fn.get(*c).copied().unwrap_or(false),
                    Inst::Realloc(..) => grow_helper,
                    _ => false,
                })
        })
        .collect();
    if !is_cold.iter().any(|c| *c) {
        return (
            IrFunc {
                name: f.name.clone(),
                lib: f.lib,
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
            },
            vec![false; n],
        );
    }
    let mut order: Vec<usize> = (0..n).filter(|b| !is_cold[*b]).collect();
    order.extend((0..n).filter(|b| is_cold[*b]));
    let mut new_id = vec![0usize; n];
    for (k, o) in order.iter().enumerate() {
        new_id[*o] = k;
    }
    let map = |t: &Term| match t {
        Term::Jmp(x) => Term::Jmp(new_id[*x]),
        Term::Br(c, a, b) => Term::Br(*c, new_id[*a], new_id[*b]),
        o => o.clone(),
    };
    let blocks: Vec<Block> = order
        .iter()
        .map(|o| Block {
            insts: f.blocks[*o].insts.clone(),
            term: map(&f.blocks[*o].term),
        })
        .collect();
    let cold: Vec<bool> = order.iter().map(|o| is_cold[*o]).collect();
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
        cold,
    )
}

/// Number of 8-byte zero-initialised global cells reachable through the `__global(i)` intrinsic.
pub const GLOBAL_CELLS: i64 = 64;
