// Checker support for the numeric types (see numeric.rs): literals, operand typing, explicit conversions,
// math built-ins and the lazily compiled prelude functions (src/prelude.yx: `m__*`).
use super::*;
use crate::numeric::*;

/// One-argument math functions (free function `sqrt(x)` or method `x.sqrt()`).
const MATH1: &[&str] = &[
    "sqrt",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "exp",
    "ln",
    "floor",
    "ceil",
    "round",
    "trunc",
    "is_nan",
    "is_inf",
    "is_finite",
    "floor_int",
    "ceil_int",
    "round_int",
    "abs",
    "to_bits",
];
const MATH2: &[&str] = &["atan2", "pow", "min", "max", "copysign"];
const WRAPPING: &[&str] = &["wrapping_add", "wrapping_sub", "wrapping_mul"];
/// `x.checked_add(y) -> Option<Int>` and the clamping forms: ordinary Yerux in prelude_num.yx.
const OVERFLOW_AWARE: &[&str] = &[
    "checked_add",
    "checked_sub",
    "checked_mul",
    "saturating_add",
    "saturating_sub",
    "saturating_mul",
];

fn is_math(m: &str) -> bool {
    MATH1.contains(&m) || MATH2.contains(&m)
}

fn is_untyped_lit(e: &Expr) -> bool {
    match &e.k {
        EK::Int(_) | EK::Float(..) => !is_num(e.ty),
        EK::Un("-", x) => is_untyped_lit(x),
        _ => false,
    }
}

fn conv(e: Expr, to: Ty) -> Expr {
    let pos = e.pos;
    Expr {
        k: EK::Conv(Box::new(e), false),
        pos,
        ty: to,
    }
}

impl<'a> Checker<'a> {
    // ───────────────────────── prelude functions ─────────────────────────

    /// Signature of a built-in function written in Yerux (src/prelude.yx, `m__*`), compiling it on first use.
    pub(super) fn prelude_fn(&mut self, name: &str) -> Option<Sig> {
        if let Some(s) = self.sigs.get(name) {
            return Some(s.clone());
        }
        let tok = *self.ps.lazy_fns.get(name)?;
        match self.with_ps(|ps| ps.parse_fn_at(tok, vec![], None, false)) {
            Ok((h, body)) => {
                let f = Func {
                    name: name.to_string(),
                    params: h.params,
                    ret: h.ret,
                    body: body.unwrap_or_default(),
                    pos: h.pos,
                };
                let sig = Sig {
                    params: f.params.iter().map(|p| (p.mode, p.ty)).collect(),
                    ret: f.ret,
                };
                self.sigs.insert(name.to_string(), sig.clone());
                self.queue
                    .push((f, "the built-in numeric library".to_string(), self.inst_origin));
                Some(sig)
            }
            Err(d) => {
                self.err(d);
                None
            }
        }
    }

    // ───────────────────────── messages ─────────────────────────

    pub(super) fn num_mismatch_notes(&mut self, d: Diag, got: Ty, want: Ty, pos: Pos) -> Diag {
        let (g, w) = (self.tn(got), self.tn(want));
        let name = self.pos_names.get(&(pos.line, pos.col)).cloned();
        let lit = name.as_ref().is_some_and(|n| {
            n.chars().next().is_some_and(|c| c.is_ascii_digit())
        });
        let mut d = d.note(format!(
            "{} and {} are different types: numbers are never converted implicitly",
            g, w
        ));
        if lit && is_int(got) && is_float(want) {
            d = d.note(format!(
                "help: write a {} literal: `{}.0`, or convert a variable with `x.to_float()`",
                w,
                name.clone().unwrap()
            ));
        } else if lit && is_float(got) && is_int(want) {
            d = d.note(format!("help: `{}` is a Float literal; write an integer literal, or convert a value with `x.{}()`", name.clone().unwrap(), conv_method(want)));
        } else {
            let n = name.unwrap_or_else(|| "x".to_string());
            let n = if n.contains(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                || n == "value"
            {
                "x".to_string()
            } else {
                n
            };
            if matches!(want, Ty::Range(..)) && is_int(got) {
                d = d.note(format!("help: convert it explicitly: `{}.from(x)` panics when it does not fit, `{}.try_from(x)` returns a Result", w, w));
            } else {
                d = d.note(format!(
                    "help: convert it explicitly: write `{}.{}()`",
                    n,
                    conv_method(want)
                ));
            }
        }
        if is_float(got) && is_int(want) {
            d = d.note("`to_int()` and friends truncate toward zero and panic on NaN or an out-of-range value; `floor_int()`, `ceil_int()` and `round_int()` round the other ways");
        } else if is_int(got) && is_int(want) && !matches!(want, Ty::Range(..)) {
            let (a, b) = (int_range(got), int_range(want));
            if a.0 < b.0 || a.1 > b.1 {
                d = d.note(format!("the conversion panics when the value does not fit {}; `{}()` keeps the low bits instead", w, trunc_method(want)));
            }
        }
        d
    }

    pub(super) fn neg_ty(&mut self, t: Ty, pos: Pos) -> Ty {
        match t {
            Ty::Err => Ty::Err,
            t if is_float(t) => t,
            Ty::Range(lo, hi) if lo != i64::MIN => Ty::Range(-hi, -lo),
            Ty::Range(..) => Ty::Int,
            t if is_signed_int(t) => t,
            t if is_int(t) => {
                let n = self.tn(t);
                self.err(
                    diag(format!("cannot negate a {}: unsigned numbers have no minus sign", n), pos)
                        .note("help: convert to a signed type first, e.g. `x.to_int()` then negate; or compute `0 - x` only where x is known to be 0")
                        .note("an unsigned subtraction that would go below 0 panics with an overflow"),
                );
                t
            }
            t => {
                self.expect(t, Ty::Int, pos);
                Ty::Int
            }
        }
    }

    /// Result type of an integer operation on `Int` / `Int<lo..hi>` operands: interval arithmetic. When the
    /// mathematical result does not fit 64 bits the operation is the ordinary checked one (it panics outside
    /// `Int`), so the result type is the interval clamped to `Int`.
    fn range_arith(&mut self, op: &str, ta: Ty, tb: Ty, pb: Pos) -> Ty {
        let ((al, ah), (bl, bh)) = (int_range(ta), int_range(tb));
        if matches!(op, "<" | ">" | "<=" | ">=") {
            return ta;
        }
        if matches!(op, "+%" | "-%" | "*%") {
            return Ty::Int; // wrapping: no interval survives
        }
        let contract = std::mem::take(&mut self.div_contract);
        if contract && matches!(op, "/" | "%") && matches!(tb, Ty::Range(..)) && bl <= 0 && bh >= 0
        {
            // a range-typed divisor is a contract: it must prove nonzero (a plain Int divisor stays a runtime check)
            let n = self.tn(tb);
            self.err(
                diag("the divisor may be 0", pb)
                    .note(format!("{} includes zero", n))
                    .note("help: use a nonzero range such as `Int<1..10>`, or keep the divisor as plain `Int` for a runtime check"),
            );
            return Ty::Err;
        }
        if ta == Ty::Int && tb == Ty::Int {
            return Ty::Int;
        }
        let (lo, hi) = match op {
            "+" => (al + bl, ah + bh),
            "-" => (al - bh, ah - bl),
            "*" => {
                let xs = [al * bl, al * bh, ah * bl, ah * bh];
                (*xs.iter().min().unwrap(), *xs.iter().max().unwrap())
            }
            "/" => {
                // truncated quotient: monotone on each sign part of the divisor
                let mut xs: Vec<i128> = vec![];
                if bl <= -1 {
                    let d2 = bh.min(-1);
                    xs.extend([al / bl, ah / bl, al / d2, ah / d2]);
                }
                if bh >= 1 {
                    let d1 = bl.max(1);
                    xs.extend([al / d1, ah / d1, al / bh, ah / bh]);
                }
                if xs.is_empty() {
                    return Ty::Int;
                }
                (*xs.iter().min().unwrap(), *xs.iter().max().unwrap())
            }
            "%" => {
                let m = (bl.abs().max(bh.abs()) - 1).max(0);
                if al >= 0 {
                    (0, ah.min(m))
                } else if ah <= 0 {
                    (al.max(-m), 0)
                } else {
                    (al.max(-m), ah.min(m))
                }
            }
            _ => return Ty::Int,
        };
        super::rng::mk_range(lo, hi)
    }

    /// Type of `a op b` for `+ - * / % < > <= >=` (and the wrapping forms): both sides must be the same number type.
    pub(super) fn arith_ty(&mut self, op: &str, ta: Ty, tb: Ty, pa: Pos, pb: Pos) -> Ty {
        if ta == Ty::Err || tb == Ty::Err {
            return if is_num(ta) {
                ta
            } else if is_num(tb) {
                tb
            } else {
                Ty::Int
            };
        }
        if matches!(ta, Ty::Int | Ty::Range(..)) && matches!(tb, Ty::Int | Ty::Range(..)) {
            return self.range_arith(op, ta, tb, pb);
        }
        if !is_num(ta) {
            self.expect(ta, Ty::Int, pa);
            return Ty::Int;
        }
        if tb != ta {
            if is_float(tb) && is_int(ta) {
                // the integer side is the one to convert: `count * price` wants `count.to_float()`
                self.expect(ta, tb, pa);
                return tb;
            }
            self.expect(tb, ta, pb);
            return ta;
        }
        if op == "%" && is_float(ta) {
            let n = self.tn(ta);
            self.err(
                diag(format!("`%` is not defined for {}", n), pa).note(
                    "help: a remainder of floats is `x - floor(x / y) * y` (see also `trunc`)",
                ),
            );
        }
        ta
    }

    /// Check both operands of a binary operator. An untyped literal takes the type of the other operand
    /// (`x + 1` with `x: UInt8` makes `1` a UInt8); two literals take the type the context expects.
    pub(super) fn operands(&mut self, a: &mut Expr, b: &mut Expr, hint: Option<Ty>) -> (Ty, Ty) {
        let (la, lb) = (is_untyped_lit(a), is_untyped_lit(b));
        if la && lb {
            let h = hint.filter(|h| is_num(*h));
            self.hint = h;
            let ta = self.expr(a);
            self.hint = h;
            let tb = self.expr(b);
            return (ta, tb);
        }
        if la {
            let tb = self.expr(b);
            self.hint = Some(tb).filter(|t| is_num(*t));
            let ta = self.expr(a);
            return (ta, tb);
        }
        let ta = self.expr(a);
        self.hint = Some(ta).filter(|t| is_num(*t));
        let tb = self.expr(b);
        (ta, tb)
    }

    // ───────────────────────── literals ─────────────────────────

    fn int_lit(&mut self, v: i64, cur: Ty, hint: Option<Ty>, neg: bool, pos: Pos) -> (Ty, i64) {
        let big = cur == Ty::Err; // more than i64::MAX, no suffix
        let from_hint = !is_int(cur) && hint.is_some_and(is_int);
        if from_hint && matches!(hint, Some(Ty::Range(..))) && !big {
            // a literal is the singleton range of its value; whether it fits is the business of the context
            let v = if neg { v.wrapping_neg() } else { v };
            self.pos_names.insert(
                (pos.line, pos.col),
                if neg {
                    format!("-{}", v.wrapping_neg())
                } else {
                    v.to_string()
                },
            );
            return (Ty::Range(v, v), v);
        }
        let t = if is_int(cur) {
            cur
        } else if from_hint {
            hint.unwrap()
        } else {
            Ty::Int
        };
        let mut vv: i128 = if big || is_u64(t) {
            v as u64 as i128
        } else {
            v as i128
        };
        if neg {
            vv = -vv;
        }
        let (lo, hi) = int_range(t);
        if vv < lo || vv > hi {
            let n = self.tn(t);
            let shown = if neg {
                format!("-{}", -vv)
            } else {
                vv.to_string()
            };
            let mut d = diag(
                format!(
                    "the literal {} does not fit {} ({} to {})",
                    shown, n, lo, hi
                ),
                pos,
            );
            if from_hint {
                d = d.note(format!(
                    "a {} is expected here, so the literal is read as a {}",
                    n, n
                ));
            }
            if neg && !t_signed(t) {
                d = d.note(format!("{} has no negative values", n));
            } else if big {
                d = d.note("help: an integer above 9223372036854775807 needs a UInt64: write it with the suffix, e.g. `18446744073709551615u64`");
            } else {
                d = d.note(format!("help: pick a type that is big enough (e.g. Int or Int64), or convert a variable at run time with `x.{}()` (panics if it does not fit) or `x.{}()` (keeps the low bits)", conv_method(t), trunc_method(t)));
            }
            self.err(d);
        }
        self.pos_names.insert((pos.line, pos.col), v.to_string());
        let bits = if is_u64(t) {
            vv as u64 as i64
        } else {
            vv as i64
        };
        (t, bits)
    }

    fn float_lit(&mut self, txt: &str, cur: Ty, hint: Option<Ty>, pos: Pos) -> (Ty, f64) {
        let t = if is_float(cur) {
            cur
        } else if hint.is_some_and(is_float) {
            hint.unwrap()
        } else {
            Ty::Num(Nk::F64)
        };
        self.pos_names.insert((pos.line, pos.col), txt.to_string());
        if is_f32(t) {
            let f: f32 = txt.parse().unwrap_or(0.0);
            if f.is_infinite() {
                self.err(
                    diag(
                        format!(
                            "the literal {} is too large for Float32 (largest 3.4028235e38)",
                            txt
                        ),
                        pos,
                    )
                    .note("help: use Float, whose largest value is about 1.8e308"),
                );
            }
            (t, f as f64)
        } else {
            let f: f64 = txt.parse().unwrap_or(0.0);
            if f.is_infinite() {
                self.err(diag(
                    format!(
                        "the literal {} is too large for Float (largest 1.8e308)",
                        txt
                    ),
                    pos,
                ));
            }
            (t, f)
        }
    }

    // ───────────────────────── entry point ─────────────────────────

    /// Number-specific expression forms. Returns `Some(type)` when `e` was handled (and possibly rewritten).
    pub(super) fn try_numeric(&mut self, e: &mut Expr, hint: Option<Ty>) -> Option<Ty> {
        let pos = e.pos;
        match &e.k {
            EK::Int(v) => {
                let (t, bits) = self.int_lit(*v, e.ty, hint, false, pos);
                e.k = EK::Int(bits);
                Some(t)
            }
            EK::Float(_, txt) => {
                let txt = txt.clone();
                let (t, f) = self.float_lit(&txt, e.ty, hint, pos);
                e.k = EK::Float(f, txt);
                Some(t)
            }
            EK::Conv(..) => Some(e.ty),
            EK::Un("-", x) if matches!(x.k, EK::Int(_)) => {
                let EK::Int(v) = x.k else { unreachable!() };
                let (t, bits) = self.int_lit(v, x.ty, hint, true, pos);
                e.k = EK::Int(bits);
                Some(t)
            }
            EK::Call(name, _)
                if is_math(name) || name == "float_from_bits" || name == "float32_from_bits" =>
            {
                if self.sigs.contains_key(name) || self.ps.gfns.iter().any(|g| g.name == *name) {
                    return None; // the program's own function of that name wins
                }
                let k = std::mem::replace(&mut e.k, EK::Int(0));
                let EK::Call(name, args) = k else {
                    unreachable!()
                };
                if name == "float_from_bits" || name == "float32_from_bits" {
                    return Some(self.from_bits(e, &name, args, pos));
                }
                Some(self.math(e, &name, args, pos, hint, false))
            }
            EK::Method { recv, m, .. } if m != "at" => {
                let m = m.clone();
                let builtin = is_math(&m)
                    || WRAPPING.contains(&m.as_str())
                    || OVERFLOW_AWARE.contains(&m.as_str())
                    || conv_target(&m).is_some();
                if !builtin {
                    return None;
                }
                if self.isp(recv) {
                    match self.peek_ty(recv) {
                        Some(t) if is_num(t) => {
                            if self.has_user_method(t, &m) {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                }
                let k = std::mem::replace(&mut e.k, EK::Int(0));
                let EK::Method { mut recv, m, args } = k else {
                    unreachable!()
                };
                let rt = self.expr(&mut recv);
                if !is_num(rt) {
                    // not a number: let the ordinary method path report it
                    e.k = EK::Method { recv, m, args };
                    return None;
                }
                if !self.isp(&recv) && self.has_user_method(rt, &m) {
                    e.k = EK::Method { recv, m, args };
                    return None;
                }
                Some(self.num_method(e, *recv, &m, args, pos, hint))
            }
            _ => None,
        }
    }

    fn has_user_method(&self, t: Ty, m: &str) -> bool {
        self.ps
            .impls
            .iter()
            .any(|im| !im.prelude && im.self_ty == t && im.methods.iter().any(|x| x.name == m))
    }

    fn fail(&mut self, e: &mut Expr, d: Diag) -> Ty {
        self.err(d);
        e.k = EK::Int(0);
        Ty::Err
    }

    /// `x.to_int8()`, `x.truncate_uint8()`, `x.wrapping_add(y)`, `x.sqrt()`, ...
    fn num_method(
        &mut self,
        e: &mut Expr,
        recv: Expr,
        m: &str,
        mut args: Vec<Expr>,
        pos: Pos,
        hint: Option<Ty>,
    ) -> Ty {
        let rt = recv.ty;
        if let Some((target, trunc)) = conv_target(m) {
            if !args.is_empty() {
                return self.fail(e, diag(format!("'{}' takes no arguments", m), pos));
            }
            if trunc && !is_int(rt) {
                let n = self.tn(rt);
                let d = diag(format!("'{}' keeps the low bits of an integer: {} is not one", m, n), pos)
                    .note(format!("help: convert with `x.to_int()` first (truncates toward zero, checked), then `{}()`", m));
                return self.fail(e, d);
            }
            self.pos_names.insert((pos.line, pos.col), String::new());
            e.k = EK::Conv(Box::new(recv), trunc);
            return target;
        }
        if WRAPPING.contains(&m) {
            if !is_int(rt) {
                let n = self.tn(rt);
                let d = diag(format!("'{}' is for integers, not {}", m, n), pos)
                    .note("floats never wrap: they overflow to infinity");
                return self.fail(e, d);
            }
            if args.len() != 1 {
                return self.fail(e, diag(format!("'{}' expects 1 argument", m), pos));
            }
            // a flow-narrowed receiver is still an Int: wrapping keeps no interval
            let rt = if matches!(rt, Ty::Range(..)) {
                Ty::Int
            } else {
                rt
            };
            self.hint = Some(rt);
            let t = self.expr(&mut args[0]);
            self.expect(t, rt, args[0].pos);
            let op = match m {
                "wrapping_add" => "+%",
                "wrapping_sub" => "-%",
                _ => "*%",
            };
            let b = args.pop().unwrap();
            e.k = EK::Bin(op, Box::new(recv), Box::new(b));
            return rt;
        }
        if OVERFLOW_AWARE.contains(&m) {
            if !matches!(rt, Ty::Int | Ty::Range(..)) {
                let n = self.tn(rt);
                let d = diag(format!("'{}' is for Int, not {}", m, n), pos)
                    .note("help: convert with `x.to_int()` and test the result against the range you need");
                return self.fail(e, d);
            }
            if args.len() != 1 {
                return self.fail(e, diag(format!("'{}' expects 1 argument", m), pos));
            }
            self.hint = Some(Ty::Int);
            let t = self.expr(&mut args[0]);
            self.expect(t, Ty::Int, args[0].pos);
            let fname = format!("m__{}_i64", m);
            let Some(sig) = self.prelude_fn(&fname) else {
                return self.fail_quiet(e);
            };
            let b = args.pop().unwrap();
            e.k = EK::Call(fname, vec![recv, b]);
            return sig.ret;
        }
        let mut all = vec![recv];
        all.append(&mut args);
        self.math(e, m, all, pos, hint, true)
    }

    fn from_bits(&mut self, e: &mut Expr, name: &str, mut args: Vec<Expr>, pos: Pos) -> Ty {
        let (want, ret) = if name == "float_from_bits" {
            (Ty::Num(Nk::U64), Ty::Num(Nk::F64))
        } else {
            (Ty::Num(Nk::U32), Ty::Num(Nk::F32))
        };
        if args.len() != 1 {
            return self.fail(e, diag(format!("{} expects 1 argument", name), pos));
        }
        self.hint = Some(want);
        let t = self.expr(&mut args[0]);
        self.expect(t, want, args[0].pos);
        e.k = EK::Call("__from_bits".into(), args);
        ret
    }

    /// Math functions on Float, Float32 and integers. `args[0]` is the receiver / first argument.
    fn math(
        &mut self,
        e: &mut Expr,
        name: &str,
        mut args: Vec<Expr>,
        pos: Pos,
        _hint: Option<Ty>,
        evaluated: bool,
    ) -> Ty {
        let two = MATH2.contains(&name);
        let want = if two { 2 } else { 1 };
        if args.len() != want {
            let d = diag(
                format!(
                    "{} expects {} argument(s) but got {}",
                    name,
                    want,
                    args.len()
                ),
                pos,
            );
            return self.fail(e, d);
        }
        // argument types (a literal second argument takes the type of the first)
        let t0 = if evaluated {
            args[0].ty
        } else {
            self.hint = None;
            self.expr(&mut args[0])
        };
        if two {
            self.hint = Some(t0).filter(|t| is_num(*t));
            let t1 = self.expr(&mut args[1]);
            if matches!(name, "min" | "max")
                && matches!(t0, Ty::Int | Ty::Range(..))
                && matches!(t1, Ty::Int | Ty::Range(..))
            {
                return self.int_minmax(e, name, args, pos);
            }
            if t0 != Ty::Err && t1 != Ty::Err && t1 != t0 {
                self.expect(t1, t0, args[1].pos);
                return self.fail_quiet(e);
            }
        }
        if t0 == Ty::Err {
            return self.fail_quiet(e);
        }
        if !is_num(t0) {
            let n = self.tn(t0);
            let d = diag(format!("{} works on numbers, not {}", name, n), args[0].pos);
            return self.fail(e, d);
        }
        let float_only = !matches!(name, "abs" | "min" | "max");
        if float_only && !is_float(t0) {
            let n = self.tn(t0);
            let nm = self
                .pos_names
                .get(&(args[0].pos.line, args[0].pos.col))
                .cloned()
                .unwrap_or_else(|| "x".into());
            let d = diag(
                format!("{} works on Float or Float32, not {}", name, n),
                args[0].pos,
            )
            .note(format!(
                "numbers are never converted implicitly: write `{}.to_float()` first",
                if nm.is_empty() { "x".to_string() } else { nm }
            ));
            return self.fail(e, d);
        }
        if name == "abs" && matches!(t0, Ty::Range(..)) {
            // |x| over an interval; the operation itself is the ordinary Int abs (it panics only for i64::MIN)
            let (lo, hi) = int_range(t0);
            let (rl, rh) = if lo >= 0 {
                (lo, hi)
            } else if hi <= 0 {
                (-hi, -lo)
            } else {
                (0, hi.max(-lo))
            };
            if self.prelude_fn("m__abs_i64").is_some() {
                args[0].ty = Ty::Int;
                let a = args.pop().unwrap();
                e.k = EK::Call("m__abs_i64".into(), vec![a]);
                return super::rng::mk_range(rl, rh);
            }
        }
        if name == "abs" && !is_signed_int(t0) && !is_float(t0) {
            let n = self.tn(t0);
            let d = diag(
                format!("abs of a {}: it has no negative values", n),
                args[0].pos,
            );
            return self.fail(e, d);
        }
        let f32_arg = is_f32(t0);
        let f64t = Ty::Num(Nk::F64);
        // ---- intrinsics: sqrt, to_bits
        if name == "to_bits" {
            let ret = if f32_arg {
                Ty::Num(Nk::U32)
            } else {
                Ty::Num(Nk::U64)
            };
            if !is_float(t0) {
                let n = self.tn(t0);
                let d = diag(
                    format!("to_bits works on Float or Float32, not {}", n),
                    args[0].pos,
                );
                return self.fail(e, d);
            }
            e.k = EK::Call("__bits".into(), args);
            return ret;
        }
        if name == "sqrt" {
            let a = args.pop().unwrap();
            if f32_arg {
                e.k = EK::Conv(
                    Box::new(Expr {
                        k: EK::Call("__sqrt".into(), vec![conv(a, f64t)]),
                        pos,
                        ty: f64t,
                    }),
                    false,
                );
            } else {
                e.k = EK::Call("__sqrt".into(), vec![a]);
            }
            return t0;
        }
        // ---- prelude functions
        let (pname, ret, widen_ints): (String, Ty, bool) = if is_float(t0) {
            let ret = match name {
                "is_nan" | "is_inf" | "is_finite" => Ty::Bool,
                "floor_int" | "ceil_int" | "round_int" => Ty::Int,
                _ => t0,
            };
            (format!("m__{}", name), ret, true)
        } else if is_u64(t0) {
            (format!("m__{}_u64", name), t0, false)
        } else {
            // other integers: computed as Int (every Int8..UInt32 value fits), then converted back
            (format!("m__{}_i64", name), t0, true)
        };
        if self.prelude_fn(&pname).is_none() {
            let d = diag(
                format!("{} is not available for {}", name, self.tn(t0)),
                pos,
            );
            return self.fail(e, d);
        }
        let _ = widen_ints;
        let wide = if is_float(t0) { f64t } else { Ty::Int };
        let narrow_needed = t0 != wide && !is_u64(t0);
        let args: Vec<Expr> = args
            .into_iter()
            .map(|a| if narrow_needed { conv(a, wide) } else { a })
            .collect();
        let call = Expr {
            k: EK::Call(pname, args),
            pos,
            ty: if ret == t0 { wide } else { ret },
        };
        if narrow_needed && ret == t0 {
            e.k = EK::Conv(Box::new(call), false);
        } else {
            e.k = call.k;
        }
        ret
    }

    /// `min(a, b)` / `max(a, b)` of integers with intervals: componentwise bounds.
    fn int_minmax(&mut self, e: &mut Expr, name: &str, mut args: Vec<Expr>, _pos: Pos) -> Ty {
        let (a, b) = (
            super::rng::ival(&args[0]).unwrap(),
            super::rng::ival(&args[1]).unwrap(),
        );
        let (lo, hi) = if name == "min" {
            (a.0.min(b.0), a.1.min(b.1))
        } else {
            (a.0.max(b.0), a.1.max(b.1))
        };
        let pname = format!("m__{}_i64", name);
        if self.prelude_fn(&pname).is_none() {
            return self.fail_quiet(e);
        }
        for a in args.iter_mut() {
            a.ty = Ty::Int;
        }
        e.k = EK::Call(pname, args);
        super::rng::mk_range(lo, hi)
    }

    fn fail_quiet(&mut self, e: &mut Expr) -> Ty {
        e.k = EK::Int(0);
        Ty::Err
    }

    /// `print(x)` of a number: Int prints natively, narrower integers are widened, UInt64 and floats call the
    /// formatting functions of the prelude.
    pub(super) fn rewrite_print(&mut self, name: &mut String, args: &mut Vec<Expr>) {
        if args.len() != 1 {
            return;
        }
        let t = args[0].ty;
        let Ty::Num(k) = t else { return };
        match k {
            Nk::F64 | Nk::F32 | Nk::U64 => {
                let n = format!("m__print_{}", k.suffix());
                if self.prelude_fn(&n).is_some() {
                    *name = n;
                }
            }
            _ => {
                let a = args.pop().unwrap();
                args.push(conv(a, Ty::Int));
            }
        }
    }
}

fn t_signed(t: Ty) -> bool {
    is_signed_int(t)
}
