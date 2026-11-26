//! No waiting while a lock is held: a closure passed to `with` (`Locked`, `Mutex`) must not reach a wait or another
//! lock through any chain of calls. The checker rejects the direct cases while it checks the closure; this pass runs
//! on the checked program and follows calls through user and standard library functions.
//!
//! Not followed: calls through stored function values and trait objects, destructors, `send` on a bounded channel
//! and `RwLock` closures.

use crate::comptime::walk_stmts;
use crate::syntax::{diag, Diag, Expr, Func, Pos, Stmt, EK, PRELUDE_LINE, SK};
use std::collections::{HashMap, VecDeque};

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

/// The name as the source would write it.
fn show(name: &str) -> String {
    if name.starts_with("closure__") {
        return "a closure".to_string();
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
    // `Y_Type__Args__method`, `Trait__Y_Type__method`
    let method = name
        .strip_prefix("Y_")
        .map(|r| (r, r))
        .or_else(|| name.find("__Y_").map(|i| (&name[i + 4..], &name[i + 4..])));
    if let Some((ty, rest)) = method {
        return format!(
            "{}.{}",
            ty.split("__").next().unwrap_or(""),
            rest.rsplit("__").next().unwrap_or("")
        );
    }
    if name.starts_with("std__") {
        return name.replace("__", ".");
    }
    name.replace("__", ".")
}

/// Closure values bound to a variable: id -> (lifted function, hidden variable made for an argument).
type Bound = HashMap<usize, (String, bool)>;

fn bound_closures(body: &[Stmt], out: &mut Bound) {
    for s in body {
        match &s.k {
            SK::Let(name, _, e, id) => {
                if let EK::MkClosure { func, .. } = &e.k {
                    out.insert(*id, (func.clone(), name.starts_with("fn__")));
                }
            }
            SK::If(_, a, b) => {
                bound_closures(a, out);
                if let Some(b) = b {
                    bound_closures(b, out);
                }
            }
            SK::While(_, b, lp) => {
                bound_closures(b, out);
                bound_closures(&lp.step, out);
            }
            SK::Block(b, _) | SK::Unsafe(b) => bound_closures(b, out),
            SK::Match(_, arms) => arms.iter().for_each(|a| bound_closures(&a.body, out)),
            _ => {}
        }
    }
}

/// The calls a function makes: the callee, and the closures it hands over (assumed to run during the call).
fn calls(func: &mut Func) -> Vec<(String, Vec<String>, Pos)> {
    let mut bound = Bound::new();
    bound_closures(&func.body, &mut bound);
    let mut out = vec![];
    let _ = walk_stmts(&mut func.body, &mut |e: &mut Expr| {
        if let EK::Sequence(stmts, _) = &e.k {
            bound_closures(stmts, &mut bound);
        }
        // a closure literal lent to a call; stored closures count only where they are called
        let closure = |a: &Expr| match &a.k {
            EK::MkClosure { func, .. } => Some(func.clone()),
            EK::Var(_, id) | EK::Move(_, id) => bound.get(id).filter(|b| b.1).map(|b| b.0.clone()),
            _ => None,
        };
        match &e.k {
            EK::Call(name, args) => out.push((
                name.clone(),
                args.iter().filter_map(closure).collect(),
                e.pos,
            )),
            EK::CallFn(callee, args) => {
                let name = match &callee.k {
                    EK::MkClosure { func, .. } => Some(func.clone()),
                    EK::Var(_, id) => bound.get(id).map(|b| b.0.clone()),
                    _ => None,
                };
                if let Some(name) = name {
                    out.push((name, args.iter().filter_map(closure).collect(), e.pos));
                }
            }
            _ => {}
        }
        Ok(false)
    });
    out
}

/// Errors for every call inside a `with` closure that may wait.
pub fn check(funcs: &mut [Func]) -> Vec<Diag> {
    let index: HashMap<String, usize> = funcs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();
    // the prelude's own bodies are not followed: its primitives are listed in `waits`
    let bodies: Vec<_> = funcs
        .iter_mut()
        .map(|f| {
            if f.pos.line >= PRELUDE_LINE {
                vec![]
            } else {
                calls(f)
            }
        })
        .collect();
    let primitive = |name: &str| {
        let prelude = index
            .get(name)
            .is_some_and(|&i| funcs[i].pos.line >= PRELUDE_LINE);
        waits(name, prelude)
    };
    // a lock closure is not a way out of waiting: the function that holds the lock waits for it
    let mut callers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, list) in bodies.iter().enumerate() {
        for (callee, closures, _) in list {
            for target in std::iter::once(callee).chain(closures) {
                callers.entry(target.as_str()).or_default().push(i);
            }
        }
    }
    // `next[f]` is the first step of a shortest chain from `f` to a primitive
    let mut next: HashMap<String, String> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for list in &bodies {
        for (callee, closures, _) in list {
            for target in std::iter::once(callee).chain(closures) {
                if primitive(target) && !queue.contains(target) {
                    queue.push_back(target.clone());
                }
            }
        }
    }
    while let Some(target) = queue.pop_front() {
        for &i in callers.get(target.as_str()).into_iter().flatten() {
            let name = &funcs[i].name;
            if !next.contains_key(name) && !primitive(name) {
                next.insert(name.clone(), target.clone());
                queue.push_back(name.clone());
            }
        }
    }
    let may_wait = |name: &str| primitive(name) || next.contains_key(name);
    let mut errs = vec![];
    for list in &bodies {
        for (callee, closures, _) in list {
            if !takes_lock(callee) {
                continue;
            }
            for body in closures {
                let Some(&b) = index.get(body) else { continue };
                for (inner, handed, pos) in &bodies[b] {
                    let Some(first) = std::iter::once(inner).chain(handed).find(|t| may_wait(t))
                    else {
                        continue;
                    };
                    let mut chain = vec![show(first)];
                    let mut at = first;
                    while let Some(n) = next.get(at.as_str()) {
                        chain.push(show(n));
                        at = n;
                    }
                    let mut d = diag("cannot wait or acquire a lock inside a with closure", *pos);
                    if chain.len() > 1 {
                        d = d.note(format!("'{}' may wait: {}", chain[0], chain.join(" -> ")));
                    }
                    errs.push(
                        d.note("why: waiting while a lock is held can deadlock every thread that needs the lock")
                            .note("help: copy what you need out of the closure, return from `with`, then call it"),
                    );
                }
            }
        }
    }
    errs
}
