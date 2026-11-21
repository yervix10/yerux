//! Compile-time execution (`comptime`). A first, deliberately small version: `comptime expr` evaluates a
//! scalar (`Int`, `Char`, `Bool`) expression while compiling, calling ordinary user functions whose
//! parameters and result are scalars. It runs on the *checked* program, so it has exactly the semantics of
//! the runtime (checked arithmetic, division by zero, `assert`, `panic`) and nothing is implemented twice.
//! Anything outside the supported subset is a compile error that says why, never a silent fallback.
//!
//! The parser turns `comptime e` into the marker call `__comptime(e)`; the checker types it as `e`;
//! `run` (after the checker) replaces every marker by the literal it evaluates to. Backends never see it.
use crate::syntax::*;
use std::collections::HashMap;

const STEP_BUDGET: u64 = 20_000_000;
const MAX_DEPTH: usize = 1000;
pub const MARKER: &str = "__comptime";

#[derive(Clone)]
enum V {
    I(i64),
    B(bool),
    /// a fixed-size array of compile-time values (arrays of scalars, nested arrays)
    A(Vec<V>),
    U,
}

enum Flow {
    Next,
    Break(usize),
    Continue(usize),
    Ret(V),
}

type Env = HashMap<usize, V>;
type R<T> = Result<T, Diag>;

struct Interp<'a> {
    funcs: &'a HashMap<String, &'a Func>,
    types: &'a Types,
    steps: u64,
    chain: Vec<(String, Pos)>,
}

fn show_line(p: Pos) -> usize {
    p.line & 0xFFFFF
}

fn unsupported(what: &str, pos: Pos, chain: &[(String, Pos)]) -> Diag {
    let d = diag(format!("{} cannot run at compile time [E-K1]", what), pos)
        .note("why: compile-time code is run by the compiler; for now it may only use Int, Char and Bool values, `let`/`var`, `if`, `while`, and calls to functions with such parameters")
        .note("fix: compute the value at run time, or rewrite the code with scalar values only");
    add_chain(d, chain)
}

/// "called from" notes, innermost first; a long chain (recursion) is cut after 6 entries.
fn add_chain(mut d: Diag, chain: &[(String, Pos)]) -> Diag {
    for (f, p) in chain.iter().rev().take(6) {
        d = d.note(format!("called from `{}` (line {})", f, show_line(*p)));
    }
    if chain.len() > 6 {
        d = d.note(format!("... and {} more calls", chain.len() - 6));
    }
    d
}

fn scalar_ty(t: Ty) -> bool {
    matches!(t, Ty::Int | Ty::Range(..) | Ty::Char | Ty::Bool)
}

/// The values comptime code may hold: scalars and arrays of them.
fn value_ty(types: &Types, t: Ty) -> bool {
    scalar_ty(t) || types.array_of(t).is_some_and(|(e, _)| value_ty(types, e))
}

impl<'a> Interp<'a> {
    fn tick(&mut self, pos: Pos) -> R<()> {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return Err(diag(format!("compile-time evaluation ran too long (more than {} steps) [E-K2]", STEP_BUDGET), pos)
                .note("why: the compiler runs this code while compiling, so it must finish; it may never end for some inputs")
                .note("fix: add a limit to the loop or make the computation smaller"));
        }
        Ok(())
    }

    fn fail(&self, msg: &str, pos: Pos) -> Diag {
        let d = diag(format!("{} [E-K6]", msg), pos).note("why: this code was being evaluated by the compiler (comptime), where a panic is a compile error");
        add_chain(d, &self.chain)
    }

    fn block(&mut self, body: &[Stmt], env: &mut Env) -> R<Flow> {
        for s in body {
            match self.stmt(s, env)? {
                Flow::Next => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Next)
    }

    fn stmt(&mut self, s: &Stmt, env: &mut Env) -> R<Flow> {
        self.tick(s.pos)?;
        match &s.k {
            SK::Let(_, _, e, id) => {
                let v = self.expr(e, env)?;
                env.insert(*id, v);
                Ok(Flow::Next)
            }
            SK::Assign(t, e) => {
                let v = self.expr(e, env)?;
                match &t.k {
                    EK::Var(_, id) => {
                        env.insert(*id, v);
                        Ok(Flow::Next)
                    }
                    _ => Err(unsupported("assignment to this place", t.pos, &self.chain)),
                }
            }
            SK::ExprS(e) => {
                self.expr(e, env)?;
                Ok(Flow::Next)
            }
            SK::If(c, a, b) => {
                if self.truth(c, env)? {
                    self.block(a, env)
                } else if let Some(b) = b {
                    self.block(b, env)
                } else {
                    Ok(Flow::Next)
                }
            }
            SK::While(c, body, lp) => {
                loop {
                    self.tick(s.pos)?;
                    if !self.truth(c, env)? {
                        break;
                    }
                    match self.block(body, env)? {
                        Flow::Next => {}
                        Flow::Break(id) if id == lp.id => break,
                        Flow::Continue(id) if id == lp.id => {}
                        f => return Ok(f),
                    }
                    match self.block(&lp.step, env)? {
                        Flow::Next => {}
                        f => return Ok(f),
                    }
                }
                Ok(Flow::Next)
            }
            SK::Break(_, id) => Ok(Flow::Break(*id)),
            SK::Continue(_, id) => Ok(Flow::Continue(*id)),
            SK::Block(b, _) => self.block(b, env),
            SK::Return(e) => {
                let v = match e {
                    Some(e) => self.expr(e, env)?,
                    None => V::U,
                };
                Ok(Flow::Ret(v))
            }
            _ => Err(unsupported("this statement", s.pos, &self.chain)),
        }
    }

    fn truth(&mut self, e: &Expr, env: &mut Env) -> R<bool> {
        match self.expr(e, env)? {
            V::B(b) => Ok(b),
            _ => Err(unsupported("this condition", e.pos, &self.chain)),
        }
    }

    fn int(&mut self, e: &Expr, env: &mut Env) -> R<i64> {
        match self.expr(e, env)? {
            V::I(i) => Ok(i),
            _ => Err(unsupported("this operand", e.pos, &self.chain)),
        }
    }

    /// `a.at(i).at(j)..` on a variable: its id and the evaluated (index, position) path.
    fn path(&mut self, e: &Expr, env: &mut Env) -> R<(usize, Vec<(i64, Pos)>)> {
        match &e.k {
            EK::Var(n, id) => {
                if !env.contains_key(id) {
                    return Err(unsupported(&format!("the run-time variable `{}`", n), e.pos, &self.chain)
                        .note("its value is not known while compiling; only variables defined inside the comptime code are"));
                }
                Ok((*id, vec![]))
            }
            EK::Method { recv, m, args } if m == "at" && args.len() == 1 => {
                let (id, mut p) = self.path(recv, env)?;
                let i = self.int(&args[0], env)?;
                p.push((i, args[0].pos));
                Ok((id, p))
            }
            _ => Err(unsupported("this place", e.pos, &self.chain)),
        }
    }
    /// The element `path` leads to inside the value of variable `id`.
    fn slot<'v>(&self, v: &'v mut V, path: &[(i64, Pos)]) -> R<&'v mut V> {
        let mut cur = v;
        for (i, pos) in path {
            cur = match cur {
                V::A(items) => {
                    let n = items.len();
                    if *i < 0 || *i as usize >= n {
                        return Err(self.fail("index out of range", *pos));
                    }
                    &mut items[*i as usize]
                }
                _ => return Err(unsupported("indexing this value", *pos, &self.chain)),
            };
        }
        Ok(cur)
    }
    fn place_mut<'v>(&mut self, e: &Expr, env: &'v mut Env) -> R<&'v mut V> {
        let (id, path) = self.path(e, env)?;
        let v = env.get_mut(&id).expect("checked by `path`");
        self.slot(v, &path)
    }

    fn expr(&mut self, e: &Expr, env: &mut Env) -> R<V> {
        self.tick(e.pos)?;
        if !value_ty(self.types, e.ty) && e.ty != Ty::Unit {
            return Err(unsupported("a value of this type", e.pos, &self.chain));
        }
        match &e.k {
            EK::Int(i) | EK::Char(i) => Ok(V::I(*i)),
            EK::Bool(b) => Ok(V::B(*b)),
            EK::ArrayLit(items) => {
                let mut vs = vec![];
                for it in items {
                    vs.push(self.expr(it, env)?);
                }
                Ok(V::A(vs))
            }
            EK::ArrayFill(x, n) => {
                let v = self.expr(x, env)?;
                Ok(V::A(vec![v; *n]))
            }
            EK::Method { recv, m, args } if self.types.array_of(recv.ty).is_some() => {
                let n = self.types.array_of(recv.ty).unwrap().1;
                match m.as_str() {
                    "len" => Ok(V::I(n as i64)),
                    "at" | "get" if args.len() == 1 => {
                        let (id, mut p) = self.path(recv, env)?;
                        let i = self.int(&args[0], env)?;
                        p.push((i, args[0].pos));
                        let v = env.get_mut(&id).expect("checked by `path`");
                        Ok(self.slot(v, &p)?.clone())
                    }
                    "set" if args.len() == 2 => {
                        let i = self.int(&args[0], env)?;
                        let x = self.expr(&args[1], env)?;
                        let ipos = args[0].pos;
                        let a = self.place_mut(recv, env)?;
                        let mut tmp = std::mem::replace(a, V::U);
                        let r = self.slot(&mut tmp, &[(i, ipos)]).map(|s| *s = x);
                        *self.place_mut(recv, env)? = tmp;
                        r.map(|_| V::U)
                    }
                    "swap" if args.len() == 2 => {
                        let i = self.int(&args[0], env)?;
                        let j = self.int(&args[1], env)?;
                        let (ip, jp) = (args[0].pos, args[1].pos);
                        let a = self.place_mut(recv, env)?;
                        let mut tmp = std::mem::replace(a, V::U);
                        let r = (|| -> R<()> {
                            let x = self.slot(&mut tmp, &[(i, ip)])?.clone();
                            let y = std::mem::replace(self.slot(&mut tmp, &[(j, jp)])?, x);
                            *self.slot(&mut tmp, &[(i, ip)])? = y;
                            Ok(())
                        })();
                        *self.place_mut(recv, env)? = tmp;
                        r.map(|_| V::U)
                    }
                    _ => Err(unsupported("this array method", e.pos, &self.chain)),
                }
            }
            EK::Var(n, id) | EK::Move(n, id) => env.get(id).cloned().ok_or_else(|| {
                unsupported(&format!("the run-time variable `{}`", n), e.pos, &self.chain)
                    .note("its value is not known while compiling; only variables defined inside the comptime code are")
            }),
            EK::Un(op, x) => match *op {
                "!" => Ok(V::B(!self.truth(x, env)?)),
                "-" => {
                    let v = self.int(x, env)?;
                    v.checked_neg().map(V::I).ok_or_else(|| self.fail("integer overflow while evaluating a constant", e.pos))
                }
                _ => Err(unsupported("this operator", e.pos, &self.chain)),
            },
            EK::Bin(op, a, b) => {
                if *op == "&&" || *op == "||" {
                    let l = self.truth(a, env)?;
                    if (*op == "&&" && !l) || (*op == "||" && l) {
                        return Ok(V::B(l));
                    }
                    return Ok(V::B(self.truth(b, env)?));
                }
                let l = self.expr(a, env)?;
                let r = self.expr(b, env)?;
                let (x, y) = match (l, r) {
                    (V::I(x), V::I(y)) => (x, y),
                    (V::B(x), V::B(y)) => {
                        return match *op {
                            "==" => Ok(V::B(x == y)),
                            "!=" => Ok(V::B(x != y)),
                            _ => Err(unsupported("this operator on Bool", e.pos, &self.chain)),
                        }
                    }
                    _ => return Err(unsupported("this operand", e.pos, &self.chain)),
                };
                let ov = || self.fail("integer overflow while evaluating a constant", e.pos);
                Ok(match *op {
                    "+" => V::I(x.checked_add(y).ok_or_else(ov)?),
                    "-" => V::I(x.checked_sub(y).ok_or_else(ov)?),
                    "*" => V::I(x.checked_mul(y).ok_or_else(ov)?),
                    "/" | "%" => {
                        if y == 0 {
                            return Err(self.fail("division by zero while evaluating a constant", e.pos));
                        }
                        let r = if *op == "/" { x.checked_div(y) } else { x.checked_rem(y) };
                        V::I(r.ok_or_else(ov)?)
                    }
                    "==" => V::B(x == y),
                    "!=" => V::B(x != y),
                    "<" => V::B(x < y),
                    ">" => V::B(x > y),
                    "<=" => V::B(x <= y),
                    ">=" => V::B(x >= y),
                    _ => return Err(unsupported("this operator", e.pos, &self.chain)),
                })
            }
            EK::Call(name, args) => self.call(name, args, e, env),
            _ => Err(unsupported("this expression", e.pos, &self.chain)),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], e: &Expr, env: &mut Env) -> R<V> {
        match name {
            MARKER => return self.expr(&args[0], env),
            "assert" if args.len() == 1 => {
                if !self.truth(&args[0], env)? {
                    return Err(self.fail("assertion failed", e.pos));
                }
                return Ok(V::U);
            }
            "assert_eq" if args.len() == 2 => {
                let a = self.expr(&args[0], env)?;
                let b = self.expr(&args[1], env)?;
                let eq = match (a, b) {
                    (V::I(x), V::I(y)) => x == y,
                    (V::B(x), V::B(y)) => x == y,
                    _ => return Err(unsupported("this comparison", e.pos, &self.chain)),
                };
                if !eq {
                    return Err(self.fail("assertion failed", e.pos));
                }
                return Ok(V::U);
            }
            "panic" => {
                let msg = match args.first().map(|a| &a.k) {
                    Some(EK::Text(t)) => format!("panic: {}", t),
                    _ => "panic".to_string(),
                };
                return Err(self.fail(&msg, e.pos));
            }
            _ => {}
        }
        let table = self.funcs;
        let f = *table
            .get(name)
            .ok_or_else(|| unsupported(&format!("a call to `{}`", name), e.pos, &self.chain))?;
        if f.pos.line >= PRELUDE_LINE
            || (!value_ty(self.types, f.ret) && f.ret != Ty::Unit)
            || f.params
                .iter()
                .any(|p| p.mode != Mode::Own || !value_ty(self.types, p.ty))
        {
            return Err(unsupported(
                &format!("a call to `{}`", name),
                e.pos,
                &self.chain,
            ));
        }
        if self.chain.len() >= MAX_DEPTH {
            return Err(self.fail("compile-time evaluation recursed too deeply", e.pos));
        }
        let mut vals = vec![];
        for a in args {
            vals.push(self.expr(a, env)?);
        }
        let mut fenv: Env = HashMap::new();
        for (p, v) in f.params.iter().zip(vals) {
            fenv.insert(p.id, v);
        }
        self.chain.push((name.to_string(), e.pos));
        let r = self.block(&f.body, &mut fenv);
        self.chain.pop();
        match r? {
            Flow::Ret(v) => Ok(v),
            _ => Ok(V::U),
        }
    }
}

// ---- traversal: find and replace the markers ----

type Visit<'f> = &'f mut dyn FnMut(&mut Expr) -> R<bool>;

fn walk_stmts(b: &mut [Stmt], f: Visit) -> R<()> {
    for s in b.iter_mut() {
        walk_stmt(s, f)?;
    }
    Ok(())
}

fn walk_stmt(s: &mut Stmt, f: Visit) -> R<()> {
    match &mut s.k {
        SK::Let(_, _, e, _) | SK::ExprS(e) | SK::Unpack(_, e) | SK::Bind(_, _, e, _) => walk(e, f)?,
        SK::Assign(a, b) => {
            walk(a, f)?;
            walk(b, f)?;
        }
        SK::If(c, a, b) => {
            walk(c, f)?;
            walk_stmts(a, f)?;
            if let Some(b) = b {
                walk_stmts(b, f)?;
            }
        }
        SK::While(c, b, lp) => {
            walk(c, f)?;
            walk_stmts(b, f)?;
            walk_stmts(&mut lp.step, f)?;
        }
        SK::Block(b, _) | SK::Unsafe(b) => walk_stmts(b, f)?,
        SK::Return(Some(e)) => walk(e, f)?,
        SK::Match(e, arms) => {
            walk(e, f)?;
            for a in arms {
                if let Some(g) = &mut a.guard {
                    walk(g, f)?;
                }
                walk_stmts(&mut a.body, f)?;
            }
        }
        SK::Defer(s) => walk_stmt(s, f)?,
        SK::For(h, b) => {
            match &mut h.iter {
                ForIter::Range(a, c, _) => {
                    walk(a, f)?;
                    walk(c, f)?;
                }
                ForIter::Expr(e) => walk(e, f)?,
            }
            walk_stmts(b, f)?;
        }
        SK::Return(None) | SK::Break(..) | SK::Continue(..) => {}
    }
    Ok(())
}

/// Visit `e`; when the callback returns true the sub-expressions are not visited.
fn walk(e: &mut Expr, f: Visit) -> R<()> {
    if f(e)? {
        return Ok(());
    }
    match &mut e.k {
        EK::Ascribe(x)
        | EK::Conv(x, _)
        | EK::Field(x, _)
        | EK::Un(_, x)
        | EK::IsVariant(x, _)
        | EK::Payload(x, _, _)
        | EK::AddrOf(x, _)
        | EK::Deref(x) => walk(x, f)?,
        EK::DynBox { value, .. } => walk(value, f)?,
        EK::AsElse(x, b) => {
            walk(x, f)?;
            walk_stmts(b, f)?;
        }
        EK::Bin(_, a, b) => {
            walk(a, f)?;
            walk(b, f)?;
        }
        EK::Call(_, args)
        | EK::Static { args, .. }
        | EK::Variant { args, .. }
        | EK::GenVariant { args, .. }
        | EK::DynCall { args, .. } => {
            for a in args {
                walk(a, f)?;
            }
        }
        EK::Method { recv, args, .. } => {
            walk(recv, f)?;
            for a in args {
                walk(a, f)?;
            }
        }
        EK::CallFn(c, args) => {
            walk(c, f)?;
            for a in args {
                walk(a, f)?;
            }
        }
        EK::StructLit { fields, .. } | EK::GenStruct { fields, .. } => {
            for (_, a) in fields {
                walk(a, f)?;
            }
        }
        EK::MkClosure { caps, .. } => {
            for a in caps {
                walk(a, f)?;
            }
        }
        EK::Sequence(ss, x) => {
            walk_stmts(ss, f)?;
            walk(x, f)?;
        }
        EK::Closure { body, .. } => walk_stmts(body, f)?,
        EK::ArrayLit(items) => {
            for a in items {
                walk(a, f)?;
            }
        }
        EK::ArrayFill(b, _) => walk(b, f)?,
        EK::SliceOf(b, lo, hi) => {
            walk(b, f)?;
            for a in lo.iter_mut().chain(hi.iter_mut()) {
                walk(a, f)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_marker(e: &Expr) -> bool {
    matches!(&e.k, EK::Call(n, a) if n == MARKER && a.len() == 1)
}

fn has_marker(funcs: &[Func]) -> bool {
    for f in funcs {
        if f.pos.line >= PRELUDE_LINE {
            continue;
        }
        let mut body = f.body.clone();
        let mut found = false;
        let _ = walk_stmts(&mut body, &mut |e| {
            if is_marker(e) {
                found = true;
            }
            Ok(found)
        });
        if found {
            return true;
        }
    }
    false
}

/// Evaluate every `comptime` marker of the checked program and replace it by its value.
pub fn run(prog: &mut Program) -> Result<(), Vec<Diag>> {
    if !has_marker(&prog.funcs) {
        return Ok(());
    }
    let snapshot: Vec<Func> = prog.funcs.clone();
    let table: HashMap<String, &Func> = snapshot.iter().map(|f| (f.name.clone(), f)).collect();
    let mut errs: Vec<Diag> = vec![];
    for f in prog.funcs.iter_mut() {
        if f.pos.line >= PRELUDE_LINE {
            continue;
        }
        let mut body = std::mem::take(&mut f.body);
        // `comptime assert(c)` as a statement: checked now, then removed
        remove_asserts(&mut body, &table, &prog.types, &mut errs);
        let r = walk_stmts(&mut body, &mut |x| {
            if !is_marker(x) {
                return Ok(false);
            }
            let mut it = Interp {
                funcs: &table,
                types: &prog.types,
                steps: 0,
                chain: vec![],
            };
            let v = match &x.k {
                EK::Call(_, a) => it.expr(&a[0], &mut HashMap::new()),
                _ => unreachable!(),
            };
            match v {
                Ok(V::I(i)) => x.k = if x.ty == Ty::Char { EK::Char(i) } else { EK::Int(i) },
                Ok(V::B(v)) => x.k = EK::Bool(v),
                Ok(V::A(_)) => errs.push(unsupported("an array as the result of `comptime`", x.pos, &[])),
                Ok(V::U) => errs.push(unsupported("a `comptime` expression with no value (use `comptime assert(..)` as a statement)", x.pos, &[])),
                Err(d) => errs.push(d),
            }
            Ok(true)
        });
        debug_assert!(r.is_ok());
        f.body = body;
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn remove_asserts(
    b: &mut Vec<Stmt>,
    table: &HashMap<String, &Func>,
    types: &Types,
    errs: &mut Vec<Diag>,
) {
    for s in b.iter_mut() {
        let is_assert = matches!(&s.k, SK::ExprS(e) if is_marker(e) && e.ty == Ty::Unit);
        if is_assert {
            if let SK::ExprS(e) = &s.k {
                if let EK::Call(_, a) = &e.k {
                    let mut it = Interp {
                        funcs: table,
                        types,
                        steps: 0,
                        chain: vec![],
                    };
                    if let Err(d) = it.expr(&a[0], &mut HashMap::new()) {
                        errs.push(d);
                    }
                }
            }
            s.k = SK::Block(vec![], false);
            continue;
        }
        match &mut s.k {
            SK::If(_, a, c) => {
                remove_asserts(a, table, types, errs);
                if let Some(c) = c {
                    remove_asserts(c, table, types, errs);
                }
            }
            SK::While(_, body, lp) => {
                remove_asserts(body, table, types, errs);
                remove_asserts(&mut lp.step, table, types, errs);
            }
            SK::Block(body, _) | SK::Unsafe(body) => remove_asserts(body, table, types, errs),
            SK::Match(_, arms) => {
                for a in arms {
                    remove_asserts(&mut a.body, table, types, errs);
                }
            }
            _ => {}
        }
    }
}
