// IR lowering of the numeric types (see numeric.rs): literals, arithmetic, comparisons, conversions.
use super::*;
use crate::numeric::*;

const F64_2P63: i64 = 0x43E0_0000_0000_0000; // 2^63 as a double

pub fn rng_of(t: Ty) -> Option<Rng> {
    Some(match t {
        Ty::Num(Nk::I8) => Rng::I8,
        Ty::Num(Nk::I16) => Rng::I16,
        Ty::Num(Nk::I32) => Rng::I32,
        Ty::Num(Nk::U8) => Rng::U8,
        Ty::Num(Nk::U16) => Rng::U16,
        Ty::Num(Nk::U32) => Rng::U32,
        Ty::Range(lo, hi) => Rng::Iv(lo, hi),
        _ => return None,
    })
}

impl<'a> L<'a> {
    fn n2(&mut self, op: NOp, a: V, b: V) -> V {
        let d = self.v();
        self.emit(Inst::N(op, d, a, b));
        d
    }
    pub(super) fn n1(&mut self, op: NOp, a: V) -> V {
        self.n2(op, a, a)
    }

    pub(super) fn num_expr(&mut self, e: &Expr) -> LR<V> {
        match &e.k {
            EK::Float(v, _) => {
                let bits = if is_f32(e.ty) {
                    (*v as f32).to_bits() as i64
                } else {
                    v.to_bits() as i64
                };
                Ok(self.konst(bits))
            }
            EK::Conv(x, trunc) => self.num_conv(x, e.ty, *trunc),
            _ => unreachable!(),
        }
    }

    /// Unary minus on a number other than Int.
    pub(super) fn num_neg(&mut self, x: V, t: Ty) -> V {
        if is_f32(t) {
            let f = self.n1(NOp::S2F, x);
            let n = self.n1(NOp::FNeg, f);
            return self.n1(NOp::F2S, n);
        }
        if is_float(t) {
            return self.n1(NOp::FNeg, x);
        }
        // signed small integers: negate in 64 bits, then check the range (Int8: -(-128) = 128)
        let d = self.v();
        self.emit(Inst::Neg(d, x));
        match rng_of(t) {
            Some(r) => self.n2(NOp::Chk(r, MSG_NEG), d, d),
            None => d,
        }
    }

    /// Binary operation on numbers (anything but Int / Bool, plus the wrapping forms of Int).
    pub(super) fn num_bin(&mut self, op: &str, a: V, b: V, t: Ty) -> LR<V> {
        if is_float(t) {
            let (mut a, mut b) = (a, b);
            let f32t = is_f32(t);
            if f32t {
                a = self.n1(NOp::S2F, a);
                b = self.n1(NOp::S2F, b);
            }
            let r = match op {
                "+" => self.n2(NOp::FAdd, a, b),
                "-" => self.n2(NOp::FSub, a, b),
                "*" => self.n2(NOp::FMul, a, b),
                "/" => self.n2(NOp::FDiv, a, b),
                "<" => return Ok(self.n2(NOp::FLt, a, b)),
                "<=" => return Ok(self.n2(NOp::FLe, a, b)),
                ">" => return Ok(self.n2(NOp::FLt, b, a)),
                ">=" => return Ok(self.n2(NOp::FLe, b, a)),
                "==" => return Ok(self.n2(NOp::FEq, a, b)),
                "!=" => return Ok(self.n2(NOp::FNe, a, b)),
                _ => {
                    return Err(format!(
                        "native backend: float operator {} is not supported",
                        op
                    ))
                }
            };
            return Ok(if f32t { self.n1(NOp::F2S, r) } else { r });
        }
        // integers
        let wrapping = matches!(op, "+%" | "-%" | "*%");
        if wrapping {
            let o = match op {
                "+%" => NOp::WAdd,
                "-%" => NOp::WSub,
                _ => NOp::WMul,
            };
            let r = self.n2(o, a, b);
            return Ok(match rng_of(t) {
                Some(rg) => self.n1(NOp::Ext(rg), r),
                None => r,
            });
        }
        if is_u64(t) {
            let o = match op {
                "+" => NOp::UAdd,
                "-" => NOp::USub,
                "*" => NOp::UMul,
                "/" => NOp::UDiv,
                "%" => NOp::UMod,
                "<" => return Ok(self.n2(NOp::ULt, a, b)),
                "<=" => return Ok(self.n2(NOp::ULe, a, b)),
                ">" => return Ok(self.n2(NOp::ULt, b, a)),
                ">=" => return Ok(self.n2(NOp::ULe, b, a)),
                "==" => return Ok(self.bin(Op::Eq, a, b)),
                _ => return Ok(self.bin(Op::Ne, a, b)),
            };
            return Ok(self.n2(o, a, b));
        }
        // Int8..UInt32: exact in 64 bits (a product of two UInt32 may overflow i64: that panics as well),
        // then the range check of the narrow type
        let (o, msg) = match op {
            "+" => (Op::Add, MSG_ADD),
            "-" => (Op::Sub, MSG_SUB),
            "*" => (Op::Mul, MSG_MUL),
            "/" => (Op::Div, MSG_DIV),
            "%" => (Op::Mod, MSG_DIV),
            "<" => (Op::Lt, 0),
            "<=" => (Op::Le, 0),
            ">" => (Op::Gt, 0),
            ">=" => (Op::Ge, 0),
            "==" => (Op::Eq, 0),
            _ => (Op::Ne, 0),
        };
        let r = self.bin(o, a, b);
        if matches!(
            o,
            Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne | Op::Mod
        ) {
            return Ok(r);
        }
        Ok(match rng_of(t) {
            Some(rg) => self.n2(NOp::Chk(rg, msg), r, r),
            None => r,
        })
    }

    /// 2^64-range unsigned value (bit pattern) -> binary64 / binary32 with a single rounding.
    fn u64_to_float(&mut self, v: V, f32t: bool) -> V {
        let res = self.v();
        let zero = self.konst(0);
        let big = self.bin(Op::Lt, v, zero); // high bit set
        let _ = self.if_else(
            big,
            |l| {
                // (v >> 1) | (v & 1) keeps the rounding information; the result is doubled afterwards
                let two = l.konst(2);
                let h = l.n2(NOp::UDiv, v, two);
                let sticky = l.n2(NOp::UMod, v, two);
                let hm = l.n2(NOp::UMod, h, two);
                let t = l.n2(NOp::WMul, hm, sticky);
                let u = l.n2(NOp::WSub, sticky, t);
                let w = l.n2(NOp::WAdd, h, u);
                if f32t {
                    let s = l.n1(NOp::I2S, w);
                    let f = l.n1(NOp::S2F, s);
                    let dbl = l.n2(NOp::FAdd, f, f);
                    let r = l.n1(NOp::F2S, dbl);
                    l.emit(Inst::Mov(res, r));
                } else {
                    let f = l.n1(NOp::I2F, w);
                    let dbl = l.n2(NOp::FAdd, f, f);
                    l.emit(Inst::Mov(res, dbl));
                }
                Ok(())
            },
            |l| {
                let r = if f32t {
                    l.n1(NOp::I2S, v)
                } else {
                    l.n1(NOp::I2F, v)
                };
                l.emit(Inst::Mov(res, r));
                Ok(())
            },
        );
        res
    }

    /// binary64 -> UInt64 (bit pattern), truncating; panics when NaN or outside 0 ..= 2^64-1.
    fn f64_to_u64(&mut self, f: V) -> V {
        let res = self.v();
        let lim = self.konst(F64_2P63);
        let ge = self.n2(NOp::FLe, lim, f); // 2^63 <= f
        let _ = self.if_else(
            ge,
            |l| {
                let y = l.n2(NOp::FSub, f, lim);
                let i = l.n1(NOp::F2I, y); // 0 <= y < 2^63 or it panics
                let min = l.konst(i64::MIN);
                let r = l.n2(NOp::WAdd, i, min);
                l.emit(Inst::Mov(res, r));
                Ok(())
            },
            |l| {
                let i = l.n1(NOp::F2I, f);
                let r = l.n2(NOp::Chk(Rng::NonNeg, MSG_F2I), i, i);
                l.emit(Inst::Mov(res, r));
                Ok(())
            },
        );
        res
    }

    fn num_conv(&mut self, x: &Expr, to: Ty, trunc: bool) -> LR<V> {
        let from = x.ty;
        let v = self.expr(x)?;
        if from == to {
            return Ok(v);
        }
        match (is_float(from), is_float(to)) {
            (true, true) => Ok(if is_f32(from) {
                self.n1(NOp::S2F, v)
            } else {
                self.n1(NOp::F2S, v)
            }),
            (false, true) => {
                let f32t = is_f32(to);
                Ok(if is_u64(from) {
                    self.u64_to_float(v, f32t)
                } else if f32t {
                    self.n1(NOp::I2S, v)
                } else {
                    self.n1(NOp::I2F, v)
                })
            }
            (true, false) => {
                let f = if is_f32(from) {
                    self.n1(NOp::S2F, v)
                } else {
                    v
                };
                if is_u64(to) {
                    return Ok(self.f64_to_u64(f));
                }
                let i = self.n1(NOp::F2I, f);
                Ok(match rng_of(to) {
                    Some(r) => self.n2(NOp::Chk(r, MSG_F2I), i, i),
                    None => i,
                })
            }
            (false, false) => {
                if trunc {
                    if let Ty::Range(lo, hi) = to {
                        // `unchecked(x)`: no test, except under --check-ranges
                        if check_ranges() {
                            self.range_check(v, lo, hi);
                        }
                        return Ok(v);
                    }
                    return Ok(match rng_of(to) {
                        Some(r) => self.n1(NOp::Ext(r), v),
                        None => v, // Int / UInt64: the 64-bit pattern is the value
                    });
                }
                let (flo, fhi) = int_range(from);
                let (tlo, thi) = int_range(to);
                if flo >= tlo && fhi <= thi {
                    return Ok(v); // widening: always fits
                }
                // a UInt64 above i64::MAX has a negative bit pattern: reject it before the narrow range test
                let v = if is_u64(from) {
                    self.n2(NOp::Chk(Rng::NonNeg, MSG_CONV), v, v)
                } else {
                    v
                };
                Ok(match rng_of(to) {
                    Some(r) => self.n2(NOp::Chk(r, MSG_CONV), v, v),
                    // target Int / UInt64: only the sign (or the top bit of a UInt64) can be wrong
                    None => self.n2(NOp::Chk(Rng::NonNeg, MSG_CONV), v, v),
                })
            }
        }
    }
}
