"""Numeric program generator for tests/fuzz.py --num and tests/a64_fuzz.py --num.

Random straight-line / small-loop programs over Float, Float32 and every sized integer type:
arithmetic (checked and wrapping), comparisons, conversions (checked and truncating), math builtins.
Floats are bit-for-bit comparable between backends (no libm, IEEE operations only), so any difference in
stdout / stderr / exit code between C and native is a bug."""
import random

INTS = {  # name -> (lo, hi, suffix)
    "Int": (-(1 << 63), (1 << 63) - 1, ""),
    "Int8": (-128, 127, "i8"),
    "Int16": (-32768, 32767, "i16"),
    "Int32": (-(1 << 31), (1 << 31) - 1, "i32"),
    "UInt8": (0, 255, "u8"),
    "UInt16": (0, 65535, "u16"),
    "UInt32": (0, (1 << 32) - 1, "u32"),
    "UInt64": (0, (1 << 64) - 1, "u64"),
}
FLOATS = {"Float": "", "Float32": "f32"}
ALL = list(INTS) + list(FLOATS)
CMP = ["<", "<=", ">", ">=", "==", "!="]


def conv(t):
    return "to_int" if t == "Int" else "to_" + t.lower()


def trunc(t):
    return "truncate_int" if t == "Int" else "truncate_" + t.lower()


class NumGen:
    def __init__(self, seed, free_order=True):
        self.r = random.Random(seed)
        self.n = 0
        self.vars = {t: [] for t in ALL}
        self.lines = []
        self.danger = self.r.choice([0.0, 0.05, 0.2])
        self.extreme = self.r.choice([0.1, 0.3, 0.6])

    def emit(self, ind, s):
        self.lines.append("    " * ind + s)

    def lit(self, t):
        r = self.r
        if t in INTS:
            lo, hi, sfx = INTS[t]
            if r.random() < self.extreme:
                v = r.choice([lo, hi, 0, 1, hi - 1, lo + 1, hi // 2, 127, 128, 255, 256, 65535, 65536, 1 << 31, (1 << 32) - 1, 1 << 32, 100])
                v = max(lo, min(hi, v))
            else:
                v = r.randint(max(lo, -20), min(hi, 20))
            if t == "Int" and v == lo:
                return "(-9223372036854775807 - 1)"
            return f"({v}{sfx})" if v < 0 else f"{v}{sfx}"
        sfx = FLOATS[t]
        c = r.choice(["0.0", "1.0", "0.5", "2.0", "3.14159", "1e-3", "100.0", "0.1", "255.5", "1e10", "123456.789", "7.0",
                      "1e30", "2.5e-8", "16777217.0", "0.3333333333333333"])
        if r.random() < 0.3:
            c = f"{r.uniform(-1000, 1000):.4f}"
        return f"({c}{sfx})" if c.startswith("-") else f"{c}{sfx}"

    def leaf(self, t):
        r = self.r
        if self.vars[t] and r.random() < 0.65:
            return r.choice(self.vars[t])
        if t in FLOATS and r.random() < 0.12:
            s, z = FLOATS[t], "fz32" if t == "Float32" else "fz"
            return r.choice([f"(1.0{s} / {z})", f"({z} / {z})", f"(-1.0{s} / {z})"])
        return self.lit(t)

    def num(self, t, d):
        """Expression of numeric type t."""
        r = self.r
        if d <= 0 or r.random() < 0.2:
            return self.leaf(t)
        a = lambda ty=t: self.num(ty, d - 1)
        if t in INTS:
            lo, hi, _ = INTS[t]
            signed = lo < 0
            k = r.random()
            if k < 0.30:  # wrapping arithmetic never panics
                return f"{a()}.wrapping_{r.choice(['add', 'sub', 'mul'])}({a()})"
            if k < 0.30 + 0.25:
                op = r.choice("+-*")
                if r.random() > self.danger:  # keep checked ops mostly in range: combine with small literals
                    return f"({a()} {op} {self.lit_small(t)})" if op != "*" or r.random() < 0.5 else f"({a()} {op} {r.choice(['0', '1', '2'])}{INTS[t][2]})"
                return f"({a()} {op} {a()})"
            if k < 0.60:
                op = r.choice("/%")
                den = self.lit_small(t, nonzero=True) if r.random() > self.danger else a()
                return f"({a()} {op} {den})"
            if k < 0.65 and signed:
                return f"(-{a()})" if r.random() < self.danger + 0.3 else f"(-{self.leaf(t)})"
            src = r.choice([x for x in ALL if x != t])
            if k < 0.80:  # truncating conversion between ints, or checked one
                if src in INTS:
                    return f"{a(src)}.{trunc(t)}()" if r.random() < 0.7 else (f"{a(src)}.{conv(t)}()" if r.random() < self.danger + 0.2 else f"{self.leaf(src)}.{trunc(t)}()")
                return self.float_to_int(t, d)
            if k < 0.9:
                return f"({a()}.{trunc(t)}())" if False else f"{a()}.wrapping_add({self.lit(t)})"
            return self.leaf(t)
        # floats
        s = FLOATS[t]
        k = r.random()
        if k < 0.35:
            return f"({a()} {r.choice('+-*/')} {a()})"
        if k < 0.42:
            return f"(-{a()})"
        if k < 0.60:
            fn = r.choice(["sqrt", "abs", "floor", "ceil", "round", "trunc"])
            return f"{fn}({a()})" if r.random() < 0.5 else f"{a()}.{fn}()"
        if k < 0.68:
            fn = r.choice(["min", "max"])
            return f"{fn}({a()}, {a()})"
        if k < 0.80:
            fn = r.choice(["sin", "cos", "exp", "ln", "atan", "tan"])
            return f"{fn}({a()})"
        if k < 0.84:
            return f"pow({a()}, {a()})" if r.random() < 0.5 else f"atan2({a()}, {a()})"
        # conversions into the float type
        src = r.choice([x for x in ALL if x != t])
        return f"{a(src)}.{conv(t)}()"

    def float_to_int(self, t, d):
        """Float -> int conversions; can panic on NaN / range."""
        r = self.r
        ft = r.choice(list(FLOATS))
        base = self.num(ft, d - 1)
        m = r.choice(["", "floor_", "ceil_", "round_"]) if t == "Int" else ""
        meth = (f"{m}int" if m else "to_int") if t == "Int" else conv(t)
        if r.random() > self.danger + 0.15:
            # clamp into a range that always converts
            return f"min(max({base}, {self.lit(ft)}), {self.lit(ft)}).{meth}()" if False else f"{self.safe_float(ft)}.{meth}()"
        return f"{base}.{meth}()"

    def safe_float(self, ft):
        s = FLOATS[ft]
        return f"({self.r.choice(['0.0', '1.5', '99.75', '-3.5', '200.5', '1e3', '2.5e4'])}{s})".replace("(-", "(-")

    def lit_small(self, t, nonzero=False):
        lo, hi, sfx = INTS[t]
        v = self.r.randint(1 if nonzero else 0, min(hi, 7))
        return f"{v}{sfx}"

    def cond(self, d):
        r = self.r
        t = r.choice(ALL)
        c = f"({self.num(t, d)} {r.choice(CMP)} {self.num(t, d)})"
        if r.random() < 0.3:
            t2 = r.choice(ALL)
            c = f"({c} {r.choice(['&&', '||'])} ({self.num(t2, d)} {r.choice(CMP)} {self.num(t2, d)}))"
        return c

    def program(self):
        r = self.r
        self.emit(0, "fn main() {")
        self.emit(1, "let fz = 0.0")
        self.emit(1, "let fz32 = 0.0f32")
        for t in ALL:
            for i in range(2):
                v = f"{t.lower()}{i}"
                self.emit(1, f"var {v} = {self.lit(t)}")
                self.vars[t].append(v)
        for _ in range(r.randint(6, 16)):
            self.stmt(1, 3)
        for t in ALL:
            for v in self.vars[t]:
                self.emit(1, f"print({v})")
        self.emit(0, "}")
        return self.lines

    def stmt(self, ind, d):
        r = self.r
        t = r.choice(ALL)
        v = r.choice(self.vars[t])
        k = r.random()
        if k < 0.6:
            self.emit(ind, f"{v} = {self.num(t, d)}")
            self.emit(ind, f"print({v})")
        elif k < 0.75:
            self.emit(ind, f"print({self.cond(d - 1)})")
        elif k < 0.88:
            self.emit(ind, f"if {self.cond(d - 1)} {{")
            self.emit(ind + 1, f"{v} = {self.num(t, d - 1)}")
            self.emit(ind, "} else {")
            self.emit(ind + 1, f"print({self.num(t, d - 1)})")
            self.emit(ind, "}")
        else:
            c = f"k{self.n}"
            self.n += 1
            self.emit(ind, f"var {c} = 0")
            self.emit(ind, f"while {c} < {r.randint(2, 6)} {{")
            self.emit(ind + 1, f"{v} = {self.num(t, d - 1)}")
            self.emit(ind + 1, f"{c} = {c} + 1")
            self.emit(ind, "}")
            self.emit(ind, f"print({v})")
