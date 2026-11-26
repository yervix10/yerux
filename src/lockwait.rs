//! No waiting while a lock is held: a closure that runs under a `Locked`, `Mutex` or `RwLock` must not reach a wait
//! or another lock through any chain of calls. The checker rejects the direct cases while it checks the closure; this
//! pass runs on the checked program and follows calls through user and standard library functions.
//!
//! Where the callee is not written at the call, every candidate counts (an over-approximation):
//! - a function value from a field, an element or a result: every function value of the same type in the program;
//! - a function-typed parameter called under the lock: the same;
//! - a trait object method: every implementation the program turns into that trait object;
//! - `send`: waits if the program creates a bounded channel of that element type;
//! - a destructor runs where an owned value that is never moved goes out of scope, or is discarded or overwritten.
//!
//! Not followed: values dropped after a conditional move or taken apart by `match`, and blocking calls that are
//! not built on the primitives below.

use crate::comptime::walk_stmts;
use crate::syntax::{
    diag, Diag, Expr, Func, Mode, Pos, Stmt, Ty, Types, UnpackPat, EK, PRELUDE_LINE, SK,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// `name` is the instantiation of the prelude function `base` (or the function itself).
fn is_inst(name: &str, base: &str) -> bool {
    name.strip_prefix(base)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("__"))
}

/// A method of an instantiated generic prelude type: `Y_Receiver__Int__recv`.
fn is_method(name: &str, ty: &str, m: &str) -> bool {
    name.strip_prefix("Y_")
        .and_then(|r| r.strip_prefix(ty))
        .is_some_and(|r| r.starts_with("__") && r.rsplit("__").next() == Some(m))
}

/// Functions that run their closure argument while holding a lock.
fn takes_lock(name: &str) -> bool {
    [
        "__locked_with",
        "__locked_with_view",
        "__gm__Mutex__with",
        "__gm__Mutex__with_view",
        "mutex_with",
        "mutex_with_view",
        "mutex_update",
        "rwlock_read",
        "rwlock_write",
    ]
    .iter()
    .any(|b| is_inst(name, b))
        || is_method(name, "Mutex", "update")
}

/// The primitives that wait or acquire a lock.
fn waits(name: &str, prelude: bool) -> bool {
    if matches!(
        name,
        "__thread_wait"
            | "__thread_start"
            | "__thread_start_recover"
            | "__futex_park"
            | "std__io__read_line"
            | "std__io__read_file"
            | "std__io__write_file"
            | "std__io__sleep_ms"
            | "std__io__sleep_ns"
            // dropping a child process that was not waited for reaps it
            | "$Drop__Y_std__process__Child__drop"
            | JOIN_ON_DROP
    ) {
        return true;
    }
    prelude
        && (takes_lock(name)
            || [
                "__spawn_owned",
                "__parallel_range",
                "__parallel_list",
                "thread_pool_map",
                "select2",
            ]
            .iter()
            .any(|b| is_inst(name, b))
            || is_method(name, "JoinHandle", "join")
            || is_method(name, "Receiver", "recv")
            || is_method(name, "Receiver", "recv_timeout")
            || is_method(name, "ThreadPool", "parallel_for")
            || is_method(name, "ThreadPool", "close"))
}

/// The callee that stands for the implicit `join` of a dropped `JoinHandle`.
const JOIN_ON_DROP: &str = "__join_on_drop";

/// The elements of a channel a method belongs to: `Y_Sender__Int__send` -> `Int`.
fn sender_of<'a>(name: &'a str, method: &str) -> Option<&'a str> {
    name.strip_prefix("Y_Sender__")?
        .strip_suffix(method)?
        .strip_suffix("__")
}

/// The name as the source would write it.
fn show(name: &str, types: &Types) -> String {
    if name.starts_with("closure__") {
        return "a closure".to_string();
    }
    if name.starts_with("fnvalue:") {
        return "a function value".to_string();
    }
    if let Some(rest) = name.strip_prefix("dyn:") {
        let mut parts = rest.split(':').map(|p| p.parse::<usize>().unwrap_or(0));
        let (object, slot) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        let method = types
            .dyn_tables
            .iter()
            .find(|t| t.object == object)
            .and_then(|t| t.methods.get(slot))
            .map(|m| m.rsplit("__").next().unwrap_or(m))
            .unwrap_or("?");
        return format!("dyn {}.{}", types.dyns[object].0, method);
    }
    if name == JOIN_ON_DROP {
        return "JoinHandle drop (join)".to_string();
    }
    for base in [
        "mutex_with",
        "mutex_with_view",
        "mutex_update",
        "rwlock_read",
        "rwlock_write",
        "select2",
        "thread_pool_map",
    ] {
        if is_inst(name, base) {
            return base.to_string();
        }
    }
    for (base, text) in [
        ("__locked_with", "Locked.with"),
        ("__locked_with_view", "Locked.with_view"),
        ("__spawn_owned", "spawn"),
        ("__parallel_range", "parallel for"),
        ("__parallel_list", "parallel for"),
    ] {
        if is_inst(name, base) {
            return text.to_string();
        }
    }
    // intrinsics keep their name
    if let Some(rest) = name.strip_prefix("__gm__") {
        let mut parts = rest.split("__");
        return format!(
            "{}.{}",
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("")
        );
    }
    if name.starts_with("__") {
        return name.to_string();
    }
    // `Y_Type__Args__method`, `Trait__Y_Type__method`; the type may carry its module (`std__fs__File`)
    let method = name
        .strip_prefix("Y_")
        .or_else(|| name.find("__Y_").map(|i| &name[i + 4..]));
    if let Some(rest) = method {
        let mut owner = vec![];
        for part in rest.split("__") {
            owner.push(part);
            if part.starts_with(|c: char| c.is_ascii_uppercase()) {
                break;
            }
        }
        let m = rest.rsplit("__").next().unwrap_or("");
        return format!("{}.{}", owner.join("."), m);
    }
    if name.starts_with("std__") {
        return name.replace("__", ".");
    }
    name.replace("__", ".")
}

/// A place where a function may run something.
struct Site {
    callee: String,
    /// function values handed over (assumed to run during the call)
    handed: Vec<String>,
    /// for a function that takes a lock: what it runs while holding it
    locked: Vec<String>,
    pos: Pos,
    /// the value whose destructor this is
    dropped: Option<String>,
}

#[derive(Default)]
struct Body {
    sites: Vec<Site>,
    /// uses of function-typed parameters: the caller answers for them, unless this code holds the lock
    params: Vec<Site>,
}

/// What a variable of function type may hold.
#[derive(Clone, Default)]
struct Value {
    known: Vec<String>,
    param: bool,
    stored: bool,
}

impl Value {
    /// Add what `other` may hold; true when that is news.
    fn merge(&mut self, other: &Value) -> bool {
        let mut changed = (other.param && !self.param) || (other.stored && !self.stored);
        self.param |= other.param;
        self.stored |= other.stored;
        for f in &other.known {
            if !self.known.contains(f) {
                self.known.push(f.clone());
                changed = true;
            }
        }
        changed
    }
}

type Values = HashMap<usize, Value>;

/// The value of the variable `id`; one nothing is known about may hold any function.
fn lookup(values: &Values, id: usize) -> Value {
    values.get(&id).cloned().unwrap_or(Value {
        stored: true,
        ..Value::default()
    })
}

enum Source {
    Closure(String),
    Var(usize),
    Other,
}

/// What the statements of a function say about its variables.
#[derive(Default)]
struct Facts {
    binds: Vec<(usize, Source)>,
    /// owned by the function: id, name, type, position
    owned: Vec<(usize, String, Ty, Pos)>,
    /// values dropped on the spot: discarded or overwritten
    discarded: Vec<(Ty, Pos, String)>,
    /// borrows by name: how a closure body names its captures
    borrows: Vec<(String, usize)>,
    moved: HashSet<usize>,
}

fn source(e: &Expr) -> Source {
    match &e.k {
        EK::MkClosure { func, .. } => Source::Closure(func.clone()),
        EK::Var(_, id) | EK::Move(_, id) => Source::Var(*id),
        _ => Source::Other,
    }
}

fn unpacked(pat: &UnpackPat, ty: Ty, pos: Pos, types: &Types, out: &mut Facts) {
    match pat {
        UnpackPat::Bind(name, _, id, _) => {
            if name == "_" {
                out.discarded.push((ty, pos, "_".to_string()));
            } else {
                out.binds.push((*id, Source::Other));
                out.owned.push((*id, name.clone(), ty, pos));
            }
        }
        UnpackPat::Tuple(items, _) => {
            if let Ty::Struct(s) = ty {
                for (item, (_, t)) in items.iter().zip(&types.structs[s].fields) {
                    unpacked(item, *t, pos, types, out);
                }
            }
        }
    }
}

fn facts(body: &[Stmt], types: &Types, out: &mut Facts) {
    for s in body {
        match &s.k {
            SK::Let(name, _, init, id) => {
                out.binds.push((*id, source(init)));
                // hidden variables of the checker hand their value on
                if !name.starts_with('$') {
                    out.owned.push((*id, name.clone(), init.ty, s.pos));
                }
            }
            SK::Assign(target, value) => {
                if let EK::Var(_, id) = target.k {
                    out.binds.push((id, source(value)));
                }
                if let EK::Var(name, _) = &target.k {
                    if !name.starts_with('$') {
                        out.discarded.push((target.ty, s.pos, name.clone()));
                    }
                } else {
                    out.discarded
                        .push((target.ty, s.pos, "the old value".to_string()));
                }
            }
            SK::Bind(name, _, _, id) => out.borrows.push((name.clone(), *id)),
            SK::ExprS(e) => {
                if !matches!(e.k, EK::Var(..) | EK::Field(..) | EK::Move(..)) {
                    out.discarded.push((e.ty, e.pos, "the result".to_string()));
                }
            }
            SK::Return(Some(e)) => {
                if let EK::Var(_, id) = e.k {
                    out.moved.insert(id);
                }
            }
            SK::Unpack(pat, e) => unpacked(pat, e.ty, s.pos, types, out),
            SK::If(_, a, b) => {
                facts(a, types, out);
                if let Some(b) = b {
                    facts(b, types, out);
                }
            }
            SK::While(_, b, lp) => {
                facts(b, types, out);
                facts(&lp.step, types, out);
            }
            SK::Block(b, _) | SK::Unsafe(b) => facts(b, types, out),
            SK::Match(_, arms) => arms.iter().for_each(|a| facts(&a.body, types, out)),
            _ => {}
        }
    }
}

fn collect(func: &mut Func, types: &Types) -> Facts {
    let mut out = Facts::default();
    facts(&func.body, types, &mut out);
    let _ = walk_stmts(&mut func.body, &mut |e: &mut Expr| {
        match &e.k {
            EK::Sequence(stmts, _) => facts(stmts, types, &mut out),
            EK::Move(_, id) => {
                out.moved.insert(*id);
            }
            // list operations that destroy elements
            EK::Method { recv, m, .. } if matches!(m.as_str(), "set" | "clear" | "truncate") => {
                out.discarded
                    .push((recv.ty, e.pos, "an element".to_string()));
            }
            _ => {}
        }
        Ok(false)
    });
    out
}

/// The destructors that run when a value of type `ty` is dropped.
struct Droppers<'a> {
    types: &'a Types,
    /// function type -> the environments of the closures of that type
    envs: HashMap<Ty, Vec<usize>>,
}

impl Droppers<'_> {
    fn of(&self, ty: Ty) -> Vec<String> {
        let mut out = vec![];
        self.walk(ty, &mut HashSet::new(), &mut out);
        out
    }

    fn walk(&self, ty: Ty, seen: &mut HashSet<Ty>, out: &mut Vec<String>) {
        if !seen.insert(ty) {
            return;
        }
        let types = self.types;
        match ty {
            Ty::Struct(i) => {
                if let Some(name) = types.user_drop.get(&i) {
                    out.push(name.clone());
                }
                if types.join_payload(i).is_some() {
                    out.push(JOIN_ON_DROP.to_string());
                }
                for (_, t) in &types.structs[i].fields {
                    self.walk(*t, seen, out);
                }
            }
            Ty::Enum(i) => {
                for (_, payload) in &types.enums[i].variants {
                    payload.iter().for_each(|t| self.walk(*t, seen, out));
                }
            }
            Ty::List(i) if !types.slices.contains(&i) => self.walk(types.lists[i], seen, out),
            Ty::Box(i) => self.walk(types.boxes[i], seen, out),
            Ty::Dyn(i) => {
                for t in types.dyn_tables.iter().filter(|t| t.object == i) {
                    self.walk(t.concrete, seen, out);
                }
            }
            Ty::Fn(_) => {
                for &env in self.envs.get(&ty).into_iter().flatten() {
                    self.walk(Ty::Struct(env), seen, out);
                }
            }
            _ => {}
        }
    }
}

fn fn_value(ty: Ty) -> Option<String> {
    match ty {
        Ty::Fn(i) => Some(format!("fnvalue:{}", i)),
        _ => None,
    }
}

/// The sites of one function. `values` says what its variables of function type may hold.
fn sites(func: &mut Func, facts: &Facts, values: &Values, droppers: &Droppers) -> Body {
    let classify = |a: &Expr| -> Value {
        match &a.k {
            EK::MkClosure { func, .. } => Value {
                known: vec![func.clone()],
                ..Value::default()
            },
            EK::Var(_, id) | EK::Move(_, id) => lookup(values, *id),
            _ => Value {
                stored: true,
                ..Value::default()
            },
        }
    };
    let plain = |callee: String, pos: Pos| Site {
        callee,
        handed: vec![],
        locked: vec![],
        pos,
        dropped: None,
    };
    let mut out = Body::default();
    let _ = walk_stmts(&mut func.body, &mut |e: &mut Expr| {
        let (callee, args): (Option<String>, &[Expr]) = match &e.k {
            EK::Call(name, args) => (Some(name.clone()), args),
            EK::DynCall { slot, args, .. } => match args.first().map(|a| a.ty) {
                Some(Ty::Dyn(object)) => (Some(format!("dyn:{}:{}", object, slot)), args),
                _ => (None, args),
            },
            EK::CallFn(callee, args) => {
                let v = classify(callee);
                if let Some(pseudo) = fn_value(callee.ty) {
                    if v.stored {
                        out.sites.push(plain(pseudo.clone(), e.pos));
                    }
                    if v.param {
                        out.params.push(plain(pseudo, e.pos));
                    }
                }
                for f in v.known {
                    out.sites.push(plain(f, e.pos));
                }
                (None, args)
            }
            _ => return Ok(false),
        };
        let mut handed = vec![];
        let mut locked = vec![];
        for a in args {
            let Some(pseudo) = fn_value(a.ty) else {
                continue;
            };
            let v = classify(a);
            handed.extend(v.known.iter().cloned());
            locked.extend(v.known);
            if v.stored {
                handed.push(pseudo.clone());
            }
            if v.param {
                out.params.push(plain(pseudo.clone(), e.pos));
            }
            if v.stored || v.param {
                locked.push(pseudo);
            }
        }
        match callee {
            Some(callee) => out.sites.push(Site {
                locked: if takes_lock(&callee) { locked } else { vec![] },
                callee,
                handed,
                pos: e.pos,
                dropped: None,
            }),
            // a call of a function value: what it is handed runs during the call
            None => out
                .sites
                .extend(handed.into_iter().map(|f| plain(f, e.pos))),
        }
        Ok(false)
    });
    // destructors
    let mut drops: Vec<(Vec<String>, Pos, String)> = vec![];
    for p in &func.params {
        if p.mode == Mode::Own && !facts.moved.contains(&p.id) {
            drops.push((droppers.of(p.ty), func.pos, p.name.clone()));
        }
    }
    for (id, name, ty, pos) in &facts.owned {
        if !facts.moved.contains(id) {
            drops.push((droppers.of(*ty), *pos, name.clone()));
        }
    }
    for (ty, pos, what) in &facts.discarded {
        drops.push((droppers.of(*ty), *pos, what.clone()));
    }
    for (callees, pos, what) in drops {
        for callee in callees {
            out.sites.push(Site {
                dropped: Some(what.clone()),
                ..plain(callee, pos)
            });
        }
    }
    out
}

/// What the variables of function type of every function may hold, following copies and, into the closures of a
/// function, captures.
fn variable_values(funcs: &mut [Func], found: &[Facts]) -> Vec<Values> {
    let index: HashMap<String, usize> = funcs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    let mut inherited: Vec<BTreeMap<String, Value>> =
        funcs.iter().map(|_| BTreeMap::new()).collect();
    let mut all: Vec<Values> = funcs.iter().map(|_| Values::new()).collect();
    let mut work: VecDeque<usize> = (0..funcs.len()).collect();
    while let Some(i) = work.pop_front() {
        if funcs[i].pos.line >= PRELUDE_LINE {
            continue;
        }
        let mut values = Values::new();
        for p in funcs[i].params.iter().filter(|p| matches!(p.ty, Ty::Fn(_))) {
            values.entry(p.id).or_default().param = true;
        }
        for (name, id) in &found[i].borrows {
            if let Some(v) = inherited[i].get(name) {
                values.entry(*id).or_default().merge(v);
            }
        }
        // copies of copies settle in a few rounds
        for _ in 0..3 {
            for (id, source) in &found[i].binds {
                let value = match source {
                    Source::Closure(f) => Value {
                        known: vec![f.clone()],
                        ..Value::default()
                    },
                    Source::Var(j) => lookup(&values, *j),
                    Source::Other => Value {
                        stored: true,
                        ..Value::default()
                    },
                };
                values.entry(*id).or_default().merge(&value);
            }
        }
        // (closure, captured name, what it holds)
        let mut passed: Vec<(String, String, Value)> = vec![];
        let _ = walk_stmts(&mut funcs[i].body, &mut |e: &mut Expr| {
            if let EK::MkClosure { func, caps, .. } = &e.k {
                for c in caps {
                    let var = match &c.k {
                        EK::AddrOf(v, _) => &v.k,
                        k => k,
                    };
                    if let EK::Var(name, id) | EK::Move(name, id) = var {
                        if let Some(v) = values.get(id) {
                            passed.push((func.clone(), name.clone(), v.clone()));
                        }
                    }
                }
            }
            Ok(false)
        });
        all[i] = values;
        for (closure, name, value) in passed {
            if let Some(&c) = index.get(&closure) {
                if inherited[c].entry(name).or_default().merge(&value) {
                    work.push_back(c);
                }
            }
        }
    }
    all
}

/// The function values the program can pass around, by type, and the environments of its closures.
type Targets = BTreeMap<String, Vec<(String, Pos)>>;

fn closures(funcs: &mut [Func], found: &[Facts]) -> (Targets, HashMap<Ty, Vec<usize>>) {
    let mut made: Vec<(String, String, Pos)> = vec![];
    // closures consumed by the runtime: nothing else can call them
    let mut runtime: HashSet<String> = HashSet::new();
    let mut envs: HashMap<Ty, Vec<usize>> = HashMap::new();
    for (f, facts) in funcs.iter_mut().zip(found) {
        if f.pos.line >= PRELUDE_LINE {
            continue;
        }
        // the hidden variables made for borrowed closure arguments
        let temps: HashMap<usize, &String> = facts
            .binds
            .iter()
            .filter_map(|(id, source)| match source {
                Source::Closure(name) => Some((*id, name)),
                _ => None,
            })
            .collect();
        let _ = walk_stmts(&mut f.body, &mut |e: &mut Expr| {
            match &e.k {
                EK::MkClosure { func, env, .. } => {
                    if let Some(pseudo) = fn_value(e.ty) {
                        made.push((pseudo, func.clone(), e.pos));
                    }
                    if let Some(env) = env {
                        envs.entry(e.ty).or_default().push(*env);
                    }
                }
                EK::Call(name, args)
                    if [
                        "__spawn_owned",
                        "__thread_start",
                        "__thread_start_recover",
                        "__parallel_range",
                        "__parallel_list",
                    ]
                    .iter()
                    .any(|b| is_inst(name, b)) =>
                {
                    for a in args {
                        match &a.k {
                            EK::MkClosure { func, .. } => {
                                runtime.insert(func.clone());
                            }
                            EK::Var(_, id) | EK::Move(_, id) => {
                                runtime.extend(temps.get(id).map(|n| n.to_string()));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            Ok(false)
        });
    }
    let mut values = Targets::new();
    for (ty, func, pos) in made {
        if !runtime.contains(&func) {
            values.entry(ty).or_default().push((func, pos));
        }
    }
    (values, envs)
}

/// Errors for everything inside a lock closure that may wait.
pub fn check(funcs: &mut [Func], types: &Types) -> Vec<Diag> {
    // the prelude's own bodies are not followed: its primitives are listed in `waits`
    let found: Vec<Facts> = funcs
        .iter_mut()
        .map(|f| {
            if f.pos.line >= PRELUDE_LINE {
                Facts::default()
            } else {
                collect(f, types)
            }
        })
        .collect();
    let (values, envs) = closures(funcs, &found);
    let droppers = Droppers { types, envs };
    let held = variable_values(funcs, &found);
    let mut names: Vec<String> = funcs.iter().map(|f| f.name.clone()).collect();
    let mut bodies: Vec<Body> = funcs
        .iter_mut()
        .enumerate()
        .map(|(i, f)| {
            if f.pos.line >= PRELUDE_LINE {
                Body::default()
            } else {
                sites(f, &found[i], &held[i], &droppers)
            }
        })
        .collect();
    // stand-ins for "some function of this type" and "some implementation of this method"
    let mut stand_in = |name: String, targets: Vec<(String, Pos)>| {
        names.push(name);
        bodies.push(Body {
            sites: targets
                .into_iter()
                .map(|(callee, pos)| Site {
                    callee,
                    handed: vec![],
                    locked: vec![],
                    pos,
                    dropped: None,
                })
                .collect(),
            params: vec![],
        });
    };
    for (name, targets) in values {
        stand_in(name, targets);
    }
    let mut methods = Targets::new();
    for t in &types.dyn_tables {
        for (slot, method) in t.methods.iter().enumerate() {
            methods
                .entry(format!("dyn:{}:{}", t.object, slot))
                .or_default()
                .push((method.clone(), types.dyns[t.object].1));
        }
    }
    for (name, targets) in methods {
        stand_in(name, targets);
    }
    let index: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    // element types the program makes bounded channels of: their `send` waits while the queue is full
    let bounded: HashSet<&str> = bodies
        .iter()
        .flat_map(|b| &b.sites)
        .filter_map(|s| sender_of(&s.callee, "bounded"))
        .collect();
    let primitive = |name: &str| {
        let prelude = index
            .get(name)
            .is_some_and(|&i| i < funcs.len() && funcs[i].pos.line >= PRELUDE_LINE);
        waits(name, prelude) || sender_of(name, "send").is_some_and(|t| bounded.contains(t))
    };
    // a lock closure is not a way out of waiting: the function that holds the lock waits for it
    let mut callers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, body) in bodies.iter().enumerate() {
        for s in &body.sites {
            for target in std::iter::once(&s.callee).chain(&s.handed) {
                callers.entry(target.as_str()).or_default().push(i);
            }
        }
    }
    // `next[f]` is the first step of a shortest chain from `f` to a primitive
    let mut next: HashMap<&str, &str> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for body in &bodies {
        for s in &body.sites {
            for target in std::iter::once(&s.callee).chain(&s.handed) {
                if primitive(target) && !queue.contains(&target.as_str()) {
                    queue.push_back(target);
                }
            }
        }
    }
    while let Some(target) = queue.pop_front() {
        for &i in callers.get(target).into_iter().flatten() {
            let name = names[i].as_str();
            if !next.contains_key(name) && !primitive(name) {
                next.insert(name, target);
                queue.push_back(name);
            }
        }
    }
    let may_wait = |name: &str| primitive(name) || next.contains_key(name);
    let error = |site: &Site, first: &str| {
        let mut chain = vec![show(first, types)];
        let mut at = first;
        while let Some(&n) = next.get(at) {
            chain.push(show(n, types));
            at = n;
        }
        let mut d = diag(
            "cannot wait or acquire a lock inside a with closure",
            site.pos,
        );
        if let Some(what) = &site.dropped {
            d = d.note(format!(
                "'{}' is dropped here and its destructor may wait: {}",
                what,
                chain.join(" -> ")
            ));
        } else if first.starts_with("fnvalue:") {
            d = d.note(format!(
                "the function called here is not known; one of the same type may wait: {}",
                chain[1..].join(" -> ")
            ));
        } else if first.starts_with("closure__") && chain.len() > 1 {
            d = d.note(format!(
                "the closure used here may wait: {}",
                chain[1..].join(" -> ")
            ));
        } else if sender_of(first, "send").is_some() {
            d = d.note(
                "the program creates a bounded channel of this type: `send` waits while it is full",
            );
        } else if chain.len() > 1 {
            d = d.note(format!("'{}' may wait: {}", chain[0], chain.join(" -> ")));
        }
        d.note("why: waiting while a lock is held can deadlock every thread that needs the lock")
            .note("help: copy what you need out of the closure, return from `with`, then call it")
    };
    let first_waiting = |s: &Site| {
        std::iter::once(&s.callee)
            .chain(&s.handed)
            .find(|t| may_wait(t))
            .cloned()
    };
    let mut errs = vec![];
    for body in &bodies {
        for lock in &body.sites {
            for target in &lock.locked {
                if target.starts_with("fnvalue:") {
                    if may_wait(target) {
                        errs.push(error(lock, target));
                    }
                    continue;
                }
                let Some(&b) = index.get(target.as_str()) else {
                    continue;
                };
                for s in &bodies[b].sites {
                    if let Some(first) = first_waiting(s) {
                        errs.push(error(s, &first));
                    }
                }
                // parameters called under the lock, also from the closures nested in it
                let mut inside = vec![b];
                let mut k = 0;
                while k < inside.len() {
                    let x = inside[k];
                    k += 1;
                    for s in &bodies[x].params {
                        if may_wait(&s.callee) {
                            errs.push(error(s, &s.callee));
                        }
                    }
                    for t in bodies[x].sites.iter().flat_map(|s| &s.handed) {
                        if let Some(&n) = index.get(t.as_str()) {
                            if t.starts_with("closure__") && !inside.contains(&n) {
                                inside.push(n);
                            }
                        }
                    }
                }
            }
        }
    }
    errs
}
