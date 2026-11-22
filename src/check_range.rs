// Checker support for range integers `Int<lo..hi>` (docs/RANGES.md): interval typing of expressions, flow
// narrowing of immutable variables by `if` / `while` conditions, diagnostics, and the conversions
// `Int<lo..hi>.from(x)` (checked, panics) and `Int<lo..hi>.try_from(x)` (a Result).
use super::*;
use crate::numeric::*;

const I64MIN: i128 = i64::MIN as i128;
const I64MAX: i128 = i64::MAX as i128;

/// The static interval of an integer expression, `None` for other types. Literals are singletons.
pub(super) fn ival(e: &Expr) -> Option<(i128, i128)> {
    match e.ty {
        Ty::Int | Ty::Range(..) => match &e.k {
            EK::Int(n) => Some((*n as i128, *n as i128)),
            _ => Some(int_range(e.ty)),
        },
        _ => None,
    }
}

/// `Int` when the interval is all of i64, else the range type.
pub(super) fn mk_range(lo: i128, hi: i128) -> Ty {
    let (lo, hi) = (lo.max(I64MIN), hi.min(I64MAX));
    if lo == I64MIN && hi == I64MAX {
        Ty::Int
    } else {
        Ty::Range(lo as i64, hi as i64)
    }
}

fn neg_op(op: &str) -> &'static str {
    match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        "==" => "!=",
        _ => "==",
    }
}
fn flip_op(op: &str) -> &'static str {
    match op {
        "<" => ">",
        "<=" => ">=",
        ">" => "<",
        ">=" => "<=",
        "==" => "==",
        _ => "!=",
    }
}

/// The interval `x` is left with when `x op other` holds, `x` in `[xl, xh]`, `other` in `[ol, oh]`.
fn constrain(op: &str, (xl, xh): (i128, i128), (ol, oh): (i128, i128)) -> Option<(i128, i128)> {
    let (l, h) = match op {
        "<" => (xl, xh.min(oh - 1)),
        "<=" => (xl, xh.min(oh)),
        ">" => (xl.max(ol + 1), xh),
        ">=" => (xl.max(ol), xh),
        "==" => (xl.max(ol), xh.min(oh)),
        _ => {
            if ol != oh {
                return None;
            }
            if ol == xl {
                (xl + 1, xh)
            } else if ol == xh {
                (xl, xh - 1)
            } else {
                return None;
            }
        }
    };
    // an empty result (1, 0) means the condition cannot hold: the branch is unreachable
    if l > h {
        Some((1, 0))
    } else {
        Some((l, h))
    }
}

impl<'a> Checker<'a> {
    /// The flow type of a read of an immutable integer variable: its declared type narrowed by the facts
    /// known at this point.
    pub(super) fn narrowed(&self, e: &Expr, ty: Ty) -> Ty {
        if let (EK::Var(_, id), Ty::Int | Ty::Range(..)) = (&e.k, ty) {
            if self.stable(*id) {
                if let Some(&(lo, hi)) = self.st.ivl.get(id) {
                    return mk_range(lo as i128, hi as i128);
                }
            }
        }
        ty
    }

    /// Can flow facts be kept about this variable? Immutable ones always. A `var` (or a by-value parameter the
    /// body assigns) only when nothing can change it behind the checker's back: it is never captured by a
    /// closure or a `defer`, never lent as `mut`, never the receiver of a method that might take `mut self`.
    /// Assignments are tracked in the flow state (see `assign_fact`).
    pub(super) fn stable(&self, id: usize) -> bool {
        let v = &self.vars[id];
        v.mode != Mode::Mut && v.ty != Ty::Err && (!v.mutable || !self.unstable.contains(&v.name))
    }

    fn narrowable(&self, e: &Expr) -> Option<usize> {
        if let (EK::Var(_, id), Ty::Int | Ty::Range(..)) = (&e.k, e.ty) {
            if self.stable(*id) {
                return Some(*id);
            }
        }
        None
    }

    /// After `x = value`: what is known about `x` is the interval of the value (inside the declared type).
    pub(super) fn assign_fact(&mut self, id: usize, val: &Expr, line: usize) {
        if !matches!(self.vars[id].ty, Ty::Int | Ty::Range(..)) {
            return;
        }
        if self.st.ivl.remove(&id).is_some() {
            self.lost_facts.insert(id, ("it is assigned on line", line));
        }
        if !self.stable(id) {
            return;
        }
        if let Some(iv) = ival(val) {
            self.apply_facts(&[(id, iv)]);
        }
    }

    /// The initial fact of a new `var`.
    pub(super) fn init_fact(&mut self, id: usize, iv: Option<(i128, i128)>) {
        if let (Some(iv), true) = (iv, self.stable(id)) {
            self.apply_facts(&[(id, iv)]);
        }
    }

    /// Entering a loop: forget the facts about every variable the loop may change.
    pub(super) fn kill_loop_facts(&mut self, c: &Expr, body: &[Stmt], step: &[Stmt], line: usize) {
        if self.st.ivl.is_empty() {
            return;
        }
        let mut names = HashSet::new();
        self.scan_expr(c, &mut names, false);
        self.scan_stmts(body, &mut names, false);
        self.scan_stmts(step, &mut names, false);
        let vars = &self.vars;
        let mut lost = vec![];
        self.st.ivl.retain(|id, _| {
            let keep = !vars[*id].mutable || !names.contains(&vars[*id].name);
            if !keep {
                lost.push(*id);
            }
            keep
        });
        for id in lost {
            self.lost_facts.insert(id, ("the loop on line", line));
        }
    }

    /// Facts that hold when the (already checked) condition `c` evaluates to `truth`.
    pub(super) fn cond_facts(&self, c: &Expr, truth: bool, out: &mut Vec<(usize, (i128, i128))>) {
        self.cond_facts_d(c, truth, out, 0)
    }

    /// `depth` bounds the walk: a very long `a && b && c ..` chain would otherwise make every `&&` re-walk its
    /// whole left side (quadratic). Facts are only an optimisation, so giving up on deep conditions is safe.
    fn cond_facts_d(&self, c: &Expr, truth: bool, out: &mut Vec<(usize, (i128, i128))>, depth: usize) {
        if depth > 64 {
            return;
        }
        match &c.k {
            EK::Un("!", x) => self.cond_facts_d(x, !truth, out, depth + 1),
            EK::Bin("&&", a, b) if truth => {
                self.cond_facts_d(a, true, out, depth + 1);
                self.cond_facts_d(b, true, out, depth + 1);
            }
            EK::Bin("||", a, b) if !truth => {
                self.cond_facts_d(a, false, out, depth + 1);
                self.cond_facts_d(b, false, out, depth + 1);
            }
            // one of the two sides holds: what both sides would prove is the hull of the two
            EK::Bin(op @ ("||" | "&&"), a, b) if (*op == "||") == truth => {
                let (mut fa, mut fb) = (vec![], vec![]);
                self.cond_facts_d(a, truth, &mut fa, depth + 1);
                self.cond_facts_d(b, truth, &mut fb, depth + 1);
                let fold = |v: Vec<(usize, (i128, i128))>| {
                    let mut m: HashMap<usize, (i128, i128)> = HashMap::new();
                    for (id, (l, h)) in v {
                        let e = m.entry(id).or_insert((l, h));
                        *e = (e.0.max(l), e.1.min(h));
                    }
                    m
                };
                let (ma, mb) = (fold(fa), fold(fb));
                let empty = |m: &HashMap<usize, (i128, i128)>| m.values().any(|(l, h)| l > h);
                if empty(&ma) && empty(&mb) {
                    out.extend(ma.into_iter().filter(|(_, (l, h))| l > h));
                } else if empty(&ma) {
                    out.extend(mb.into_iter().filter(|(_, (l, h))| l <= h));
                } else if empty(&mb) {
                    out.extend(ma.into_iter().filter(|(_, (l, h))| l <= h));
                } else {
                    for (id, (alo, ahi)) in ma {
                        if let Some(&(blo, bhi)) = mb.get(&id) {
                            out.push((id, (alo.min(blo), ahi.max(bhi))));
                        }
                    }
                }
            }
            EK::Bin(op @ ("<" | "<=" | ">" | ">=" | "==" | "!="), a, b) => {
                let (Some(ia), Some(ib)) = (ival(a), ival(b)) else {
                    return;
                };
                let op = if truth { *op } else { neg_op(op) };
                if let Some(id) = self.narrowable(a) {
                    if let Some(r) = constrain(op, ia, ib) {
                        out.push((id, r));
                        if let Some(&o) = self.alias.get(&id) {
                            out.push((o, r));
                        }
                    }
                }
                if let Some(id) = self.narrowable(b) {
                    if let Some(r) = constrain(flip_op(op), ib, ia) {
                        out.push((id, r));
                        if let Some(&o) = self.alias.get(&id) {
                            out.push((o, r));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Record facts (intersecting with what is known already).
    pub(super) fn apply_facts(&mut self, facts: &[(usize, (i128, i128))]) {
        for (id, (lo, hi)) in facts {
            let cur = self
                .st
                .ivl
                .get(id)
                .map(|&(l, h)| (l as i128, h as i128))
                .unwrap_or_else(|| int_range(self.vars[*id].ty));
            let (l, h) = (cur.0.max(*lo), cur.1.min(*hi));
            if l > h {
                // the condition cannot hold here: this code never runs, so anything may be assumed in it
                self.st.dead = true;
                continue;
            }
            if l == I64MIN && h == I64MAX {
                continue;
            }
            self.st.ivl.insert(*id, (l as i64, h as i64));
        }
    }

    pub(super) fn facts_of(&self, c: &Expr, truth: bool) -> Vec<(usize, (i128, i128))> {
        let mut v = vec![];
        self.cond_facts(c, truth, &mut v);
        v
    }

    /// `expected Int<lo..hi> but this is ...` for a source that is not provably inside the target.
    pub(super) fn range_mismatch(&mut self, got: Ty, want: Ty, pos: Pos) {
        let Ty::Range(wlo, whi) = want else { return };
        if self.st.dead {
            return; // unreachable code: the facts contradict each other, so nothing needs proving
        }
        let g = self.tn(got);
        let w = self.tn(want);
        let name = self
            .pos_names
            .get(&(pos.line, pos.col))
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .cloned();
        let n = name.as_deref().unwrap_or("x");
        let lost = self
            .pos_vars
            .get(&(pos.line, pos.col))
            .and_then(|id| self.lost_facts.get(id))
            .copied();
        let holds = match (wlo == i64::MIN, whi == i64::MAX) {
            (_, true) => format!("{} holds the values from {} upwards", w, wlo),
            (true, _) => format!("{} holds the values up to {}", w, whi),
            _ => format!(
                "{} holds the values from {} to {} (both ends included)",
                w, wlo, whi
            ),
        };
        let test = match (wlo == i64::MIN, whi == i64::MAX) {
            (_, true) => format!("{} >= {}", n, wlo),
            (true, _) => format!("{} <= {}", n, whi),
            _ => format!("{} >= {} && {} <= {}", n, wlo, n, whi),
        };
        let mut d = match got {
            Ty::Range(a, b) if a == b => {
                diag(format!("the value {} does not fit {}", a, w), pos).note(holds)
            }
            Ty::Range(..) => diag(format!("this value may be outside {}", w), pos)
                .note(format!(
                    "the compiler can only prove that it has type {}",
                    g
                ))
                .note(holds),
            _ => diag(format!("this value may be outside {}", w), pos)
                .note(format!(
                    "it has type {}, so the compiler cannot prove that it fits {}",
                    g, w
                ))
                .note(holds),
        };
        if let Some((why, line)) = lost {
            d = d.note(if why.starts_with("the loop") {
                format!(
                    "{} was narrower earlier, but {} {} may change it",
                    n, why, line
                )
            } else {
                format!("{} was narrower earlier, but {} {}", n, why, line)
            });
        }
        let single = matches!(got, Ty::Range(a, b) if a == b);
        if !single {
            d = d
                .note(format!("help: test it first: `if {} {{ ... }}`; inside, the compiler knows the narrower range", test))
                .note(format!("help: or convert it explicitly: `{}.try_from({})` gives a Result, `{}.from({})` panics when it does not fit", w, n, w, n));
        }
        self.err(d);
    }

    /// `x as Int<lo..hi> else { .. }`: the value of `x` when it fits, else the block (which must not continue).
    /// Free when the compiler can prove that `x` fits; afterwards `x` itself is narrowed too.
    pub(super) fn range_as_else(&mut self, e: &mut Expr) -> Ty {
        let pos = e.pos;
        let rt = e.ty;
        let Ty::Range(lo, hi) = rt else {
            return Ty::Err;
        };
        let EK::AsElse(x, mut blk) = std::mem::replace(&mut e.k, EK::Int(0)) else {
            return Ty::Err;
        };
        let rn = self.tn(rt);
        if self.in_defer {
            self.err(
                diag("`as .. else` cannot be used in a deferred statement", pos)
                    .note("the else block returns or jumps, which a deferred statement cannot do"),
            );
            return Ty::Err;
        }
        let x_id = if let EK::Var(n, _) = &x.k {
            self.lookup(n)
        } else {
            None
        };
        self.hint = None;
        self.hid += 1;
        let vn = format!("$ae{}", self.hid);
        let outer_pre = std::mem::take(&mut self.pre);
        self.scopes.push(HashMap::new());
        let mut out = std::mem::take(&mut self.pre);
        let xpos = x.pos;
        self.chk(&mut out, s_let(&vn, false, *x, pos));
        let at = self.lookup(&vn).map(|i| self.vars[i].ty).unwrap_or(Ty::Err);
        let bad = if at == Ty::Err {
            true
        } else if !is_int(at) {
            let n = self.tn(at);
            self.err(
                diag(
                    format!("`as {}` converts an integer, but this is {}", rn, n),
                    xpos,
                )
                .note(
                    "help: convert a Float to an integer first: `x.to_int()`, `x.floor_int()`, ...",
                ),
            );
            true
        } else if is_u64(at) {
            self.err(
                diag(format!("`as {}` does not take a UInt64", rn), xpos).note(
                    "help: convert it with `x.to_int()` first (panics above 9223372036854775807)",
                ),
            );
            true
        } else {
            false
        };
        if bad {
            self.scopes.pop();
            self.pre = outer_pre;
            return Ty::Err;
        }
        let mut vname = vn.clone();
        if !matches!(at, Ty::Int | Ty::Range(..)) {
            vname = format!("$aw{}", self.hid);
            let src = Expr {
                k: EK::Var(vn.clone(), self.lookup(&vn).unwrap_or(0)),
                pos,
                ty: at,
            };
            let conv = Expr {
                k: EK::Conv(Box::new(src), false),
                pos,
                ty: Ty::Int,
            };
            self.chk(&mut out, s_let(&vname, false, conv, pos));
        }
        let vty = self
            .lookup(&vname)
            .map(|i| self.vars[i].ty)
            .unwrap_or(Ty::Err);
        let vid = self.lookup(&vname).unwrap();
        let (ilo, ihi) = int_range(vty);
        let free = ilo >= lo as i128 && ihi <= hi as i128;
        // the tests `v < lo` and `v > hi`, as two separate branches so that the optimizer learns both bounds
        let mut fail_conds: Vec<Expr> = vec![];
        if lo != i64::MIN {
            fail_conds.push(s_bin("<", s_var(&vname, pos), s_int(lo, pos), pos));
        }
        if hi != i64::MAX {
            fail_conds.push(s_bin(">", s_var(&vname, pos), s_int(hi, pos), pos));
        }
        let mut facts_ok = vec![];
        for c in fail_conds.iter_mut() {
            let ct = self.expr(c);
            self.expect(ct, Ty::Bool, pos);
            facts_ok.extend(self.facts_of(c, false));
        }
        // the else block: checked in a branch of its own; it has to end in `return`, `break`, `continue` or a panic
        let before = self.st.clone();
        self.block(&mut blk);
        let diverges = self.st.dead;
        self.st = before;
        if !diverges {
            let bpos = blk.last().map_or(pos, |s| s.pos);
            self.err(
                diag(format!("the `else` block of `as {}` must not continue", rn), bpos)
                    .note("when the value does not fit there is no value to give, so the block has to leave: `return`, `break`, `continue` or `panic(..)`")
                    .note("help: to use a default instead, write `x.clamp(lo, hi)` or `Int<lo..hi>.try_from(x)` and `match` on the Result"),
            );
        }
        if !free {
            let mut facts = facts_ok;
            if let Some(o) = x_id {
                if self.stable(o) && matches!(self.vars[o].ty, Ty::Int | Ty::Range(..)) {
                    let extra: Vec<_> = facts
                        .iter()
                        .filter(|(i, _)| *i == vid)
                        .map(|(_, r)| (o, *r))
                        .collect();
                    facts.extend(extra);
                }
            }
            self.apply_facts(&facts);
            for c in fail_conds {
                out.push(s_stmt(SK::If(c, blk.clone(), None), pos));
            }
        }
        let mut res = s_var(&vname, pos);
        self.expr(&mut res);
        self.pre = outer_pre;
        self.scopes.pop();
        let rty = res.ty;
        e.k = EK::Sequence(out, Box::new(res));
        rty
    }

    /// `unchecked(x)`: `x` taken as the range type the context expects, without a test (unsafe code only).
    pub(super) fn range_unchecked(&mut self, e: &mut Expr, hint: Option<Ty>) -> Ty {
        let pos = e.pos;
        let EK::Call(_, args) = &mut e.k else {
            return Ty::Err;
        };
        let mut arg = args.pop().unwrap();
        if self.unsafe_depth == 0 {
            self.err(
                diag("`unchecked(x)` is only allowed in an `unsafe` block", pos)
                    .note("it gives `x` a range type without testing it; if the value is outside the range the optimizer's assumptions break (run with `--check-ranges` to catch it)")
                    .note("help: use `x as Int<lo..hi> else { .. }` or `Int<lo..hi>.try_from(x)` to test it, or wrap the statement in `unsafe { .. }`"),
            );
            return Ty::Err;
        }
        let Some(rt @ Ty::Range(..)) = hint else {
            self.err(
                diag("`unchecked(x)` needs to know the range it produces", pos)
                    .note("it takes the range type from where it is used: a typed `var`, an assignment, an argument or a return value")
                    .note("help: write `var y: Int<0..255> = 0` first, then `unsafe { y = unchecked(x) }`"),
            );
            return Ty::Err;
        };
        let at = self.expr(&mut arg);
        if at == Ty::Err {
            return Ty::Err;
        }
        if !is_int(at) || is_u64(at) {
            let n = self.tn(at);
            self.err(diag(format!("`unchecked(x)` takes an Int (or a sized integer except UInt64), but this is {}", n), arg.pos));
            return Ty::Err;
        }
        // a bit-preserving conversion: `Conv(.., true)` into a range type is a no-op (a test under --check-ranges)
        e.k = EK::Conv(Box::new(arg), true);
        rt
    }

    /// `Int<lo..hi>.from(x)` and `Int<lo..hi>.try_from(x)`.
    pub(super) fn range_static(&mut self, e: &mut Expr) -> Ty {
        let pos = e.pos;
        let EK::Static { ty, f, args } = &mut e.k else {
            return Ty::Err;
        };
        let (rt, f) = (*ty, f.clone());
        let Ty::Range(lo, hi) = rt else {
            return Ty::Err;
        };
        let rn = self.tn(rt);
        if f != "from" && f != "try_from" {
            self.err(diag(format!("{} has no function '{}'", rn, f), pos).note("functions: from(x) (checked, panics when x is out of range), try_from(x) (returns a Result)"));
            return Ty::Err;
        }
        if args.len() != 1 {
            self.err(diag(format!("{}.{}() expects 1 argument(s)", rn, f), pos));
            return Ty::Err;
        }
        let mut arg = args.pop().unwrap();
        self.hint = None;
        if f == "from" {
            let at = self.expr(&mut arg);
            if at == Ty::Err {
                return Ty::Err;
            }
            if !is_int(at) {
                let n = self.tn(at);
                self.err(
                    diag(format!("{}.from() converts an integer, but this is {}", rn, n), arg.pos)
                        .note("help: convert a Float to an integer first: `x.to_int()`, `x.floor_int()`, ..."),
                );
                return Ty::Err;
            }
            e.k = EK::Conv(Box::new(arg), false);
            return rt;
        }
        // try_from: Result<Int<lo..hi>, Int>, Err carrying the rejected value
        let Ok(res) = self.with_ps(|ps| ps.generic_instance("Result", vec![rt, Ty::Int], pos))
        else {
            self.err(diag("Result is not available here", pos));
            return Ty::Err;
        };
        let Ty::Enum(reid) = res else { return Ty::Err };
        let vid = |this: &Self, n: &str| {
            this.types.enums[reid]
                .variants
                .iter()
                .position(|v| v.0 == n)
                .unwrap_or(0)
        };
        self.hid += 1;
        let (an, rn_) = (format!("$ra{}", self.hid), format!("$rr{}", self.hid));
        let mut vn = an.clone();
        let outer_pre = std::mem::take(&mut self.pre);
        self.scopes.push(HashMap::new());
        let mut out = std::mem::take(&mut self.pre);
        let apos = arg.pos;
        self.chk(&mut out, s_let(&an, false, arg, pos));
        let at = self.lookup(&an).map(|i| self.vars[i].ty).unwrap_or(Ty::Err);
        let bad = if at == Ty::Err {
            true
        } else if !is_int(at) {
            let n = self.tn(at);
            self.err(
                diag(
                    format!("{}.try_from() converts an integer, but this is {}", rn, n),
                    apos,
                )
                .note(
                    "help: convert a Float to an integer first: `x.to_int()`, `x.floor_int()`, ...",
                ),
            );
            true
        } else if is_u64(at) {
            self.err(
                diag(format!("{}.try_from() does not take a UInt64", rn), apos).note(
                    "help: convert it with `x.to_int()` first (panics above 9223372036854775807)",
                ),
            );
            true
        } else {
            false
        };
        if bad {
            self.scopes.pop();
            self.pre = outer_pre;
            return Ty::Err;
        }
        if !matches!(at, Ty::Int | Ty::Range(..)) {
            vn = format!("$rw{}", self.hid);
            let src = Expr {
                k: EK::Var(an.clone(), self.lookup(&an).unwrap_or(0)),
                pos,
                ty: at,
            };
            let conv = Expr {
                k: EK::Conv(Box::new(src), false),
                pos,
                ty: Ty::Int,
            };
            self.chk(&mut out, s_let(&vn, false, conv, pos));
        }
        let err_v = sx(
            EK::Variant {
                eid: reid,
                vname: "Err".into(),
                vid: vid(self, "Err"),
                args: vec![s_var(&vn, pos)],
            },
            pos,
        );
        self.chk(&mut out, s_let(&rn_, true, err_v, pos));
        let mut cond: Option<Expr> = None;
        if lo != i64::MIN {
            cond = Some(s_bin(">=", s_var(&vn, pos), s_int(lo, pos), pos));
        }
        if hi != i64::MAX {
            let c = s_bin("<=", s_var(&vn, pos), s_int(hi, pos), pos);
            cond = Some(match cond {
                Some(a) => s_bin("&&", a, c, pos),
                None => c,
            });
        }
        let ok_v = sx(
            EK::Variant {
                eid: reid,
                vname: "Ok".into(),
                vid: vid(self, "Ok"),
                args: vec![s_var(&vn, pos)],
            },
            pos,
        );
        let set = s_stmt(SK::Assign(s_var(&rn_, pos), ok_v), pos);
        match cond {
            Some(c) => self.chk(&mut out, s_stmt(SK::If(c, vec![set], None), pos)),
            None => self.chk(&mut out, set),
        }
        self.pre = outer_pre;
        let id = self.lookup(&rn_).unwrap();
        self.scopes.pop();
        e.k = EK::Sequence(
            out,
            Box::new(Expr {
                k: EK::Var(rn_, id),
                pos,
                ty: res,
            }),
        );
        res
    }
}

// ───────────────────── which parameters can change ─────────────────────

const PURE_PREFIXES: &[&str] = &[
    "to_",
    "truncate_",
    "wrapping_",
    "saturating_",
    "floor",
    "ceil",
    "round",
];
const PURE_METHODS: &[&str] = &[
    "signum",
    "cmp",
    "hash",
    "is_even",
    "is_odd",
    "push",
    "set",
    "get",
    "at",
    "len",
    "swap",
    "push_int",
    "push_char",
    "push_byte",
    "push_str",
    "eq",
    "char_at",
    "byte",
    "min",
    "max",
    "abs",
    "pow",
    "clamp",
    "wrapping_add",
    "wrapping_sub",
    "wrapping_mul",
];

impl<'a> Checker<'a> {
    /// Names that the body may assign, pass to a `mut` parameter or use inside a closure. A by-value
    /// parameter outside this set is declared immutable, so the flow facts about it cannot go stale (a missed
    /// mutation is then a compile error, never an unsound fact).
    pub(super) fn may_change(&self, body: &[Stmt]) -> HashSet<String> {
        let mut out = HashSet::new();
        for s in body {
            self.scan_stmt(s, &mut out, false);
        }
        out
    }

    fn root_name(e: &Expr) -> Option<&str> {
        match &e.k {
            EK::Var(n, _) | EK::Move(n, _) => Some(n),
            EK::Field(b, _) | EK::Deref(b) | EK::AddrOf(b, _) => Self::root_name(b),
            EK::Method { recv, m, .. } if m == "at" || m == "get" => Self::root_name(recv),
            _ => None,
        }
    }

    fn call_mut_arg(&self, name: &str, i: usize) -> bool {
        if let Some(sig) = self.sigs.get(name) {
            return sig.params.get(i).is_some_and(|p| p.0 == Mode::Mut);
        }
        if let Some(g) = self.ps.gfns.iter().find(|g| g.name == name) {
            return g.params.get(i).is_some_and(|p| p.0 == Mode::Mut);
        }
        if matches!(
            name,
            "print"
                | "assert"
                | "assert_eq"
                | "panic"
                | "todo"
                | "unreachable"
                | "min"
                | "max"
                | "abs"
        ) {
            return false;
        }
        if let Some(modes) = self.lazy_modes.get(name) {
            return modes.get(i).is_some_and(|m| *m == Mode::Mut);
        }
        self.unknown_calls.borrow_mut().insert(name.to_string());
        true // unknown callee (lazy library function, function value): assume the worst
    }

    /// Run a scan with the signatures of the lazily loaded library functions it needs.
    pub(super) fn scan_with_lazy(&mut self, body: &[Stmt]) -> (HashSet<String>, HashSet<String>) {
        for _ in 0..2 {
            self.unknown_calls.borrow_mut().clear();
            self.scan_assign.set(true);
            let _ = self.may_change(body);
            let names: Vec<String> = self.unknown_calls.borrow().iter().cloned().collect();
            let mut any = false;
            for n in names {
                if !self.lazy_modes.contains_key(&n) {
                    if let Some(sig) = self.lazy_header(&n) {
                        self.lazy_modes
                            .insert(n, sig.params.iter().map(|p| p.0).collect());
                        any = true;
                    }
                }
            }
            if !any {
                break;
            }
        }
        let changing = self.may_change(body);
        self.scan_assign.set(false);
        let unstable = self.may_change(body);
        self.scan_assign.set(true);
        (changing, unstable)
    }

    fn scan_stmts(&self, b: &[Stmt], out: &mut HashSet<String>, in_closure: bool) {
        for s in b {
            self.scan_stmt(s, out, in_closure);
        }
    }

    fn scan_stmt(&self, s: &Stmt, out: &mut HashSet<String>, cl: bool) {
        match &s.k {
            SK::Let(_, _, e, _) | SK::ExprS(e) | SK::Bind(_, _, e, _) => self.scan_expr(e, out, cl),
            SK::Assign(t, v) => {
                if self.scan_assign.get() {
                    self.scan_target(t, out);
                }
                self.scan_expr(t, out, cl);
                self.scan_expr(v, out, cl);
            }
            SK::If(c, a, b) => {
                self.scan_expr(c, out, cl);
                self.scan_stmts(a, out, cl);
                if let Some(b) = b {
                    self.scan_stmts(b, out, cl);
                }
            }
            SK::While(c, body, meta) => {
                self.scan_expr(c, out, cl);
                self.scan_stmts(body, out, cl);
                self.scan_stmts(&meta.step, out, cl);
            }
            SK::Block(b, _) | SK::Unsafe(b) => self.scan_stmts(b, out, cl),
            SK::Unpack(_, e) => self.scan_expr(e, out, cl),
            SK::For(h, body) => {
                match &h.iter {
                    ForIter::Range(a, b, _) => {
                        self.scan_expr(a, out, cl);
                        self.scan_expr(b, out, cl);
                    }
                    ForIter::Expr(e) => {
                        self.scan_expr(e, out, cl);
                        if h.mutable {
                            self.scan_target(e, out);
                        }
                    }
                }
                if let Some(st) = &h.step {
                    self.scan_expr(st, out, cl);
                }
                self.scan_stmts(body, out, cl);
            }
            SK::Return(e) => {
                if let Some(e) = e {
                    self.scan_expr(e, out, cl);
                }
            }
            SK::Match(e, arms) => {
                self.scan_expr(e, out, cl);
                for a in arms {
                    if let Some(g) = &a.guard {
                        self.scan_expr(g, out, cl);
                    }
                    self.scan_stmts(&a.body, out, cl);
                }
            }
            // a deferred statement runs at scope exit, when the variables may have other values
            SK::Defer(st) => self.scan_stmt(st, out, cl || !self.scan_assign.get()),
            SK::Break(..) | SK::Continue(..) => {}
        }
    }

    /// An assignment target (possibly a tuple of targets): its root variables may change.
    fn scan_target(&self, t: &Expr, out: &mut HashSet<String>) {
        match &t.k {
            EK::GenStruct { fields, .. } => {
                for (_, f) in fields {
                    self.scan_target(f, out);
                }
            }
            _ => {
                if let Some(n) = Self::root_name(t) {
                    out.insert(n.to_string());
                }
            }
        }
    }

    fn scan_expr(&self, e: &Expr, out: &mut HashSet<String>, cl: bool) {
        match &e.k {
            EK::Var(n, _) | EK::Move(n, _) => {
                if cl {
                    out.insert(n.clone());
                }
            }
            EK::AsElse(b, blk) => {
                self.scan_expr(b, out, cl);
                self.scan_stmts(blk, out, cl);
            }
            EK::IfExpr(c, t, e) => {
                self.scan_expr(c, out, cl);
                self.scan_stmts(t, out, cl);
                self.scan_stmts(e, out, cl);
            }
            EK::MatchExpr(x, arms) => {
                self.scan_expr(x, out, cl);
                for a in arms {
                    if let Some(g) = &a.guard {
                        self.scan_expr(g, out, cl);
                    }
                    self.scan_stmts(&a.body, out, cl);
                }
            }
            EK::Uninit => {}
            EK::Un("mut", b) if !self.scan_assign.get() => {
                if let Some(n) = Self::root_name(b) {
                    out.insert(n.to_string());
                }
                self.scan_expr(b, out, cl)
            }
            EK::Ascribe(b)
            | EK::Conv(b, _)
            | EK::Field(b, _)
            | EK::Un(_, b)
            | EK::IsVariant(b, _)
            | EK::Payload(b, ..)
            | EK::Deref(b) => self.scan_expr(b, out, cl),
            EK::AddrOf(b, _) => {
                if let Some(n) = Self::root_name(b) {
                    out.insert(n.to_string());
                }
            }
            EK::Bin(_, a, b) => {
                self.scan_expr(a, out, cl);
                self.scan_expr(b, out, cl);
            }
            EK::Call(name, args) => {
                for (i, a) in args.iter().enumerate() {
                    if matches!(a.k, EK::Var(..)) && self.call_mut_arg(name, i) {
                        if let Some(n) = Self::root_name(a) {
                            out.insert(n.to_string());
                        }
                    }
                    self.scan_expr(a, out, cl);
                }
            }
            EK::Method { recv, m, args } => {
                if !self.scan_assign.get()
                    && matches!(recv.k, EK::Var(..))
                    && !PURE_METHODS.contains(&m.as_str())
                    && !PURE_PREFIXES.iter().any(|p| m.starts_with(p))
                {
                    // a user method with `mut self` on an integer variable
                    if let Some(n) = Self::root_name(recv) {
                        out.insert(n.to_string());
                    }
                }
                self.scan_expr(recv, out, cl);
                for a in args {
                    if matches!(a.k, EK::Var(..)) && !PURE_METHODS.contains(&m.as_str()) {
                        if let Some(n) = Self::root_name(a) {
                            out.insert(n.to_string());
                        }
                    }
                    self.scan_expr(a, out, cl);
                }
            }
            EK::Static { args, .. }
            | EK::Variant { args, .. }
            | EK::GenVariant { args, .. }
            | EK::DynCall { args, .. } => {
                for a in args {
                    self.scan_expr(a, out, cl);
                }
            }
            EK::CallFn(f, args) => {
                self.scan_expr(f, out, cl);
                for a in args {
                    if let Some(n) = Self::root_name(a) {
                        out.insert(n.to_string());
                    }
                    self.scan_expr(a, out, cl);
                }
            }
            EK::StructLit { fields, .. } | EK::GenStruct { fields, .. } => {
                for (_, f) in fields {
                    self.scan_expr(f, out, cl);
                }
            }
            EK::DynBox { value, .. } => self.scan_expr(value, out, cl),
            EK::Closure { body, .. } => self.scan_stmts(body, out, true),
            EK::MkClosure { caps, .. } => {
                for c in caps {
                    self.scan_expr(c, out, cl);
                }
            }
            EK::Sequence(stmts, r) => {
                self.scan_stmts(stmts, out, cl);
                self.scan_expr(r, out, cl);
            }
            EK::ArrayLit(items) => items.iter().for_each(|a| self.scan_expr(a, out, cl)),
            EK::ArrayFill(b, _) => self.scan_expr(b, out, cl),
            EK::SliceOf(b, lo, hi) => {
                self.scan_expr(b, out, cl);
                lo.iter().chain(hi).for_each(|a| self.scan_expr(a, out, cl));
            }
            EK::Int(_) | EK::Float(..) | EK::Char(_) | EK::Bool(_) | EK::Text(_) => {}
        }
    }
}
