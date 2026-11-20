#!/usr/bin/env python3
"""Differential fuzzer for the NEWER language features: closures (captures by move / view / mut,
returned and stored closures), owned enums with payloads, Box and recursive types, Option / Result
with `?` and `defer`, `dyn Trait` objects with owned fields, match on temporaries, and
deterministic std.io (args / stdin free).

Every program is run through the C backend, native, native --no-opt (and optionally aarch64 under
Unicorn). stdout, stderr and the exit code must agree. Non-panicking programs also run with
`--native-debug-alloc`, which must end with `live: 0` (leak counters).

usage: tests/fuzz_new.py [--closures] [--enums] [--box] [--dyn] [--result] [--io-free] [--all]
                          [-n COUNT] [--seed SEED] [-j JOBS] [--a64] [--max-saves N]
Without a feature flag, every feature is mixed (--all).
"""
import argparse, hashlib, os, random, shutil, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import contextlib
import fuzz as F

PRELUDE_ENUM = """enum Msg { Text(String), Nums(List<Int>), Pair(String, Int), Wrap(Box<Msg>), Quit }

fn msize(m: view Msg) -> Int {
  match m {
    Text(s) => { return s.len() }
    Nums(l) => { return l.len() }
    Pair(s, n) => { return s.len() + n }
    Wrap(b) => { return msize(*b) + 1 }
    Quit => { return 0 }
  }
}

fn mgrow(m: mut Msg, k: Int) {
  match mut m {
    Text(s) => { s.push_str("!") }
    Nums(l) => { l.push(k) }
    Pair(s, n) => { s.push_int(k) }
    Wrap(b) => { mgrow(*b, k) }
    Quit => { }
  }
}

fn mtext(m: Msg) -> String {
  match move m {
    Text(s) => { return s }
    Nums(l) => { return String.from("n") }
    Pair(s, n) => {
      var o = move s
      o.push_int(n)
      return o
    }
    Wrap(b) => { return mtext(b.take()) }
    Quit => { return String.from("q") }
  }
}

fn pass<T>(x: T) -> T { return x }
fn some_of<T>(x: T) -> Option<T> { return Some(move x) }
fn unwrap_or2<T>(o: Option<T>, d: T) -> T {
  match move o {
    Some(v) => { return v }
    None => { return d }
  }
}
struct Pair2<A, B> { a: A, b: B }
fn mkpair<A, B>(a: A, b: B) -> Pair2<A, B> { return Pair2 { a: move a, b: move b } }
fn lmsg(key: view String, k: Int) -> Msg { return mkmsg(k, String.from(key)) }
fn lopt(key: view String, k: Int) -> Option<Msg> {
  if k % 3 == 0 { return None }
  return Some(mkmsg(k, String.from(key)))
}
fn lres(key: view String, k: Int) -> Result<Msg, String> {
  if k % 4 == 0 { return Err(String.from(key)) }
  return Ok(mkmsg(k, String.from(key)))
}
fn sink_s(s: String) -> Int { return s.len() }
fn sink_m(m: Msg) -> Int { return msize(m) + 1 }
fn sink_l(l: List<Msg>) -> Int { return l.len() }
fn mkmsg(k: Int, s: String) -> Msg {
  if k % 5 == 0 { return Msg.Text(move s) }
  if k % 5 == 1 {
    var l = List<Int>.new()
    for i in 0..(k % 4) { l.push(i + k) }
    return Msg.Nums(move l)
  }
  if k % 5 == 2 { return Msg.Pair(move s, k) }
  if k % 5 == 3 { return Msg.Wrap(Box.new(Msg.Text(move s))) }
  return Msg.Quit
}
"""

PRELUDE_TREE = """enum Tree { Leaf, Node(Box<Tree>, Int, Box<Tree>) }

fn tinsert(t: Tree, v: Int) -> Tree {
  match move t {
    Leaf => { return Tree.Node(Box.new(Tree.Leaf), v, Box.new(Tree.Leaf)) }
    Node(l, x, r) => {
      if v < x { return Tree.Node(Box.new(tinsert(l.take(), v)), x, move r) }
      return Tree.Node(move l, x, Box.new(tinsert(r.take(), v)))
    }
  }
}

fn tsum(t: view Tree) -> Int {
  match t {
    Leaf => { return 0 }
    Node(l, x, r) => { return tsum(*l) + x + tsum(*r) }
  }
}

fn tdepth(t: view Tree) -> Int {
  match t {
    Leaf => { return 0 }
    Node(l, _, r) => {
      let a = tdepth(*l)
      let b = tdepth(*r)
      if a > b { return a + 1 }
      return b + 1
    }
  }
}

fn tmin(t: view Tree) -> Option<Int> {
  match t {
    Leaf => { return None }
    Node(l, x, _) => {
      let m = tmin(*l)
      match m {
        Some(v) => { return Some(v) }
        None => { return Some(x) }
      }
    }
  }
}

fn tbump(t: mut Tree, k: Int) {
  match mut t {
    Leaf => { }
    Node(l, x, r) => {
      x = x + k
      tbump(*l, k)
      tbump(*r, k)
    }
  }
}

fn sink_t(t: Tree) -> Int { return tdepth(t) }
fn tflip(t: Tree) -> Tree {
  match move t {
    Leaf => { return Tree.Leaf }
    Node(l, x, r) => { return Tree.Node(Box.new(tflip(r.take())), x, Box.new(tflip(l.take()))) }
  }
}

struct Chain { val: Int, next: Option<Box<Chain>> }

fn chain_build(n: Int) -> Option<Box<Chain>> {
  var head = Option<Box<Chain>>.None
  for i in 0..n {
    head = Some(Box.new(Chain { val: i * 3 + 1, next: move head }))
  }
  return head
}

fn chain_sum(c: view Option<Box<Chain>>) -> Int {
  match c {
    Some(b) => { return b.get().val + chain_sum(b.get().next) }
    None => { return 0 }
  }
}
"""

PRELUDE_CLOS = """fn apply(f: view fn(Int) -> Int, x: Int) -> Int { return f(x) }
fn apply_n(f: view fn(Int) -> Int, x: Int, n: Int) -> Int {
  var v = x
  for _i in 0..n { v = f(v) }
  return v
}
fn call_owned(f: fn(Int) -> Int, x: Int) -> Int { return f(x) }
fn make_adder(n: Int) -> fn(Int) -> Int { return move |x| x + n }
fn make_scaler(n: Int) -> fn(Int) -> Int { return move |x| x * n - 1 }
fn make_counter(start: Int) -> fn(Int) -> Int {
  var n = start
  return move |x| {
    n = n + x
    return n
  }
}
fn make_len(s: String) -> fn(Int) -> Int { return move |x| x + s.len() }
fn make_dropper(s: String, l: List<Int>) -> fn(Int) -> Int {
  var calls = 0
  return move |x| {
    calls = calls + 1
    return x + s.len() + l.len() * calls
  }
}
fn compose(f: fn(Int) -> Int, g: fn(Int) -> Int) -> fn(Int) -> Int { return move |x| g(f(x)) }
fn pick(flag: Bool, a: fn(Int) -> Int, b: fn(Int) -> Int) -> fn(Int) -> Int {
  if flag { return move a }
  return move b
}
fn early(s: String, k: Int) -> fn(Int) -> Int {
  var tag = String.from("tag")
  if k % 2 == 0 { return move |x| x + s.len() }
  tag.push_str("zz")
  return move |x| x - tag.len()
}
fn curry3(a: Int) -> fn(Int) -> fn(Int) -> Int {
  return move |b: Int| { return move |c: Int| a * 100 + b * 10 + c }
}
fn make_msgf(m: Msg) -> fn(Int) -> Int { return move |x: Int| x + msize(m) }
fn make_msgmut(m: Msg) -> fn(Int) -> Int {
  var mm = move m
  return move |x: Int| {
    mgrow(mm, x)
    return msize(mm)
  }
}
fn make_listf(l: List<Msg>, tag: String) -> fn(Int) -> Int {
  return move |x: Int| {
    var t = x + tag.len()
    for i in 0..l.len() { t = t + msize(l.at(i)) }
    return t
  }
}
fn make_opt(n: Int) -> fn(Int) -> Int {
  let h = move |x: Int| -> Option<Int> {
    if x % 3 == 0 { return None }
    return Some(x + n)
  }
  return move |x: Int| {
    let a = h(x)
    match a { Some(v) => { return v } None => { return -n } }
  }
}
fn make_try(n: Int) -> fn(Int) -> Int {
  let h = move |x: Int| -> Result<Int, String> {
    if x % 4 == 0 { return Err(String.from("four")) }
    return Ok(x * n)
  }
  return move |x: Int| {
    match try_pair(h, x) { Ok(v) => { return v } Err(e) => { return e.len() } }
  }
}
fn try_pair(h: view fn(Int) -> Result<Int, String>, y: Int) -> Result<Int, String> {
  let v = h(y)?
  let w = h(v + 1)?
  return Ok(v + w)
}
fn make_div(n: Int) -> fn(Int) -> Int { return move |x: Int| 1000 / (x - n) }
struct Cb { f: fn(Int) -> Int, g: fn(Int) -> Int, name: String }
fn run_cb(c: view Cb, x: Int) -> Int { return c.f(c.g(x)) + c.name.len() }
fn each(l: view List<Int>, f: view fn(Int)) { for x in l { f(x) } }
fn repeat(n: Int, f: view fn()) { for _i in 0..n { f() } }
struct Task { name: String, run: fn(Int) -> Int }
fn run_task(t: view Task, x: Int) -> Int { return t.run(x) + t.name.len() }
"""

PRELUDE_RESULT = """fn step(x: Int) -> Result<Int, String> {
  if x % 7 == 0 { return Err(String.from("seven")) }
  return Ok(x + 1)
}
fn find(l: view List<Int>, k: Int) -> Option<Int> {
  for i in 0..l.len() {
    if l.get(i) == k { return Some(i) }
  }
  return None
}
fn chain2(x: Int, log: mut List<String>) -> Result<Int, String> {
  var tag = String.from("c")
  defer log.push(String.from("c2 done"))
  tag.push_int(x)
  let a = step(x)?
  log.push(String.from(tag))
  defer log.push(String.from("mid"))
  let b = step(a * 2)?
  return Ok(a + b)
}
fn opt_chain(l: view List<Int>, a: Int, b: Int) -> Option<Int> {
  let i = find(l, a)?
  let j = find(l, b)?
  return Some(i * 10 + j)
}
fn owned_opt(s: String, k: Int) -> Option<String> {
  var t = String.from("o")
  var u = String.from("u")
  defer u.push_str("!")
  if k < 0 { return None }
  let n = find_len(s, k)?
  t.push_int(n)
  return Some(move t)
}
fn find_len(s: view String, k: Int) -> Option<Int> {
  if s.len() > k { return Some(s.len()) }
  return None
}
fn res_text(m: Msg, k: Int) -> Result<String, String> {
  var out = String.new()
  match move m {
    Text(s) => { out.push_str(s) }
    Nums(l) => { out.push_int(l.len()) }
    Pair(s, n) => {
      if n == k { return Err(move s) }
      out.push_str(s)
    }
    Wrap(b) => { out = mtext(b.take()) }
    Quit => { return Err(String.from("quit")) }
  }
  return Ok(move out)
}
fn bubble(m: Msg, k: Int) -> Result<Int, String> {
  defer print("bubble")
  let s = res_text(move m, k)?
  return Ok(s.len())
}
"""

PRELUDE_DYN = """trait Shape {
  fn area(view self) -> Int
  fn grow(mut self, k: Int)
  fn name(view self) -> String
}
struct Rect { w: Int, h: Int, label: String }
struct Poly { pts: List<Int>, tags: List<String> }
struct Nested { inner: Box<dyn Shape>, extra: String }
struct Dot { x: Int }
impl Shape for Rect {
  fn area(view self) -> Int { return self.w * self.h + self.label.len() }
  fn grow(mut self, k: Int) {
    self.w = self.w + k
    self.label.push_str("r")
  }
  fn name(view self) -> String { return String.from(self.label) }
}
impl Shape for Poly {
  fn area(view self) -> Int {
    var t = 0
    for p in self.pts { t = t + p }
    return t + self.tags.len()
  }
  fn grow(mut self, k: Int) {
    self.pts.push(k)
    self.tags.push(String.from("t"))
  }
  fn name(view self) -> String {
    var s = String.from("poly")
    s.push_int(self.pts.len())
    return s
  }
}
impl Shape for Nested {
  fn area(view self) -> Int { return self.inner.area() * 2 + self.extra.len() }
  fn grow(mut self, k: Int) {
    self.inner.grow(k)
    self.extra.push_str("n")
  }
  fn name(view self) -> String {
    var s = String.from("nest-")
    let n = self.inner.name()
    s.push_str(n)
    return s
  }
}
impl Shape for Dot {
  fn area(view self) -> Int { return self.x }
  fn grow(mut self, k: Int) { self.x = self.x + k }
  fn name(view self) -> String { return String.from("dot") }
}
fn mkshape(k: Int, s: String) -> Box<dyn Shape> {
  if k % 4 == 0 { return Box<dyn Shape>.new(Rect { w: k, h: 2, label: move s }) }
  if k % 4 == 1 {
    var p = Poly { pts: List<Int>.new(), tags: List<String>.new() }
    p.pts.push(k)
    p.tags.push(move s)
    return Box<dyn Shape>.new(move p)
  }
  if k % 4 == 2 {
    let inner = mkshape(k + 1, String.from(s))
    return Box<dyn Shape>.new(Nested { inner: move inner, extra: move s })
  }
  return Box<dyn Shape>.new(Dot { x: k })
}
fn total_area(l: view List<Box<dyn Shape>>) -> Int {
  var t = 0
  for i in 0..l.len() { t = t + l.at(i).area() }
  return t
}
fn area_of(s: view dyn Shape) -> Int { return s.area() }
fn grow_it(s: mut dyn Shape, k: Int) { s.grow(k) }
fn gen_area<T: Shape>(x: view T) -> Int { return x.area() + 1 }
"""

PRELUDE_IO = """import std.io
fn io_probe(n: Int, path: view String, text: view String) -> Int {
  var score = io.args().len()
  match io.read_line() {
    Ok(l) => { match l { Some(s) => { score = score + 100 } None => { score = score + 1 } } }
    Err(e) => { score = score - 1000 }
  }
  match io.read_all() {
    Ok(s) => { score = score + s.len() }
    Err(e) => { score = score - 1000 }
  }
  match io.env_var("YX_FUZZ_SURELY_UNSET") {
    Some(v) => { score = score + 5000 }
    None => { score = score + 7 }
  }
  if io.file_exists(path) { score = score + 20000 }
  match io.read_file(path) {
    Ok(s) => { score = score + 30000 }
    Err(e) => { score = score + e.code }
  }
  match io.write_file(path, text) {
    Ok(k) => { score = score + k }
    Err(e) => { score = score - 77 }
  }
  match io.append_file(path, text) {
    Ok(k) => { score = score + k * 3 }
    Err(e) => { score = score - 88 }
  }
  match io.read_file(path) {
    Ok(s) => { print(s); score = score + s.len() * 11 }
    Err(e) => { score = score - 99 }
  }
  match io.read_file("/tmp") {
    Ok(s) => { score = score + 1 }
    Err(e) => { score = score + e.code }
  }
  match io.remove_file(path) {
    Ok(k) => { score = score + k + 1 }
    Err(e) => { score = score + e.code }
  }
  if io.file_exists(path) { score = score + 40000 }
  return score + n
}
"""


class NewGen:
    def __init__(self, seed, feats):
        self.seed = seed
        self.r = random.Random(seed)
        self.feats = feats
        self.n = 0
        self.lines = []
        self.loop_id = 0
        self.loop_next = 0
        self.fn_kind = "main"
        self.nfun = 0
        self.excl = set()
        self.loop_stack = []
        self.danger = self.r.choice([0, 0, 0.3, 1, 2])
        self.fns = []   # generated helper fns: (name, kind)

    # ---- helpers
    def fresh(self, p="v"):
        self.n += 1
        return f"{p}{self.n}"

    def emit(self, ind, s):
        self.lines.append("  " * ind + s)

    def has(self, f):
        return f in self.feats

    # env: dict type -> list of (name, loop_id)
    @staticmethod
    def copy_env(env):
        return {k: list(v) for k, v in env.items()}

    def names(self, env, t):
        return [n for n, _ in env.get(t, []) if n not in self.excl]

    def movable(self, env, t):
        return [n for n, lid in env.get(t, []) if lid == self.loop_id and n not in self.excl]

    def add(self, env, t, name):
        env.setdefault(t, []).append((name, self.loop_id))

    def drop_var(self, env, t, name):
        env[t] = [(n, l) for n, l in env[t] if n != name]

    def pick(self, lst):
        return self.r.choice(lst)

    # ---- expressions
    def lit(self):
        return str(self.r.choice([0, 1, 2, 3, 5, 7, 14, 21, 100, -1, -4]))

    def text(self):
        return self.r.choice(["a", "bc", "hello", "", "x y", "é", "k" * 9])

    def int_e(self, env, d=0):
        r = self.r
        opts = [lambda: self.lit()]
        ints = self.names(env, "Int") + self.names(env, "IntRO")
        if ints:
            opts += [lambda: self.pick(ints)] * 3
        strs = self.names(env, "String")
        if strs:
            opts.append(lambda: self.pick(strs) + ".len()")
        lst = self.names(env, "List")
        if lst:
            opts.append(lambda: self.pick(lst) + ".len()")
        if self.has("enums"):
            ms = self.names(env, "Msg")
            if ms:
                opts.append(lambda: f"msize({self.pick(ms)})")
        if self.has("box"):
            ts = self.names(env, "Tree")
            if ts:
                opts.append(lambda: f"tsum({self.pick(ts)})")
                opts.append(lambda: f"tdepth({self.pick(ts)})")
        if self.has("closures"):
            fs = self.names(env, "Fn")
            if fs:
                opts.append(lambda: f"{self.pick(fs)}({self.small(env, d + 1)})")
                opts.append(lambda: f"apply({self.pick(fs)}, {self.small(env, d + 1)})")
        if self.has("dyn"):
            ss = self.names(env, "Shapes")
            if ss:
                opts.append(lambda: f"total_area({self.pick(ss)})")
        if d < 2 and r.random() < 0.5:
            op = r.choice(["+", "-", "+", "*"])
            a = self.int_e(env, d + 1)
            b = self.int_e(env, d + 1)
            if op == "*":
                return f"(({a}) % 50) * (({b}) % 50)"
            return f"({a} {op} {b})"
        return r.choice(opts)()

    def small_ints(self, env):
        ints = self.names(env, "Int") + self.names(env, "IntRO")
        a = self.pick(ints) if ints else "2"
        return f"(({a} * {self.lit()}) % 100)"

    def small_ex(self, env, *ex):
        old = set(self.excl)
        self.excl |= set(ex)
        try:
            return self.small(env)
        finally:
            self.excl = old

    def cond_ex(self, env, *ex):
        old = set(self.excl)
        self.excl |= set(ex)
        try:
            return self.cond(env)
        finally:
            self.excl = old

    def small(self, env, d=0):
        return f"(({self.int_e(env, d)}) % 100)"

    def cond(self, env):
        r = self.r
        a = self.int_e(env, 1)
        return r.choice([f"{a} % 2 == 0", f"{a} > {self.lit()}", f"{a} % 3 != 1", f"{a} < 5"])

    # ---- statements
    def block(self, ind, env, n, allow_ret):
        env = self.copy_env(env)
        moved_outer = []
        for _ in range(n):
            self.stmt(ind, env, allow_ret)
        return env

    def kill_moved(self, outer, inner):
        """After a conditionally executed block: anything moved inside is unavailable afterwards."""
        for t, lst in outer.items():
            alive = {n for n, _ in inner.get(t, [])}
            outer[t] = [(n, l) for n, l in lst if n in alive]

    def stmt(self, ind, env, allow_ret, top=False):
        r = self.r
        choices = []
        W = choices.append
        # scalar / string / list basics
        W((3, self.s_int))
        W((3, self.s_print))
        W((2, self.s_string))
        W((2, self.s_list))
        W((2, self.s_if))
        if ind < 4 and self.loop_next < 6:
            W((2, self.s_loop))
        if allow_ret:
            W((1, self.s_ret))
        if self.has("enums"):
            W((4, self.s_enum))
            W((4, self.s_msgs))
            W((3, self.s_optmsg))
            W((2, self.s_tuple))
            W((4, self.s_mtemp))
            W((3, self.s_generic))
            if self.loop_id != 0:
                W((3, self.s_moveout))
        if self.has("box"):
            W((4, self.s_tree))
        if self.has("closures"):
            W((5, self.s_clos))
        if self.has("dyn"):
            W((5, self.s_dyn))
        if self.has("result"):
            W((5, self.s_result))
        if self.has("io"):
            W((6, self.s_io))
        if self.has("defer"):
            W((2, self.s_defer))
        W((self.danger, self.s_danger))
        tot = sum(w for w, _ in choices)
        x = r.random() * tot
        for w, f in choices:
            x -= w
            if x <= 0:
                f(ind, env, allow_ret)
                return
        choices[-1][1](ind, env, allow_ret)

    def s_int(self, ind, env, ret):
        v = self.fresh("i")
        self.emit(ind, f"var {v} = {self.small(env)}")
        self.add(env, "Int", v)
        ints = [n for n in self.names(env, "Int") if n != v]
        if ints and self.r.random() < 0.6:
            self.emit(ind, f"{self.pick(ints)} = {self.small(env)}")

    def s_print(self, ind, env, ret):
        r = self.r
        k = r.random()
        strs = self.names(env, "String")
        if k < 0.6 or not strs:
            self.emit(ind, f"print({self.int_e(env)})")
        else:
            self.emit(ind, f"print({self.pick(strs)})")

    def s_string(self, ind, env, ret):
        r = self.r
        strs = self.names(env, "String")
        k = r.random()
        if not strs or k < 0.4:
            v = self.fresh("s")
            self.emit(ind, f"var {v} = String.from(\"{self.text()}\")")
            self.add(env, "String", v)
        elif k < 0.7:
            self.emit(ind, f"{self.pick(strs)}.push_str(\"{self.text()}\")")
        elif k < 0.85:
            self.emit(ind, f"{self.pick(strs)}.push_int({self.small(env)})")
        else:
            m = self.movable(env, "String")
            if m:
                s = self.pick(m)
                v = self.fresh("s")
                self.emit(ind, f"var {v} = move {s}")
                self.drop_var(env, "String", s)
                self.add(env, "String", v)

    def s_list(self, ind, env, ret):
        r = self.r
        ls = self.names(env, "List")
        k = r.random()
        if not ls or k < 0.3:
            v = self.fresh("l")
            self.emit(ind, f"var {v} = List<Int>.new()")
            self.add(env, "List", v)
        elif k < 0.8:
            self.emit(ind, f"{self.pick(ls)}.push({self.small(env)})")
        else:
            l = self.pick(ls)
            self.emit(ind, f"if {l}.len() > 0 {{ print({l}.pop()) }}")

    def s_if(self, ind, env, ret):
        self.emit(ind, f"if {self.cond(env)} {{")
        e1 = self.block(ind + 1, env, self.r.randint(1, 3), ret)
        self.kill_moved(env, e1)
        if self.r.random() < 0.5:
            self.emit(ind, "} else {")
            e2 = self.block(ind + 1, env, self.r.randint(1, 3), ret)
            self.kill_moved(env, e2)
        self.emit(ind, "}")

    def s_loop(self, ind, env, ret):
        r = self.r
        self.loop_next += 1
        old = self.loop_id
        self.loop_stack.append(self.loop_next)
        self.loop_id = self.loop_next
        i = self.fresh("k")
        lo = r.choice([0, 0, 1, -2])
        hi = r.choice([1, 2, 3, 4])
        kind = r.random()
        if kind < 0.7:
            self.emit(ind, f"for {i} in {lo}..{hi} {{")
            inner = self.copy_env(env)
            self.add(inner, "IntRO", i)
        else:
            w = self.fresh("w")
            self.emit(ind, f"var {w} = 0")
            self.emit(ind, f"while {w} < {hi} {{")
            inner = self.copy_env(env)
            self.add(env, "IntRO", w)
            self.add(inner, "IntRO", w)
            self.emit(ind + 1, f"{w} = {w} + 1")
        for _ in range(r.randint(1, 4)):
            self.stmt(ind + 1, inner, ret)
            if r.random() < 0.15:
                self.emit(ind + 1, f"if {self.cond(inner)} {{ {r.choice(['break', 'continue'])} }}")
        self.emit(ind, "}")
        self.loop_id = old
        self.loop_stack.pop()
        self.kill_moved(env, inner)

    def s_ret(self, ind, env, ret):
        if self.fn_kind == "int":
            self.emit(ind, f"if {self.cond(env)} {{ return {self.small(env)} }}")

    def s_danger(self, ind, env, ret):
        r = self.r
        k = r.random()
        ls = self.names(env, "List")
        if k < 0.3 and ls:
            self.emit(ind, f"print({self.pick(ls)}.get({self.small(env)}))")
        elif k < 0.5:
            self.emit(ind, f"print(100 / ({self.small(env)} - 3))")
        elif k < 0.6 and ls:
            self.emit(ind, f"print({self.pick(ls)}.pop())")
        elif k < 0.75:
            self.emit(ind, f"print({self.small(env)} * 4611686018427387904)")
        elif self.has("result") and k < 0.9:
            self.emit(ind, f"print(step({self.small(env)}).unwrap_or(-1))")
        elif self.has("closures"):
            fs = self.names(env, "Fn")
            if fs:
                self.emit(ind, f"print({self.pick(fs)}(9223372036854775807))")
        else:
            self.emit(ind, f"print(-9223372036854775807 - {self.small(env)} - 2)")

    def s_defer(self, ind, env, ret):
        ints = self.names(env, "Int")
        if ints and self.r.random() < 0.5:
            self.emit(ind, f"defer {self.pick(ints)} = {self.pick(ints)} + 1")
        elif ints:
            self.emit(ind, f"defer print({self.pick(ints)})")
        else:
            self.emit(ind, "defer print(7)")

    # ---- enums
    def fresh_msg(self, env):
        return f"mkmsg({self.small(env)}, String.from(\"{self.text()}\"))"

    def s_msgs(self, ind, env, ret):
        r = self.r
        ls = self.names(env, "Msgs")
        k = r.random()
        if not ls or k < 0.15:
            v = self.fresh("ml")
            self.emit(ind, f"var {v} = List<Msg>.new()")
            self.add(env, "Msgs", v)
            return
        l = self.pick(ls)
        if k < 0.4:
            ms = self.movable(env, "Msg")
            if ms and r.random() < 0.6:
                m = self.pick(ms)
                self.emit(ind, f"{l}.push(move {m})")
                self.drop_var(env, "Msg", m)
            else:
                self.emit(ind, f"{l}.push({self.fresh_msg(env)})")
        elif k < 0.5:
            self.emit(ind, f"if {l}.len() > 0 {{ let top = {l}.pop()\n{'  ' * ind}  print(msize(top)) }}")
        elif k < 0.6:
            self.emit(ind, f"if {l}.len() > 0 {{ mgrow({l}.at(0), {self.small(env)}) }}")
        elif k < 0.67:
            self.emit(ind, f"if {l}.len() > 1 {{ let rm = {l}.remove(1)\n{'  ' * ind}  print(mtext(move rm)) }}")
        elif k < 0.73:
            self.emit(ind, f"{l}.insert(0, {self.fresh_msg(env)})")
        elif k < 0.78:
            self.emit(ind, f"if {l}.len() > 1 {{ {l}.swap(0, {l}.len() - 1) }}")
        elif k < 0.83:
            self.emit(ind, f"if {l}.len() > 0 {{ {l}.set(0, {self.fresh_msg(env)}) }}")
        elif k < 0.87:
            self.emit(ind, f"{l}.truncate({r.randint(0, 3)})")
        elif k < 0.9:
            self.emit(ind, f"{l}.clear()")
        elif k < 0.94:
            self.emit(ind, f"for m in {l} {{ print(msize(m)) }}")
        elif k < 0.97:
            self.emit(ind, f"for mut m in {l} {{ mgrow(m, {r.randint(0, 9)}) }}")
        else:
            mv = self.movable(env, "Msgs")
            if l in mv:
                self.emit(ind, f"for m in move {l} {{")
                self.emit(ind + 1, f"print(msize(m))")
                if r.random() < 0.5:
                    self.emit(ind + 1, f"if msize(m) > 2 {{ break }}")
                self.emit(ind, "}")
                self.drop_var(env, "Msgs", l)

    def s_optmsg(self, ind, env, ret):
        r = self.r
        os_ = self.names(env, "OptMsg")
        k = r.random()
        if not os_ or k < 0.2:
            v = self.fresh("om")
            if r.random() < 0.5:
                self.emit(ind, f"var {v} = Option<Msg>.None")
            else:
                self.emit(ind, f"var {v} = Some({self.fresh_msg(env)})")
            self.add(env, "OptMsg", v)
        elif k < 0.45:
            self.emit(ind, f"{self.pick(os_)} = Some({self.fresh_msg(env)})")
        elif k < 0.6:
            self.emit(ind, f"{self.pick(os_)} = None")
        elif k < 0.75:
            o = self.pick(os_)
            self.emit(ind, f"match {o} {{")
            self.emit(ind + 1, "Some(m) => { print(msize(m)) }")
            self.emit(ind + 1, "None => { print(-7) }")
            self.emit(ind, "}")
        elif k < 0.9:
            o = self.pick(os_)
            self.emit(ind, f"match mut {o} {{")
            self.emit(ind + 1, f"Some(m) => {{ mgrow(m, {self.small_ints(env)}) }}")
            self.emit(ind + 1, "None => { }")
            self.emit(ind, "}")
        else:
            mv = self.movable(env, "OptMsg")
            if mv:
                o = self.pick(mv)
                self.emit(ind, f"match move {o} {{")
                self.emit(ind + 1, "Some(m) => { print(mtext(move m)) }")
                self.emit(ind + 1, "None => { print(-8) }")
                self.emit(ind, "}")
                self.drop_var(env, "OptMsg", o)

    def s_generic(self, ind, env, ret):
        r = self.r
        k = r.random()
        mm = self.movable(env, "Msg")
        ss = self.movable(env, "String")
        if k < 0.3 and mm:
            m = self.pick(mm)
            v = self.fresh("m")
            self.emit(ind, f"var {v} = pass(move {m})")
            self.drop_var(env, "Msg", m)
            self.add(env, "Msg", v)
        elif k < 0.5 and ss:
            s_ = self.pick(ss)
            o = self.fresh("om")
            v = self.fresh("s")
            self.emit(ind, f"let {o} = some_of(move {s_})")
            self.emit(ind, f"var {v} = unwrap_or2(move {o}, String.from(\"dflt\"))")
            self.drop_var(env, "String", s_)
            self.add(env, "String", v)
        elif k < 0.7 and mm:
            m = self.pick(mm)
            v = self.fresh("m")
            self.emit(ind, f"var {v} = unwrap_or2(None, move {m})" if False else f"var {v} = unwrap_or2(Option<Msg>.None, move {m})")
            self.drop_var(env, "Msg", m)
            self.add(env, "Msg", v)
        elif k < 0.9 and mm and ss:
            m = self.pick(mm)
            s_ = self.pick(ss)
            p = self.fresh("pp")
            self.emit(ind, f"var {p} = mkpair(move {s_}, move {m})")
            self.drop_var(env, "Msg", m)
            self.drop_var(env, "String", s_)
            self.emit(ind, f"print({p}.a.len() + msize({p}.b))")
        else:
            self.emit(ind, f"print(unwrap_or2(some_of({self.small(env)}), 5))")

    def arm_exit(self, ind, env, ret):
        """Optionally leave the enclosing loop / function from inside a match arm (temporaries must be dropped)."""
        r = self.r
        k = r.random()
        if self.loop_id != 0 and k < 0.2:
            self.emit(ind, r.choice(["break", "continue"]))
        elif ret and self.fn_kind == "int" and k < 0.3:
            self.emit(ind, f"return {self.small(env)}")
        elif self.fn_kind == "result" and k < 0.3:
            self.emit(ind, f"return Err(String.from(\"arm\"))")

    def s_mtemp(self, ind, env, ret):
        r = self.r
        key = f'"{r.choice(["a", "key", "", "xyz"])}"'
        kk = self.small(env)
        which = r.random()
        if self.fn_kind == "result" and which < 0.5:
            self.emit(ind, f"match lres({key}, {kk})? {{")
            self.emit(ind + 1, "Text(s) => {")
            self.emit(ind + 2, "print(s)")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "Pair(s, n) => { print(n) }")
            self.emit(ind + 1, "_ => { print(0) }")
            self.emit(ind, "}")
            return
        if self.fn_kind == "option" and which < 0.5:
            self.emit(ind, f"match lopt({key}, {kk})? {{")
            self.emit(ind + 1, "Text(s) => {")
            self.emit(ind + 2, "print(s)")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "_ => { print(0) }")
            self.emit(ind, "}")
            return
        if which < 0.35:
            self.emit(ind, f"match lmsg({key}, {kk}) {{")
            self.emit(ind + 1, "Text(s) => {")
            self.emit(ind + 2, "print(s)")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "Pair(s, n) => { print(n) }")
            self.emit(ind + 1, "Wrap(b) => {")
            self.emit(ind + 2, "print(msize(*b))")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "_ => { print(0) }")
            self.emit(ind, "}")
        elif which < 0.65:
            self.emit(ind, f"match lopt({key}, {kk}) {{")
            self.emit(ind + 1, "Some(m) => {")
            self.emit(ind + 2, "print(msize(m))")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "None => { print(-3) }")
            self.emit(ind, "}")
        elif which < 0.85:
            self.emit(ind, f"match lres({key}, {kk}) {{")
            self.emit(ind + 1, "Ok(m) => {")
            self.emit(ind + 2, "print(mtext(move m))")
            self.arm_exit(ind + 2, env, ret)
            self.emit(ind + 1, "}")
            self.emit(ind + 1, "Err(e) => { print(e) }")
            self.emit(ind, "}")
        else:
            v = self.fresh("n")
            tmp = self.fresh("m")
            self.emit(ind, f"var {tmp} = lmsg({key}, {kk})")
            self.emit(ind, f"var {v} = msize({tmp})")
            self.add(env, "Msg", tmp)
            self.add(env, "Int", v)

    def s_tuple(self, ind, env, ret):
        r = self.r
        t = self.fresh("tp")
        ss = self.movable(env, "String")
        if ss and r.random() < 0.5:
            s = self.pick(ss)
            self.excl.add(s)
            self.emit(ind, f"var {t} = (move {s}, {self.fresh_msg(env)}, {self.small(env)})")
            self.excl.discard(s)
            self.drop_var(env, "String", s)
        else:
            self.emit(ind, f"var {t} = (String.from(\"{self.text()}\"), {self.fresh_msg(env)}, {self.small(env)})")
        if r.random() < 0.5:
            a, b, c = self.fresh("s"), self.fresh("m"), self.fresh("i")
            self.emit(ind, f"var ({a}, {b}, {c}) = move {t}")
            self.add(env, "String", a)
            self.add(env, "Msg", b)
            self.add(env, "IntRO", c)
        else:
            self.emit(ind, f"print({t}.0.len() + msize({t}.1) + {t}.2)")

    def s_moveout(self, ind, env, ret):
        """Inside a loop: consume an outer-declared owned value and leave the loop at once (legal), or conditionally move."""
        r = self.r
        cands = [(t, n) for t in ("String", "Msg", "Msgs", "Tree") for n, lid in env.get(t, []) if lid == (self.loop_stack[-2] if len(self.loop_stack) > 1 else 0) and n not in self.excl]
        if not cands:
            return
        t, n = r.choice(cands)
        sink = {"String": "sink_s", "Msg": "sink_m", "Msgs": "sink_l", "Tree": "sink_t"}[t]
        self.emit(ind, f"if {self.cond(env)} {{")
        self.emit(ind + 1, f"print({sink}(move {n}))")
        self.emit(ind + 1, "break")
        self.emit(ind, "}")
        self.drop_var(env, t, n)

    def s_enum(self, ind, env, ret):
        r = self.r
        ms = self.names(env, "Msg")
        k = r.random()
        if not ms or k < 0.3:
            v = self.fresh("m")
            ss = self.movable(env, "String")
            if ss and r.random() < 0.6:
                s = self.pick(ss)
                self.emit(ind, f"var {v} = mkmsg({self.small_ex(env, s)}, move {s})")
                self.drop_var(env, "String", s)
            else:
                self.emit(ind, f"var {v} = mkmsg({self.small(env)}, String.from(\"{self.text()}\"))")
            self.add(env, "Msg", v)
        elif k < 0.5:
            self.emit(ind, f"mgrow({self.pick(ms)}, {self.small(env)})")
        elif k < 0.7:
            m = self.pick(ms)
            self.emit(ind, f"match {m} {{")
            self.emit(ind + 1, "Text(s) => { print(s) }")
            self.emit(ind + 1, "Nums(l) => { print(l.len()) }")
            self.emit(ind + 1, f"Pair(s, n) => {{ print(s.len() + n) }}")
            self.emit(ind + 1, "Wrap(b) => { print(msize(*b)) }")
            self.emit(ind + 1, "Quit => { print(-1) }")
            self.emit(ind, "}")
        elif k < 0.8:
            m = self.pick(ms)
            self.emit(ind, f"match mut {m} {{")
            self.emit(ind + 1, "Text(s) => { s.push_str(\"?\") }")
            self.emit(ind + 1, "Nums(l) => { l.push(1) }")
            self.emit(ind + 1, "_ => { }")
            self.emit(ind, "}")
        elif k < 0.92:
            mv = self.movable(env, "Msg")
            if mv:
                m = self.pick(mv)
                v = self.fresh("s")
                self.emit(ind, f"var {v} = mtext(move {m})")
                self.drop_var(env, "Msg", m)
                self.add(env, "String", v)
        else:
            # match on a temporary (owned) enum
            self.emit(ind, f"match mkmsg({self.small(env)}, String.from(\"{self.text()}\")) {{")
            self.emit(ind + 1, "Text(s) => { print(s) }")
            self.emit(ind + 1, "Pair(s, n) => { print(n) }")
            self.emit(ind + 1, "_ => { print(0) }")
            self.emit(ind, "}")

    # ---- box
    def s_tree(self, ind, env, ret):
        r = self.r
        ts = self.names(env, "Tree")
        k = r.random()
        if not ts or k < 0.25:
            v = self.fresh("t")
            self.emit(ind, f"var {v} = Tree.Leaf")
            self.add(env, "Tree", v)
        elif k < 0.55:
            t = self.pick(self.movable(env, "Tree") or ts)
            if t in self.movable(env, "Tree"):
                self.emit(ind, f"{t} = tinsert(move {t}, {self.small_ex(env, t)})")
            else:
                self.emit(ind, f"print(tsum({t}))")
        elif k < 0.65:
            self.emit(ind, f"tbump({self.pick(ts)}, {self.small(env)})")
        elif k < 0.75:
            self.show(ind, f"tmin({self.pick(ts)})")
        elif k < 0.85:
            t = self.pick(self.movable(env, "Tree") or ts)
            if t in self.movable(env, "Tree"):
                self.emit(ind, f"{t} = tflip(move {t})")
        elif k < 0.93:
            c = self.fresh("c")
            self.emit(ind, f"let {c} = chain_build({r.randint(0, 5)})")
            self.emit(ind, f"print(chain_sum({c}))")
        else:
            self.emit(ind, f"for q in 0..{r.randint(1, 4)} {{ {self.pick(ts)} = tinsert(move {self.pick(ts)}, q) }}" if False else f"print(tdepth({self.pick(ts)}))")

    # ---- closures
    def clos_expr(self, env, owned_only=True):
        """An owned closure expression of type fn(Int) -> Int. May consume movable owned vars (returns list of moved)."""
        r = self.r
        moved = []
        k = r.random()
        fs_mov = self.movable(env, "Fn")
        strs = self.movable(env, "String")
        lists = self.movable(env, "List")
        if k < 0.15:
            e = f"make_adder({self.small(env)})"
        elif k < 0.25:
            e = f"make_scaler({self.small(env)})"
        elif k < 0.35:
            e = f"make_counter({self.small(env)})"
        elif k < 0.5 and strs:
            s = self.pick(strs)
            moved.append(("String", s))
            e = f"make_len(move {s})"
        elif k < 0.6 and strs and lists:
            s = self.pick(strs)
            l = self.pick(lists)
            moved += [("String", s), ("List", l)]
            e = f"make_dropper(move {s}, move {l})"
        elif k < 0.7 and len(fs_mov) >= 2:
            a, b = r.sample(fs_mov, 2)
            moved += [("Fn", a), ("Fn", b)]
            e = f"compose(move {a}, move {b})"
        elif k < 0.78 and len(fs_mov) >= 2:
            a, b = r.sample(fs_mov, 2)
            moved += [("Fn", a), ("Fn", b)]
            e = f"pick({self.cond_ex(env, a, b)}, move {a}, move {b})"
        elif k < 0.86 and strs:
            s = self.pick(strs)
            moved.append(("String", s))
            e = f"early(move {s}, {self.small_ex(env, s)})"
        elif k < 0.89 and self.has("enums") and self.movable(env, "Msg"):
            m = self.pick(self.movable(env, "Msg"))
            moved.append(("Msg", m))
            e = f"{r.choice(['make_msgf', 'make_msgmut'])}(move {m})"
        elif k < 0.9 and self.has("enums") and self.movable(env, "Msgs") and strs:
            l = self.pick(self.movable(env, "Msgs"))
            s = self.pick(strs)
            moved += [("Msgs", l), ("String", s)]
            e = f"make_listf(move {l}, move {s})"
        elif k < 0.91:
            e = f"curry3({self.small(env)})({self.small(env)})"
        elif k < 0.92:
            e = r.choice(["make_opt", "make_try", "make_div"]) + f"({self.small(env)})"
        elif k < 0.93 and strs:
            s = self.pick(strs)
            moved.append(("String", s))
            moved.append(("String", s))
            e = f"move |x: Int| x + {s}.len() + {self.small_ints(env)}"
        else:
            ints = self.names(env, "Int") + self.names(env, "IntRO")
            c = self.pick(ints) if ints else "3"
            e = f"move |x: Int| x * {c} + 1"
        return e, moved

    def s_clos(self, ind, env, ret):
        r = self.r
        fs = self.names(env, "Fn")
        k = r.random()
        if not fs or k < 0.35:
            e, moved = self.clos_expr(env)
            v = self.fresh("f")
            self.emit(ind, f"var {v} = {e}")
            for t, n in moved:
                self.drop_var(env, t, n)
            self.add(env, "Fn", v)
        elif k < 0.55:
            self.emit(ind, f"print({self.pick(fs)}({self.small(env)}))")
        elif k < 0.65:
            # non-move closure borrowing captures, passed straight to a view fn param
            ints = self.names(env, "Int") + self.names(env, "IntRO")
            strs = self.names(env, "String")
            body = "x + " + (self.pick(ints) if ints else "1")
            if strs and r.random() < 0.5:
                body += f" + {self.pick(strs)}.len()"
            self.emit(ind, f"print(apply(|x: Int| {body}, {self.small(env)}))")
        elif k < 0.75:
            ints = self.names(env, "Int")
            if ints:
                t = self.pick(ints)
                self.emit(ind, f"repeat({r.randint(0, 4)}, || {t} = {t} + {r.randint(1, 3)})")
        elif k < 0.78:
            ls = self.names(env, "List")
            strs = self.names(env, "String")
            if len(ls) >= 2:
                a, b = r.sample(ls, 2)
                self.emit(ind, f"each({a}, |x: Int| {b}.push(x * {r.randint(1, 3)}))")
            elif strs:
                t = self.pick(strs)
                self.emit(ind, f"repeat({r.randint(0, 3)}, || {t}.push_str(\"z\"))")
        elif k < 0.82:
            ls = self.names(env, "List")
            ints = self.names(env, "Int")
            if ls and ints:
                t = self.pick(ints)
                self.emit(ind, f"each({self.pick(ls)}, |x: Int| {t} = {t} + x)")
        elif k < 0.9:
            mv = self.movable(env, "Fn")
            if mv:
                f = self.pick(mv)
                self.emit(ind, f"print(call_owned(move {f}, {self.small_ex(env, f)}))")
                self.drop_var(env, "Fn", f)
        elif k < 0.93 and len(self.movable(env, "Fn")) >= 2:
            f, g = r.sample(self.movable(env, "Fn"), 2)
            v = self.fresh("cb")
            self.emit(ind, f"let {v} = Cb {{ f: move {f}, g: move {g}, name: String.from(\"cb\") }}")
            self.emit(ind, f"print(run_cb({v}, {self.small_ex(env, f, g)}))")
            self.drop_var(env, "Fn", f)
            self.drop_var(env, "Fn", g)
        elif k < 0.95:
            mv = self.movable(env, "Fn")
            if mv:
                f = self.pick(mv)
                strs = self.movable(env, "String")
                nm = self.pick(strs) if strs else None
                v = self.fresh("tk")
                name = f"move {nm}" if nm else "String.from(\"task\")"
                self.emit(ind, f"let {v} = Task {{ name: {name}, run: move {f} }}")
                self.emit(ind, f"print(run_task({v}, {self.small_ex(env, f, *( [nm] if nm else []))}))")
                self.drop_var(env, "Fn", f)
                if nm:
                    self.drop_var(env, "String", nm)
        else:
            # reassign: drops the previous closure
            mv = self.movable(env, "Fn")
            if mv:
                f = self.pick(mv)
                e, moved = self.clos_expr(env)
                if all(n != f for _, n in moved):
                    self.emit(ind, f"{f} = {e}")
                    for t, n in moved:
                        self.drop_var(env, t, n)

    # ---- dyn
    def s_dyn(self, ind, env, ret):
        r = self.r
        ss = self.names(env, "Shapes")
        k = r.random()
        if not ss or k < 0.2:
            v = self.fresh("sh")
            self.emit(ind, f"var {v} = List<Box<dyn Shape>>.new()")
            self.add(env, "Shapes", v)
            return
        s = self.pick(ss)
        if k < 0.5:
            strs = self.movable(env, "String")
            if strs and r.random() < 0.5:
                nm = self.pick(strs)
                self.emit(ind, f"{s}.push(mkshape({self.small_ex(env, nm)}, move {nm}))")
                self.drop_var(env, "String", nm)
            else:
                self.emit(ind, f"{s}.push(mkshape({self.small(env)}, String.from(\"{self.text()}\")))")
        elif k < 0.6:
            self.emit(ind, f"if {s}.len() > 0 {{ {s}.at({s}.len() - 1).grow({self.small(env)}) }}")
        elif k < 0.7:
            self.emit(ind, f"if {s}.len() > 0 {{ grow_it({s}.at(0).get(), {self.small(env)}) }}")
        elif k < 0.78:
            self.emit(ind, f"if {s}.len() > 0 {{ print(area_of({s}.at(0).get())) }}")
        elif k < 0.86:
            self.emit(ind, f"if {s}.len() > 0 {{ let nm = {s}.at(0).name()\n{'  ' * ind}  print(nm) }}")
        elif k < 0.92:
            self.emit(ind, f"if {s}.len() > 0 {{ let top = {s}.pop()\n{'  ' * ind}  print(top.area()) }}")
        elif k < 0.96:
            self.emit(ind, f"{s}.set(0, mkshape({self.small(env)}, String.from(\"r\")))" if False else f"print(total_area({s}))")
        else:
            self.emit(ind, f"print(gen_area(Dot {{ x: {self.small(env)} }}))")

    # ---- Option / Result
    def s_result(self, ind, env, ret):
        r = self.r
        k = r.random()
        if self.fn_kind in ("result",) and k < 0.4:
            v = self.fresh("q")
            self.emit(ind, f"var {v} = step({self.small(env)})?")
            self.add(env, "Int", v)
            return
        if self.fn_kind == "option" and k < 0.4:
            ls = self.names(env, "List")
            if ls:
                v = self.fresh("q")
                self.emit(ind, f"var {v} = find({self.pick(ls)}, {self.small(env)})?")
                self.add(env, "Int", v)
                return
        if k < 0.3:
            self.show(ind, f"step({self.small(env)})")
        elif k < 0.5:
            ls = self.names(env, "List")
            if ls:
                self.show(ind, f"opt_chain({self.pick(ls)}, {self.small(env)}, {self.small(env)})")
        elif k < 0.7:
            strs = self.names(env, "String")
            if strs:
                self.show(ind, f"owned_opt(String.from({self.pick(strs)}), {self.small(env)})")
        elif k < 0.85:
            lg = self.names(env, "Logs")
            if not lg:
                v = self.fresh("lg")
                self.emit(ind, f"var {v} = List<String>.new()")
                self.add(env, "Logs", v)
                lg = [v]
            self.show(ind, f"chain2({self.small(env)}, {self.pick(lg)})")
            self.emit(ind, f"print({self.pick(lg)}.len())")
        else:
            if self.has("enums"):
                self.show(ind, f"bubble(mkmsg({self.small(env)}, String.from(\"{self.text()}\")), {self.small(env)})")
            else:
                self.show(ind, f"step({self.small(env)})")

    def show(self, ind, expr):
        v = self.fresh("r")
        self.emit(ind, f"let {v} = {expr}")
        self.emit(ind, f'print("{{{v}}}")')

    def s_io(self, ind, env, ret):
        self.emit(ind, f"print(io_probe({self.small(env)}, \"/tmp/yfz_new_{self.seed}.txt\", \"{self.text()}\"))")

    # ---- program
    def gen_fn(self, kind):
        """A random helper function. kinds: int -> Int, result -> Result<Int,String>, option -> Option<Int>."""
        name = self.fresh("g")
        self.fn_kind = kind
        sig = {"int": "Int", "result": "Result<Int, String>", "option": "Option<Int>"}[kind]
        self.emit(0, f"fn {name}(pa: Int, pb: Int, pl: List<Int>) -> {sig} {{")
        env = {"Int": [("pa", 0), ("pb", 0)], "List": [("pl", 0)]}
        self.loop_id = 0
        for _ in range(self.r.randint(2, 7)):
            self.stmt(1, env, True)
        if kind == "int":
            self.emit(1, f"return {self.small(env)}")
        elif kind == "result":
            self.emit(1, f"return Ok({self.small(env)})")
        else:
            self.emit(1, f"return Some({self.small(env)})")
        self.emit(0, "}")
        self.fns.append((name, kind))

    def program(self):
        f = self.feats
        if "enums" in f or "result" in f or "closures" in f:
            self.lines += PRELUDE_ENUM.splitlines()
        if "box" in f:
            self.lines += PRELUDE_TREE.splitlines()
        if "closures" in f:
            self.lines += PRELUDE_CLOS.splitlines()
        if "result" in f:
            self.lines += PRELUDE_RESULT.splitlines()
        if "dyn" in f:
            self.lines += PRELUDE_DYN.splitlines()
        if "io" in f:
            self.lines = PRELUDE_IO.splitlines() + self.lines
        for _ in range(self.r.randint(0, 3)):
            self.gen_fn(self.r.choice(["int", "int", "result", "option"] if "result" in f else ["int"]))
            self.loop_next = 0
        self.fn_kind = "main"
        self.emit(0, "fn main() {")
        env = {"Int": [("seed", 0)]}
        self.emit(1, f"var seed = {self.r.randint(0, 20)}")
        self.loop_id = 0
        self.loop_next = 0
        for _ in range(self.r.randint(6, 22)):
            self.stmt(1, env, False, top=True)
            # call a generated helper now and then
            if self.fns and self.r.random() < 0.25:
                name, kind = self.r.choice(self.fns)
                lst = self.names(env, "List")
                self.show(1, f"{name}({self.small(env)}, {self.small(env)}, List<Int>.new())")
        # observable final state
        for n in self.names(env, "String"):
            self.emit(1, f"print({n})")
        for n in self.names(env, "List"):
            self.emit(1, f"print({n}.len())")
        for n in self.names(env, "Msg"):
            self.emit(1, f"print(msize({n}))")
        for n in self.names(env, "Tree"):
            self.emit(1, f"print(tsum({n}))")
        for n in self.names(env, "Msgs"):
            self.emit(1, f"print({n}.len())")
        for n in self.names(env, "Fn"):
            self.emit(1, f"print({n}(1))")
        for n in self.names(env, "Shapes"):
            self.emit(1, f"print(total_area({n}))")
        self.emit(0, "}")
        return self.lines


FEATS = ["closures", "enums", "box", "dyn", "result", "io", "defer"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=300)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("-j", type=int, default=1)
    ap.add_argument("--a64", action="store_true")
    ap.add_argument("--max-saves", type=int, default=15)
    ap.add_argument("--no-leak", action="store_true")
    ap.add_argument("--io-free", dest="io", action="store_true", help="include std.io (args) probes")
    for f in ("closures", "enums", "box", "dyn", "result"):
        ap.add_argument("--" + f, action="store_true")
    ap.add_argument("--all", action="store_true")
    a = ap.parse_args()
    feats = [f for f in ("closures", "enums", "box", "dyn", "result") if getattr(a, f)]
    if a.all or not feats:
        feats = ["closures", "enums", "box", "dyn", "result"]
    feats.append("defer")
    if a.io and not a.a64:
        feats.append("io")
    seed0 = a.seed if a.seed is not None else random.randrange(1 << 30)
    os.makedirs(F.BUGS, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="yerux_fuzznew_")
    runner = F.Runner(tmp, F.detect_noopt(), a.a64)
    print(f"fuzz_new: seed={seed0} n={a.n} jobs={a.j} feats={feats} backends={[b for b, _ in runner.backends()]}", flush=True)
    stats = dict(run=0, invalid=0, mismatch=0, skipped=0, panics=0)
    found = {}
    lock = threading.Lock()

    def one(i):
        seed = seed0 + i
        lines = NewGen(seed, feats).program()
        res, err = runner.evaluate(lines)
        with lock:
            stats["run"] += 1
            if res is None:
                stats["invalid"] += 1
                if stats["invalid"] <= 8:
                    path = os.path.join(tmp, f"invalid_{seed}.yx")
                    open(path, "w").write("\n".join(lines) + "\n")
                    print(f"generator reject: seed {seed}: {err.decode()[:300]}", flush=True)
                return
            if any(v == "SKIP" for v in res.values()):
                stats["skipped"] += 1
                return
            if res["c"][0] == 101:
                stats["panics"] += 1
            key = F.mismatch_key(res)
            panicked = res["c"][0] != 0
        if key is None and not a.no_leak and not panicked:
            bad = F.leak_check(runner, lines)
            if bad:
                key = (("native-debug-alloc", bad),)
        if key is None:
            return
        with lock:
            stats["mismatch"] += 1
        print(f"MISMATCH seed={seed} {key}; minimizing ({len(lines)} lines)", flush=True)
        small = F.minimize(lines, runner, key) if not key[0][0] == "native-debug-alloc" else lines
        res2, _ = runner.evaluate(small)
        text = "\n".join(small) + "\n"
        h = hashlib.sha1(text.encode()).hexdigest()[:8]
        with lock:
            if h in found or len(found) >= a.max_saves:
                return
            found[h] = key
            base = os.path.join(F.BUGS, f"fuzznew_{h}")
            with open(base + ".yx", "w") as fh:
                fh.write(text)
            with open(base + ".txt", "w") as fh:
                fh.write(f"seed: {seed}\nmismatch: {key}\n\n")
                for name, v in (res2 or res).items():
                    fh.write(f"=== {name}\n{F.fmt(v)}\n")
            print(f"  saved {base}.yx ({len(small)} lines)", flush=True)

    with ThreadPoolExecutor(a.j) as ex:
        list(ex.map(one, range(a.n)))
    shutil.rmtree(tmp, ignore_errors=True)
    print(f"done: {stats} distinct reproducers saved: {len(found)}")
    return 1 if stats["mismatch"] else 0


if __name__ == "__main__":
    sys.exit(main())
