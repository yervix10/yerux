// Yerux checker: types + ownership.
//
// Ownership rules (no lifetimes):
//  1. Int, Bool, Text and enums are copied. String, List<T>, and any struct holding one, have a single owner.
//  2. Handing over ownership must be written explicitly: `move x`.
//  3. Borrows (`view`/`mut`) only exist as function parameters: they cannot be
//     stored, returned or put in structs => lifetimes are not needed.
//  4. In a single call, a variable lent as `mut` (or moved) cannot appear twice.
//  5. You cannot move a field out of a struct (borrow it instead). The same goes for list elements:
//     `l.get(i)` copies only Copy elements; `l.at(i)` is a *place* that borrows the element for the
//     duration of a call (`l.at(i).len()`, `f(l.at(i))`), and `l.pop()` moves the last one out.
//  6. String arguments of `push_str`/`String.from`/... are borrowed (no `move`), and may not alias the
//     string being modified.
use crate::layout::GLOBAL_CELLS;
use crate::numeric::{int_range, is_float, is_int, is_num};
use crate::syntax::*;
use std::collections::{HashMap, HashSet};

#[path = "check_fn.rs"]
mod fnv;
#[path = "check_num.rs"]
mod num;
#[path = "check_range.rs"]
mod rng;

#[derive(Clone)]
pub struct Sig {
    pub params: Vec<(Mode, Ty)>,
    pub ret: Ty,
}
pub type Sigs = HashMap<String, Sig>;

struct VarInfo {
    name: String,
    ty: Ty,
    mode: Mode,
    mutable: bool,
}

#[derive(Clone, Default)]
struct State {
    moved: HashMap<usize, Pos>,
    dead: bool,
    /// Flow facts: the interval an immutable integer variable is known to lie in at this point
    /// (established by dominating `if` / `while` conditions and by its initialiser).
    ivl: HashMap<usize, (i64, i64)>,
}

fn merge(a: State, b: State) -> State {
    if a.dead {
        return b;
    }
    if b.dead {
        return a;
    }
    let mut m = a.moved;
    for (k, v) in b.moved {
        m.entry(k).or_insert(v);
    }
    // a fact survives a join only when both paths know it (the interval hull)
    let mut ivl = HashMap::new();
    for (k, (alo, ahi)) in a.ivl {
        if let Some((blo, bhi)) = b.ivl.get(&k) {
            ivl.insert(k, (alo.min(*blo), ahi.max(*bhi)));
        }
    }
    State {
        moved: m,
        dead: false,
        ivl,
    }
}

fn always_returns(b: &[Stmt]) -> bool {
    match b.last() {
        Some(Stmt {
            k: SK::Return(_), ..
        }) => true,
        // a call that never returns
        Some(Stmt {
            k: SK::ExprS(Expr {
                k: EK::Call(n, _), ..
            }),
            ..
        }) if matches!(
            n.as_str(),
            "panic" | "std__panic_owned" | "todo" | "unreachable"
        ) =>
        {
            true
        }
        Some(Stmt {
            k: SK::If(_, t, Some(e)),
            ..
        }) => always_returns(t) && always_returns(e),
        Some(Stmt {
            k: SK::Match(_, arms),
            ..
        }) => !arms.is_empty() && arms.iter().all(|a| always_returns(&a.body)),
        Some(Stmt {
            k: SK::Block(_, exits),
            ..
        }) => *exits,
        Some(Stmt {
            k: SK::While(_, _, meta),
            ..
        }) => meta.no_fallthrough,
        _ => false,
    }
}

/// Every variable id an expression mentions.
fn expr_ids(e: &Expr, out: &mut Vec<usize>) {
    walk_ids(e, out, false)
}

/// Ids of the variables `e` moves (`move x` anywhere inside it).
fn expr_moves(e: &Expr, out: &mut Vec<usize>) {
    walk_ids(e, out, true)
}

fn walk_ids(e: &Expr, out: &mut Vec<usize>, only_moves: bool) {
    match &e.k {
        EK::Var(_, id) => {
            if !only_moves {
                out.push(*id)
            }
        }
        EK::Move(_, id) => out.push(*id),
        EK::Ascribe(b) | EK::AsElse(b, _) => walk_ids(b, out, only_moves),
        EK::Sequence(stmts, result) => {
            for stmt in stmts {
                if let SK::Let(_, _, init, _) = &stmt.k {
                    walk_ids(init, out, only_moves);
                }
            }
            walk_ids(result, out, only_moves);
        }
        EK::Field(b, _) | EK::Un(_, b) => walk_ids(b, out, only_moves),
        EK::Bin(_, a, b) => {
            walk_ids(a, out, only_moves);
            walk_ids(b, out, only_moves);
        }
        EK::DynBox { value, .. } => walk_ids(value, out, only_moves),
        EK::DynCall { args, .. }
        | EK::Call(_, args)
        | EK::Static { args, .. }
        | EK::Variant { args, .. } => args.iter().for_each(|a| walk_ids(a, out, only_moves)),
        EK::Method { recv, args, .. } => {
            walk_ids(recv, out, only_moves);
            args.iter().for_each(|a| walk_ids(a, out, only_moves));
        }
        EK::StructLit { fields, .. } | EK::GenStruct { fields, .. } => fields
            .iter()
            .for_each(|(_, a)| walk_ids(a, out, only_moves)),
        EK::GenVariant { args, .. } => args.iter().for_each(|a| walk_ids(a, out, only_moves)),
        EK::Conv(b, _) => walk_ids(b, out, only_moves),
        EK::IsVariant(b, _) | EK::Payload(b, _, _) => walk_ids(b, out, only_moves),
        EK::Int(_) | EK::Float(..) | EK::Char(_) | EK::Bool(_) | EK::Text(_) => {}
        EK::MkClosure { caps, .. } => caps.iter().for_each(|a| walk_ids(a, out, only_moves)),
        EK::CallFn(c, args) => {
            walk_ids(c, out, only_moves);
            args.iter().for_each(|a| walk_ids(a, out, only_moves));
        }
        EK::AddrOf(b, _) | EK::Deref(b) => walk_ids(b, out, only_moves),
        EK::ArrayLit(items) => items.iter().for_each(|a| walk_ids(a, out, only_moves)),
        EK::ArrayFill(b, _) => walk_ids(b, out, only_moves),
        EK::SliceOf(b, lo, hi) => {
            walk_ids(b, out, only_moves);
            lo.iter()
                .chain(hi)
                .for_each(|a| walk_ids(a, out, only_moves));
        }
        EK::Closure { .. } => {}
    }
}

/// Whether evaluating this expression can propagate a failure out of the function.
fn can_propagate(e: &Expr) -> bool {
    match &e.k {
        EK::Sequence(stmts, result) => {
            stmts.iter().any(|s| match &s.k {
                SK::If(..) => true,
                SK::Let(_, _, init, _) => can_propagate(init),
                _ => false,
            }) || can_propagate(result)
        }
        EK::Field(b, _)
        | EK::Un(_, b)
        | EK::Conv(b, _)
        | EK::IsVariant(b, _)
        | EK::Payload(b, ..) => can_propagate(b),
        EK::Bin(_, a, b) => can_propagate(a) || can_propagate(b),
        EK::DynBox { value, .. } => can_propagate(value),
        EK::DynCall { args, .. }
        | EK::Call(_, args)
        | EK::Static { args, .. }
        | EK::Variant { args, .. } => args.iter().any(can_propagate),
        EK::Method { recv, args, .. } => can_propagate(recv) || args.iter().any(can_propagate),
        EK::StructLit { fields, .. } => fields.iter().any(|(_, value)| can_propagate(value)),
        EK::ArrayLit(items) => items.iter().any(can_propagate),
        EK::ArrayFill(b, _) => can_propagate(b),
        EK::SliceOf(b, lo, hi) => can_propagate(b) || lo.iter().chain(hi).any(|a| can_propagate(a)),
        _ => false,
    }
}

fn place_str(e: &Expr) -> String {
    match &e.k {
        EK::Var(n, _) => n.clone(),
        EK::Field(b, f) => format!("{}.{}", place_str(b), f),
        EK::Method { recv, m, .. } if m == "at" => format!("{}.at(..)", place_str(recv)),
        EK::Method { recv, m, args } if m == "get" && args.is_empty() => {
            format!("*{}", place_str(recv))
        }
        EK::Payload(b, ..) => place_str(b),
        _ => "value".to_string(),
    }
}

fn is_at(e: &Expr) -> bool {
    matches!(&e.k, EK::Method { m, .. } if m == "at")
}
/// `b.get()` / `*b`: the value inside a Box, a place.
fn is_deref(e: &Expr) -> bool {
    matches!(&e.k, EK::Method { m, args, .. } if m == "get" && args.is_empty())
}
fn is_place(e: &Expr) -> bool {
    matches!(e.k, EK::Var(..) | EK::Field(..)) || is_at(e)
}
// ── constructors for the synthetic code that `for` loops are rewritten into ──
fn sx(k: EK, pos: Pos) -> Expr {
    Expr {
        k,
        pos,
        ty: Ty::Unit,
    }
}
fn s_var(n: &str, pos: Pos) -> Expr {
    sx(EK::Var(n.to_string(), 0), pos)
}
fn s_int(n: i64, pos: Pos) -> Expr {
    sx(EK::Int(n), pos)
}
fn s_bin(op: &'static str, a: Expr, b: Expr, pos: Pos) -> Expr {
    sx(EK::Bin(op, Box::new(a), Box::new(b)), pos)
}
fn s_meth(recv: Expr, m: &str, args: Vec<Expr>, pos: Pos) -> Expr {
    sx(
        EK::Method {
            recv: Box::new(recv),
            m: m.to_string(),
            args,
        },
        pos,
    )
}
fn s_stmt(k: SK, pos: Pos) -> Stmt {
    Stmt { k, pos }
}
fn s_let(n: &str, mutable: bool, init: Expr, pos: Pos) -> Stmt {
    s_stmt(SK::Let(n.to_string(), mutable, init, 0), pos)
}
fn s_inc(n: &str, by: Expr, pos: Pos) -> Stmt {
    s_stmt(
        SK::Assign(s_var(n, pos), s_bin("+", s_var(n, pos), by, pos)),
        pos,
    )
}
fn s_while(
    c: Expr,
    body: Vec<Stmt>,
    step: Vec<Stmt>,
    label: Option<(String, Pos)>,
    freeze: Option<(Expr, bool)>,
    pos: Pos,
) -> Stmt {
    s_stmt(
        SK::While(
            c,
            body,
            Loop {
                label,
                id: 0,
                step,
                no_fallthrough: false,
                freeze,
            },
        ),
        pos,
    )
}
/// A place that can be evaluated again and again without effects (`l`, `s.items`, `grid.at(i)`).
fn simple_place(e: &Expr) -> bool {
    match &e.k {
        EK::Var(..) => true,
        EK::Field(b, _) => simple_place(b),
        EK::Method { recv, m, args } if m == "at" => {
            simple_place(recv) && args.len() == 1 && matches!(args[0].k, EK::Var(..) | EK::Int(_))
        }
        EK::Method { recv, m, args } if m == "get" && args.is_empty() => simple_place(recv),
        EK::Payload(b, ..) => simple_place(b),
        _ => false,
    }
}

/// (root variable, path of field names; "[]" stands for any list element)
fn path(e: &Expr) -> (usize, Vec<String>) {
    match &e.k {
        EK::Var(_, id) => (*id, vec![]),
        EK::Field(b, f) => {
            let (r, mut p) = path(b);
            p.push(f.clone());
            (r, p)
        }
        EK::Method { recv, .. } => {
            let (r, mut p) = path(recv);
            p.push("[]".into());
            (r, p)
        }
        _ => (usize::MAX, vec![]),
    }
}
/// The hidden variable a payload place is rooted in (`$m3` for `Payload($m3, ..)`), if any.
fn hidden_root(e: &Expr) -> Option<String> {
    match &e.k {
        EK::Var(n, _) if n.starts_with("$m") => Some(n.clone()),
        EK::Payload(b, _, _) | EK::Field(b, _) => hidden_root(b),
        _ => None,
    }
}
fn overlap(a: &Expr, b: &Expr) -> bool {
    let ((ra, pa), (rb, pb)) = (path(a), path(b));
    ra == rb && (pa.starts_with(&pb) || pb.starts_with(&pa))
}

#[derive(Clone, Copy)]
struct PlaceInfo {
    root: usize,
    ty: Ty,
    mutable: bool,
    root_mode: Mode,
}

/// A resolved `match` pattern (types checked, names of variants turned into indexes).
#[derive(Clone)]
enum RP {
    Wild,
    Bind(String, Pos, Option<Box<RP>>),
    /// inclusive bounds, either end open; true for Char bounds (a single literal has lo == hi)
    Range(Option<i64>, Option<i64>, bool),
    Bool(bool),
    Variant(usize, Vec<RP>),
    Tuple(Vec<RP>),
    Or(Vec<RP>),
}

fn pat_has_bind(p: &Pat) -> bool {
    match p {
        Pat::Bind(..) => true,
        Pat::Variant(_, s, _) | Pat::Tuple(s, _) | Pat::Or(s, _) => s.iter().any(pat_has_bind),
        _ => false,
    }
}

/// Names bound by a pattern (an or-pattern contributes the names of its first alternative).
fn pat_bound_names(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Bind(n, sub, _) => {
            out.push(n.clone());
            if let Some(s) = sub {
                pat_bound_names(s, out);
            }
        }
        Pat::Variant(_, s, _) | Pat::Tuple(s, _) => s.iter().for_each(|q| pat_bound_names(q, out)),
        Pat::Or(alts, _) => {
            if let Some(a) = alts.first() {
                pat_bound_names(a, out);
            }
        }
        _ => {}
    }
}

/// First or-pattern (at any depth) whose alternatives bind different names:
/// (position, names of the first alternative, names of the offending one).
fn pat_or_mismatch(p: &Pat) -> Option<(Pos, Vec<String>, Vec<String>)> {
    match p {
        Pat::Bind(_, Some(s), _) => pat_or_mismatch(s),
        Pat::Variant(_, s, _) | Pat::Tuple(s, _) => s.iter().find_map(pat_or_mismatch),
        Pat::Or(alts, pos) => {
            if let Some(m) = alts.iter().find_map(pat_or_mismatch) {
                return Some(m);
            }
            let names = |a: &Pat| {
                let mut v = vec![];
                pat_bound_names(a, &mut v);
                v.sort();
                v
            };
            let first = names(alts.first()?);
            alts.iter()
                .skip(1)
                .map(names)
                .find(|n| *n != first)
                .map(|n| (*pos, first, n))
        }
        _ => None,
    }
}

/// Or-patterns that bind names are split into one pattern per alternative (the arm is repeated).
fn pat_expand(p: &Pat) -> Vec<Pat> {
    match p {
        Pat::Or(alts, _) if alts.iter().any(pat_has_bind) => {
            alts.iter().flat_map(pat_expand).collect()
        }
        Pat::Variant(n, subs, pos) => cross(subs)
            .into_iter()
            .map(|v| Pat::Variant(n.clone(), v, *pos))
            .collect(),
        Pat::Tuple(subs, pos) => cross(subs)
            .into_iter()
            .map(|v| Pat::Tuple(v, *pos))
            .collect(),
        Pat::Or(alts, pos) => {
            let _ = alts;
            vec![Pat::Or(alts.clone(), *pos)]
        }
        Pat::Bind(n, Some(sub), pos) => pat_expand(sub)
            .into_iter()
            .map(|e| Pat::Bind(n.clone(), Some(Box::new(e)), *pos))
            .collect(),
        _ => vec![p.clone()],
    }
}
fn cross(subs: &[Pat]) -> Vec<Vec<Pat>> {
    let mut acc: Vec<Vec<Pat>> = vec![vec![]];
    for s in subs {
        let ex = pat_expand(s);
        let mut next = vec![];
        for a in &acc {
            for e in &ex {
                let mut v = a.clone();
                v.push(e.clone());
                next.push(v);
            }
        }
        acc = next;
    }
    acc
}

/// Pattern without binding names, with or-patterns in the head position spread over several rows.
fn rp_strip(p: &RP) -> RP {
    match p {
        RP::Bind(_, _, Some(sub)) => rp_strip(sub),
        RP::Bind(_, _, None) => RP::Wild,
        RP::Variant(v, s) => RP::Variant(*v, s.iter().map(rp_strip).collect()),
        RP::Tuple(s) => RP::Tuple(s.iter().map(rp_strip).collect()),
        RP::Or(s) => RP::Or(s.iter().map(rp_strip).collect()),
        o => o.clone(),
    }
}
fn rp_rows(row: &[RP], out: &mut Vec<Vec<RP>>) {
    if let Some(RP::Or(alts)) = row.first() {
        for a in alts {
            let mut r = vec![a.clone()];
            r.extend_from_slice(&row[1..]);
            rp_rows(&r, out);
        }
    } else {
        out.push(row.to_vec());
    }
}

struct Checker<'a> {
    types: Types,
    sigs: Sigs,
    extern_names: HashSet<String>,
    /// C variadic externs (`...`): calls may pass extra scalar arguments after the fixed ones.
    variadic_externs: HashSet<String>,
    unsafe_depth: usize,
    vars: Vec<VarInfo>,
    scopes: Vec<HashMap<String, usize>>,
    st: State,
    errs: Vec<Diag>,
    /// `for i in a..b`: the interval of the loop variable, keyed by the hidden counter's name
    loop_ivl: HashMap<String, (i128, i128)>,
    /// the interval of each variable's initialiser as typed (before the variable's own declared type applies)
    init_ivl: HashMap<usize, (i128, i128)>,
    /// variables whose range type was written by the programmer (parameters, annotated lets)
    explicit_range: HashSet<usize>,
    div_contract: bool,
    /// names of variables that flow facts must never be kept about (see `stable`)
    unstable: HashSet<String>,
    /// Id-based audit of the name-based scans: variable ids that the checker saw being lent as `mut`,
    /// captured by reference for writing or `match mut`ed (`ev_escape`), or assigned (`ev_assign`). After a
    /// function is checked, a variable whose name is missing from the scan sets makes the function be
    /// checked again with that name added (see `func`).
    ev_escape: Vec<usize>,
    ev_assign: Vec<usize>,
    /// when false the scanner of `may_change` does not count plain assignments (it then collects `unstable`)
    scan_assign: std::cell::Cell<bool>,
    /// a hidden copy of a variable (the scrutinee of a `match`) -> the variable: facts about one hold for the other
    alias: HashMap<usize, usize>,
    /// parameter modes of lazily loaded library functions (for `may_change`), and the callees it could not resolve
    lazy_modes: HashMap<String, Vec<Mode>>,
    unknown_calls: std::cell::RefCell<HashSet<String>>,
    ret: Ty,
    worker_panics: bool,
    // generics
    ps: &'a mut Parser,
    in_generic: bool, // checking a generic body with abstract type parameters
    queue: Vec<(Func, String)>, // instantiations still to check (with a description for notes)
    inst_fns: HashMap<(String, Vec<Ty>), (String, Sig)>,
    hint: Option<Ty>, // expected type of the next expression (to infer `Maybe.None`)
    rename: Option<String>, // set by a generic call: the instantiation to call instead
    n_inst: usize,
    broken: HashSet<String>, // generic functions whose own body has errors (not instantiated)
    dscopes: Vec<Vec<Stmt>>, // deferred statements per open scope (parallel to `scopes`)
    pre: Vec<Stmt>, // statements to insert before the statement just checked (return expansion)
    ptr_op: Option<(&'static str, Option<i64>)>, // `Ptr<T>` operation to rewrite the call just checked into (name, element size)
    arg_temps: Vec<Stmt>, // temporaries lent to `view` parameters: the enclosing call expression owns them
    reported: HashSet<(usize, usize, usize)>,
    impl_done: HashSet<String>, // instantiated methods of generic (built-in) impls
    pos_vars: HashMap<(usize, usize), usize>, // the variable read at a position (for range diagnostics)
    /// variables that had a range fact which an assignment or a loop dropped: why, and the line
    lost_facts: HashMap<usize, (&'static str, usize)>,
    pos_names: HashMap<(usize, usize), String>, // source text of operands (for "write x.to_float()" help)
    loops: Vec<LoopCtx>,
    next_loop: usize,
    hid: usize,
    in_defer: bool,
    /// places that the loops being checked iterate over: (place, exclusive, line of the loop)
    frozen: Vec<(Expr, bool, usize, bool)>,
    /// hidden `$m` variable of a borrowing `match` -> (scrutinee place, exclusive, line)
    match_scruts: HashMap<String, (Expr, bool, usize)>,
    // function values and closures (check_fn.rs)
    lifted: Vec<Func>, // closure bodies and thunks, emitted as ordinary functions
    thunks: HashSet<String>, // function items already given a thunk
    nclos: usize,
    unit_ret: bool,
    closure_depth: usize,
    capture_ids: HashSet<usize>, // variables that are captures inside a closure body
    closure_borrow_pos: HashSet<(usize, usize)>, // positions of closure literals that borrow variables
    ret_free: bool,        // checking a closure whose result type is not known yet
    free_pids: Vec<usize>, // type parameters of the generic callee that are still unknown
    closure_loans: HashMap<usize, Vec<(usize, u8, Pos)>>, // hidden closure variable -> what it borrows
}

/// How a `match` looks at its scrutinee.
#[derive(Clone, Copy, PartialEq)]
enum MK {
    /// a copyable value: payloads are copied into the bindings
    Copy,
    /// `match e` / `match mut e` on a place with owned payloads: bindings are second-class borrows
    Borrow(Mode),
    /// `match move e` or a temporary: the value is consumed, bindings own their payloads
    Move,
}

/// An enclosing `while`/`for` loop while its body is being checked.
struct LoopCtx {
    label: Option<String>,
    id: usize,
    dscope_depth: usize,
    breaks: Vec<State>,
    conts: Vec<State>,
}

impl<'a> Checker<'a> {
    fn err(&mut self, d: Diag) {
        self.errs.push(d);
    }
    fn tn(&self, t: Ty) -> String {
        self.types.name(t)
    }
    fn lookup(&self, n: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(n).copied())
    }
    fn declare(&mut self, name: &str, ty: Ty, mode: Mode, mutable: bool) -> usize {
        let id = self.vars.len();
        self.vars.push(VarInfo {
            name: name.to_string(),
            ty,
            mode,
            mutable,
        });
        self.scopes.last_mut().unwrap().insert(name.to_string(), id);
        id
    }
    fn expect(&mut self, got: Ty, want: Ty, pos: Pos) {
        if got == Ty::Err || want == Ty::Err || got == want {
            return;
        }
        if matches!(got, Ty::Range(..)) && matches!(want, Ty::Int | Ty::Range(..)) {
            let (glo, ghi) = int_range(got);
            let (wlo, whi) = int_range(want);
            if wlo <= glo && ghi <= whi {
                return;
            }
        }
        if matches!(want, Ty::Range(..)) && matches!(got, Ty::Int | Ty::Range(..)) {
            self.range_mismatch(got, want, pos);
            return;
        }
        // a range value that fails for another reason is just an Int in the message
        let got = if matches!(got, Ty::Range(..)) {
            Ty::Int
        } else {
            got
        };
        let (g, w) = (self.tn(got), self.tn(want));
        let mut d = diag(format!("expected {} but this is {}", w, g), pos);
        if is_num(got) && is_num(want) {
            d = self.num_mismatch_notes(d, got, want, pos);
        }
        for t in [got, want] {
            if let Ty::Param(i) = t {
                let b = &self.types.params[i].bounds;
                let have = if b.is_empty() {
                    "nothing about it".to_string()
                } else {
                    format!("only {}", b.join(" + "))
                };
                d = d.note(format!(
                    "{} is a type parameter: inside a generic function the compiler knows {} (a bound promises operations)",
                    self.types.params[i].name, have
                ));
                d = d.note(format!("help: add a bound that provides what you need, e.g. `{}: Ordered`, `{}: Equal` or `{}: Show`", self.types.params[i].name, self.types.params[i].name, self.types.params[i].name));
                break;
            }
        }
        self.err(d);
    }
    fn unknown(&mut self, name: &str, pos: Pos) {
        self.err(diag(format!("variable '{}' does not exist", name), pos));
    }
    fn check_moved(&mut self, id: usize, pos: Pos) {
        if let Some(mp) = self.st.moved.get(&id).copied() {
            let name = self.vars[id].name.clone();
            self.err(
                diag(format!("cannot use '{}': it was already moved", name), pos)
                    .note(format!("'{}' was moved on line {}", name, mp.line))
                    .note("help: if the function only needs to read it, declare that parameter as `view` and it won't be moved"),
            );
            self.st.moved.remove(&id);
        }
    }
    fn immutable_reason(&self, mode: Mode) -> &'static str {
        if mode == Mode::View {
            "it is only borrowed for reading (`view`)"
        } else {
            "it was declared with `let` (immutable)"
        }
    }

    // Resolve a place expression (variable or field chain).
    fn place_inner(&mut self, e: &mut Expr, check_moved: bool) -> Option<PlaceInfo> {
        let pos = e.pos;
        match &mut e.k {
            EK::Var(name, id) => match self.lookup(name) {
                None => {
                    self.unknown(name, pos);
                    e.ty = Ty::Err;
                    None
                }
                Some(i) => {
                    *id = i;
                    if check_moved {
                        self.check_moved(i, pos);
                    }
                    let v = &self.vars[i];
                    let pi = PlaceInfo {
                        root: i,
                        ty: v.ty,
                        mutable: v.mutable,
                        root_mode: v.mode,
                    };
                    e.ty = v.ty;
                    Some(pi)
                }
            },
            EK::Field(base, f) => {
                let b = if self.isp(base) {
                    self.place_inner(base, check_moved)?
                } else {
                    // a field of a temporary (`pair().0`): fine while the temporary owns no memory
                    let t = self.expr(base);
                    if t == Ty::Err {
                        return None;
                    }
                    if self.types.owned(t) {
                        let n = self.tn(t);
                        if let EK::Move(v, _) = &base.k {
                            // `move t.s` parses as `(move t).s`: what was meant is moving the field out
                            let what = format!("{}.{}", v, f);
                            self.err(
                                diag(format!("cannot move '{}' out of '{}': a field cannot be moved out of an owned {}", what, v, n), pos)
                                    .note("`move` takes a whole variable; moving one field would leave the struct half freed")
                                    .note(format!("help: borrow the field instead (pass '{}' as `view`/`mut`, or call a method on it), or move all of '{}'", what, v)),
                            );
                            return None;
                        }
                        self.err(
                            diag(format!("cannot take a field of a temporary {} that owns memory", n), pos)
                                .note("the rest of the value would be thrown away without being freed")
                                .note("help: store it first: `let (a, b) = move value`, or `let v = value` and then borrow its fields"),
                        );
                        return None;
                    }
                    PlaceInfo {
                        root: usize::MAX,
                        ty: t,
                        mutable: false,
                        root_mode: Mode::Own,
                    }
                };
                let fty = match b.ty {
                    Ty::Struct(s) if pos.line < PRELUDE_LINE && self.types.opaque_struct(s) => {
                        self.err(diag(
                            format!("the fields of {} are private", self.tn(b.ty)),
                            pos,
                        ));
                        return None;
                    }
                    Ty::Struct(s) => self.types.structs[s]
                        .fields
                        .iter()
                        .find(|(n, _)| n == f)
                        .map(|x| x.1),
                    Ty::Err => return None,
                    t => {
                        let n = self.tn(t);
                        self.err(diag(format!("type {} has no fields", n), pos));
                        return None;
                    }
                };
                match fty {
                    None => {
                        let n = self.tn(b.ty);
                        if let (Some(es), Some(k)) = (self.tuple_elems(b.ty), f.strip_prefix('_')) {
                            self.err(
                                diag(format!("the tuple {} has no element .{}", n, k), pos).note(
                                    format!(
                                        "it has {} element(s): .0 to .{}",
                                        es.len(),
                                        es.len() - 1
                                    ),
                                ),
                            );
                        } else {
                            self.err(diag(format!("struct {} has no field '{}'", n, f), pos));
                        }
                        None
                    }
                    Some(t) => {
                        e.ty = t;
                        Some(PlaceInfo { ty: t, ..b })
                    }
                }
            }
            EK::Method { recv, m, args } if m == "at" => {
                let b = self.place_inner(recv, check_moved)?;
                let elem = match b.ty {
                    Ty::List(i) => self.types.lists[i],
                    t if self.types.array_of(t).is_some() => {
                        let (elem, n) = self.types.array_of(t).unwrap();
                        if args.len() == 1 {
                            let t = self.expr(&mut args[0]);
                            self.expect(t, Ty::Int, args[0].pos);
                            self.const_index(&args[0], n, b.ty);
                        } else {
                            self.err(diag("'at' expects 1 argument(s)", pos));
                        }
                        e.ty = elem;
                        return Some(PlaceInfo { ty: elem, ..b });
                    }
                    Ty::Err => return None,
                    t => {
                        let n = self.tn(t);
                        self.err(diag(format!("type {} has no method 'at'", n), pos));
                        return None;
                    }
                };
                if args.len() != 1 {
                    self.err(diag("'at' expects 1 argument(s)", pos));
                } else {
                    let t = self.expr(&mut args[0]);
                    self.expect(t, Ty::Int, args[0].pos);
                }
                e.ty = elem;
                Some(PlaceInfo { ty: elem, ..b })
            }
            EK::Method { recv, m, args }
                if pos.line >= PRELUDE_LINE && m == "locked_value" && args.is_empty() =>
            {
                let b = self.place_inner(recv, check_moved)?;
                let ty = self.types.locked_payload(b.ty)?;
                e.ty = ty;
                Some(PlaceInfo {
                    ty,
                    mutable: true,
                    ..b
                })
            }
            EK::Method { recv, m, args } if m == "get" && args.is_empty() => {
                let b = self.place_inner(recv, check_moved)?;
                if let Some(payload) = self.types.shared_payload(b.ty) {
                    e.ty = payload;
                    return Some(PlaceInfo {
                        ty: payload,
                        mutable: false,
                        root_mode: Mode::View,
                        ..b
                    });
                }
                let inner = match b.ty {
                    Ty::Box(i) => self.types.boxes[i],
                    Ty::Err => return None,
                    t => {
                        let n = self.tn(t);
                        self.err(
                            diag(
                                format!("only a Box can be dereferenced, this is {}", n),
                                pos,
                            )
                            .note("help: `*b` and `b.get()` give the value inside a `Box<T>`"),
                        );
                        return None;
                    }
                };
                e.ty = inner;
                Some(PlaceInfo { ty: inner, ..b })
            }
            // payload of an enum place (created by the checker for `match` bindings)
            EK::Payload(base, vid, k) => {
                let b = self.place_inner(base, check_moved)?;
                match b.ty {
                    Ty::Enum(eid) => {
                        let t = self.types.enums[eid].variants[*vid].1[*k];
                        e.ty = t;
                        Some(PlaceInfo { ty: t, ..b })
                    }
                    _ => None,
                }
            }
            _ => {
                self.err(diag("expected a variable or a field here", pos));
                None
            }
        }
    }
    fn place(&mut self, e: &mut Expr) -> Option<PlaceInfo> {
        let r = self.place_inner(e, true);
        if r.is_some() {
            self.frozen_check(e, false, e.pos);
        }
        r
    }

    /// A list or string that a `for` loop is iterating must not change while the loop borrows it (and
    /// with `for mut x in l` it must not even be read: `x` is the only way in).
    fn frozen_check(&mut self, e: &Expr, write: bool, pos: Pos) {
        let hit = self
            .frozen
            .iter()
            .find(|(fz, excl, _, _)| (*excl || write) && overlap(fz, e))
            .map(|(fz, excl, line, m)| (place_str(fz), *excl, *line, *m));
        if let Some((fname, excl, line, from_match)) = hit {
            let n = place_str(e);
            let d = if from_match && excl {
                diag(format!("cannot use '{}' inside the `match mut` arm that borrows it", n), pos)
                    .note(format!("`match mut` on line {} borrows '{}' exclusively: the names the pattern binds are the only way to reach its payloads", line, fname))
                    .note("help: use the bound names inside the arm, or change/read the value after the `match`")
            } else if from_match {
                diag(format!("cannot change '{}' while a `match` arm is borrowing its payloads", n), pos)
                    .note(format!("the `match` on line {} lends the payloads of '{}' to the names its pattern binds; they would dangle if it changed or moved", line, fname))
                    .note("help: do the change after the `match`, or use `match move` to take the payloads out")
            } else if excl {
                diag(format!("cannot use '{}' inside the loop that is changing its elements in place", n), pos)
                    .note(format!("`for mut ..` on line {} borrows '{}' exclusively until the loop ends", line, fname))
                    .note("help: use the loop variable to read or change the current element, or loop over indexes: `for i in 0..n { .. l.at(i) .. }`")
            } else {
                diag(format!("cannot change '{}' while a loop is reading it", n), pos)
                    .note(format!("the `for` loop on line {} borrows '{}' until the loop ends", line, fname))
                    .note("help: collect what you want to change and apply it after the loop, or loop over indexes: `for i in 0..n { .. l.at(i) .. }`")
            };
            self.err(d);
        }
    }

    fn resolve_ids(&self, e: &mut Expr) {
        match &mut e.k {
            EK::Var(n, id) => {
                if let Some(i) = self.lookup(n) {
                    *id = i;
                }
            }
            EK::Field(b, _) => self.resolve_ids(b),
            EK::Method { recv, args, .. } => {
                self.resolve_ids(recv);
                for a in args {
                    self.resolve_ids(a);
                }
            }
            _ => {}
        }
    }

    fn expr(&mut self, e: &mut Expr) -> Ty {
        let mark = self.arg_temps.len();
        let t = self.expr_checked(e);
        if self.arg_temps.len() > mark {
            // temporaries lent to `view` parameters of this call live exactly as long as the call expression
            let temps = self.arg_temps.split_off(mark);
            let k = std::mem::replace(&mut e.k, EK::Int(0));
            e.k = EK::Sequence(
                temps,
                Box::new(Expr {
                    k,
                    pos: e.pos,
                    ty: e.ty,
                }),
            );
        }
        t
    }

    fn expr_checked(&mut self, e: &mut Expr) -> Ty {
        let pos = e.pos;
        // a `const` (not shadowed by a local): its folded value
        if let EK::Var(n, _) = &e.k {
            if self.lookup(n).is_none() {
                if let Some(&(v, t)) = self.types.consts.get(n) {
                    e.k = match t {
                        Ty::Bool => EK::Bool(v != 0),
                        Ty::Char => EK::Char(v),
                        _ => EK::Int(v),
                    };
                    e.ty = t;
                    return t;
                }
            }
        }
        let hint = self.hint.take();
        // `let x: T = e`
        if matches!(e.k, EK::Ascribe(_)) {
            let want = e.ty;
            let EK::Ascribe(mut inner) = std::mem::replace(&mut e.k, EK::Int(0)) else {
                unreachable!()
            };
            self.hint = Some(want);
            let t = self.expr(&mut inner);
            self.expect(t, want, inner.pos);
            *e = *inner;
            return if t == Ty::Err { Ty::Err } else { want };
        }
        // `x as Int<lo..hi> else { .. }`
        if matches!(e.k, EK::AsElse(..)) {
            let t = self.range_as_else(e);
            e.ty = t;
            return t;
        }
        // `unchecked(x)`: an integer taken as the range the context asks for, without a test
        if matches!(&e.k, EK::Call(n, a) if n == "unchecked" && a.len() == 1)
            && !self.sigs.contains_key("unchecked")
            && !self.ps.gfns.iter().any(|g| g.name == "unchecked")
        {
            let t = self.range_unchecked(e, hint);
            e.ty = t;
            return t;
        }
        // `Int<lo..hi>.from(x)` / `.try_from(x)`
        if matches!(
            &e.k,
            EK::Static {
                ty: Ty::Range(..),
                ..
            }
        ) {
            let t = self.range_static(e);
            e.ty = t;
            return t;
        }
        // `Some(x)`, `Ok(x)`, `Err(e)`, `None`: the variants of Option / Result, usable without the enum's name
        match &mut e.k {
            EK::Call(n, args)
                if matches!(n.as_str(), "Some" | "Ok" | "Err")
                    && !self.sigs.contains_key(n.as_str())
                    && !self.ps.gfns.iter().any(|g| g.name == *n) =>
            {
                let vname = n.clone();
                let args = std::mem::take(args);
                let name = if vname == "Some" { "Option" } else { "Result" }.to_string();
                e.k = EK::GenVariant {
                    name,
                    targs: None,
                    vname,
                    args,
                };
            }
            EK::Var(n, _) if n == "None" && self.lookup("None").is_none() => {
                e.k = EK::GenVariant {
                    name: "Option".into(),
                    targs: None,
                    vname: "None".into(),
                    args: vec![],
                };
            }
            _ => {}
        }
        // `f()?`
        if matches!(&e.k, EK::Un("?", _)) {
            self.hint = hint;
            let t = self.try_op(e);
            e.ty = t;
            return t;
        }
        // numbers: literals, conversions, math functions (see check_num.rs)
        if let Some(t) = self.try_numeric(e, hint) {
            e.ty = t;
            return t;
        }
        // function values: a closure literal, a function name used as a value, a call of a variable of function type
        if matches!(e.k, EK::Closure { .. }) {
            let t = self.closure_expr(e, hint, false, false, false);
            e.ty = t;
            return t;
        }
        if let EK::Var(n, _) = &e.k {
            if self.lookup(n).is_none() {
                let n = n.clone();
                if self.lazy_known(&n) || self.ps.gfns.iter().any(|g| g.name == n) {
                    return match self.fn_item(&n, pos) {
                        Some((t, k)) => {
                            e.k = k;
                            e.ty = t;
                            t
                        }
                        None => {
                            e.ty = Ty::Err;
                            Ty::Err
                        }
                    };
                }
            }
        }
        if let EK::Call(n, _) = &e.k {
            if let Some(id) = self.lookup(n) {
                if matches!(self.vars[id].ty, Ty::Fn(_) | Ty::Err) {
                    if let EK::Call(n, args) = std::mem::replace(&mut e.k, EK::Int(0)) {
                        e.k = EK::CallFn(
                            Box::new(Expr {
                                k: EK::Var(n, id),
                                pos,
                                ty: Ty::Unit,
                            }),
                            args,
                        );
                    }
                }
            }
        }
        if let EK::Method { recv, m, .. } = &e.k {
            let rt = self.peek_ty(recv);
            let direct_lock = rt.and_then(|t| self.types.locked_payload(t)).is_some();
            let shared_lock = rt
                .and_then(|t| self.types.shared_payload(t))
                .and_then(|t| self.types.locked_payload(t))
                .is_some();
            if (direct_lock || shared_lock) && matches!(m.as_str(), "with" | "with_view") {
                let EK::Method { recv, m, args } = std::mem::replace(&mut e.k, EK::Int(0)) else {
                    unreachable!()
                };
                let recv = if shared_lock {
                    s_meth(*recv, "get", vec![], pos)
                } else {
                    *recv
                };
                let mut all = vec![recv];
                all.extend(args);
                e.k = EK::Call(format!("__locked_{}", m), all);
            }
        }
        if let EK::Method { recv, m, .. } = &mut e.k {
            if !matches!(m.as_str(), "clone" | "get")
                && self
                    .peek_ty(recv)
                    .and_then(|t| self.types.shared_payload(t))
                    .is_some()
            {
                let original = std::mem::replace(recv, Box::new(s_var("$shared", pos)));
                **recv = s_meth(*original, "get", vec![], pos);
            }
        }
        // `s.cb(args)` where `cb` is a field of function type: call the stored function value
        if let EK::Method { recv, m, .. } = &e.k {
            if m != "at" && is_place(recv) {
                if let Some(Ty::Struct(sid)) = self.peek_ty(recv) {
                    if self.types.structs[sid]
                        .fields
                        .iter()
                        .any(|(n, t)| n == m && matches!(t, Ty::Fn(_)))
                    {
                        if let EK::Method { recv, m, args } =
                            std::mem::replace(&mut e.k, EK::Int(0))
                        {
                            let fpos = recv.pos;
                            e.k = EK::CallFn(
                                Box::new(Expr {
                                    k: EK::Field(recv, m),
                                    pos: fpos,
                                    ty: Ty::Unit,
                                }),
                                args,
                            );
                        }
                    }
                }
            }
        }
        if let EK::Method { recv, m, .. } = &e.k {
            if m != "at" && !self.isp(recv) {
                return self.temporary_method(e, hint);
            }
        }
        if let EK::Method { recv, m, .. } = &mut e.k {
            let rt = self.peek_ty(recv);
            let dyn_ty = match rt {
                Some(Ty::Box(i))
                    if m != "get" && m != "take" && matches!(self.types.boxes[i], Ty::Dyn(_)) =>
                {
                    let pos = recv.pos;
                    let inner = std::mem::replace(recv, Box::new(s_var("$dyn", pos)));
                    **recv = sx(
                        EK::Method {
                            recv: inner,
                            m: "get".into(),
                            args: vec![],
                        },
                        pos,
                    );
                    Some(self.types.boxes[i])
                }
                _ => rt,
            };
            if let Some(Ty::Dyn(i)) = dyn_ty {
                return self.dyn_call(e, i);
            }
        }
        if let EK::Static { ty, f, args } = &mut e.k {
            if *ty == Ty::Err
                && matches!(hint, Some(Ty::Box(i)) if matches!(self.types.boxes[i], Ty::Dyn(_)))
            {
                *ty = hint.unwrap();
            }
            if let Ty::Box(i) = *ty {
                if let Ty::Dyn(d) = self.types.boxes[i] {
                    if f == "new" && args.len() == 1 {
                        return self.dyn_box(e, d);
                    }
                }
            }
        }
        // Keep an owned replacement in a normal parameter until the bounds check succeeds.
        if self.worker_panics && pos.line < PRELUDE_LINE {
            if let EK::Method { recv, m, args } = &e.k {
                if m == "set" && args.len() == 2 && self.isp(recv) {
                    if let Some(Ty::List(i)) = self.peek_ty(recv) {
                        if self.types.owned(self.types.lists[i]) {
                            let EK::Method { recv, args, .. } =
                                std::mem::replace(&mut e.k, EK::Int(0))
                            else {
                                unreachable!()
                            };
                            let mut all = vec![*recv];
                            all.extend(args);
                            e.k = EK::Call("__worker_list_set".into(), all);
                        }
                    }
                }
            }
        }
        // arrays and slices: `==`, `<`, .., and `show()` go through the generic slice helpers of prelude_fmt.yx
        self.seq_rewrite(e);
        // `x.m(..)` on a user type / built-in trait becomes a plain call `Trait__Type__m(x, ..)`
        if let EK::Method { recv, m, .. } = &e.k {
            if m != "at" && self.isp(recv) {
                if let Some(rt) = self.peek_ty(recv) {
                    let builtin = match rt {
                        // a slice has only the built-in element methods: the library ones may grow a List
                        Ty::List(_) if self.types.slice_elem(rt).is_some() => true,
                        Ty::Struct(_) if self.types.array_of(rt).is_some() => true,
                        Ty::List(_) => ["push", "pop", "len", "get", "set", "clear", "swap"]
                            .contains(&m.as_str()),
                        Ty::String => [
                            "len",
                            "push_str",
                            "push_int",
                            "push_char",
                            "eq",
                            "byte",
                            "char_at",
                            "char_size",
                            "clear",
                            "push_byte",
                            "push_slice",
                            "raw_addr",
                            "push_raw",
                        ]
                        .contains(&m.as_str()),
                        Ty::Char => ["to_int", "utf8_len"].contains(&m.as_str()),
                        Ty::Box(_) => ["get", "take"].contains(&m.as_str()),
                        Ty::Struct(_) if m == "get" && self.types.shared_payload(rt).is_some() => {
                            true
                        }
                        Ty::Struct(_)
                            if m == "locked_value"
                                && pos.line >= PRELUDE_LINE
                                && self.types.locked_payload(rt).is_some() =>
                        {
                            true
                        }
                        Ty::Err => true,
                        _ => false,
                    };
                    // methods of `Int` (trait impls) apply to range values
                    let rt = if matches!(rt, Ty::Range(..)) {
                        Ty::Int
                    } else {
                        rt
                    };
                    if !builtin && self.rewrite_method(e, rt).is_err() {
                        e.ty = Ty::Err;
                        return Ty::Err;
                    }
                }
            }
        }
        // `a + b` on a String variable: a new String (`std__concat`); b is a String variable or a Text
        if let EK::Bin("+", a, _) = &e.k {
            if self.isp(a) && self.peek_ty(a) == Some(Ty::String) {
                let k = std::mem::replace(&mut e.k, EK::Int(0));
                if let EK::Bin(_, a, b) = k {
                    e.k = EK::Call("std__concat".into(), vec![*a, *b]);
                }
            }
        }
        // arithmetic and logic on a type parameter: only what the bounds promise is available
        if let EK::Bin(op, a, b) = &e.k {
            if matches!(*op, "+" | "-" | "*" | "/" | "%" | "&&" | "||") {
                for x in [a, b] {
                    if let Some(Ty::Param(i)) = if self.isp(x) { self.peek_ty(x) } else { None } {
                        let tn = self.types.params[i].name.clone();
                        self.err(
                            diag(format!("cannot use `{}` on a value of type {}: it is a type parameter", op, tn), pos)
                                .note(format!("generic code can only do what the bounds of {} promise, and `{}` is not available through any bound", tn, op))
                                .note("help: take the operation as a function parameter, or make the function work on Int"),
                        );
                        e.ty = Ty::Err;
                        return Ty::Err;
                    }
                }
            }
        }
        // `a == b`, `a < b` on a type that implements Equal / Ordered
        if let EK::Bin(op, a, _) = &e.k {
            // a Text literal on the left (`"abc" < s`) is compared like a Text variable
            let lit = if matches!(a.k, EK::Text(_)) {
                Some(Ty::Text)
            } else {
                None
            };
            if matches!(*op, "==" | "!=" | "<" | ">" | "<=" | ">=")
                && (lit.is_some() || self.isp(a))
            {
                if let Some(t) = lit.or_else(|| self.peek_ty(a)) {
                    let ordering = !matches!(*op, "==" | "!=");
                    if (matches!(
                        t,
                        Ty::Struct(_)
                            | Ty::Enum(_)
                            | Ty::Text
                            | Ty::String
                            | Ty::Param(_)
                            | Ty::List(_)
                    ) || (ordering && t == Ty::Bool))
                        && self.rewrite_op(e, t).is_err()
                    {
                        e.ty = Ty::Err;
                        return Ty::Err;
                    }
                }
            }
        }
        if self.isp(e) {
            let ty = match self.place(e) {
                None => Ty::Err,
                Some(p) if self.types.owned(p.ty) => {
                    let n = place_str(e);
                    let d = if is_at(e) {
                        diag(format!("cannot move '{}' out of its List", n), pos).note(
                            "help: borrow the element instead (`l.at(i).len()`, or pass `l.at(i)` to a `view`/`mut` parameter); `l.pop()` moves the last element out",
                        )
                    } else if is_deref(e) {
                        diag(format!("cannot move '{}' out of its Box", n), pos)
                            .note("help: `b.take()` moves the value out and frees the Box; or borrow it (`b.get().len()`, or pass `b.get()` to a `view`/`mut` parameter)")
                    } else if matches!(e.k, EK::Field(..)) {
                        diag(format!("cannot move '{}' out of its struct", n), pos)
                            .note("help: borrow it instead: call a method on it, or pass it to a `view`/`mut` parameter")
                    } else {
                        let mut d = diag(
                            format!("'{}' is a {} and is not copied", n, self.tn(p.ty)),
                            pos,
                        );
                        if let Ty::Param(i) = p.ty {
                            d = d.note(format!(
                                "{} could be any type, including an owned one (String, List), so it is moved, not copied",
                                self.types.params[i].name
                            ));
                        }
                        if p.root_mode != Mode::Own {
                            d = d.note(format!("'{}' only borrows its value (a `view`/`mut` parameter or a `for` loop variable), so it cannot be copied or moved out", n));
                            if let Some(nt) = self.fn_borrow_note(p.ty) {
                                d = d.note(nt);
                            }
                            if p.ty == Ty::String {
                                d = d.note(format!(
                                    "help: make an independent copy with `String.from({})`",
                                    n
                                ));
                            }
                            d
                        } else {
                            d.note(format!(
                                "help: use `move {}` to hand it over, or pass it to a function that takes it as `view`/`mut`{}",
                                n,
                                if matches!(p.ty, Ty::Param(_)) { "; or declare the parameter `T: Copy` to allow copies" } else { "" }
                            ))
                        }
                    };
                    self.err(d);
                    Ty::Err
                }
                Some(p) => p.ty,
            };
            let ty = self.narrowed(e, ty);
            e.ty = ty;
            if is_num(ty) {
                self.pos_names.insert((pos.line, pos.col), place_str(e));
                if let EK::Var(_, id) = &e.k {
                    self.pos_vars.insert((pos.line, pos.col), *id);
                }
            }
            return ty;
        }
        let ty = match &mut e.k {
            EK::Var(..) | EK::Field(..) => unreachable!(),
            EK::Ascribe(..) => unreachable!("handled at the top of expr"),
            EK::AsElse(..) => unreachable!("handled at the top of expr"),
            EK::Int(_) | EK::Float(..) | EK::Conv(..) => unreachable!("handled by try_numeric"),
            EK::IsVariant(b, _) => {
                self.expr_base(b);
                Ty::Bool
            }
            EK::Payload(b, vid, k) => {
                let t = self.expr_base(b);
                match t {
                    Ty::Enum(eid) => self.types.enums[eid].variants[*vid].1[*k],
                    _ => Ty::Err,
                }
            }
            EK::Char(_) => Ty::Char,
            EK::Bool(_) => Ty::Bool,
            EK::Text(_) if hint == Some(Ty::String) => {
                // a Text literal where a String is expected: `let s: String = "hi"` is `String.from("hi")`
                let lit = std::mem::replace(
                    e,
                    Expr {
                        k: EK::Int(0),
                        pos,
                        ty: Ty::Err,
                    },
                );
                e.k = EK::Static {
                    ty: Ty::String,
                    f: "from".into(),
                    args: vec![lit],
                };
                e.ty = Ty::String;
                return self.expr(e);
            }
            EK::Text(_) => Ty::Text,
            EK::Move(name, id) => match self.lookup(name) {
                None => {
                    self.unknown(name, pos);
                    Ty::Err
                }
                Some(i) => {
                    *id = i;
                    self.check_moved(i, pos);
                    let (vty, vmode) = (self.vars[i].ty, self.vars[i].mode);
                    if !self.types.owned(vty) {
                        vty
                    } else if vmode != Mode::Own && self.capture_ids.contains(&i) {
                        let d = diag(format!("cannot move '{}' out of the closure that captured it", name), pos)
                            .note("a closure can be called many times, and after the first call the captured value would be gone")
                            .note("help: use it by reference inside the closure (`s.len()`, `s.push_str(..)`), or create the value you hand over inside the closure (`String.from(..)`)");
                        self.err(d);
                        vty
                    } else if vmode != Mode::Own {
                        let mut d =
                            diag(format!("cannot move '{}': it is only borrowed", name), pos).note(
                                "help: whoever lent it is still the owner; only move what you own",
                            );
                        if let Some(nt) = self.fn_borrow_note(vty) {
                            d = d.note(nt);
                        }
                        self.err(d);
                        vty
                    } else {
                        let probe = sx(EK::Var(name.clone(), i), pos);
                        self.st.moved.insert(i, pos);
                        self.frozen_check(&probe, true, pos);
                        vty
                    }
                }
            },
            EK::Un(op, x) => {
                if *op == "!" {
                    let t = self.expr(x);
                    self.expect(t, Ty::Bool, x.pos);
                    Ty::Bool
                } else {
                    self.hint = hint;
                    let t = self.expr(x);
                    self.neg_ty(t, x.pos)
                }
            }
            EK::Bin(op, a, b) => {
                let op = *op;
                let (ta, tb) = if matches!(op, "&&" | "||") {
                    let ta = self.expr(a);
                    let before = self.st.clone();
                    // `x >= 0 && f(x)`: the right side is only evaluated when the left one held (for `||`: failed)
                    let facts = self.facts_of(a, op == "&&");
                    self.apply_facts(&facts);
                    let tb = self.expr(b);
                    self.st = merge(before, std::mem::take(&mut self.st));
                    (ta, tb)
                } else {
                    self.operands(a, b, hint)
                };
                match op {
                    "+" | "-" | "*" | "/" | "%" | "+%" | "-%" | "*%" => {
                        if ta == Ty::Char || tb == Ty::Char {
                            let at = if ta == Ty::Char { a.pos } else { b.pos };
                            self.err(
                                diag(format!("cannot use `{}` on a Char", op), at)
                                    .note("a Char is a character, not a number, so it has no arithmetic")
                                    .note("help: `c.to_int()` gives its Unicode number and `Char.from_int(n)` turns a number back into a Char"),
                            );
                            Ty::Int
                        } else {
                            // a plain Int divisor that flow facts narrowed to include 0 is still just an Int
                            // (runtime-checked division); only declared range types promise a nonzero divisor
                            // only a divisor whose range type the programmer wrote (a parameter, an annotated
                            // variable, a field) is a contract that must exclude 0; intervals that arithmetic
                            // or flow facts produced leave the ordinary runtime-checked division
                            self.div_contract = matches!(op, "/" | "%")
                                && match &b.k {
                                    EK::Var(_, id) => {
                                        self.explicit_range.contains(id)
                                            && matches!(self.vars[*id].ty, Ty::Range(..))
                                    }
                                    EK::Field(..) => matches!(b.ty, Ty::Range(..)),
                                    _ => false,
                                };
                            let r = self.arith_ty(op, ta, tb, a.pos, b.pos);
                            self.div_contract = false;
                            r
                        }
                    }
                    "<" | ">" | "<=" | ">=" => {
                        if ta == Ty::Char {
                            self.expect(tb, Ty::Char, b.pos);
                        } else if !matches!(ta, Ty::Err | Ty::Int | Ty::Range(..)) && !is_num(ta) {
                            // a non-numeric value that is not a variable (`mk() < x`, `String.from(..) < s`):
                            // only places are rewritten to an `Ordered` call above
                            let n = self.tn(ta);
                            let d = if self.implements(ta, "Ordered") {
                                diag(format!("cannot use `{}` here: the left side is a {} value that is not a variable", op, n), a.pos)
                                    .note(format!("`{}` on {} calls `cmp`, which borrows its left side", op, n))
                                    .note("help: store the value with `let` first, then compare the variable")
                            } else {
                                diag(format!("cannot use `{}` on values of type {}: it does not implement Ordered", op, n), a.pos)
                                    .note("help: `<`, `>`, `<=` and `>=` work on numbers, Char, and types that implement Ordered")
                            };
                            self.err(d);
                        } else {
                            self.arith_ty("<", ta, tb, a.pos, b.pos);
                        }
                        Ty::Bool
                    }
                    "&&" | "||" => {
                        self.expect(ta, Ty::Bool, a.pos);
                        self.expect(tb, Ty::Bool, b.pos);
                        Ty::Bool
                    }
                    _ => {
                        if matches!(ta, Ty::Int | Ty::Range(..))
                            && matches!(tb, Ty::Int | Ty::Range(..))
                        {
                            // Different integer intervals still have the same i64
                            // representation and support value equality.
                        } else if matches!(ta, Ty::Char | Ty::Bool | Ty::Err) || is_num(ta) {
                            self.expect(tb, ta, b.pos);
                        } else {
                            let n = self.tn(ta);
                            self.err(diag(format!("cannot compare values of type {}", n), a.pos)
                                .note("help: == and != work on numbers and Bool, and on variables whose type implements Equal (Text, String, your own types); use `match` for enums"));
                        }
                        Ty::Bool
                    }
                }
            }
            EK::Call(name, args) => {
                if name == "assert_eq" && args.len() == 2 {
                    // assert_eq(a, b) is assert(a == b), with the operand types checked first
                    let ta = self.expr(&mut args[0]);
                    if is_num(ta) {
                        self.hint = Some(ta);
                    }
                    let tb = self.expr(&mut args[1]);
                    if !matches!(ta, Ty::Bool | Ty::Err) && !is_num(ta) {
                        let n = self.tn(ta);
                        self.err(
                            diag(
                                format!("assert_eq compares numbers or Bool values, not {}", n),
                                args[0].pos,
                            )
                            .note("help: for strings use `assert(a.eq(b))`"),
                        );
                    } else {
                        self.expect(tb, ta, args[1].pos);
                    }
                    let b = args.pop().unwrap();
                    let a = args.pop().unwrap();
                    let cmp = Expr {
                        pos: span(a.pos, b.pos),
                        ty: Ty::Bool,
                        k: EK::Bin("==", Box::new(a), Box::new(b)),
                    };
                    *name = "assert".into();
                    args.push(cmp);
                    Ty::Unit
                } else {
                    let n = name.clone();
                    let t = self.call(&n, args, pos);
                    if let Some(nn) = self.rename.take() {
                        *name = nn;
                    }
                    if n == "print" {
                        self.rewrite_print(name, args);
                    }
                    t
                }
            }
            EK::Method { recv, m, args } => {
                let m = m.clone();
                self.method(recv, &m, args, pos)
            }
            EK::Static { ty, f, args } => {
                let (t0, f) = (*ty, f.clone());
                let r = self.static_call(t0, &f, args, pos);
                if t0 == Ty::Err {
                    *ty = r; // `Box.new(x)`: the Box type comes from the argument
                }
                r
            }
            EK::StructLit { sid, fields } => {
                let sid = *sid;
                self.struct_lit(sid, fields, pos, None)
            }
            EK::ArrayLit(items) => self.array_lit(items, hint, pos),
            EK::ArrayFill(v, n) => {
                let n = *n;
                self.array_fill(v, n, hint, pos)
            }
            EK::SliceOf(..) => {
                self.err(
                    diag("a slice `x[from..to]` can only be passed to a `view [T]` or `mut [T]` parameter", pos)
                        .note("slices are borrows: they cannot be stored in a variable, returned or kept")
                        .note("help: pass it directly to a function that takes `view [T]`, or index the original: `x[i]`"),
                );
                Ty::Err
            }
            EK::Variant {
                eid,
                vname,
                vid,
                args,
            } => {
                let eid = *eid;
                let vname = vname.clone();
                let (t, v) = self.variant(eid, &vname, args, pos, None);
                *vid = v;
                t
            }
            EK::Sequence(..) => e.ty,
            EK::GenStruct { .. } | EK::GenVariant { .. } => {
                let k = std::mem::replace(&mut e.k, EK::Int(0));
                let (t, nk) = self.generic_lit(k, pos, hint);
                e.k = nk;
                t
            }
            EK::CallFn(callee, args) => self.call_value(callee, args, pos),
            EK::Closure { .. } => unreachable!("handled above"),
            EK::DynBox { .. }
            | EK::DynCall { .. }
            | EK::MkClosure { .. }
            | EK::AddrOf(..)
            | EK::Deref(_) => e.ty, // built by the checker itself
        };
        e.ty = ty;
        if let Some((op, size)) = self.ptr_op.take() {
            // `Ptr<T>` construction and management: an intrinsic call the backends know
            let mut args = match std::mem::replace(&mut e.k, EK::Int(0)) {
                EK::Method { recv, args, .. } => {
                    let mut a = vec![*recv];
                    a.extend(args);
                    a
                }
                EK::Static { args, .. } => args,
                k => {
                    e.k = k;
                    return ty;
                }
            };
            if let Some(n) = size {
                args.push(Expr {
                    k: EK::Int(n),
                    pos,
                    ty: Ty::Int,
                });
            }
            e.k = EK::Call(op.into(), args);
            return ty;
        }
        if ty != Ty::Err {
            self.protect_operands(e);
        }
        ty
    }

    /// `Ptr<T>.null()`, `Ptr<T>.alloc(n)`: raw pointers made by the language (no `malloc` declaration needed).
    fn ptr_static(&mut self, ty: Ty, f: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        let Ty::Ptr(pi) = ty else { return Ty::Err };
        let tn = self.tn(ty);
        let nargs = match f {
            "null" => 0,
            "alloc" => 1,
            _ => {
                self.err(
                    diag(format!("{} has no function '{}'", tn, f), pos)
                        .note("functions: null(), alloc(n)"),
                );
                return Ty::Err;
            }
        };
        if args.len() != nargs {
            self.err(diag(
                format!("{}.{}() expects {} argument(s)", tn, f, nargs),
                pos,
            ));
            return ty;
        }
        if f == "null" {
            self.ptr_op = Some(("__ptr_null", None));
            return ty;
        }
        if self.unsafe_depth == 0 {
            self.err(diag(format!("{}.alloc(n) requires an unsafe block", tn), pos)
                .note("help: wrap it in `unsafe { ... }`; the block owns the pointer and must release it with `p.free()`"));
        }
        let t = self.expr(&mut args[0]);
        self.expect(t, Ty::Int, args[0].pos);
        let size = match self.types.ptrs[pi] {
            Ty::Int => 8,
            Ty::Num(k) => (k.bits() / 8) as i64,
            Ty::Ptr(_) => 8,
            Ty::Err => 8,
            other => {
                let n = self.tn(other);
                self.err(
                    diag(
                        format!(
                            "{}.alloc(n) needs a numeric or pointer element type, not {}",
                            tn, n
                        ),
                        pos,
                    )
                    .note("help: use Ptr<UInt8> for a byte buffer and pass its size in bytes"),
                );
                8
            }
        };
        self.ptr_op = Some(("__ptr_alloc", Some(size)));
        ty
    }

    /// Methods of `Ptr<T>`: `p.is_null()` and `p.free()` (null is allowed; only for pointers made by `Ptr<T>.alloc`).
    fn ptr_method(&mut self, m: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        let (op, ret) = match m {
            "is_null" => ("__ptr_is_null", Ty::Bool),
            "free" => ("__ptr_free", Ty::Unit),
            _ => {
                self.err(diag(format!("Ptr has no method '{}'", m), pos).note(
                    "methods: is_null(), free()   (functions: Ptr<T>.null(), Ptr<T>.alloc(n))",
                ));
                return Ty::Err;
            }
        };
        if !args.is_empty() {
            self.err(diag(format!("'{}' expects 0 argument(s)", m), pos));
        }
        if m == "free" && self.unsafe_depth == 0 {
            self.err(diag("p.free() requires an unsafe block", pos)
                .note("help: free only pointers made by `Ptr<T>.alloc(n)`, once; a pointer from C's malloc is released with C's free"));
        }
        self.ptr_op = Some((op, None));
        ret
    }

    /// Give a temporary Option/Result receiver a scope so methods can borrow or consume it.
    fn temporary_method(&mut self, e: &mut Expr, hint: Option<Ty>) -> Ty {
        let EK::Method { recv, m, args } = &mut e.k else {
            unreachable!()
        };
        let outer_pre = std::mem::take(&mut self.pre);
        let t = self.expr(recv);
        let option_or_result = match t {
            Ty::Enum(i) => self.types.enums[i]
                .origin
                .as_ref()
                .is_some_and(|(name, _)| matches!(name.as_str(), "Option" | "Result")),
            _ => false,
        };
        if !option_or_result && !self.types.owned(t) {
            let ty = match t {
                Ty::Char => self.char_method(m, args, e.pos),
                Ty::Err => Ty::Err,
                _ => {
                    self.err(diag("expected a variable or a field here", recv.pos));
                    Ty::Err
                }
            };
            let pre = std::mem::replace(&mut self.pre, outer_pre);
            self.pre.extend(pre);
            e.ty = ty;
            return ty;
        }
        self.scopes.push(HashMap::new());
        self.hid += 1;
        let name = format!("$receiver{}", self.hid);
        let id = self.declare(&name, t, Mode::Own, false);
        let pos = recv.pos;
        let init = std::mem::replace(
            &mut **recv,
            Expr {
                k: EK::Var(name.clone(), id),
                ty: t,
                pos,
            },
        );
        self.pre.push(s_stmt(SK::Let(name, false, init, id), pos));
        self.hint = hint;
        let ty = self.expr(e);
        self.scopes.pop();
        let pre = std::mem::replace(&mut self.pre, outer_pre);
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        e.k = EK::Sequence(pre, Box::new(Expr { k, ..*e }));
        ty
    }

    /// If a later operand can exit the expression early, give earlier owned operands a scoped owner until
    /// all operands have been evaluated. Move them into their destination on success.
    fn protect_operands(&mut self, e: &mut Expr) {
        if self.worker_panics && self.protect_worker_call(e) {
            return;
        }
        let mut protect_panics = self.worker_panics;
        let (mut operands, later_place): (Vec<&mut Expr>, bool) = match &mut e.k {
            EK::DynCall { sig, args, .. } => {
                protect_panics = false;
                self.call_operands(args, &sig.params)
            }
            EK::Call(name, args) => match self.sigs.get(name) {
                Some(sig) => {
                    protect_panics = false;
                    self.call_operands(args, &sig.params)
                }
                None => return,
            },
            EK::Variant { args, .. } => (args.iter_mut().collect(), false),
            EK::StructLit { fields, .. } => {
                (fields.iter_mut().map(|(_, value)| value).collect(), false)
            }
            EK::Method { recv, args, .. } if matches!(recv.ty, Ty::List(_)) => {
                protect_panics = false;
                (args.iter_mut().collect(), can_propagate(recv))
            }
            _ => return,
        };
        let mut owned = false;
        let mut needs_scope = false;
        for arg in &operands {
            let can_panic = protect_panics
                && !matches!(
                    arg.k,
                    EK::Int(_)
                        | EK::Float(..)
                        | EK::Char(_)
                        | EK::Bool(_)
                        | EK::Text(_)
                        | EK::Var(..)
                        | EK::Move(..)
                );
            needs_scope |= owned && (can_panic || can_propagate(arg));
            owned |= self.types.owned(arg.ty);
        }
        needs_scope |= owned && later_place;
        if !needs_scope {
            return;
        }
        let mut stmts = Vec::new();
        for arg in &mut operands {
            stmts.push(self.capture_operand(arg));
        }
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        e.k = EK::Sequence(stmts, Box::new(Expr { k, ..*e }));
    }

    fn protect_worker_call(&mut self, e: &mut Expr) -> bool {
        let (args, params) = match &mut e.k {
            EK::Call(name, args) => match self.sigs.get(name) {
                Some(sig) => (args, sig.params.clone()),
                None => return false,
            },
            EK::DynCall { args, sig, .. } => (args, sig.params.clone()),
            EK::CallFn(callee, args) if matches!(callee.k, EK::Var(..)) => {
                let Ty::Fn(id) = callee.ty else { return false };
                (args, self.types.fns[id].params.clone())
            }
            _ => return false,
        };
        let by_value =
            |mode, ty| mode == Mode::Own || (mode == Mode::View && !self.types.owned(ty));
        let mut values = Vec::new();
        let mut places = Vec::new();
        let mut owned = false;
        let mut needs_scope = false;
        for (arg, (mode, ty)) in args.iter_mut().zip(params) {
            if by_value(mode, ty) {
                needs_scope |= owned
                    && !matches!(
                        arg.k,
                        EK::Int(_)
                            | EK::Float(..)
                            | EK::Char(_)
                            | EK::Bool(_)
                            | EK::Text(_)
                            | EK::Var(..)
                            | EK::Move(..)
                    );
                owned |= self.types.owned(arg.ty);
                values.push(arg);
            } else if !matches!(arg.k, EK::Var(..)) {
                places.push(arg);
            }
        }
        if !needs_scope && !(owned && !places.is_empty()) {
            return false;
        }
        let mut stmts: Vec<_> = values
            .into_iter()
            .map(|arg| self.capture_operand(arg))
            .collect();
        // Evaluate indices before forming any borrowed addresses. This keeps an index
        // expression that changes a list from invalidating another argument's address.
        for place in &mut places {
            self.capture_place_inputs(place, &mut stmts);
        }
        // Check the addresses while the argument temporaries are still live. The final
        // call can form them again without running any user code. In particular, a packed
        // Bool list element retains its ordinary copy-in/copy-out lowering.
        for place in places {
            let pos = place.pos;
            let check = Expr {
                k: EK::AddrOf(Box::new(place.clone()), false),
                pos,
                ty: Ty::Int,
            };
            stmts.push(s_stmt(SK::ExprS(check), pos));
        }
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        e.k = EK::Sequence(stmts, Box::new(Expr { k, ..*e }));
        true
    }

    fn capture_place_inputs(&mut self, place: &mut Expr, stmts: &mut Vec<Stmt>) {
        match &mut place.k {
            EK::Field(base, _) | EK::Payload(base, ..) => self.capture_place_inputs(base, stmts),
            EK::Method { recv, m, args } => {
                self.capture_place_inputs(recv, stmts);
                if m == "at" {
                    stmts.push(self.capture_operand(&mut args[0]));
                }
            }
            EK::Deref(address) => stmts.push(self.capture_operand(address)),
            _ => {}
        }
    }

    fn call_operands<'b>(
        &self,
        args: &'b mut [Expr],
        params: &[(Mode, Ty)],
    ) -> (Vec<&'b mut Expr>, bool) {
        let mut values = Vec::new();
        let mut later_place = false;
        for (arg, (mode, ty)) in args.iter_mut().zip(params) {
            if *mode == Mode::Own || (*mode == Mode::View && !self.types.owned(*ty)) {
                values.push(arg);
            } else {
                later_place |= can_propagate(arg);
            }
        }
        (values, later_place)
    }

    fn capture_operand(&mut self, arg: &mut Expr) -> Stmt {
        self.hid += 1;
        let name = format!("$arg{}", self.hid);
        let ty = arg.ty;
        let pos = arg.pos;
        let id = self.vars.len();
        self.vars.push(VarInfo {
            name: name.clone(),
            ty,
            mode: Mode::Own,
            mutable: false,
        });
        let k = if self.types.owned(ty) {
            EK::Move(name.clone(), id)
        } else {
            EK::Var(name.clone(), id)
        };
        let init = std::mem::replace(arg, Expr { k, pos, ty });
        s_stmt(SK::Let(name, false, init, id), pos)
    }

    /// `operand?`: if it is the error (or `None`), return it from the function; otherwise its value.
    /// Expanded into an expression with its own scope:
    ///   let $q = operand
    ///   if $q is the error { let $e = <its payload>; <forget $q>; return Err(convert($e)) }
    ///   let $v = <the payload of Ok / Some>; <forget $q>        (the expression becomes `$v`)
    fn try_op(&mut self, e: &mut Expr) -> Ty {
        let pos = e.pos;
        let EK::Un(_, inner) = &mut e.k else {
            return Ty::Err;
        };
        if self.in_defer {
            self.err(diag("`?` cannot return from a deferred statement", pos));
            inner.ty = Ty::Err;
            return Ty::Err;
        }
        let outer_pre = std::mem::take(&mut self.pre);
        self.hint = None;
        let t = self.expr(inner);
        let mut out = std::mem::replace(&mut self.pre, outer_pre);
        if t == Ty::Err {
            return Ty::Err;
        }
        let family = |this: &Self, t: Ty| -> Option<(String, Vec<Ty>, usize)> {
            if let Ty::Enum(i) = t {
                if let Some((n, a)) = &this.types.enums[i].origin {
                    if n == "Result" || n == "Option" {
                        return Some((n.clone(), a.clone(), i));
                    }
                }
            }
            None
        };
        let Some((fam, targs, _)) = family(self, t) else {
            let n = self.tn(t);
            self.err(
                diag(format!("`?` needs a Result or an Option, but this is {}", n), pos)
                    .note("`?` unwraps the success value and returns the error (or `None`) from the current function")
                    .note("help: only use it on calls that return `Result<T, E>` or `Option<T>`"),
            );
            return Ty::Err;
        };
        let ret = self.ret;
        let rfam = family(self, ret);
        let rn = self.tn(ret);
        match &rfam {
            None => {
                let actual = if ret == Ty::Unit { "nothing" } else { &rn };
                let expected = if fam == "Result" {
                    "Result<T, E>"
                } else {
                    "Option<T>"
                };
                let failure = if fam == "Result" {
                    "the error"
                } else {
                    "`None`"
                };
                let d = diag(format!("cannot use `?` in a function that returns {}", actual), pos)
                    .note(format!("`?` returns {} early, so the function must return `{}`", failure, expected))
                    .note("help: handle the value with `match` here, or change the function's return type");
                self.err(d);
                return Ty::Err;
            }
            Some((rf, _, _)) if *rf != fam => {
                let mut d = diag(
                    format!(
                        "`?` is used on {} but this function returns {}",
                        self.tn(t),
                        rn
                    ),
                    pos,
                );
                if fam == "Option" {
                    d = d.note("an Option has no error to return from a function that returns a Result")
                        .note("help: turn the missing value into an error first: `ok_or(value, \"what was missing\")?`");
                } else {
                    d = d.note("a Result carries an error that an Option cannot hold")
                        .note("help: drop the error with `.ok()`: `value.ok()?`, or make the function return a Result");
                }
                self.err(d);
                return Ty::Err;
            }
            _ => {}
        }
        let (_, rargs, reid) = rfam.unwrap();
        // the error conversion (Result only)
        let mut conv: Option<String> = None;
        if fam == "Result" && targs[1] != rargs[1] {
            let (e1, e2) = (self.tn(targs[1]), self.tn(rargs[1]));
            let fname = format!("{}_from_{}", snake_case(&e2), snake_case(&e1));
            if !self.sigs.contains_key(&fname) && !self.in_generic {
                self.load_lazy(&fname);
            }
            match self.sigs.get(&fname).cloned() {
                Some(sg)
                    if sg.params.len() == 1
                        && sg.params[0] == (Mode::Own, targs[1])
                        && sg.ret == rargs[1] =>
                {
                    conv = Some(fname)
                }
                found => {
                    let mut d = diag(
                        format!(
                            "`?` cannot turn the error {} into the {} this function returns",
                            e1, e2
                        ),
                        pos,
                    )
                    .note(format!(
                        "the function returns {}, but this value fails with {}",
                        rn, e1
                    ));
                    if found.is_some() {
                        d = d.note(format!(
                            "a function named `{}` exists, but it must be `fn {}(e: {}) -> {}`",
                            fname, fname, e1, e2
                        ));
                    }
                    d = d.note(format!("help: write the conversion: `fn {}(e: {}) -> {} {{ .. }}` and `?` will call it", fname, e1, e2));
                    self.err(d);
                    return Ty::Err;
                }
            }
        }
        // statements
        let inner_e = std::mem::replace(
            &mut **inner,
            Expr {
                k: EK::Int(0),
                pos,
                ty: Ty::Unit,
            },
        );
        let vids = |this: &Self, n: &str| {
            this.types.enums[reid]
                .variants
                .iter()
                .position(|v| v.0 == n)
                .unwrap_or(0)
        };
        let enum_def_q = |this: &Self, n: &str| match t {
            Ty::Enum(i) => this.types.enums[i]
                .variants
                .iter()
                .position(|v| v.0 == n)
                .unwrap_or(0),
            _ => 0,
        };
        self.hid += 1;
        let (qn, en, vn) = (
            format!("$q{}", self.hid),
            format!("$e{}", self.hid),
            format!("$v{}", self.hid),
        );
        let outer_pre = std::mem::take(&mut self.pre);
        self.scopes.push(HashMap::new());
        let qid = self.declare(&qn, t, Mode::Own, false);
        out.push(s_stmt(SK::Let(qn.clone(), false, inner_e, qid), pos));
        let (bad, good) = if fam == "Result" {
            ("Err", "Ok")
        } else {
            ("None", "Some")
        };
        let cond = sx(
            EK::IsVariant(Box::new(s_var(&qn, pos)), enum_def_q(self, bad)),
            pos,
        );
        let forget = |pos: Pos| {
            s_stmt(
                SK::ExprS(Expr {
                    k: EK::Move(qn.clone(), 0),
                    pos,
                    ty: Ty::Unit,
                }),
                pos,
            )
        };
        let mut body: Vec<Stmt> = vec![];
        let ret_val = if fam == "Result" {
            body.push(s_let(
                &en,
                false,
                sx(
                    EK::Payload(Box::new(s_var(&qn, pos)), enum_def_q(self, "Err"), 0),
                    pos,
                ),
                pos,
            ));
            body.push(forget(pos));
            let ev = sx(EK::Move(en.clone(), 0), pos);
            let arg = match &conv {
                Some(f) => sx(EK::Call(f.clone(), vec![ev]), pos),
                None => ev,
            };
            sx(
                EK::Variant {
                    eid: reid,
                    vname: "Err".into(),
                    vid: vids(self, "Err"),
                    args: vec![arg],
                },
                pos,
            )
        } else {
            sx(
                EK::Variant {
                    eid: reid,
                    vname: "None".into(),
                    vid: vids(self, "None"),
                    args: vec![],
                },
                pos,
            )
        };
        body.push(s_stmt(SK::Return(Some(ret_val)), pos));
        self.chk(&mut out, s_stmt(SK::If(cond, body, None), pos));
        let payload_ty = targs[0];
        self.chk(
            &mut out,
            s_let(
                &vn,
                false,
                sx(
                    EK::Payload(Box::new(s_var(&qn, pos)), enum_def_q(self, good), 0),
                    pos,
                ),
                pos,
            ),
        );
        let f = forget(pos);
        self.chk(&mut out, f);
        self.pre = outer_pre;
        let id = self.lookup(&vn).unwrap();
        self.scopes.pop();
        let k = if self.types.owned(payload_ty) {
            EK::Move(vn, id)
        } else {
            EK::Var(vn, id)
        };
        e.k = EK::Sequence(
            out,
            Box::new(Expr {
                k,
                pos,
                ty: payload_ty,
            }),
        );
        payload_ty
    }

    /// Is `e` a place (a variable, a field, `l.at(i)`, or the cell of a Box)?
    fn isp(&self, e: &Expr) -> bool {
        match &e.k {
            EK::Var(n, _) => self.lookup(n).is_some() || !self.types.consts.contains_key(n),
            EK::Field(..) => true,
            EK::Method { recv, m, .. } if m == "at" => {
                !matches!(self.peek_ty(recv), Some(Ty::Dyn(_)) | Some(Ty::Box(_)))
            }
            EK::Method { recv, m, args }
                if e.pos.line >= PRELUDE_LINE && m == "locked_value" && args.is_empty() =>
            {
                self.peek_ty(recv)
                    .and_then(|t| self.types.locked_payload(t))
                    .is_some()
            }
            EK::Method { recv, m, args } if m == "get" && args.is_empty() => {
                self.peek_ty(recv).is_some_and(|ty| {
                    matches!(ty, Ty::Box(_)) || self.types.shared_payload(ty).is_some()
                })
            }
            _ => false,
        }
    }

    /// The enum (or other aggregate) a synthetic `IsVariant` / `Payload` looks into: a place is only inspected,
    /// never moved or copied, so owned types are fine here.
    fn expr_base(&mut self, b: &mut Expr) -> Ty {
        if self.isp(b) {
            match self.place(b) {
                Some(p) => {
                    b.ty = p.ty;
                    p.ty
                }
                None => Ty::Err,
            }
        } else {
            self.expr(b)
        }
    }

    fn need_mut(&mut self, p: PlaceInfo, recv: &Expr, pos: Pos) {
        self.frozen_check(recv, true, pos);
        if !p.mutable {
            let n = place_str(recv);
            let why = self.immutable_reason(p.root_mode);
            let help = if p.root_mode == Mode::View {
                "help: declare the parameter as `mut` to be able to modify it"
            } else {
                "help: declare it with `var` instead of `let`"
            };
            self.err(diag(format!("cannot modify '{}': {}", n, why), pos).note(help));
        }
    }

    /// An argument that is a Text or a borrowed String (read-only, never moved).
    fn text_arg(&mut self, a: &mut Expr) {
        let apos = a.pos;
        if matches!(a.k, EK::Move(..)) {
            self.err(
                diag("this only borrows its String argument: remove `move`", apos)
                    .note("the String stays yours; it is only read during the call"),
            );
        } else if self.isp(a) {
            if let Some(p) = self.place(a) {
                if !matches!(p.ty, Ty::Text | Ty::String | Ty::Err) {
                    let n = self.tn(p.ty);
                    self.err(diag(
                        format!("expected Text or String but this is {}", n),
                        apos,
                    ));
                }
            }
        } else {
            let t = self.expr(a);
            match t {
                Ty::Text | Ty::Err => {}
                Ty::String => self.err(
                    diag("pass a variable here (the function borrows it)", apos)
                        .note("help: store the String with `let` first, then pass the variable"),
                ),
                t => {
                    let n = self.tn(t);
                    self.err(diag(
                        format!("expected Text or String but this is {}", n),
                        apos,
                    ));
                }
            }
        }
    }

    fn char_method(&mut self, m: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        let ret = match m {
            "to_int" | "utf8_len" => Ty::Int,
            _ => {
                self.err(diag(format!("Char has no method '{}'", m), pos).note("methods: to_int(), utf8_len()   (functions: Char.from_int(n), Char.is_valid(n))"));
                return Ty::Err;
            }
        };
        if !args.is_empty() {
            self.err(diag(format!("'{}' expects 0 argument(s)", m), pos));
        }
        ret
    }

    fn method(&mut self, recv: &mut Expr, m: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        // a Char can be a receiver even when it is not a place: `s.char_at(i).to_int()`
        if !self.isp(recv) {
            let t = self.expr(recv);
            return match t {
                Ty::Char => self.char_method(m, args, pos),
                Ty::Ptr(_) => self.ptr_method(m, args, pos),
                Ty::Err => Ty::Err,
                _ => {
                    self.err(diag("expected a variable or a field here", recv.pos));
                    Ty::Err
                }
            };
        }
        if m == "take" && matches!(self.peek_ty(recv), Some(Ty::Box(_))) {
            return self.box_take(recv, args, pos);
        }
        let p = match self.place(recv) {
            None => return Ty::Err,
            Some(p) => p,
        };
        match p.ty {
            Ty::Err => Ty::Err,
            Ty::Box(_) => {
                let n = self.tn(p.ty);
                self.err(diag(format!("{} has no method '{}'", n, m), pos).note("methods: get() (or `*b`) is the value inside, take() moves it out; `Box.new(x)` creates one"));
                Ty::Err
            }
            Ty::List(li) => self.list_method(p, recv, m, args, pos, self.types.lists[li]),
            t if self.types.array_of(t).is_some() => {
                let (elem, n) = self.types.array_of(t).unwrap();
                self.array_method(p, recv, m, args, pos, elem, n)
            }
            Ty::String => self.string_method(p, recv, m, args, pos),
            Ty::Ptr(_) => self.ptr_method(m, args, pos),
            Ty::Char => self.char_method(m, args, pos),
            t => {
                let n = self.tn(t);
                self.err(diag(format!("type {} has no methods", n), pos));
                Ty::Err
            }
        }
    }

    /// `[a, b, c]`: every element has the element type (from the context, else from the first element).
    fn array_lit(&mut self, items: &mut [Expr], hint: Option<Ty>, pos: Pos) -> Ty {
        let want = hint.and_then(|h| {
            self.types
                .array_of(h)
                .map(|(e, _)| e)
                .or_else(|| self.types.slice_elem(h))
        });
        let mut elem = want;
        for it in items.iter_mut() {
            self.hint = elem;
            let t = self.expr(it);
            if t == Ty::Err {
                return Ty::Err;
            }
            match elem {
                None => {
                    elem = Some(if matches!(t, Ty::Range(..)) {
                        Ty::Int
                    } else {
                        t
                    });
                }
                Some(w) => self.expect(t, w, it.pos),
            }
        }
        let elem = elem.unwrap_or(Ty::Err);
        self.make_array(elem, items.len(), pos)
    }
    /// `[v; N]`: `v` is evaluated once and copied N times.
    fn array_fill(&mut self, v: &mut Expr, n: usize, hint: Option<Ty>, pos: Pos) -> Ty {
        self.hint = hint.and_then(|h| self.types.array_of(h).map(|(e, _)| e));
        let want = self.hint;
        let t = self.expr(v);
        if t == Ty::Err {
            return Ty::Err;
        }
        let elem = match want {
            Some(w) => {
                self.expect(t, w, v.pos);
                w
            }
            None if matches!(t, Ty::Range(..)) => Ty::Int,
            None => t,
        };
        self.make_array(elem, n, pos)
    }
    fn make_array(&mut self, elem: Ty, n: usize, pos: Pos) -> Ty {
        if elem == Ty::Err {
            return Ty::Err;
        }
        if elem == Ty::Unit {
            self.err(diag("an array element must be a value", pos));
            return Ty::Err;
        }
        if self.types.owned(elem) {
            self.err(self.array_copy_error(elem, pos));
            return Ty::Err;
        }
        if let Err(d) = crate::syntax::check_array_len(&self.types, elem, n as i64, pos) {
            self.err(d);
            return Ty::Err;
        }
        self.types.intern_array(elem, n, pos)
    }
    fn array_copy_error(&self, elem: Ty, pos: Pos) -> Diag {
        let n = self.tn(elem);
        let mut d = diag(
            format!("the elements of an array must be Copy, but {} owns memory", n),
            pos,
        )
        .note("fixed-size arrays hold plain values (numbers, Bool, Char, Text, Copy structs, other arrays)");
        if matches!(elem, Ty::Param(_)) {
            d = d.note(format!("help: add the bound `{}: Copy`", n));
        } else {
            d = d.note(format!("help: use `List<{}>` for owned elements", n));
        }
        d
    }
    /// A literal index is checked against the array length at compile time.
    fn const_index(&mut self, idx: &Expr, n: usize, arr: Ty) {
        let k = match &idx.k {
            EK::Int(k) => Some(*k),
            EK::Un("-", x) => match x.k {
                EK::Int(k) => Some(-k),
                _ => None,
            },
            _ => None,
        };
        if let Some(k) = k {
            if k < 0 || k >= n as i64 {
                let an = self.tn(arr);
                self.err(
                    diag(
                        format!("index {} is out of bounds for the array {}", k, an),
                        idx.pos,
                    )
                    .note(format!("valid indexes are 0 to {}", n - 1)),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn array_method(
        &mut self,
        p: PlaceInfo,
        recv: &mut Expr,
        m: &str,
        args: &mut Vec<Expr>,
        pos: Pos,
        elem: Ty,
        n: usize,
    ) -> Ty {
        let (want, ret, mutates): (Vec<Ty>, Ty, bool) = match m {
            "len" => (vec![], Ty::Int, false),
            "get" => (vec![Ty::Int], elem, false),
            "set" => (vec![Ty::Int, elem], Ty::Unit, true),
            "swap" => (vec![Ty::Int, Ty::Int], Ty::Unit, true),
            _ => {
                self.err(diag(format!("{} has no method '{}'", self.tn(p.ty), m), pos)
                    .note("methods: len(), get(i), set(i, x), swap(i, j); `a[i]`, `a[i] = x`, `a[from..to]` (for a `view [T]`/`mut [T]` parameter), `for x in a`"));
                return Ty::Err;
            }
        };
        if args.len() != want.len() {
            self.err(diag(
                format!("'{}' expects {} argument(s)", m, want.len()),
                pos,
            ));
            return ret;
        }
        for (k, (a, w)) in args.iter_mut().zip(want).enumerate() {
            self.hint = Some(w);
            let t = self.expr(a);
            self.expect(t, w, a.pos);
            if w == Ty::Int && (k == 0 || m == "swap") {
                self.const_index(a, n, p.ty);
            }
        }
        if mutates {
            self.need_mut(p, recv, pos);
        }
        ret
    }

    /// An argument for a `view [T]` / `mut [T]` parameter: a slice, an array or a List place (lent), a slice
    /// expression `x[a..b]`, or (for `view`) a temporary array or List kept alive for the call.
    #[allow(clippy::too_many_arguments)]
    fn slice_arg(
        &mut self,
        arg: &mut Expr,
        mode: Mode,
        pty: Ty,
        selem: Ty,
        pre: Option<Ty>,
        before: &mut [Expr],
        params: &[(Mode, Ty)],
        loans: &mut Vec<(usize, u8, Pos)>,
    ) {
        let apos = arg.pos;
        if let EK::SliceOf(b, lo, hi) = &mut arg.k {
            for x in lo.iter_mut().chain(hi.iter_mut()) {
                self.hint = Some(Ty::Int);
                let t = self.expr(x);
                self.expect(t, Ty::Int, x.pos);
            }
            if !self.isp(b) {
                self.err(diag("cannot take a slice of a temporary", b.pos).note(
                    "help: store the array or List in a variable first, then slice the variable",
                ));
                return;
            }
            let Some(p) = self.place(b) else {
                return;
            };
            if self.lend_slice(p, b, mode, pty, selem, apos, loans) {
                arg.ty = pty;
            }
            return;
        }
        if matches!(arg.k, EK::Move(..)) {
            self.err(
                diag("this function only borrows: remove `move`", apos)
                    .note("the parameter is declared `view` or `mut`, so you remain the owner"),
            );
            return;
        }
        if pre.is_none() && self.isp(arg) {
            let Some(p) = self.place(arg) else {
                return;
            };
            if self.lend_slice(p, arg, mode, pty, selem, apos, loans)
                && self.types.array_of(p.ty).is_some()
            {
                Self::wrap_slice(arg, pty);
            }
            return;
        }
        // a temporary array or List (`sum([1, 2, 3])`, `sum(make())`)
        let t = match pre {
            Some(t) => t,
            None => {
                self.hint = Some(pty);
                self.expr(arg)
            }
        };
        if t == Ty::Err {
            return;
        }
        if self.types.seq_elem(t) != Some(selem) {
            self.err(diag(
                format!("expected {} but this is {}", self.tn(pty), self.tn(t)),
                apos,
            ).note("a slice parameter accepts an array, a List or a slice of the same element type"));
            return;
        }
        if mode == Mode::Mut {
            self.err(
                diag("pass a variable here (the function borrows it)", apos)
                    .note("a temporary would be modified and thrown away"),
            );
            return;
        }
        self.hoist_view_temp(arg, t, before, params);
        if self.types.array_of(t).is_some() {
            Self::wrap_slice(arg, pty);
        }
    }
    fn wrap_slice(arg: &mut Expr, pty: Ty) {
        let pos = arg.pos;
        let inner = std::mem::replace(arg, sx(EK::Int(0), pos));
        *arg = Expr {
            k: EK::SliceOf(Box::new(inner), None, None),
            pos,
            ty: pty,
        };
    }
    /// Lend the place `place` (already checked: `p`) to a slice parameter. Returns false after an error.
    #[allow(clippy::too_many_arguments)]
    fn lend_slice(
        &mut self,
        p: PlaceInfo,
        place: &Expr,
        mode: Mode,
        pty: Ty,
        selem: Ty,
        apos: Pos,
        loans: &mut Vec<(usize, u8, Pos)>,
    ) -> bool {
        if p.ty == Ty::Err {
            return false;
        }
        if self.types.seq_elem(p.ty) != Some(selem) {
            self.err(
                diag(
                    format!("expected {} but this is {}", self.tn(pty), self.tn(p.ty)),
                    apos,
                )
                .note("a slice parameter accepts an array, a List or a slice of the same element type"),
            );
            return false;
        }
        if mode == Mode::Mut && !p.mutable {
            let n = place_str(place);
            let why = self.immutable_reason(p.root_mode);
            self.err(
                diag(format!("cannot lend '{}' as `mut`: {}", n, why), apos)
                    .note("help: declare it with `var` (or receive it as `mut`)"),
            );
        }
        if mode == Mode::Mut {
            self.frozen_check(place, true, apos);
            self.ev_escape.push(p.root);
        }
        loans.push((p.root, if mode == Mode::Mut { 1 } else { 0 }, apos));
        if let Some(ls) = self.closure_loans.get(&p.root).cloned() {
            loans.extend(ls);
        }
        true
    }

    fn list_method(
        &mut self,
        p: PlaceInfo,
        recv: &mut Expr,
        m: &str,
        args: &mut Vec<Expr>,
        pos: Pos,
        elem: Ty,
    ) -> Ty {
        if self.types.slice_elem(p.ty).is_some() && !matches!(m, "len" | "get" | "set" | "swap") {
            self.err(
                diag(format!("{} has no method '{}'", self.tn(p.ty), m), pos)
                    .note("a slice borrows elements of an array or a List: it can read and replace them, but never add, remove or reorder through library methods")
                    .note("methods: len(), get(i), set(i, x), swap(i, j), `s[i]`, `s[i] = x`, `s[from..to]`, `for x in s`"),
            );
            return Ty::Err;
        }
        let (nargs, ret, mutates) = match m {
            "push" => (1, Ty::Unit, true),
            "len" => (0, Ty::Int, false),
            "get" => (1, elem, false),
            "pop" => (0, elem, true),
            "set" => (2, Ty::Unit, true),
            "clear" => (0, Ty::Unit, true),
            "swap" => (2, Ty::Unit, true),
            _ => {
                self.err(diag(format!("{} has no method '{}'", self.tn(p.ty), m), pos)
                    .note("methods: push(x), pop(), len(), get(i), at(i), set(i, x), clear(), swap(i, j), and the library methods (insert, remove, sort, ...)"));
                return Ty::Err;
            }
        };
        if args.len() != nargs {
            self.err(diag(format!("'{}' expects {} argument(s)", m, nargs), pos));
            return ret;
        }
        let want: Vec<Ty> = match m {
            "push" => vec![elem],
            "get" => vec![Ty::Int],
            "set" => vec![Ty::Int, elem],
            "swap" => vec![Ty::Int, Ty::Int],
            _ => vec![],
        };
        for (a, w) in args.iter_mut().zip(want) {
            self.hint = Some(w);
            let t = self.expr(a);
            self.expect(t, w, a.pos);
            if let EK::Move(_, id) = &a.k {
                if *id == p.root && self.types.owned(t) {
                    let n = self.vars[*id].name.clone();
                    self.err(
                        diag(format!("'{}' appears twice in the same call and one use modifies or moves it", n), a.pos)
                            .note("help: a list cannot be moved into itself"),
                    );
                }
            }
        }
        if mutates {
            self.need_mut(p, recv, pos);
        }
        if m == "get" && self.types.owned(elem) {
            let n = self.tn(p.ty);
            self.err(
                diag(format!("cannot copy an element out of a {}: its elements are owned, not copied", n), pos)
                    .note("help: borrow it with `at(i)` (e.g. `l.at(i).len()`, or pass `l.at(i)` to a `view` parameter), or take the last one with `pop()`"),
            );
            return Ty::Err;
        }
        ret
    }

    fn string_method(
        &mut self,
        p: PlaceInfo,
        recv: &mut Expr,
        m: &str,
        args: &mut Vec<Expr>,
        pos: Pos,
    ) -> Ty {
        let (nargs, ret, mutates) = match m {
            "len" => (0, Ty::Int, false),
            "push_str" => (1, Ty::Unit, true),
            "push_int" => (1, Ty::Unit, true),
            "push_char" => (1, Ty::Unit, true),
            "eq" => (1, Ty::Bool, false),
            "byte" => (1, Ty::Int, false),
            "char_at" => (1, Ty::Char, false),
            "char_size" => (1, Ty::Int, false),
            "clear" => (0, Ty::Unit, true),
            "push_byte" => (1, Ty::Unit, true),
            "push_slice" => (3, Ty::Unit, true),
            "raw_addr" => (0, Ty::Int, false),
            "push_raw" => (2, Ty::Unit, true),
            _ => {
                self.err(diag(format!("String has no method '{}'", m), pos)
                    .note("methods: len() (bytes), push_str(x), push_int(n), push_char(c), push_byte(b), push_slice(x, from, to), eq(x), byte(i), char_at(i), char_size(i), clear(), and the library methods (starts_with, find, trim, ...)")
                    .note("to visit the characters write `for c in s.chars() { .. }`"));
                return Ty::Err;
            }
        };
        if args.len() != nargs {
            self.err(diag(format!("'{}' expects {} argument(s)", m, nargs), pos));
            return ret;
        }
        for (k, a) in args.iter_mut().enumerate() {
            match m {
                "push_str" | "eq" | "push_slice" if k == 0 => {
                    self.text_arg(a);
                    if m != "eq" && self.isp(a) && overlap(recv, a) {
                        let n = place_str(a);
                        self.err(
                            diag(
                                format!(
                                    "cannot append '{}' here: it may be the string being modified",
                                    n
                                ),
                                a.pos,
                            )
                            .note("help: copy it first: `let c = String.from(x)` and append `c`"),
                        );
                    }
                }
                "push_char" => {
                    let t = self.expr(a);
                    self.expect(t, Ty::Char, a.pos);
                }
                _ => {
                    let t = self.expr(a);
                    self.expect(t, Ty::Int, a.pos);
                }
            }
        }
        if mutates {
            self.need_mut(p, recv, pos);
        }
        ret
    }

    /// `b.take()`: moves the value out of the Box (and frees the Box). Consumes `b`.
    fn box_take(&mut self, recv: &mut Expr, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        if !args.is_empty() {
            self.err(diag("'take' expects 0 argument(s)", pos));
        }
        match &recv.k {
            EK::Var(n, _) => {
                let (n, p) = (n.clone(), recv.pos);
                let bt = self.peek_ty(recv).unwrap_or(Ty::Err);
                if matches!(bt, Ty::Box(i) if matches!(self.types.boxes[i], Ty::Dyn(_))) {
                    self.err(
                        diag("cannot take an erased value out of a Box", pos)
                            .note("help: borrow it with `box.get()`"),
                    );
                    return Ty::Err;
                }
                *recv = Expr {
                    k: EK::Move(n, 0),
                    pos: p,
                    ty: bt,
                };
                match self.expr(recv) {
                    Ty::Box(i) => self.types.boxes[i],
                    _ => Ty::Err,
                }
            }
            _ => {
                let n = place_str(recv);
                self.err(
                    diag(format!("cannot take the value out of '{}': it does not own the Box", n), pos)
                        .note("`take()` consumes the Box, so it needs a variable that owns it")
                        .note("help: borrow the value instead (`*b`), or move the Box out with `match move` / `let (a, b) = move pair` first"),
                );
                Ty::Err
            }
        }
    }

    /// `Box.new(x)` / `Box<T>.new(x)`.
    fn box_static(&mut self, ty: Ty, f: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        if f == "from_runtime_word" && pos.line >= PRELUDE_LINE {
            if let Ty::Box(_) = ty {
                if args.len() == 1 {
                    let word = self.expr(&mut args[0]);
                    self.expect(word, Ty::Int, args[0].pos);
                    return ty;
                }
            }
        }
        if f != "new" {
            let n = self.tn(ty);
            self.err(
                diag(
                    format!(
                        "{} has no function '{}'",
                        if ty == Ty::Err { "Box".to_string() } else { n },
                        f
                    ),
                    pos,
                )
                .note("functions: Box.new(x)"),
            );
            return Ty::Err;
        }
        if args.len() != 1 {
            self.err(
                diag("Box.new() expects 1 argument(s)", pos)
                    .note("usage: `Box.new(value)`: the value moves into a new heap cell"),
            );
            return if let Ty::Box(_) = ty { ty } else { Ty::Err };
        }
        if let Ty::Box(i) = ty {
            self.hint = Some(self.types.boxes[i]);
        }
        let t = self.expr(&mut args[0]);
        match ty {
            Ty::Box(i) => {
                let want = self.types.boxes[i];
                self.expect(t, want, args[0].pos);
                ty
            }
            _ => {
                if t == Ty::Err {
                    return Ty::Err;
                }
                if t == Ty::Unit {
                    self.err(diag("this expression produces no value", args[0].pos));
                    return Ty::Err;
                }
                self.with_ps(|ps| ps.intern_box(t))
            }
        }
    }

    fn static_call(&mut self, ty: Ty, f: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        if matches!(ty, Ty::Ptr(_)) {
            return self.ptr_static(ty, f, args, pos);
        }
        if ty == Ty::Err || matches!(ty, Ty::Box(_)) {
            return self.box_static(ty, f, args, pos);
        }
        if ty == Ty::Char {
            let ret = match f {
                "from_int" => Ty::Char,
                "is_valid" => Ty::Bool,
                _ => {
                    self.err(diag(format!("Char has no function '{}'", f), pos).note("functions: from_int(n) (panics when n is not a Unicode scalar value), is_valid(n)"));
                    return Ty::Err;
                }
            };
            if args.len() != 1 {
                self.err(diag(format!("Char.{}() expects 1 argument(s)", f), pos));
            } else {
                let t = self.expr(&mut args[0]);
                self.expect(t, Ty::Int, args[0].pos);
            }
            return ret;
        }
        if let Ty::List(i) = ty {
            if self.types.contains_join_handle(self.types.lists[i]) {
                self.err(diag("a List cannot contain JoinHandle values", pos));
                return Ty::Err;
            }
        }
        let tn = self.tn(ty);
        let (nargs, known) = match (ty, f) {
            (Ty::List(_), "new") | (Ty::String, "new") => (0, true),
            (Ty::String, "from") => (1, true),
            (Ty::String, "concat") => (2, true),
            _ => (0, false),
        };
        if !known {
            let funcs = if ty == Ty::String {
                "new(), from(x), concat(a, b)"
            } else {
                "new()"
            };
            self.err(
                diag(format!("{} has no function '{}'", tn, f), pos)
                    .note(format!("functions: {}", funcs)),
            );
            return Ty::Err;
        }
        if args.len() != nargs {
            self.err(diag(
                format!("{}.{}() expects {} argument(s)", tn, f, nargs),
                pos,
            ));
            return ty;
        }
        for a in args.iter_mut() {
            self.text_arg(a);
        }
        ty
    }

    fn call(&mut self, name: &str, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        if self.extern_names.contains(name) && self.unsafe_depth == 0 {
            self.err(diag(format!("call to extern function '{}' requires an unsafe block", name), pos)
                .note("help: wrap the call in `unsafe { ... }` after checking its pointer and lifetime requirements"));
        }
        match name {
            "__comptime" if args.len() == 1 => {
                // `comptime e`: typed as `e`; src/comptime.rs evaluates it after checking
                let t = self.expr(&mut args[0]);
                if !matches!(
                    t,
                    Ty::Int | Ty::Range(..) | Ty::Char | Ty::Bool | Ty::Unit | Ty::Err
                ) {
                    let n = self.tn(t);
                    self.err(diag(format!("a `comptime` expression must be an Int, Char or Bool, found {} [E-K3]", n), args[0].pos)
                        .note("why: for now only scalar values can leave compile time")
                        .note("fix: use comptime for the number or the condition, and build the aggregate at run time"));
                }
                return t;
            }
            "assert" | "assert_eq" => {
                let want = if name == "assert" { 1 } else { 2 };
                if args.len() != want {
                    let usage = if want == 1 {
                        "assert(condition)"
                    } else {
                        "assert_eq(left, right)"
                    };
                    self.err(
                        diag(format!("{} expects {} argument(s)", name, want), pos)
                            .note(format!("usage: {}", usage)),
                    );
                    return Ty::Unit;
                }
                let t = self.expr(&mut args[0]);
                self.expect(t, Ty::Bool, args[0].pos);
                return Ty::Unit;
            }
            "panic" => {
                if args.len() != 1 {
                    self.err(diag("panic expects exactly 1 argument", pos).note("usage: `panic(\"what went wrong\")`; the message is a Text or a String, e.g. `panic(\"bad value {x}\")`"));
                } else {
                    let t = if self.isp(&args[0]) {
                        self.place(&mut args[0]).map_or(Ty::Err, |p| p.ty)
                    } else {
                        self.expr(&mut args[0])
                    };
                    if t == Ty::String && !self.isp(&args[0]) {
                        // a temporary String message: panic with it, freeing is moot (the program ends)
                        self.load_lazy("std__panic_owned");
                        self.rename = Some("std__panic_owned".into());
                    } else if !matches!(t, Ty::Text | Ty::String | Ty::Err) {
                        let n = self.tn(t);
                        self.err(diag(format!("panic needs a Text or a String message, found {}", n), args[0].pos)
                            .note("help: build a message with interpolation: `panic(\"unexpected value {x}\")`"));
                    }
                }
                self.st.dead = true;
                return Ty::Unit;
            }
            "__lock_word" if pos.line < PRELUDE_LINE => {
                self.err(diag(
                    "use with() to access the contents of a Locked value",
                    pos,
                ));
                return Ty::Err;
            }
            "__parallel_range" | "__parallel_list" if pos.line < PRELUDE_LINE => {
                if !matches!(
                    args.last().map(|e| &e.k),
                    Some(EK::Closure { parallel: true, .. })
                ) {
                    self.err(diag("use parallel for to start parallel iterations", pos));
                    return Ty::Err;
                }
            }
            "__parallel_address" if pos.line >= PRELUDE_LINE => {
                let ft = self.types.intern_fn(FnTy {
                    params: vec![(Mode::Own, Ty::Int)],
                    ret: Ty::Unit,
                });
                return self.call_args(
                    Sig {
                        params: vec![(Mode::View, ft)],
                        ret: Ty::Int,
                    },
                    args,
                    vec![None],
                );
            }
            "__parallel_call" if pos.line >= PRELUDE_LINE => {
                return self.call_args(
                    Sig {
                        params: vec![(Mode::Own, Ty::Int); 2],
                        ret: Ty::Unit,
                    },
                    args,
                    vec![None; 2],
                );
            }
            "__locked_unbox" if pos.line >= PRELUDE_LINE && args.len() == 1 => {
                let ty = self.expr(&mut args[0]);
                if let Some(payload) = self.types.locked_payload(ty) {
                    return self.with_ps(|ps| ps.intern_box(payload));
                }
                return Ty::Err;
            }
            "__box_address" | "__check_shared_content" if pos.line >= PRELUDE_LINE => {
                let ty = self.place(&mut args[0]).map_or(Ty::Err, |p| p.ty);
                if name == "__box_address" {
                    debug_assert!(matches!(ty, Ty::Box(_) | Ty::Err));
                    return Ty::Int;
                }
                if !self.thread_transferable(ty, &mut HashSet::new()) {
                    self.err(diag(
                        format!(
                            "Shared cannot contain {} because it is not thread-transferable",
                            self.tn(ty)
                        ),
                        pos,
                    ));
                }
                return Ty::Unit;
            }
            "__thread_pack" if pos.line >= PRELUDE_LINE => {
                if args.len() == 1 && matches!(self.expr(&mut args[0]), Ty::Box(_)) {
                    return Ty::Int;
                }
                self.err(diag("thread result packing requires a Box", pos));
                return Ty::Err;
            }
            "__thread_start_recover" if pos.line < PRELUDE_LINE => {
                self.err(diag("use spawn to start a recoverable worker", pos));
                return Ty::Err;
            }
            "__thread_start" | "__thread_start_recover" | "__thread_wait" => {
                if args.len() != 1 {
                    self.err(diag(format!("{} expects exactly 1 argument", name), pos));
                    return Ty::Int;
                }
                let arg = if name != "__thread_wait" {
                    self.types.intern_fn(FnTy {
                        params: vec![],
                        ret: Ty::Int,
                    })
                } else {
                    Ty::Int
                };
                return self.call_args(
                    Sig {
                        params: vec![(Mode::Own, arg)],
                        ret: Ty::Int,
                    },
                    args,
                    vec![None],
                );
            }
            // ---- raw intrinsics (docs/STDLIB_IO.md): the building blocks of the std.io module ----
            "__syscall" | "__global" | "__alloc" | "__free" | "__load8" | "__load64"
            | "__store8" | "__store64" | "__atomic_load" | "__atomic_store"
            | "__atomic_cmpxchg" | "__atomic_fetch_add" | "__futex_park" | "__futex_wake" => {
                let (want, ret) = match name {
                    "__syscall" => (7, Ty::Int),
                    "__global" | "__alloc" | "__load8" | "__load64" => (1, Ty::Int),
                    "__atomic_load" => (1, Ty::Int),
                    "__atomic_cmpxchg" => (3, Ty::Int),
                    "__atomic_fetch_add" | "__futex_park" | "__futex_wake" => (2, Ty::Int),
                    "__free" => (1, Ty::Unit),
                    _ => (2, Ty::Unit),
                };
                if args.len() != want {
                    self.err(diag(
                        format!("{} expects {} Int argument(s)", name, want),
                        pos,
                    ));
                    return ret;
                }
                for a in args.iter_mut() {
                    let t = self.expr(a);
                    self.expect(t, Ty::Int, a.pos);
                }
                if name == "__global"
                    && !matches!(args[0].k, EK::Int(n) if (0..GLOBAL_CELLS).contains(&n))
                {
                    self.err(diag(
                        format!(
                            "__global needs an integer literal between 0 and {}",
                            GLOBAL_CELLS - 1
                        ),
                        pos,
                    ));
                }
                return ret;
            }
            "print" => {
                if args.len() == 1 && self.is_seq_expr(&args[0]) {
                    let a = std::mem::replace(&mut args[0], sx(EK::Int(0), pos));
                    let p = a.pos;
                    args[0] = sx(EK::Call("std__seq_show".into(), vec![a]), p);
                }
                if args.len() != 1 {
                    self.err(diag("print expects exactly 1 argument", pos));
                } else {
                    // a String (or a list element) is only borrowed by print
                    let t = if self.isp(&args[0]) {
                        self.place(&mut args[0]).map_or(Ty::Err, |p| p.ty)
                    } else {
                        self.expr(&mut args[0])
                    };
                    // an array value made by a call: kept in a hidden variable and shown element-wise
                    let t = if !self.isp(&args[0]) && self.types.array_of(t).is_some() {
                        let p = args[0].pos;
                        let name = format!("tmp__{}", self.vars.len());
                        let id = self.declare(&name, t, Mode::Own, false);
                        let init = std::mem::replace(&mut args[0], sx(EK::Int(0), p));
                        self.arg_temps.push(Stmt {
                            k: SK::Let(name.clone(), false, init, id),
                            pos: p,
                        });
                        args[0] = sx(EK::Call("std__seq_show".into(), vec![s_var(&name, p)]), p);
                        self.expr(&mut args[0])
                    } else {
                        t
                    };
                    if t == Ty::String && !self.isp(&args[0]) {
                        // a temporary String: print it, then free it
                        self.load_lazy("std__print_owned");
                        self.rename = Some("std__print_owned".into());
                    } else if !matches!(t, Ty::Char | Ty::Bool | Ty::Text | Ty::String | Ty::Err)
                        && !is_num(t)
                    {
                        self.err(diag(
                            "print only accepts numbers, Char, Bool, Text or String",
                            args[0].pos,
                        ));
                    }
                }
                return Ty::Unit;
            }
            _ => {}
        }
        if let Some(gi) = self.ps.gfns.iter().position(|g| g.name == name) {
            return self.generic_call(gi, args, pos);
        }
        if !self.sigs.contains_key(name) && !self.in_generic {
            self.load_lazy(name);
        }
        let sig = match self.sigs.get(name).cloned().or_else(|| {
            if self.in_generic {
                self.lazy_header(name)
            } else {
                None
            }
        }) {
            Some(s) => s,
            None => {
                if let Some(i) = self.lookup(name) {
                    let t = self.tn(self.vars[i].ty);
                    self.err(
                        diag(format!("'{}' is a variable of type {}, not a function, so it cannot be called", name, t), pos)
                            .note("only function values (closures and function names) can be called with `name(args)`"),
                    );
                } else {
                    self.err(diag(format!("function '{}' does not exist", name), pos));
                }
                return Ty::Err;
            }
        };
        let variadic = self.variadic_externs.contains(name);
        if sig.params.len() > args.len() || (!variadic && sig.params.len() != args.len()) {
            let at_least = if variadic { "at least " } else { "" };
            self.err(diag(
                format!(
                    "'{}' expects {}{} argument(s) but got {}",
                    name,
                    at_least,
                    sig.params.len(),
                    args.len()
                ),
                pos,
            ));
            return sig.ret;
        }
        // C variadic call: the extra arguments follow the C default argument promotions
        // (Float32 becomes Float; narrower integers are widened by the backends) and must be scalars or pointers.
        let mut extra = args.split_off(sig.params.len());
        let pre = vec![None; args.len()];
        let r = self.call_args(sig, args, pre);
        for a in extra.iter_mut() {
            self.hint = None;
            let t = self.expr(a);
            let apos = a.pos;
            match t {
                Ty::Err => {}
                Ty::Num(crate::numeric::Nk::F32) => {
                    let inner = std::mem::replace(
                        a,
                        Expr {
                            k: EK::Bool(false),
                            pos: apos,
                            ty: Ty::Err,
                        },
                    );
                    *a = Expr {
                        k: EK::Conv(Box::new(inner), false),
                        pos: apos,
                        ty: Ty::Num(crate::numeric::Nk::F64),
                    };
                }
                Ty::Int | Ty::Char | Ty::Bool | Ty::Text | Ty::Ptr(_) | Ty::Num(_) => {}
                _ => {
                    let tn = self.tn(t);
                    self.err(diag(format!("cannot pass a value of type {} as a variadic argument to '{}'", tn, name), apos)
                        .note("variadic C arguments must be integers, Bool, Char, Float, Float32 (promoted to Float), Text, or Ptr<T>"));
                }
            }
        }
        args.append(&mut extra);
        r
    }

    /// Check the arguments of a call against a (concrete) signature. `pre[k]` holds the type of an
    /// argument that was already checked while inferring type arguments.
    fn call_args(&mut self, sig: Sig, args: &mut Vec<Expr>, mut pre: Vec<Option<Ty>>) -> Ty {
        // (root variable, use kind: 0 read / 1 write / 2 move, position)
        let mut loans: Vec<(usize, u8, Pos)> = vec![];
        let nerr_call = self.errs.len();
        for k in 0..args.len() {
            let Some(&(mode, pty)) = sig.params.get(k) else {
                break;
            };
            let (before, rest) = args.split_at_mut(k);
            let arg = &mut rest[0];
            let apos = arg.pos;
            if let (Some(selem), true) = (self.types.slice_elem(pty), mode != Mode::Own) {
                self.slice_arg(
                    arg,
                    mode,
                    pty,
                    selem,
                    pre[k],
                    before,
                    &sig.params,
                    &mut loans,
                );
                continue;
            }
            // a function value (closure literal, function name, call result) for a `view fn` / `mut fn` parameter
            // is created in a hidden variable, which is then lent like any other variable
            if matches!(pty, Ty::Fn(_)) && mode != Mode::Own {
                match pre[k] {
                    None => {
                        if self.needs_fn_temp(arg, mode, pty)
                            && self.hoist_fn_arg(arg, mode, pty).is_none()
                        {
                            continue;
                        }
                    }
                    Some(t) if !is_place(arg) => {
                        self.hoist_temp(arg, t, mode);
                        pre[k] = None;
                    }
                    _ => {}
                }
            } else if matches!(pty, Ty::Fn(_)) && pre[k].is_none() {
                self.norm_fn_arg(arg);
            }
            if mode == Mode::View && pty == Ty::String && pre[k].is_none() {
                self.hoist_text(arg);
            }
            let by_value = mode == Mode::Own || (mode == Mode::View && !self.types.owned(pty));
            if by_value || pre[k].is_some() {
                let t = match pre[k] {
                    Some(t) => t,
                    None => {
                        self.hint = Some(pty);
                        self.expr(arg)
                    }
                };
                if !by_value {
                    // an already-evaluated argument for a borrowing parameter must have been a place
                    if mode == Mode::View
                        && t != Ty::Err
                        && !is_place(arg)
                        && !matches!(arg.k, EK::Move(..))
                    {
                        self.expect(t, pty, apos);
                        self.hoist_view_temp(arg, t, before, &sig.params);
                        continue;
                    }
                    if matches!(arg.k, EK::Move(..)) {
                        self.err(
                            diag("this function only borrows: remove `move`", apos)
                                .note("the parameter is declared `view` or `mut`, so you remain the owner"),
                        );
                    } else {
                        self.err(diag("pass a variable here (the function borrows it)", apos));
                    }
                    continue;
                }
                self.expect(t, pty, apos);
                if let EK::Move(_, id) = &arg.k {
                    if self.types.owned(pty) {
                        loans.push((*id, 2, apos));
                    }
                } else {
                    // by-value arguments run before any borrowed argument is taken: a `move` buried in one of them
                    // (`show(m, takes(move m))`) would leave the borrowed `m` dangling
                    let mut mv = vec![];
                    expr_moves(arg, &mut mv);
                    for id in mv {
                        loans.push((id, 3, apos));
                    }
                }
                continue;
            }
            // borrow (`view` of an owned type, or any `mut`): the argument must be a place
            if self.isp(arg) {
                if let Some(p) = self.place(arg) {
                    self.expect(p.ty, pty, apos);
                    if mode == Mode::Mut && !p.mutable {
                        let n = place_str(arg);
                        let why = self.immutable_reason(p.root_mode);
                        self.err(
                            diag(format!("cannot lend '{}' as `mut`: {}", n, why), apos)
                                .note("help: declare it with `var` (or receive it as `mut`)"),
                        );
                    }
                    if mode == Mode::Mut {
                        self.frozen_check(arg, true, apos);
                        self.ev_escape.push(p.root);
                    }
                    loans.push((p.root, if mode == Mode::Mut { 1 } else { 0 }, apos));
                    if let Some(ls) = self.closure_loans.get(&p.root).cloned() {
                        loans.extend(ls);
                    }
                }
            } else if matches!(arg.k, EK::Move(..)) {
                self.err(
                    diag("this function only borrows: remove `move`", apos)
                        .note("the parameter is declared `view` or `mut`, so you remain the owner"),
                );
            } else if mode == Mode::View {
                // a temporary (`show(make())`, `show(Point { .. })`): kept alive in a hidden variable for the call
                self.hint = Some(pty);
                let t = self.expr(arg);
                if t != Ty::Err {
                    self.expect(t, pty, apos);
                    self.hoist_view_temp(arg, t, before, &sig.params);
                }
            } else {
                self.err(diag("pass a variable here (the function borrows it)", apos));
            }
        }
        for a in 0..loans.len() {
            for b in a + 1..loans.len() {
                if loans[a].0 == loans[b].0 && (loans[a].1 != 0 || loans[b].1 != 0) {
                    // a move buried in an argument that was already reported as a use of a moved value
                    if (loans[a].1 == 3 || loans[b].1 == 3) && self.errs.len() > nerr_call {
                        continue;
                    }
                    let n = self.vars[loans[b].0].name.clone();
                    let mut d = diag(
                        format!(
                            "'{}' appears twice in the same call and one use modifies or moves it",
                            n
                        ),
                        loans[b].2,
                    );
                    for l in [loans[a], loans[b]] {
                        if self.closure_borrow_pos.contains(&(l.2.line, l.2.col)) {
                            d = d.note(format!(
                                "the closure on line {} borrows '{}' for the whole call",
                                l.2.line, n
                            ));
                        }
                    }
                    self.err(d.note("help: split the call in two steps, or pass a copy"));
                }
            }
        }
        sig.ret
    }

    /// An owned temporary lent to a `view` parameter: it lives in a hidden variable that the enclosing call
    /// expression creates right before the call and drops right after it. Earlier by-value arguments that
    /// are not trivial are evaluated first (into hidden variables too), so the left-to-right order is kept.
    fn hoist_view_temp(
        &mut self,
        arg: &mut Expr,
        t: Ty,
        before: &mut [Expr],
        params: &[(Mode, Ty)],
    ) {
        for (j, prev) in before.iter_mut().enumerate() {
            let (m, pt) = params[j];
            let by_value = m == Mode::Own || (m == Mode::View && !self.types.owned(pt));
            let trivial = matches!(
                prev.k,
                EK::Int(_)
                    | EK::Float(..)
                    | EK::Char(_)
                    | EK::Bool(_)
                    | EK::Text(_)
                    | EK::Var(..)
                    | EK::Move(..)
            );
            if by_value && !trivial && prev.ty != Ty::Err {
                let st = self.capture_operand(prev);
                self.arg_temps.push(st);
            }
        }
        let pos = arg.pos;
        let name = format!("tmp__{}", self.vars.len());
        let id = self.declare(&name, t, Mode::Own, false);
        let init = std::mem::replace(
            arg,
            Expr {
                k: EK::Var(name.clone(), id),
                pos,
                ty: t,
            },
        );
        self.arg_temps.push(Stmt {
            k: SK::Let(name, false, init, id),
            pos,
        });
    }

    /// A Text literal (or Text variable) passed where a `view String` is expected: the call gets a hidden
    /// String copy, made just before the statement and freed with the enclosing scope.
    fn hoist_text(&mut self, arg: &mut Expr) {
        let is_text = match &arg.k {
            EK::Text(_) => true,
            EK::Var(..) | EK::Field(..) => self.peek_ty(arg) == Some(Ty::Text),
            _ => false,
        };
        if !is_text {
            return;
        }
        let t = self.expr(arg);
        if t != Ty::Text {
            return;
        }
        let pos = arg.pos;
        let name = format!("text__{}", self.vars.len());
        let id = self.declare(&name, Ty::String, Mode::Own, false);
        let old = std::mem::replace(
            arg,
            Expr {
                k: EK::Var(name.clone(), id),
                pos,
                ty: Ty::String,
            },
        );
        let init = Expr {
            k: EK::Static {
                ty: Ty::String,
                f: "from".into(),
                args: vec![old],
            },
            pos,
            ty: Ty::String,
        };
        self.pre.push(Stmt {
            k: SK::Let(name, false, init, id),
            pos,
        });
    }

    fn block(&mut self, b: &mut Vec<Stmt>) {
        self.scopes.push(HashMap::new());
        self.dscopes.push(vec![]);
        // a `match` arm's payload borrows freeze the scrutinee until the end of this block
        let fz_mark = self.frozen.len();
        self.stmts(b);
        self.close_scope(b);
        self.frozen.truncate(fz_mark);
        self.scopes.pop();
    }

    /// Check a statement list. `defer` statements are removed from it (they are re-inserted at every exit
    /// of the scope by `close_scope` / `expand_return`), so the backends never see them.
    fn stmts(&mut self, b: &mut Vec<Stmt>) {
        let old = std::mem::take(b);
        // hoisted temporaries of the enclosing statement (made while its scrutinee/condition was checked) must
        // not be swallowed by the first statement of this nested block
        let outer = std::mem::take(&mut self.pre);
        for mut s in old {
            self.stmt(&mut s);
            let pre = std::mem::take(&mut self.pre);
            b.extend(pre);
            if !matches!(s.k, SK::Defer(_)) {
                b.push(s);
            }
        }
        self.pre = outer;
    }

    /// Normal exit of a scope: run its deferred statements in reverse order (unless the end is unreachable).
    fn close_scope(&mut self, b: &mut Vec<Stmt>) {
        let ds = self.dscopes.pop().unwrap_or_default();
        if !self.st.dead && !ds.is_empty() {
            self.check_defers(&ds);
            b.extend(ds.into_iter().rev());
        }
    }

    /// A deferred statement runs at scope exit, so everything it uses must still be alive there.
    fn check_defers(&mut self, ds: &[Stmt]) {
        for d in ds {
            let mut ids = vec![];
            match &d.k {
                SK::ExprS(e) => expr_ids(e, &mut ids),
                SK::Assign(a, b) => {
                    expr_ids(a, &mut ids);
                    expr_ids(b, &mut ids);
                }
                _ => {}
            }
            for id in ids {
                if let Some(mp) = self.st.moved.get(&id).copied() {
                    if self.reported.insert((d.pos.line, d.pos.col, id)) {
                        let n = self.vars[id].name.clone();
                        self.err(
                            diag(format!("this deferred statement uses '{}', but '{}' has already been moved when it runs", n, n), d.pos)
                                .note(format!("'{}' was moved on line {}", n, mp.line))
                                .note("help: `defer` runs when the scope ends, after everything written below it; keep the value until then, or defer a call that does not need it"),
                        );
                    }
                }
            }
        }
    }

    /// `return` leaves every open scope: evaluate the value first, then run all pending deferred statements
    /// (innermost scope first, reverse order), then return the saved value.
    fn expand_return(&mut self, s: &mut Stmt) {
        let all: Vec<Stmt> = self
            .dscopes
            .iter()
            .rev()
            .flat_map(|d| d.iter().rev().cloned())
            .collect();
        if all.is_empty() {
            return;
        }
        self.check_defers(&all);
        let mut pre = vec![];
        if let SK::Return(Some(e)) = &mut s.k {
            if !matches!(e.ty, Ty::Unit | Ty::Err) {
                let (ty, epos) = (e.ty, e.pos);
                let id = self.vars.len();
                self.vars.push(VarInfo {
                    name: "ret_".into(),
                    ty,
                    mode: Mode::Own,
                    mutable: false,
                });
                let k = if self.types.owned(ty) {
                    EK::Move("ret_".into(), id)
                } else {
                    EK::Var("ret_".into(), id)
                };
                let init = std::mem::replace(e, Expr { k, pos: epos, ty });
                pre.push(Stmt {
                    k: SK::Let("ret_".into(), false, init, id),
                    pos: s.pos,
                });
            }
        }
        pre.extend(all);
        let mut first = std::mem::take(&mut self.pre); // hoisted temporaries come first
        first.extend(pre);
        self.pre = first;
    }

    fn stmt(&mut self, s: &mut Stmt) {
        let was_dead = self.st.dead;
        let nerr = self.errs.len();
        self.stmt_inner(s);
        if std::mem::take(&mut self.unit_ret) {
            // `|x| print(x)`: the body expression has no value, so it is a statement
            if let SK::Return(Some(e)) = std::mem::replace(&mut s.k, SK::Block(vec![], false)) {
                s.k = SK::ExprS(e);
                self.st.dead = was_dead;
            }
        }
        if matches!(s.k, SK::Return(_)) && !was_dead && self.errs.len() == nerr {
            self.expand_return(s);
        }
    }

    fn stmt_inner(&mut self, s: &mut Stmt) {
        let pos = s.pos;
        if matches!(&s.k, SK::ExprS(Expr { k: EK::Call(name, args), .. })
            if name == "__spawn_owned" && matches!(args.first().map(|arg| &arg.k), Some(EK::Closure { worker: true, .. })))
        {
            self.hid += 1;
            let name = format!("$worker{}", self.hid);
            let SK::ExprS(worker) = std::mem::replace(&mut s.k, SK::Block(vec![], false)) else {
                unreachable!()
            };
            s.k = SK::Let(name, false, worker, 0);
        }
        if matches!(s.k, SK::For(..)) {
            let k = std::mem::replace(&mut s.k, SK::Block(vec![], false));
            if let SK::For(h, body) = k {
                let v = self.for_stmt(*h, body, pos);
                s.k = SK::Block(v, false);
            }
            return;
        }
        match &mut s.k {
            SK::Let(name, mutable, init, id) => {
                let annotated = matches!(init.k, EK::Ascribe(_));
                let loop_fact = match &init.k {
                    EK::Var(n, _) if !*mutable => self.loop_ivl.get(n).copied(),
                    _ => None,
                };
                let t = self.expr(init);
                let ty = if t == Ty::Unit {
                    self.err(diag("this expression produces no value", init.pos));
                    Ty::Err
                } else if *mutable && !annotated && matches!(t, Ty::Range(..)) {
                    // an inferred `var` keeps the full Int: its later values are not tied to this initialiser
                    Ty::Int
                } else {
                    t
                };
                *id = self.declare(name, ty, Mode::Own, *mutable);
                if annotated && matches!(ty, Ty::Range(..)) {
                    self.explicit_range.insert(*id);
                }
                let init_iv = rng::ival(init);
                if let Some(iv) = init_iv {
                    self.init_ivl.insert(*id, iv);
                }
                if matches!(ty, Ty::Int | Ty::Range(..))
                    && matches!(init.ty, Ty::Int | Ty::Range(..))
                {
                    init.ty = ty; // the initialiser is seen as the declared type: backends use it for the variable
                }
                if *mutable && matches!(ty, Ty::Int | Ty::Range(..)) {
                    self.init_fact(*id, init_iv);
                }
                if !*mutable && matches!(ty, Ty::Int | Ty::Range(..)) {
                    // the initialiser's interval is a fact about an immutable variable
                    let mut facts = vec![];
                    if annotated {
                        if let Some(iv) = init_iv {
                            facts.push((*id, iv));
                        }
                    }
                    if let Some(iv) = loop_fact {
                        facts.push((*id, iv));
                    }
                    self.apply_facts(&facts);
                }
            }
            SK::Assign(target, val) if matches!(&target.k, EK::GenStruct { name, .. } if name.starts_with("Tuple")) =>
            {
                // `(a, b) = value`: evaluate the value, then assign the elements in order (copyable elements only)
                let targets: Vec<Expr> = match &target.k {
                    EK::GenStruct { fields, .. } => fields.iter().map(|(_, e)| e.clone()).collect(),
                    _ => unreachable!(),
                };
                self.hid += 1;
                let tn = format!("$t{}", self.hid);
                self.scopes.push(HashMap::new());
                self.dscopes.push(vec![]);
                let mut out = vec![];
                self.chk(&mut out, s_let(&tn, false, val.clone(), val.pos));
                let ty = self.lookup(&tn).map(|i| self.vars[i].ty).unwrap_or(Ty::Err);
                let n = self.tuple_elems(ty).map(|e| e.len());
                if ty != Ty::Err && n != Some(targets.len()) {
                    let tyn = self.tn(ty);
                    self.err(
                        diag(
                            format!(
                                "cannot assign {} to a tuple of {} places",
                                tyn,
                                targets.len()
                            ),
                            val.pos,
                        )
                        .note("the right side must be a tuple with the same number of elements"),
                    );
                } else if self.types.owned(ty) {
                    self.err(
                        diag("a tuple assignment can only copy values", target.pos)
                            .note("the tuple holds a String, a List or a struct that owns memory, so its elements must be moved")
                            .note("help: declare new variables instead: `let (a, b) = move pair`, or assign the elements one by one"),
                    );
                } else if ty != Ty::Err {
                    for (k, tg) in targets.into_iter().enumerate() {
                        let tp = tg.pos;
                        let src = sx(EK::Field(Box::new(s_var(&tn, tp)), format!("_{}", k)), tp);
                        self.chk(&mut out, s_stmt(SK::Assign(tg, src), pos));
                    }
                }
                self.close_scope(&mut out);
                self.scopes.pop();
                s.k = SK::Block(out, false);
            }
            SK::Unpack(pat, init) => {
                let t = self.expr(init);
                if t == Ty::Unit {
                    self.err(diag("this expression produces no value", init.pos));
                }
                let p = init.pos;
                self.declare_unpack(pat, t, p);
            }
            SK::Assign(target, val) => {
                self.hint = self.peek_ty(target);
                let t = self.expr(val);
                if pos.line < PRELUDE_LINE && self.types.contains_join_handle(t) {
                    self.err(diag("a JoinHandle cannot be stored by assignment", val.pos));
                    return;
                }
                if is_at(target) {
                    let n = place_str(target);
                    self.err(
                        diag(format!("cannot assign to '{}' directly", n), pos)
                            .note("help: use `l.set(i, value)` to replace an element"),
                    );
                    return;
                }
                let is_var = matches!(target.k, EK::Var(..));
                if let Some(p) = self.place_inner(target, !is_var) {
                    if self.types.slice_elem(p.ty).is_some() {
                        let n = place_str(target);
                        self.err(
                            diag(format!("cannot assign to the slice '{}' itself", n), pos)
                                .note("a slice parameter is a borrow of someone else's elements")
                                .note(format!("help: replace elements one by one: `{}[i] = x`", n)),
                        );
                        return;
                    }
                    self.ev_assign.push(p.root);
                    self.frozen_check(target, true, pos);
                    if !p.mutable {
                        let n = place_str(target);
                        let why = self.immutable_reason(p.root_mode);
                        self.err(
                            diag(format!("cannot reassign '{}': {}", n, why), pos)
                                .note("help: declare it with `var`"),
                        );
                    } else {
                        self.expect(t, p.ty, val.pos);
                    }
                    if is_var {
                        self.st.moved.remove(&p.root);
                        if p.mutable {
                            self.assign_fact(p.root, val, pos.line);
                        }
                    }
                    if self.types.owned(t) && can_propagate(target) {
                        let captured = self.capture_operand(val);
                        let name = format!("$dst{}", self.hid);
                        let id = self.vars.len();
                        self.vars.push(VarInfo {
                            name: name.clone(),
                            ty: p.ty,
                            mode: Mode::Mut,
                            mutable: true,
                        });
                        let place = std::mem::replace(
                            target,
                            Expr {
                                k: EK::Var(name.clone(), id),
                                pos,
                                ty: p.ty,
                            },
                        );
                        let bind = s_stmt(SK::Bind(name, Mode::Mut, place, id), pos);
                        let assign = s_stmt(SK::Assign(target.clone(), val.clone()), pos);
                        s.k = SK::Block(vec![captured, bind, assign], false);
                    }
                }
            }
            SK::ExprS(Expr {
                k: EK::Move(n, id),
                ty,
                ..
            }) if n.starts_with('$') => {
                // synthetic "this hidden value has been taken apart" marker (see `arm_binds`): no ownership effect here
                if let Some(i) = self.lookup(n) {
                    *id = i;
                    *ty = self.vars[i].ty;
                }
            }
            SK::ExprS(e) => {
                let t = self.expr(e);
                if self.types.owned(t) {
                    let n = self.tn(t);
                    self.err(
                        diag(
                            format!("a {} is created and immediately discarded (memory leak)", n),
                            e.pos,
                        )
                        .note("help: store it with `let` or `var`"),
                    );
                }
            }
            SK::If(c, th, el) => {
                let t = self.expr(c);
                self.expect(t, Ty::Bool, c.pos);
                let (tf, ff) = (self.facts_of(c, true), self.facts_of(c, false));
                let before = self.st.clone();
                self.apply_facts(&tf);
                self.block(th);
                let a = std::mem::replace(&mut self.st, before);
                self.apply_facts(&ff);
                if let Some(e) = el {
                    self.block(e);
                }
                let b = std::mem::take(&mut self.st);
                self.st = merge(a, b);
            }
            SK::While(c, body, meta) => {
                self.kill_loop_facts(c, body, &meta.step, pos.line);
                let before_condition = self.st.clone();
                let t = self.expr(c);
                self.expect(t, Ty::Bool, c.pos);
                self.next_loop += 1;
                meta.id = self.next_loop;
                if let Some((n, lp)) = &meta.label {
                    if self
                        .loops
                        .iter()
                        .any(|l| l.label.as_deref() == Some(n.as_str()))
                    {
                        self.err(
                            diag(format!("the label '{}' is already used by an enclosing loop", n), *lp)
                                .note("help: give each nested loop its own label, e.g. `outer:` and `inner:`"),
                        );
                    }
                }
                let entry = self.vars.len();
                let before = self.st.clone();
                self.loops.push(LoopCtx {
                    label: meta.label.as_ref().map(|l| l.0.clone()),
                    id: meta.id,
                    dscope_depth: self.dscopes.len(),
                    breaks: vec![],
                    conts: vec![],
                });
                if let Some((place, excl)) = &meta.freeze {
                    self.frozen.push((place.clone(), *excl, pos.line, false));
                }
                let tf = self.facts_of(c, true);
                self.apply_facts(&tf);
                self.block(body);
                if meta.freeze.is_some() {
                    self.frozen.pop();
                }
                // `continue` paths join the end of the body, where the step (if any) runs
                let mut end = std::mem::take(&mut self.st);
                let conts = std::mem::take(&mut self.loops.last_mut().unwrap().conts);
                for c in conts {
                    end = merge(end, c);
                }
                self.st = end;
                // the step is checked even when the body never reaches its end (`break`, `return`, an
                // impossible branch): the backends still emit it, so its literals must be typed
                if !meta.step.is_empty() {
                    self.stmts(&mut meta.step);
                }
                let ctx = self.loops.pop().unwrap();
                meta.no_fallthrough = matches!(c.k, EK::Bool(true)) && ctx.breaks.is_empty();
                if !self.st.dead {
                    let mut late: Vec<(usize, Pos)> = self
                        .st
                        .moved
                        .iter()
                        .filter(|(k, _)| **k < entry && !before_condition.moved.contains_key(k))
                        .map(|(k, v)| (*k, *v))
                        .collect();
                    late.sort_by_key(|x| x.1.line);
                    for (id, mp) in late {
                        let n = self.vars[id].name.clone();
                        self.err(
                            diag(format!("'{}' is moved inside the loop and would be used again on the next iteration", n), mp)
                                .note("help: move it outside the loop, or reassign it (with `var`) before the iteration ends; or `break` right after moving it"),
                        );
                    }
                }
                let after = std::mem::take(&mut self.st);
                let mut st = merge(before, after);
                for b in ctx.breaks {
                    st = merge(st, b);
                }
                self.st = st;
            }
            SK::Break(label, id) => {
                let i = self.jump(true, label, pos);
                *id = i;
            }
            SK::Continue(label, id) => {
                let i = self.jump(false, label, pos);
                *id = i;
            }
            SK::Block(b, exits) => {
                self.block(b);
                *exits = self.st.dead;
            }
            SK::Unsafe(b) => {
                self.unsafe_depth += 1;
                self.block(b);
                self.unsafe_depth -= 1;
                s.k = SK::Block(std::mem::take(b), self.st.dead);
            }
            SK::For(..) => unreachable!("`for` is rewritten before checking"),
            SK::Bind(name, mode, place, id) => {
                // the borrowed element is created by the loop itself: its own place is exempt from the freeze
                let saved = std::mem::take(&mut self.frozen);
                let p = self.place(place);
                self.frozen = saved;
                // a payload bound by a borrowing `match` arm: the scrutinee may not change or move while the name lives
                if let Some(root) = hidden_root(place) {
                    if let Some((fz, excl, line)) = self.match_scruts.get(&root).cloned() {
                        self.frozen.push((fz, excl, line, true));
                    }
                }
                let ty = match p {
                    Some(p) => {
                        if *mode == Mode::Mut && !p.mutable {
                            let n = place_str(place).trim_end_matches(".at(..)").to_string();
                            let why = self.immutable_reason(p.root_mode);
                            self.err(
                                diag(format!("cannot loop over '{}' with `mut`: {}", n, why), pos)
                                    .note("help: declare the variable with `var` (or receive it as `mut`) to change its elements in place"),
                            );
                        }
                        if *mode == Mode::Mut {
                            self.ev_escape.push(p.root);
                        }
                        p.ty
                    }
                    None => Ty::Err,
                };
                *id = self.declare(name, ty, *mode, *mode == Mode::Mut);
            }
            SK::Match(scrut, arms) => {
                // What is matched, and how are the payloads bound? (see `MK`)
                let has_general = arms.iter().any(|a| a.pat.is_some() || a.guard.is_some());
                let mut kind = MK::Copy;
                let mut checked: Option<Ty> = None;
                if matches!(&scrut.k, EK::Un("mut", _)) {
                    let inner = match std::mem::replace(&mut scrut.k, EK::Int(0)) {
                        EK::Un(_, b) => *b,
                        _ => unreachable!(),
                    };
                    *scrut = inner;
                    if self.isp(scrut) {
                        kind = MK::Borrow(Mode::Mut);
                    } else {
                        self.err(
                            diag("`match mut` needs a variable or a field to modify in place", scrut.pos)
                                .note("help: store the value in a `var` first, then `match mut` that variable"),
                        );
                        return;
                    }
                } else if let EK::Move(n, _) = &scrut.k {
                    if let Some(i) = self.lookup(n) {
                        if self.types.owned(self.vars[i].ty) {
                            kind = MK::Move;
                        }
                    }
                } else if self.isp(scrut) {
                    if let Some(t) = self.peek_ty(scrut) {
                        if matches!(t, Ty::Enum(_)) && self.types.owned(t) {
                            kind = MK::Borrow(Mode::View);
                        }
                    }
                } else {
                    let t = self.expr(scrut);
                    checked = Some(t);
                    if matches!(t, Ty::Enum(_)) && self.types.owned(t) {
                        kind = MK::Move; // a temporary: its payloads are moved out
                    }
                }
                if has_general || kind != MK::Copy {
                    let sc = std::mem::replace(scrut, s_int(0, pos));
                    let ar = std::mem::take(arms);
                    let (out, exits) = self.match_general(sc, ar, pos, kind, checked);
                    s.k = SK::Block(out, exits);
                    return;
                }
                let t = match checked {
                    Some(t) => t,
                    None => self.expr(scrut),
                };
                let eid = match t {
                    Ty::Enum(e) => Some(e),
                    Ty::Err => None,
                    other => {
                        let n = self.tn(other);
                        self.err(diag(format!("`match` needs an enum value here, found {}", n), scrut.pos)
                            .note("`match` also works on tuples, Int, Char and Bool when the arms are patterns such as `0`, `1..=9`, `'a'`, `(x, y)` or `n`"));
                        None
                    }
                };
                let before = self.st.clone();
                let mut acc: Option<State> = None;
                let mut seen: HashSet<usize> = HashSet::new();
                let mut wildcard = false;
                for arm in arms.iter_mut() {
                    self.st = before.clone();
                    self.scopes.push(HashMap::new());
                    let mut payload: Vec<Ty> = vec![];
                    if let Some(e) = eid {
                        if arm.vname == "_" {
                            wildcard = true;
                        } else {
                            let def = self.types.enums[e].clone();
                            match def.variants.iter().position(|v| v.0 == arm.vname) {
                                None => self.err(diag(
                                    format!("enum {} has no variant '{}'", def.name, arm.vname),
                                    arm.pos,
                                )),
                                Some(vi) => {
                                    arm.variant = Some(vi);
                                    if !seen.insert(vi) {
                                        self.err(diag(
                                            format!("variant '{}' is matched twice", arm.vname),
                                            arm.pos,
                                        ));
                                    }
                                    payload = def.variants[vi].1.clone();
                                    if payload.len() != arm.binds.len() {
                                        self.err(diag(
                                            format!(
                                                "{}.{} carries {} value(s) but the pattern binds {}",
                                                def.name,
                                                arm.vname,
                                                payload.len(),
                                                arm.binds.len()
                                            ),
                                            arm.pos,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    for (k, (bn, bid)) in arm.binds.iter_mut().enumerate() {
                        let ty = payload.get(k).copied().unwrap_or(Ty::Err);
                        *bid = self.declare(bn, ty, Mode::Own, false);
                    }
                    self.dscopes.push(vec![]);
                    self.stmts(&mut arm.body);
                    self.close_scope(&mut arm.body);
                    self.scopes.pop();
                    let after = std::mem::take(&mut self.st);
                    acc = Some(match acc {
                        None => after,
                        Some(a) => merge(a, after),
                    });
                }
                if let Some(e) = eid {
                    if !wildcard {
                        let def = self.types.enums[e].clone();
                        let missing: Vec<&str> = def
                            .variants
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !seen.contains(i))
                            .map(|(_, v)| v.0.as_str())
                            .collect();
                        if !missing.is_empty() {
                            self.err(
                                diag(
                                    format!(
                                        "`match` is not exhaustive: missing {}",
                                        missing.join(", ")
                                    ),
                                    pos,
                                )
                                .note("help: add the missing arms, or a catch-all arm `_ => { }`"),
                            );
                        }
                    }
                }
                self.st = acc.unwrap_or(before);
            }
            SK::Defer(inner) => {
                // type-check the statement now, but it only runs at scope exit: moves it makes are not
                // applied here, and `check_defers` verifies at each exit that what it uses is still alive.
                let saved = self.st.clone();
                self.in_defer = true;
                self.stmt(inner);
                self.in_defer = false;
                self.st = saved;
                let c = (**inner).clone();
                if let Some(d) = self.dscopes.last_mut() {
                    d.push(c);
                }
            }
            SK::Return(opt) => {
                if let Some(e) = opt {
                    // `return x` with an owned local x: implicit move
                    let conv = if let EK::Var(n, _) = &e.k {
                        match self.lookup(n) {
                            Some(i) if self.types.owned(self.vars[i].ty) => {
                                Some((n.clone(), self.vars[i].mode, self.vars[i].ty))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some((n, mode, cty)) = conv {
                        if mode != Mode::Own {
                            let mut d = diag(format!("cannot return '{}': it is only borrowed", n), e.pos)
                                .note("help: borrows cannot leave the function; take the value by ownership (without `view`/`mut`)");
                            if let Some(nt) = self.fn_borrow_note(cty) {
                                d = d.note(nt);
                            }
                            self.err(d);
                            self.st.dead = true;
                            return;
                        }
                        e.k = EK::Move(n, 0);
                    }
                    self.hint = if self.ret_free { None } else { Some(self.ret) };
                    let t = self.expr(e);
                    if pos.line < PRELUDE_LINE && self.types.contains_join_handle(t) {
                        self.err(diag(
                            "a JoinHandle cannot leave its scope through a return",
                            e.pos,
                        ));
                    }
                    if self.ret_free {
                        // a closure without a declared result type: the first `return` decides it
                        self.ret_free = false;
                        if t == Ty::Unit {
                            self.ret = Ty::Unit;
                            self.unit_ret = true;
                        } else {
                            self.ret = t;
                        }
                    } else if self.ret == Ty::Unit {
                        self.err(diag("this function returns no value", e.pos));
                    } else {
                        let r = self.ret;
                        let n0 = self.errs.len();
                        self.expect(t, r, e.pos);
                        if self.closure_depth > 0
                            && self.errs.len() > n0
                            && t != Ty::Err
                            && r != Ty::Err
                        {
                            let (g, w) = (self.tn(t), self.tn(r));
                            let last = self.errs.len() - 1;
                            self.errs[last].notes.push(format!("this closure returns {}, but the function type it is given to says it returns {}", g, w));
                        }
                    }
                } else if self.ret_free {
                    self.ret = Ty::Unit;
                    self.ret_free = false;
                } else if self.ret != Ty::Unit {
                    let n = self.tn(self.ret);
                    self.err(diag(format!("this function must return a {}", n), pos));
                }
                self.st.dead = true;
            }
        }
    }

    fn resolve_pat(&mut self, p: &Pat, ty: Ty, names: &mut Vec<(String, Ty)>) -> RP {
        let tn = self.tn(ty);
        let mismatch = |this: &mut Self, what: &str, pos: Pos| {
            this.err(diag(
                format!(
                    "this pattern is {} but the value being matched is {}",
                    what, tn
                ),
                pos,
            ));
            RP::Wild
        };
        if ty == Ty::Err {
            return RP::Wild;
        }
        match p {
            Pat::Wild(_) => RP::Wild,
            Pat::Bind(n, None, pos) => {
                if let Ty::Enum(e) = ty {
                    if let Some(vi) = self.types.enums[e].variants.iter().position(|v| v.0 == *n) {
                        if self.types.enums[e].variants[vi].1.is_empty() {
                            return RP::Variant(vi, vec![]);
                        }
                    }
                }
                if names.iter().any(|(x, _)| x == n) {
                    self.err(diag(
                        format!("the name '{}' is bound twice in this pattern", n),
                        *pos,
                    ));
                }
                names.push((n.clone(), ty));
                RP::Bind(n.clone(), *pos, None)
            }
            Pat::Bind(n, Some(sub), pos) => {
                if names.iter().any(|(x, _)| x == n) {
                    self.err(diag(
                        format!("the name '{}' is bound twice in this pattern", n),
                        *pos,
                    ));
                }
                names.push((n.clone(), ty));
                let sp = self.resolve_pat(sub, ty, names);
                RP::Bind(n.clone(), *pos, Some(Box::new(sp)))
            }
            Pat::Int(n, pos) => {
                if !matches!(ty, Ty::Int | Ty::Range(..)) {
                    return mismatch(self, "an Int", *pos);
                }
                RP::Range(Some(*n), Some(*n), false)
            }
            Pat::Char(n, pos) => {
                if ty != Ty::Char {
                    return mismatch(self, "a Char", *pos);
                }
                RP::Range(Some(*n), Some(*n), true)
            }
            Pat::Range(lo, hi, ch, pos) => {
                if if *ch {
                    ty != Ty::Char
                } else {
                    !matches!(ty, Ty::Int | Ty::Range(..))
                } {
                    return mismatch(
                        self,
                        if *ch { "a Char range" } else { "an Int range" },
                        *pos,
                    );
                }
                RP::Range(*lo, *hi, *ch)
            }
            Pat::Bool(b, pos) => {
                if ty != Ty::Bool {
                    return mismatch(self, "a Bool", *pos);
                }
                RP::Bool(*b)
            }
            Pat::Variant(n, subs, pos) => {
                let Ty::Enum(e) = ty else {
                    return mismatch(self, "an enum variant", *pos);
                };
                let def = self.types.enums[e].clone();
                let Some(vi) = def.variants.iter().position(|v| v.0 == *n) else {
                    let vs: Vec<&str> = def.variants.iter().map(|v| v.0.as_str()).collect();
                    self.err(
                        diag(format!("enum {} has no variant '{}'", def.name, n), *pos)
                            .note(format!("variants: {}", vs.join(", "))),
                    );
                    return RP::Wild;
                };
                let payload = def.variants[vi].1.clone();
                if payload.len() != subs.len() {
                    self.err(diag(
                        format!(
                            "{}.{} carries {} value(s) but the pattern binds {}",
                            def.name,
                            n,
                            payload.len(),
                            subs.len()
                        ),
                        *pos,
                    ));
                    return RP::Wild;
                }
                let rs = subs
                    .iter()
                    .zip(payload)
                    .map(|(sp, pt)| self.resolve_pat(sp, pt, names))
                    .collect();
                RP::Variant(vi, rs)
            }
            Pat::Tuple(subs, pos) => match self.tuple_elems(ty) {
                Some(es) if es.len() == subs.len() => RP::Tuple(
                    subs.iter()
                        .zip(es)
                        .map(|(sp, et)| self.resolve_pat(sp, et, names))
                        .collect(),
                ),
                Some(es) => {
                    self.err(diag(
                        format!(
                            "this pattern has {} elements but {} has {}",
                            subs.len(),
                            tn,
                            es.len()
                        ),
                        *pos,
                    ));
                    RP::Wild
                }
                None => mismatch(self, "a tuple", *pos),
            },
            Pat::Or(alts, pos) => {
                let mut first: Option<Vec<(String, Ty)>> = None;
                let mut out = vec![];
                for a in alts {
                    let mut nm = vec![];
                    out.push(self.resolve_pat(a, ty, &mut nm));
                    match &first {
                        None => first = Some(nm),
                        Some(f) => {
                            let mut x: Vec<&String> = f.iter().map(|q| &q.0).collect();
                            let mut y: Vec<&String> = nm.iter().map(|q| &q.0).collect();
                            x.sort();
                            y.sort();
                            if x != y {
                                self.err(diag(
                                    "every alternative of an or-pattern must bind the same names",
                                    *pos,
                                ));
                            }
                        }
                    }
                }
                names.extend(first.unwrap_or_default());
                RP::Or(out)
            }
        }
    }

    /// Condition that `subj` matches `p` (None: always).
    fn pat_test(&self, p: &RP, subj: &Expr, ty: Ty, pos: Pos) -> Option<Expr> {
        let and = |a: Option<Expr>, b: Option<Expr>| match (a, b) {
            (None, x) | (x, None) => x,
            (Some(a), Some(b)) => Some(s_bin("&&", a, b, pos)),
        };
        match p {
            RP::Wild | RP::Bind(_, _, None) => None,
            RP::Bind(_, _, Some(sub)) => self.pat_test(sub, subj, ty, pos),
            RP::Range(lo, hi, ch) => {
                let lit = |n: i64| sx(if *ch { EK::Char(n) } else { EK::Int(n) }, pos);
                match (lo, hi) {
                    (Some(a), Some(b)) if a == b => Some(s_bin("==", subj.clone(), lit(*a), pos)),
                    (a, b) => {
                        let l = a.map(|a| s_bin(">=", subj.clone(), lit(a), pos));
                        let h = b.map(|b| s_bin("<=", subj.clone(), lit(b), pos));
                        and(l, h)
                    }
                }
            }
            RP::Bool(true) => Some(subj.clone()),
            RP::Bool(false) => Some(sx(EK::Un("!", Box::new(subj.clone())), pos)),
            RP::Variant(vi, subs) => {
                let Ty::Enum(e) = ty else { return None };
                let payload = self.types.enums[e].variants[*vi].1.clone();
                let mut c = Some(sx(EK::IsVariant(Box::new(subj.clone()), *vi), pos));
                for (k, (sp, pt)) in subs.iter().zip(payload).enumerate() {
                    let sub = sx(EK::Payload(Box::new(subj.clone()), *vi, k), pos);
                    c = and(c, self.pat_test(sp, &sub, pt, pos));
                }
                c
            }
            RP::Tuple(subs) => {
                let es = self.tuple_elems(ty).unwrap_or_default();
                let mut c = None;
                for (k, (sp, et)) in subs.iter().zip(es).enumerate() {
                    let sub = sx(EK::Field(Box::new(subj.clone()), format!("_{}", k)), pos);
                    c = and(c, self.pat_test(sp, &sub, et, pos));
                }
                c
            }
            RP::Or(alts) => {
                let mut acc: Option<Expr> = None;
                for a in alts {
                    {
                        let t = self.pat_test(a, subj, ty, pos)?;
                        acc = Some(match acc {
                            None => t,
                            Some(x) => s_bin("||", x, t, pos),
                        })
                    }
                }
                acc
            }
        }
    }

    /// (name, position, value, type) for every binding of `p`, reading from `subj`.
    fn pat_binds(
        &self,
        p: &RP,
        subj: &Expr,
        ty: Ty,
        pos: Pos,
        out: &mut Vec<(String, Pos, Expr, Ty)>,
    ) {
        match p {
            RP::Bind(n, bp, sub) => {
                out.push((n.clone(), *bp, subj.clone(), ty));
                if let Some(sub) = sub {
                    self.pat_binds(sub, subj, ty, pos, out);
                }
            }
            RP::Variant(vi, subs) => {
                if let Ty::Enum(e) = ty {
                    let payload = self.types.enums[e].variants[*vi].1.clone();
                    for (k, (sp, pt)) in subs.iter().zip(payload).enumerate() {
                        let sub = sx(EK::Payload(Box::new(subj.clone()), *vi, k), pos);
                        self.pat_binds(sp, &sub, pt, pos, out);
                    }
                }
            }
            RP::Tuple(subs) => {
                let es = self.tuple_elems(ty).unwrap_or_default();
                for (k, (sp, et)) in subs.iter().zip(es).enumerate() {
                    let sub = sx(EK::Field(Box::new(subj.clone()), format!("_{}", k)), pos);
                    self.pat_binds(sp, &sub, et, pos, out);
                }
            }
            RP::Or(alts) => {
                if let Some(a) = alts.first() {
                    self.pat_binds(a, subj, ty, pos, out);
                }
            }
            _ => {}
        }
    }

    /// Is there a value of the types `tys` that no row matches? Returns a description of one.
    fn missing(&self, rows: &[Vec<RP>], tys: &[Ty]) -> Option<Vec<String>> {
        if tys.is_empty() {
            return if rows.is_empty() { Some(vec![]) } else { None };
        }
        let mut rs: Vec<Vec<RP>> = vec![];
        for r in rows {
            rp_rows(&r.iter().map(rp_strip).collect::<Vec<_>>(), &mut rs);
        }
        let (ty, rest) = (tys[0], &tys[1..]);
        let wilds = |n: usize| vec![RP::Wild; n];
        let spec = |f: &dyn Fn(&RP) -> Option<Vec<RP>>, arity: usize| -> Vec<Vec<RP>> {
            rs.iter()
                .filter_map(|r| {
                    let mut head = match &r[0] {
                        RP::Wild => Some(wilds(arity)),
                        h => f(h),
                    }?;
                    head.extend_from_slice(&r[1..]);
                    Some(head)
                })
                .collect()
        };
        let join = |name: String, w: &[String], arity: usize, wrap: bool| -> Vec<String> {
            let mut out = vec![if wrap {
                format!("{}({})", name, w[..arity].join(", "))
            } else {
                name
            }];
            out.extend_from_slice(&w[arity..]);
            out
        };
        match ty {
            Ty::Range(lo, hi) => {
                // The one-column interval merge below proves finite scalar range
                // matches. For tuple columns, keep the conservative wildcard rule
                // until the product-space interval matrix analysis is available.
                if !rest.is_empty() {
                    return if rs.iter().any(|r| matches!(r[0], RP::Wild)) {
                        None
                    } else {
                        Some(vec!["_".to_string()])
                    };
                }
                let mut spans: Vec<(i64, i64)> = vec![];
                for r in &rs {
                    match &r[0] {
                        RP::Wild => return None,
                        RP::Range(a, b, false) => {
                            let a = a.unwrap_or(lo).max(lo);
                            let b = b.unwrap_or(hi).min(hi);
                            if a <= b {
                                spans.push((a, b));
                            }
                        }
                        _ => {}
                    }
                }
                spans.sort_unstable();
                let mut next = lo as i128;
                for (a, b) in spans {
                    if (a as i128) > next {
                        return Some(vec![next.to_string()]);
                    }
                    next = next.max(b as i128 + 1);
                    if next > hi as i128 {
                        return None;
                    }
                }
                if next <= hi as i128 {
                    Some(vec![next.to_string()])
                } else {
                    None
                }
            }
            Ty::Enum(e) => {
                let def = &self.types.enums[e];
                for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                    let sp = spec(
                        &|h| {
                            if let RP::Variant(v, s) = h {
                                if *v == vi {
                                    Some(s.clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        },
                        ps.len(),
                    );
                    let mut t2 = ps.clone();
                    t2.extend_from_slice(rest);
                    if let Some(w) = self.missing(&sp, &t2) {
                        return Some(join(vn.clone(), &w, ps.len(), !ps.is_empty()));
                    }
                }
                None
            }
            Ty::Bool => {
                for b in [true, false] {
                    let sp = spec(
                        &|h| {
                            if let RP::Bool(x) = h {
                                if *x == b {
                                    Some(vec![])
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        },
                        0,
                    );
                    if let Some(w) = self.missing(&sp, rest) {
                        return Some(join(b.to_string(), &w, 0, false));
                    }
                }
                None
            }
            t if self.tuple_elems(t).is_some() => {
                let es = self.tuple_elems(t).unwrap();
                let sp = spec(
                    &|h| {
                        if let RP::Tuple(s) = h {
                            Some(s.clone())
                        } else {
                            None
                        }
                    },
                    es.len(),
                );
                let mut t2 = es.clone();
                t2.extend_from_slice(rest);
                let w = self.missing(&sp, &t2)?;
                Some(
                    vec![format!("({})", w[..es.len()].join(", "))]
                        .into_iter()
                        .chain(w[es.len()..].iter().cloned())
                        .collect(),
                )
            }
            _ => {
                // Int / Char: literals never cover everything, so only rows that accept any value count
                let sp = spec(&|_| None, 0);
                let w = self.missing(&sp, rest)?;
                Some(std::iter::once("_".to_string()).chain(w).collect())
            }
        }
    }

    /// `match` with guards, `|`, literals, ranges, tuples, nested patterns or `name @ pattern`: rewritten into
    /// `let $m = scrutinee` and a chain of `if` tests, so the backends see nothing new.
    fn match_general(
        &mut self,
        scrut: Expr,
        arms: Vec<Arm>,
        pos: Pos,
        kind: MK,
        checked: Option<Ty>,
    ) -> (Vec<Stmt>, bool) {
        // temporaries hoisted while the scrutinee was checked (a Text literal lent to a `view String` parameter)
        // stay in front of the whole match; `chk` below must not move them behind the hidden scrutinee variable
        let outer = std::mem::take(&mut self.pre);
        let r = self.match_general_inner(scrut, arms, pos, kind, checked);
        self.pre = outer;
        r
    }

    fn match_general_inner(
        &mut self,
        scrut: Expr,
        arms: Vec<Arm>,
        pos: Pos,
        kind: MK,
        checked: Option<Ty>,
    ) -> (Vec<Stmt>, bool) {
        self.hid += 1;
        let (nm, nd) = (format!("$m{}", self.hid), format!("$d{}", self.hid));
        self.scopes.push(HashMap::new());
        self.dscopes.push(vec![]);
        let mut out: Vec<Stmt> = vec![];
        let sp = scrut.pos;
        match kind {
            MK::Borrow(m) => {
                // the scrutinee stays where it is: `$m` is a borrow of it
                let mut m = m;
                let mut probe = scrut.clone();
                if let Some(p) = self.place(&mut probe) {
                    if m == Mode::Mut {
                        self.ev_escape.push(p.root);
                    }
                    if m == Mode::Mut && !p.mutable {
                        let n = place_str(&scrut);
                        let why = self.immutable_reason(p.root_mode);
                        self.err(
                            diag(format!("cannot `match mut` on '{}': {}", n, why), sp)
                                .note("help: declare it with `var` (or receive it as `mut`) to modify its payloads in place"),
                        );
                        m = Mode::View;
                    }
                }
                let mut fz = scrut.clone();
                self.resolve_ids(&mut fz);
                self.match_scruts
                    .insert(nm.clone(), (fz, m == Mode::Mut, sp.line));
                self.chk(&mut out, s_stmt(SK::Bind(nm.clone(), m, scrut, 0), sp));
            }
            _ => match checked {
                Some(t) => {
                    // already checked (a temporary): declare the hidden variable directly
                    let id = self.declare(
                        &nm,
                        if t == Ty::Unit { Ty::Err } else { t },
                        Mode::Own,
                        false,
                    );
                    out.push(s_stmt(SK::Let(nm.clone(), false, scrut, id), sp));
                }
                None => {
                    // `match x { .. }` on an integer variable: what the arms prove about the copy holds for `x` too
                    let orig = if let EK::Var(n, _) = &scrut.k {
                        self.lookup(n)
                    } else {
                        None
                    };
                    self.chk(&mut out, s_let(&nm, false, scrut, sp));
                    if let (Some(o), Some(m)) = (orig, self.lookup(&nm)) {
                        if self.stable(o) && matches!(self.vars[o].ty, Ty::Int | Ty::Range(..)) {
                            self.alias.insert(m, o);
                        }
                    }
                }
            },
        }
        let ty = self.lookup(&nm).map(|i| self.vars[i].ty).unwrap_or(Ty::Err);
        let nm_id = self.lookup(&nm).unwrap_or(0);
        let ok = matches!(
            ty,
            Ty::Int | Ty::Range(..) | Ty::Char | Ty::Bool | Ty::Enum(_)
        ) || self.tuple_elems(ty).is_some();
        let mut exits = false;
        if !ok {
            if ty != Ty::Err {
                let n = self.tn(ty);
                self.err(
                    diag(format!("cannot `match` on a value of type {}", n), sp)
                        .note(
                            "`match` works on enums, tuples of copyable values, Int, Char and Bool",
                        )
                        .note(
                            "help: for Text and String compare with `==` or `.eq(..)` in an `if`",
                        ),
                );
            }
        } else if self.types.owned(ty) && !matches!(ty, Ty::Enum(_)) {
            self.err(
                diag("cannot `match` on a tuple that owns memory", sp)
                    .note("help: take it apart first: `let (a, b) = move pair`"),
            );
        } else {
            struct G {
                rp: RP,
                guard: Option<Expr>,
                body: Vec<Stmt>,
                pos: Pos,
            }
            let nerr0 = self.errs.len();
            let mut gs: Vec<G> = vec![];
            for arm in arms {
                let pat = arm.pat.clone().unwrap_or_else(|| {
                    // a plain arm mixed with general ones
                    if arm.vname == "_" {
                        Pat::Wild(arm.pos)
                    } else {
                        Pat::Variant(
                            arm.vname.clone(),
                            arm.binds
                                .iter()
                                .map(|b| {
                                    if b.0 == "_" {
                                        Pat::Wild(arm.pos)
                                    } else {
                                        Pat::Bind(b.0.clone(), None, arm.pos)
                                    }
                                })
                                .collect(),
                            arm.pos,
                        )
                    }
                });
                let nerr_arm = self.errs.len();
                // checked before expanding: once split, each alternative looks like its own arm
                if let Some((pos, a, b)) = pat_or_mismatch(&pat) {
                    let show = |v: &[String]| {
                        if v.is_empty() {
                            "nothing".to_string()
                        } else {
                            v.iter()
                                .map(|n| format!("'{}'", n))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    };
                    self.err(
                        diag("every alternative of an or-pattern must bind the same names", pos)
                            .note(format!("one alternative binds {} but another binds {}", show(&a), show(&b)))
                            .note("help: use the same names in every alternative, or split them into separate arms"),
                    );
                }
                let bad = self.errs.len() > nerr_arm;
                let pats = pat_expand(&pat);
                if pats.len() > 64 {
                    self.err(
                        diag("this or-pattern expands to too many alternatives", arm.pos)
                            .note("help: split it into several arms"),
                    );
                    continue;
                }
                for p in pats {
                    let mut names = vec![];
                    let rp = self.resolve_pat(&p, ty, &mut names);
                    // a bad or-pattern still counts for exhaustiveness; its body is not checked
                    let body = if bad { vec![] } else { arm.body.clone() };
                    gs.push(G {
                        rp,
                        guard: arm.guard.clone(),
                        body,
                        pos: arm.pos,
                    });
                }
            }
            // exhaustiveness: only arms without a guard count
            let rows: Vec<Vec<RP>> = gs
                .iter()
                .filter(|g| g.guard.is_none())
                .map(|g| vec![g.rp.clone()])
                .collect();
            let exhaustive = self.missing(&rows, &[ty]).is_none();
            if !exhaustive {
                let mut list = vec![];
                if let Ty::Enum(e) = ty {
                    let def = self.types.enums[e].clone();
                    let mut rs: Vec<Vec<RP>> = vec![];
                    for r in &rows {
                        rp_rows(&r.iter().map(rp_strip).collect::<Vec<_>>(), &mut rs);
                    }
                    for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                        let sp: Vec<Vec<RP>> = rs
                            .iter()
                            .filter_map(|r| match &r[0] {
                                RP::Variant(v, s) if *v == vi => Some(s.clone()),
                                RP::Wild => Some(vec![RP::Wild; ps.len()]),
                                _ => None,
                            })
                            .collect();
                        if let Some(w) = self.missing(&sp, ps) {
                            list.push(if ps.is_empty() {
                                vn.clone()
                            } else {
                                format!("{}({})", vn, w.join(", "))
                            });
                        }
                    }
                }
                if list.is_empty() {
                    if let Some(w) = self.missing(&rows, &[ty]) {
                        list.push(if w[0] == "_" {
                            "other values".to_string()
                        } else {
                            w[0].clone()
                        });
                    }
                }
                let mut d = diag(
                    format!("`match` is not exhaustive: missing {}", list.join(", ")),
                    pos,
                );
                if gs.iter().any(|g| g.guard.is_some()) {
                    d = d.note("arms with a guard (`if ..`) do not count: the compiler cannot know when a guard holds");
                }
                self.err(d.note("help: add the missing arms, or a catch-all arm `_ => { }`"));
            }
            // an arm after a catch-all can never run
            for i in 0..gs.len().saturating_sub(1) {
                if self.errs.len() == nerr0
                    && gs[i].guard.is_none()
                    && self
                        .pat_test(&gs[i].rp, &s_var(&nm, pos), ty, pos)
                        .is_none()
                {
                    self.err(
                        diag(
                            "this arm can never run: an earlier arm already matches every value",
                            gs[i + 1].pos,
                        )
                        .note(format!(
                            "the arm on line {} accepts everything",
                            gs[i].pos.line
                        ))
                        .note("help: put the catch-all arm last, or remove this one"),
                    );
                    break;
                }
            }
            let subj = s_var(&nm, pos);
            let any_guard = gs.iter().any(|g| g.guard.is_some());
            let all_exit = gs.iter().all(|g| always_returns(&g.body));
            // per arm: test, guard, bindings, body
            let mut parts: Vec<(Option<Expr>, Option<Expr>, Vec<Stmt>, Vec<Stmt>)> = vec![];
            for g in gs {
                let test = self.pat_test(&g.rp, &subj, ty, g.pos);
                let lets =
                    self.arm_binds(&g.rp, &subj, ty, g.pos, kind, &nm, nm_id, g.guard.is_some());
                parts.push((test, g.guard, lets, g.body));
            }
            let n_parts = parts.len();
            let mut chain: Vec<Stmt> = vec![];
            if !any_guard {
                let mut acc: Option<Vec<Stmt>> = None;
                for (i, (test, _, lets, body)) in parts.into_iter().enumerate().rev() {
                    let body: Vec<Stmt> = lets.into_iter().chain(body).collect();
                    match test {
                        None => acc = Some(body),
                        Some(_) if i + 1 == n_parts && exhaustive => acc = Some(body),
                        Some(t) => acc = Some(vec![s_stmt(SK::If(t, body, acc.take()), pos)]),
                    }
                }
                chain = acc.unwrap_or_default();
            } else {
                self.chk(&mut out, s_let(&nd, true, sx(EK::Bool(false), pos), pos));
                for (test, guard, lets, body) in parts {
                    let not_done = sx(EK::Un("!", Box::new(s_var(&nd, pos))), pos);
                    let cond = match test {
                        Some(t) => s_bin("&&", not_done, t, pos),
                        None => not_done,
                    };
                    let set = s_stmt(SK::Assign(s_var(&nd, pos), sx(EK::Bool(true), pos)), pos);
                    let run: Vec<Stmt> = std::iter::once(set).chain(body).collect();
                    let mut inner = lets;
                    match guard {
                        None => inner.extend(run),
                        Some(gd) => inner.push(s_stmt(SK::If(gd, run, None), pos)),
                    }
                    chain.push(s_stmt(SK::If(cond, inner, None), pos));
                }
            }
            for st in chain {
                self.chk(&mut out, st);
            }
            exits = exhaustive && all_exit;
        }
        self.close_scope(&mut out);
        self.scopes.pop();
        (out, exits)
    }

    /// The statements that bind the names of one `match` arm, according to how the scrutinee is looked at.
    fn arm_binds(
        &mut self,
        rp: &RP,
        subj: &Expr,
        ty: Ty,
        pos: Pos,
        kind: MK,
        nm: &str,
        nm_id: usize,
        guarded: bool,
    ) -> Vec<Stmt> {
        let mut out: Vec<Stmt> = vec![];
        let mut binds = vec![];
        match kind {
            MK::Move if self.types.owned(ty) => {
                match rp {
                    // `x => ..` / `x @ ..` on the whole value: it moves into x
                    RP::Bind(n, bp, sub) => {
                        if sub.is_some() {
                            self.err(
                                diag("a `name @ pattern` cannot move an owned value", *bp).note(
                                    "help: bind the whole value with a plain name, then `match` it",
                                ),
                            );
                        }
                        if n != "_" {
                            let mv = Expr {
                                k: EK::Move(nm.to_string(), nm_id),
                                pos,
                                ty,
                            };
                            out.push(s_let(n, false, mv, *bp));
                        }
                    }
                    RP::Variant(vi, subs) => {
                        let Ty::Enum(e) = ty else { return out };
                        let payload = self.types.enums[e].variants[*vi].1.clone();
                        let owned_pos: Vec<usize> = (0..payload.len())
                            .filter(|k| self.types.owned(payload[*k]))
                            .collect();
                        let moves = owned_pos.iter().any(|k| !matches!(subs[*k], RP::Wild));
                        for (k, (sp, pt)) in subs.iter().zip(payload.iter()).enumerate() {
                            let place = sx(EK::Payload(Box::new(subj.clone()), *vi, k), pos);
                            if owned_pos.contains(&k) {
                                match sp {
                                    RP::Wild => {
                                        if moves {
                                            // the rest of the payloads are moved out too, so this one must still be freed
                                            self.hid += 1;
                                            let hn = format!("$w{}", self.hid);
                                            out.push(s_let(&hn, false, place, pos));
                                        }
                                    }
                                    RP::Bind(n, bp, None) => {
                                        if guarded {
                                            self.err(
                                                diag("a guard (`if ..`) cannot be used on an arm that moves an owned payload", *bp)
                                                    .note("the guard might fail after the payload was already moved into the name, and the next arm could no longer use it")
                                                    .note("help: use `match e { .. }` (borrow the payloads) for the guarded arms, or test inside the arm body"),
                                            );
                                        }
                                        out.push(s_let(n, false, place, *bp));
                                    }
                                    other => {
                                        let _ = other;
                                        self.err(
                                            diag("cannot look inside an owned payload in `match move`", pos)
                                                .note("moving the payload out and testing its parts at the same time is not supported")
                                                .note("help: bind it to a name and `match` the name in the arm body, or use `match e { .. }` to borrow"),
                                        );
                                    }
                                }
                            } else {
                                self.pat_binds(sp, &place, *pt, pos, &mut binds);
                            }
                        }
                        for (n, bp, e, _) in binds.drain(..) {
                            if n != "_" {
                                out.push(s_let(&n, false, e, bp));
                            }
                        }
                        if moves {
                            // every payload now lives in a name of its own: the enum itself must not be freed again
                            let mv = Expr {
                                k: EK::Move(nm.to_string(), nm_id),
                                pos,
                                ty,
                            };
                            out.push(s_stmt(SK::ExprS(mv), pos));
                        }
                    }
                    _ => {
                        self.pat_binds(rp, subj, ty, pos, &mut binds);
                        for (n, bp, e, bty) in binds.drain(..) {
                            if n == "_" {
                                continue;
                            }
                            if self.types.owned(bty) {
                                self.err(diag(format!("cannot bind '{}' here: it is an owned value inside a pattern `match move` cannot take apart", n), bp));
                            } else {
                                out.push(s_let(&n, false, e, bp));
                            }
                        }
                    }
                }
            }
            _ => {
                self.pat_binds(rp, subj, ty, pos, &mut binds);
                for (n, bp, e, bty) in binds {
                    if n == "_" {
                        continue;
                    }
                    match kind {
                        MK::Borrow(m) if self.types.owned(bty) || m == Mode::Mut => {
                            out.push(s_stmt(SK::Bind(n, m, e, 0), bp))
                        }
                        _ => out.push(s_let(&n, false, e, bp)),
                    }
                }
            }
        }
        out
    }

    /// Check one synthetic statement and append it (with whatever `defer` expansion it produced) to `out`.
    fn chk(&mut self, out: &mut Vec<Stmt>, mut st: Stmt) {
        self.stmt(&mut st);
        let pre = std::mem::take(&mut self.pre);
        out.extend(pre);
        out.push(st);
    }

    /// Rewrite and check `for`. Everything becomes `let` / `while` (with a `step` that also runs on
    /// `continue`) so both backends only ever see plain loops:
    ///   `for i in a..b`      var $i = a; let $e = b; while $i < $e { let i = $i; .. ; $i = $i + 1 }
    ///   `for x in l`         let $n = l.len(); var $i = 0; while $i < $n { <x borrows l.at($i)>; .. }
    ///   `for x in move l`    the list is reversed into a hidden list and popped, so early exits drop the rest
    ///   `for c in s.chars()` walks the bytes of `s`, decoding one UTF-8 character at a time
    fn for_stmt(&mut self, h: ForHead, body: Vec<Stmt>, pos: Pos) -> Vec<Stmt> {
        self.hid += 1;
        let id = self.hid;
        let (ni, ne, ns, nn, nl, nr) = (
            format!("$i{id}"),
            format!("$e{id}"),
            format!("$s{id}"),
            format!("$n{id}"),
            format!("$l{id}"),
            format!("$r{id}"),
        );
        self.scopes.push(HashMap::new());
        self.dscopes.push(vec![]);
        let mut out: Vec<Stmt> = vec![];
        let label = h.label.clone();
        let nerr = self.errs.len();
        match h.iter.clone() {
            ForIter::Range(a, b, incl) => {
                let (ap, bp) = (a.pos, b.pos);
                self.chk(&mut out, s_let(&ni, true, a, ap));
                self.chk(&mut out, s_let(&ne, false, b, bp));
                // the loop variable lies between the start and the end (exclusive unless `..=`)
                let init_of = |this: &Self, n: &str| -> Option<(i128, i128)> {
                    this.lookup(n).and_then(|i| this.init_ivl.get(&i).copied())
                };
                if let (Some((alo, _)), Some((_, bhi))) = (init_of(self, &ni), init_of(self, &ne)) {
                    let hi = if incl { bhi } else { bhi - 1 };
                    if alo <= hi && !h.mutable {
                        self.loop_ivl.insert(ni.clone(), (alo, hi));
                    }
                }
                let mut step_expr: Option<Expr> = None;
                if let Some(se) = h.step.clone() {
                    let sp = se.pos;
                    // a literal step, including a negated one (`step -1`), is checked at compile time
                    let lit = match &se.k {
                        EK::Int(n) => Some(*n as i128),
                        EK::Un("-", inner) => match inner.k {
                            EK::Int(n) => Some(-(n as i128)),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(n) = lit {
                        if n <= 0 {
                            self.err(
                                diag("the step of a `for` loop must be positive", sp)
                                    .note("ranges always count upwards from the start towards the end")
                                    .note("help: to count down, loop upwards and compute the value: `for k in 0..n { let i = n - 1 - k }`"),
                            );
                        }
                        step_expr = Some(se);
                    } else {
                        self.chk(&mut out, s_let(&ns, false, se, sp));
                        let c = s_bin(">", s_var(&ns, sp), s_int(0, sp), sp);
                        self.chk(
                            &mut out,
                            s_stmt(SK::ExprS(sx(EK::Call("assert".into(), vec![c]), sp)), sp),
                        );
                        step_expr = Some(s_var(&ns, sp));
                    }
                }
                let mut body2 = vec![];
                if h.var != "_" {
                    body2.push(s_let(&h.var, h.mutable, s_var(&ni, h.var_pos), h.var_pos));
                }
                body2.extend(body);
                let cond = s_bin(
                    if incl { "<=" } else { "<" },
                    s_var(&ni, ap),
                    s_var(&ne, bp),
                    pos,
                );
                let step = match step_expr {
                    None if !incl => vec![s_inc(&ni, s_int(1, pos), pos)],
                    None => vec![s_stmt(
                        SK::If(
                            s_bin("<", s_var(&ni, pos), s_var(&ne, pos), pos),
                            vec![s_inc(&ni, s_int(1, pos), pos)],
                            Some(vec![s_stmt(SK::Break(None, 0), pos)]),
                        ),
                        pos,
                    )],
                    Some(se) => {
                        // leave before `$i + step` could overflow: such a value is past the end anyway
                        let lim = s_bin("-", s_int(i64::MAX, pos), se.clone(), pos);
                        vec![
                            s_stmt(
                                SK::If(
                                    s_bin(">", s_var(&ni, pos), lim, pos),
                                    vec![s_stmt(SK::Break(None, 0), pos)],
                                    None,
                                ),
                                pos,
                            ),
                            s_inc(&ni, se, pos),
                        ]
                    }
                };
                self.chk(&mut out, s_while(cond, body2, step, label, None, pos));
            }
            ForIter::Expr(e) => self.for_each(&mut out, &h, e, body, id, pos),
        }
        let _ = (nn, nl, nr, nerr);
        self.close_scope(&mut out);
        self.scopes.pop();
        out
    }

    /// `for x in <list place | move list | list value | string.chars()>`.
    fn for_each(
        &mut self,
        out: &mut Vec<Stmt>,
        h: &ForHead,
        e: Expr,
        body: Vec<Stmt>,
        id: usize,
        pos: Pos,
    ) {
        let (ni, nn, nl, nr) = (
            format!("$i{id}"),
            format!("$n{id}"),
            format!("$l{id}"),
            format!("$r{id}"),
        );
        let ep = e.pos;
        let label = h.label.clone();
        if let Some(sp) = &h.step {
            self.err(
                diag("`step` only works with ranges", sp.pos)
                    .note("example: `for i in 0..100 step 5 { .. }`"),
            );
        }
        let teach = |d: Diag| {
            d.note("you can loop over a range (`for i in 0..n`), a List (`for x in list`, `for mut x in list`, `for x in move list`) or the characters of a String (`for c in s.chars()`)")
        };
        // ── characters of a String ──
        if let EK::Method { recv, m, args } = &e.k {
            if m == "chars" && args.is_empty() {
                let mut place = (**recv).clone();
                if !simple_place(&place) {
                    self.err(
                        diag("`chars()` needs a variable or a field here", ep)
                            .note("help: store the String in a variable first"),
                    );
                    return;
                }
                let mut probe = place.clone();
                let Some(p) = self.place(&mut probe) else {
                    return;
                };
                if p.ty != Ty::String {
                    let n = self.tn(p.ty);
                    self.err(
                        diag(
                            format!("`chars()` is a String method, but this is {}", n),
                            ep,
                        )
                        .note("help: for a Text, make a String first: `let s = String.from(text)`"),
                    );
                    return;
                }
                if h.mutable {
                    self.err(diag(
                        "a `for` over `chars()` cannot be `mut`: characters are read-only",
                        h.var_pos,
                    ));
                }
                self.resolve_ids(&mut place);
                self.chk(
                    out,
                    s_let(&nn, false, s_meth(place.clone(), "len", vec![], ep), ep),
                );
                self.chk(out, s_let(&ni, true, s_int(0, ep), ep));
                let mut body2 = vec![s_let(
                    &h.var,
                    false,
                    s_meth(place.clone(), "char_at", vec![s_var(&ni, ep)], ep),
                    h.var_pos,
                )];
                body2.extend(body);
                let step = vec![s_inc(
                    &ni,
                    s_meth(place.clone(), "char_size", vec![s_var(&ni, ep)], ep),
                    ep,
                )];
                let cond = s_bin("<", s_var(&ni, ep), s_var(&nn, ep), ep);
                self.chk(
                    out,
                    s_while(cond, body2, step, label, Some((place, false)), pos),
                );
                return;
            }
        }
        // `for x in l[lo..hi]`: the borrowing loop over the indexes lo..hi of the place `l`
        let (e, range) = match e.k {
            EK::SliceOf(b, lo, hi) => (*b, Some((lo, hi))),
            k => (Expr { k, ..e }, None),
        };
        let borrow = !matches!(e.k, EK::Move(..)) && self.isp(&e);
        if range.is_some() && !borrow {
            self.err(diag("cannot loop over a slice of a temporary", ep).note(
                "help: store the array or List in a variable first, then loop over `v[from..to]`",
            ));
            return;
        }
        if borrow {
            if !simple_place(&e) {
                self.err(
                    diag("a `for` loop over a list needs a variable or a field", ep)
                        .note("the list is read again on every iteration, so it must be a plain place such as `l`, `s.items` or `grid.at(i)`")
                        .note("help: store it in a variable first, or use `for x in move expr` to consume a list value"),
                );
                return;
            }
            let mut probe = e.clone();
            let Some(p) = self.place(&mut probe) else {
                return;
            };
            let Some(et) = self.types.seq_elem(p.ty) else {
                let n = self.tn(p.ty);
                if p.ty != Ty::Err {
                    self.err(teach(diag(
                        format!("cannot loop over a value of type {}", n),
                        ep,
                    )));
                }
                return;
            };
            let mut place = e.clone();
            self.resolve_ids(&mut place);
            let len = s_meth(place.clone(), "len", vec![], ep);
            match range {
                None => {
                    self.chk(out, s_let(&nn, false, len, ep));
                    self.chk(out, s_let(&ni, true, s_int(0, ep), ep));
                }
                Some((lo, hi)) => {
                    // bounds left to right, then `0 <= lo <= hi <= len` (else the slice panic)
                    let lo = lo.map(|x| *x).unwrap_or_else(|| s_int(0, ep));
                    self.chk(out, s_let(&ni, true, lo, ep));
                    let hi = hi.map(|x| *x).unwrap_or_else(|| len.clone());
                    self.chk(out, s_let(&nn, false, hi, ep));
                    let bad = s_bin(
                        "||",
                        s_bin(
                            "||",
                            s_bin("<", s_var(&ni, ep), s_int(0, ep), ep),
                            s_bin(">", s_var(&ni, ep), s_var(&nn, ep), ep),
                            ep,
                        ),
                        s_bin(">", s_var(&nn, ep), len, ep),
                        ep,
                    );
                    let panic = s_stmt(
                        SK::ExprS(sx(
                            EK::Call(
                                "panic".into(),
                                vec![sx(EK::Text("index out of range".into()), ep)],
                            ),
                            ep,
                        )),
                        ep,
                    );
                    self.chk(out, s_stmt(SK::If(bad, vec![panic], None), ep));
                }
            }
            let at = s_meth(place.clone(), "at", vec![s_var(&ni, ep)], ep);
            let first = if !h.mutable && !self.types.owned(et) {
                s_let(&h.var, false, at, h.var_pos)
            } else {
                s_stmt(
                    SK::Bind(
                        h.var.clone(),
                        if h.mutable { Mode::Mut } else { Mode::View },
                        at,
                        0,
                    ),
                    h.var_pos,
                )
            };
            let mut body2 = vec![first];
            body2.extend(body);
            let cond = s_bin("<", s_var(&ni, ep), s_var(&nn, ep), ep);
            let step = vec![s_inc(&ni, s_int(1, ep), ep)];
            self.chk(
                out,
                s_while(cond, body2, step, label, Some((place, h.mutable)), pos),
            );
            return;
        }
        // ── consume a list: `move l` or any list value ──
        self.chk(out, s_let(&nl, true, e, ep));
        let lt = self.lookup(&nl).map(|i| self.vars[i].ty).unwrap_or(Ty::Err);
        if self.types.array_of(lt).is_some() {
            // an array value (`for x in [1, 2, 3]`): kept in the hidden variable and read in place
            let mut h2 = h.clone();
            h2.step = None;
            self.for_each(out, &h2, s_var(&nl, ep), body, id, pos);
            return;
        }
        let Ty::List(_) = lt else {
            if lt != Ty::Err {
                let n = self.tn(lt);
                self.err(teach(diag(
                    format!("cannot loop over a value of type {}", n),
                    ep,
                )));
            }
            return;
        };
        self.chk(
            out,
            s_let(
                &nr,
                true,
                sx(
                    EK::Static {
                        ty: lt,
                        f: "new".into(),
                        args: vec![],
                    },
                    ep,
                ),
                ep,
            ),
        );
        let pop = s_meth(s_var(&nl, ep), "pop", vec![], ep);
        let push = s_stmt(SK::ExprS(s_meth(s_var(&nr, ep), "push", vec![pop], ep)), ep);
        let c1 = s_bin(
            ">",
            s_meth(s_var(&nl, ep), "len", vec![], ep),
            s_int(0, ep),
            ep,
        );
        self.chk(out, s_while(c1, vec![push], vec![], None, None, ep));
        let mut body2 = vec![s_let(
            &h.var,
            h.mutable,
            s_meth(s_var(&nr, ep), "pop", vec![], ep),
            h.var_pos,
        )];
        body2.extend(body);
        let c2 = s_bin(
            ">",
            s_meth(s_var(&nr, ep), "len", vec![], ep),
            s_int(0, ep),
            ep,
        );
        self.chk(out, s_while(c2, body2, vec![], label, None, pos));
    }

    /// Element types of a tuple type (`(Int, Text)`), or None for any other type.
    fn tuple_elems(&self, t: Ty) -> Option<Vec<Ty>> {
        if let Ty::Struct(i) = t {
            let d = &self.types.structs[i];
            if let Some((n, args)) = &d.origin {
                if n.len() == 6 && n.starts_with("Tuple") {
                    return Some(args.clone());
                }
            }
        }
        None
    }

    fn declare_unpack(&mut self, pat: &mut UnpackPat, ty: Ty, pos: Pos) {
        match pat {
            UnpackPat::Bind(name, mutable, id, _) => {
                *id = self.declare(name, ty, Mode::Own, *mutable)
            }
            UnpackPat::Tuple(items, ppos) => {
                let elems = self.tuple_elems(ty);
                let ppos = *ppos;
                match elems {
                    Some(es) if es.len() == items.len() => {
                        for (it, et) in items.iter_mut().zip(es) {
                            self.declare_unpack(it, et, pos);
                        }
                    }
                    other => {
                        if ty != Ty::Err {
                            let tn = self.tn(ty);
                            let d = match other {
                                Some(es) => diag(format!("this pattern has {} names but {} has {} elements", items.len(), tn, es.len()), ppos)
                                    .note("help: write one name per element, or `_` for the ones you do not need"),
                                None => diag(format!("cannot destructure a value of type {}: only tuples can be taken apart this way", tn), ppos)
                                    .note("help: for a struct, read its fields: `let a = p.x`"),
                            };
                            self.err(d);
                        }
                        for it in items.iter_mut() {
                            self.declare_unpack(it, Ty::Err, pos);
                        }
                    }
                }
            }
        }
    }

    /// `break` / `continue`: find the target loop, queue the deferred statements of every scope left
    /// (innermost first) and record the state for the code after the loop. Returns the loop id.
    fn jump(&mut self, is_break: bool, label: &Option<(String, Pos)>, pos: Pos) -> usize {
        let kw = if is_break { "break" } else { "continue" };
        let idx = match label {
            None => self.loops.len().checked_sub(1),
            Some((n, _)) => self
                .loops
                .iter()
                .rposition(|l| l.label.as_deref() == Some(n.as_str())),
        };
        let i = match idx {
            Some(i) => i,
            None => {
                let d = match label {
                    None => diag(
                        format!("`{}` can only be used inside a `while` or `for` loop", kw),
                        pos,
                    )
                    .note(format!(
                        "`{}` {} the nearest enclosing loop",
                        kw,
                        if is_break {
                            "leaves"
                        } else {
                            "jumps to the next iteration of"
                        }
                    ))
                    .note(if is_break {
                        "help: to leave the whole function use `return` instead"
                    } else {
                        "help: put it inside a loop body"
                    }),
                    Some((n, lp)) => {
                        let known: Vec<String> =
                            self.loops.iter().filter_map(|l| l.label.clone()).collect();
                        let mut d = diag(format!("no enclosing loop is labeled '{}'", n), *lp);
                        d = if known.is_empty() {
                            d.note("this `break`/`continue` is not inside any labeled loop")
                        } else {
                            d.note(format!("labels in scope here: {}", known.join(", ")))
                        };
                        d.note(format!("help: label the loop you mean: `{}: while .. {{ }}` (or `for`), then write `{} {}`", n, kw, n))
                    }
                };
                self.err(d);
                self.st.dead = true;
                return 0;
            }
        };
        let id = self.loops[i].id;
        if self.st.dead {
            return id;
        }
        let d = self.loops[i].dscope_depth;
        let defers: Vec<Stmt> = self.dscopes[d.min(self.dscopes.len())..]
            .iter()
            .rev()
            .flat_map(|x| x.iter().rev().cloned())
            .collect();
        self.check_defers(&defers);
        self.pre = defers;
        let st = self.st.clone();
        if is_break {
            self.loops[i].breaks.push(st);
        } else {
            self.loops[i].conts.push(st);
        }
        self.st.dead = true;
        id
    }

    fn func(&mut self, f: &mut Func) {
        // The name-based scans (`scan_with_lazy`) decide which variables flow facts may be kept about. The
        // checker then records, by variable id, every variable it sees assigned, lent as `mut`, `match mut`ed
        // or captured for writing. A variable that the scans did not know about is a hole in them: the
        // function is checked again from its original text with that name added to the scan sets.
        let orig = f.clone();
        let (n_errs, n_lifted, n_reported) =
            (self.errs.len(), self.lifted.len(), self.reported.clone());
        let mut extra: HashSet<String> = HashSet::new();
        for _round in 0..4 {
            let (changing, unstable) = self.scan_with_lazy(&f.body);
            let changing: HashSet<String> = changing.union(&extra).cloned().collect();
            let unstable: HashSet<String> = unstable.union(&extra).cloned().collect();
            self.ev_escape.clear();
            self.ev_assign.clear();
            self.func_once(f, &changing, unstable.clone());
            let mut missed: Vec<String> = vec![];
            let named = |this: &Self, id: usize| {
                this.vars
                    .get(id)
                    .filter(|v| matches!(v.ty, Ty::Int | Ty::Range(..)))
                    .map(|v| v.name.clone())
                    .filter(|n| !n.starts_with('$'))
            };
            for id in self.ev_escape.clone() {
                if let Some(n) = named(self, id) {
                    if !unstable.contains(&n) || !changing.contains(&n) {
                        missed.push(n);
                    }
                }
            }
            for id in self.ev_assign.clone() {
                if let Some(n) = named(self, id) {
                    if !changing.contains(&n) {
                        missed.push(n);
                    }
                }
            }
            if missed.is_empty() || missed.iter().all(|n| extra.contains(n)) {
                break;
            }
            extra.extend(missed);
            *f = orig.clone();
            self.errs.truncate(n_errs);
            self.lifted.truncate(n_lifted);
            self.reported = n_reported.clone();
        }
    }

    fn func_once(&mut self, f: &mut Func, changing: &HashSet<String>, unstable: HashSet<String>) {
        self.vars.clear();
        self.scopes = vec![HashMap::new()];
        self.st = State::default();
        self.dscopes.clear();
        self.loops.clear();
        self.frozen.clear();
        self.ret = f.ret;
        self.unstable = unstable;
        self.alias.clear();
        for p in f.params.iter_mut() {
            // a by-value integer parameter that the body never changes is immutable: flow facts may narrow it
            let mutable = match p.mode {
                Mode::Own => !matches!(p.ty, Ty::Int | Ty::Range(..)) || changing.contains(&p.name),
                m => m != Mode::View,
            };
            p.id = self.declare(&p.name, p.ty, p.mode, mutable);
            if matches!(p.ty, Ty::Range(..)) {
                self.explicit_range.insert(p.id);
            }
        }
        self.block(&mut f.body);
        if f.ret != Ty::Unit && !always_returns(&f.body) {
            let n = self.tn(f.ret);
            self.err(diag(
                format!("function '{}' must return a {} on every path", f.name, n),
                f.pos,
            ));
        }
    }
}

// ───────────────────────── generics and traits ─────────────────────────

/// Match a parameter type that mentions type parameters (`pids`) against a concrete type, recording
/// what each type parameter stands for. `Err(prior)` reports a clash with an earlier binding.
fn unify(
    types: &Types,
    pat: Ty,
    conc: Ty,
    pids: &[usize],
    bind: &mut Vec<Option<Ty>>,
) -> Result<(), Option<Ty>> {
    if conc == Ty::Err {
        return Ok(());
    }
    if let Ty::Param(p) = pat {
        if let Some(k) = pids.iter().position(|x| *x == p) {
            // An erased pointee can only be named behind a box or a borrow.
            if matches!(conc, Ty::Dyn(_)) {
                return Err(None);
            }
            // inference never invents a range type: a type parameter bound from an `Int<lo..hi>` value is `Int`
            let conc = if matches!(conc, Ty::Range(..)) {
                Ty::Int
            } else {
                conc
            };
            return match bind[k] {
                None => {
                    bind[k] = Some(conc);
                    Ok(())
                }
                Some(b) if b == conc => Ok(()),
                Some(b) => Err(Some(b)),
            };
        }
    }
    // identical types need no work, unless they mention the parameters being bound: a recursive call of a
    // generic function passes `List<T>` for `List<T>` and still has to bind `T` (to itself)
    if pat == conc && !types.has_param(pat) {
        return Ok(());
    }
    // a range value passes where `Int` (or a containing range) is declared
    if let Ty::Range(cl, ch) = conc {
        match pat {
            Ty::Int => return Ok(()),
            Ty::Range(pl, ph) if pl <= cl && ch <= ph => return Ok(()),
            _ => {}
        }
    }
    match (pat, conc) {
        (Ty::List(a), Ty::List(b)) => unify(types, types.lists[a], types.lists[b], pids, bind),
        // an array is lent to a slice parameter
        (Ty::List(a), Ty::Struct(_)) if types.slices.contains(&a) => match types.array_of(conc) {
            Some((e, _)) => unify(types, types.lists[a], e, pids, bind),
            None => Err(None),
        },
        (Ty::Fn(a), Ty::Fn(b)) => {
            let (fa, fb) = (&types.fns[a], &types.fns[b]);
            if fa.params.len() != fb.params.len()
                || fa.params.iter().zip(&fb.params).any(|(x, y)| x.0 != y.0)
            {
                return Err(None);
            }
            for (x, y) in fa.params.iter().zip(&fb.params) {
                unify(types, x.1, y.1, pids, bind)?;
            }
            unify(types, fa.ret, fb.ret, pids, bind)
        }
        (Ty::Box(a), Ty::Box(b)) => unify(types, types.boxes[a], types.boxes[b], pids, bind),
        (Ty::Struct(_), Ty::Struct(_)) if types.array_of(pat).is_some() => {
            match (types.array_of(pat), types.array_of(conc)) {
                (Some((e1, n1)), Some((e2, n2))) if n1 >= crate::syntax::CONST_LEN && n2 < crate::syntax::CONST_LEN => {
                    // `[T; N]` with a const parameter N: bind N to the length (as the type `Int<n..n>`)
                    let p = n1 - crate::syntax::CONST_LEN;
                    let len = Ty::Range(n2 as i64, n2 as i64);
                    if let Some(k) = pids.iter().position(|x| *x == p) {
                        match bind[k] {
                            None => bind[k] = Some(len),
                            Some(b) if b == len => {}
                            Some(b) => return Err(Some(b)),
                        }
                    }
                    unify(types, e1, e2, pids, bind)
                }
                (Some((e1, n1)), Some((e2, n2))) if n1 == n2 => unify(types, e1, e2, pids, bind),
                _ => Err(None),
            }
        }
        (Ty::Struct(a), Ty::Struct(b)) => {
            match (&types.structs[a].origin, &types.structs[b].origin) {
                (Some((n1, a1)), Some((n2, a2))) if n1 == n2 => {
                    for (x, y) in a1.iter().zip(a2) {
                        unify(types, *x, *y, pids, bind)?;
                    }
                    Ok(())
                }
                _ => Err(None),
            }
        }
        (Ty::Enum(a), Ty::Enum(b)) => match (&types.enums[a].origin, &types.enums[b].origin) {
            (Some((n1, a1)), Some((n2, a2))) if n1 == n2 => {
                for (x, y) in a1.iter().zip(a2) {
                    unify(types, *x, *y, pids, bind)?;
                }
                Ok(())
            }
            _ => Err(None),
        },
        _ => Err(None),
    }
}

impl<'a> Checker<'a> {
    /// Run `f` on the parser (which owns instantiation) with the type table lent to it.
    fn with_ps<R>(&mut self, f: impl FnOnce(&mut Parser) -> R) -> R {
        self.ps.types = std::mem::take(&mut self.types);
        let r = f(&mut *self.ps);
        self.types = std::mem::take(&mut self.ps.types);
        let errs = std::mem::take(&mut self.ps.inst_errs);
        self.errs.extend(errs);
        r
    }

    /// Type of a place without checking it (no errors, no ownership effects).
    fn peek_ty(&self, e: &Expr) -> Option<Ty> {
        match &e.k {
            EK::Var(n, _) => self.lookup(n).map(|i| self.vars[i].ty),
            EK::Field(b, f) => match self.peek_ty(b)? {
                Ty::Struct(s) => self.types.structs[s]
                    .fields
                    .iter()
                    .find(|(n, _)| n == f)
                    .map(|x| x.1),
                _ => None,
            },
            EK::Method { recv, m, .. } if m == "at" => self.types.seq_elem(self.peek_ty(recv)?),
            EK::Method { recv, m, args }
                if e.pos.line >= PRELUDE_LINE && m == "locked_value" && args.is_empty() =>
            {
                self.peek_ty(recv)
                    .and_then(|t| self.types.locked_payload(t))
            }
            EK::Method { recv, m, args } if m == "get" && args.is_empty() => {
                match self.peek_ty(recv)? {
                    Ty::Box(i) => Some(self.types.boxes[i]),
                    ty => self.types.shared_payload(ty),
                }
            }
            _ => None,
        }
    }

    fn implements(&self, ty: Ty, tr: &str) -> bool {
        if tr == "Copy" {
            return !self.types.owned(ty) && ty != Ty::Unit;
        }
        if let Ty::Param(i) = ty {
            return self.types.params[i].bounds.iter().any(|b| b == tr);
        }
        // arrays and slices compare and show element-wise (see `seq_rewrite`)
        if let Some(e) = self
            .types
            .array_of(ty)
            .map(|(e, _)| e)
            .or_else(|| self.types.slice_elem(ty))
        {
            if matches!(tr, "Show" | "Equal" | "Ordered") {
                return self.implements(e, tr);
            }
        }
        self.ps
            .impls
            .iter()
            .any(|im| im.trait_name.as_deref() == Some(tr) && self.impl_binds(im, ty).is_some())
    }

    /// Does impl `im` apply to type `ty`? For a generic impl (`impl<T: Show> Show for List<T>`): the type
    /// arguments it binds, provided every bound holds. For a plain impl: no arguments.
    fn impl_binds(&self, im: &ImplDef, ty: Ty) -> Option<Vec<Ty>> {
        if im.generics.is_empty() {
            return if im.self_ty == ty { Some(vec![]) } else { None };
        }
        if matches!(ty, Ty::Err) {
            return None;
        }
        let mut bind = vec![None; im.pids.len()];
        unify(&self.types, im.self_ty, ty, &im.pids, &mut bind).ok()?;
        let args: Vec<Ty> = bind.into_iter().collect::<Option<Vec<Ty>>>()?;
        for (g, a) in im.generics.iter().zip(&args) {
            for (b, _) in &g.bounds {
                if !self.implements(*a, b) {
                    return None;
                }
            }
        }
        Some(args)
    }

    /// Why a method of a generic built-in impl is not available for `ty` (an unmet bound), for error notes.
    fn unmet_bound_note(&self, ty: Ty, m: &str) -> Option<String> {
        for im in &self.ps.impls {
            if im.generics.is_empty() || !im.methods.iter().any(|x| x.name == m) {
                continue;
            }
            let mut bind = vec![None; im.pids.len()];
            if unify(&self.types, im.self_ty, ty, &im.pids, &mut bind).is_err() {
                continue;
            }
            for (g, a) in im.generics.iter().zip(&bind) {
                let a = (*a)?;
                for (b, _) in &g.bounds {
                    if !self.implements(a, b) {
                        let an = self.tn(a);
                        let mut note = format!(
                            "'{}' needs {}: {}, but {} does not implement {}",
                            m, g.name, b, an, b
                        );
                        if b == "Copy" {
                            note += &format!("; {} values are owned and cannot be copied", an);
                            match ty {
                                Ty::List(_) => {
                                    note += "\nhelp: borrow an element with `at(i)` or move it out with `pop()`";
                                }
                                Ty::Struct(id)
                                    if self.types.structs[id]
                                        .origin
                                        .as_ref()
                                        .is_some_and(|(name, _)| name == "Map")
                                        && matches!(m, "get" | "get_or") =>
                                {
                                    note += "\nhelp: use `m.find(key)` and, if the index is nonnegative, borrow `m.vals.at(i)`; `m.take(key)` removes and returns an existing value";
                                }
                                _ => {}
                            }
                        } else if matches!(a, Ty::Struct(_) | Ty::Enum(_)) {
                            note += &format!("\nhelp: add `impl {} for {} {{ .. }}`", b, an);
                        }
                        return Some(note);
                    }
                }
            }
        }
        None
    }

    /// A non-generic function of the built-in library is compiled the first time something calls it.
    fn load_lazy(&mut self, name: &str) {
        let tok = match self.ps.lazy_fns.get(name) {
            Some(t) => *t,
            None => return,
        };
        self.ps.lazy_fns.remove(name);
        match self.with_ps(|ps| ps.parse_fn_at(tok, vec![], None, false)) {
            Ok((h, body)) => {
                let f = Func {
                    name: name.to_string(),
                    params: h.params,
                    ret: h.ret,
                    body: body.unwrap_or_default(),
                    pos: h.pos,
                };
                self.sigs.insert(
                    name.to_string(),
                    Sig {
                        params: f.params.iter().map(|p| (p.mode, p.ty)).collect(),
                        ret: f.ret,
                    },
                );
                self.queue
                    .push((f, format!("the built-in function {}", name)));
            }
            Err(d) => self.err(d),
        }
    }
    /// Signature only (inside a generic body, where nothing is compiled).
    fn lazy_header(&mut self, name: &str) -> Option<Sig> {
        let tok = *self.ps.lazy_fns.get(name)?;
        let (h, _) = self
            .with_ps(|ps| ps.parse_fn_at(tok, vec![], None, true))
            .ok()?;
        Some(Self::sig_of(&h))
    }

    fn sig_of(h: &FnHeader) -> Sig {
        Sig {
            params: h.params.iter().map(|p| (p.mode, p.ty)).collect(),
            ret: h.ret,
        }
    }

    /// `fn name(view self, other: view Point) -> Int`, for help messages.
    fn show_sig(&self, h: &FnHeader) -> String {
        let ps: Vec<String> = h
            .params
            .iter()
            .map(|p| {
                let m = match p.mode {
                    Mode::View => "view ",
                    Mode::Mut => "mut ",
                    Mode::Own => "",
                };
                if p.name == "self" {
                    format!("{}self", m)
                } else {
                    format!("{}: {}{}", p.name, m, self.tn(p.ty))
                }
            })
            .collect();
        let r = if h.ret == Ty::Unit {
            String::new()
        } else {
            format!(" -> {}", self.tn(h.ret))
        };
        format!("fn {}({}){}", h.name, ps.join(", "), r)
    }

    fn object_methods(&mut self, id: usize, pos: Pos) -> Option<Vec<(String, FnTy)>> {
        let ty = Ty::Dyn(id);
        let name = self.types.dyns[id].0.clone();
        let Some(def) = self.ps.traits.iter().find(|t| t.name == name) else {
            self.err(diag(format!("unknown trait '{}'", name), pos));
            return None;
        };
        let mut methods = Vec::new();
        for method in &def.methods {
            let sig = &method.sig;
            let reason = if sig.params[0].0 == Mode::Own {
                Some("takes self by value")
            } else if !matches!(sig.params[0].1, Ty::Param(_)) {
                Some("declares a receiver type other than Self")
            } else if self.types.has_param(sig.ret)
                || sig.params[1..]
                    .iter()
                    .any(|(_, ty)| self.types.has_param(*ty))
            {
                Some("uses Self outside the receiver")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.err(diag(
                    format!(
                        "trait '{}' is not object-safe: method '{}' {}",
                        name, method.name, reason
                    ),
                    pos,
                ));
                return None;
            }
            let mut sig = sig.clone();
            sig.params[0].1 = ty;
            methods.push((method.name.clone(), sig));
        }
        Some(methods)
    }

    fn dyn_box(&mut self, e: &mut Expr, id: usize) -> Ty {
        let EK::Static { ty, args, .. } = &mut e.k else {
            unreachable!()
        };
        let box_ty = *ty;
        let concrete = self.expr(&mut args[0]);
        e.ty = Ty::Err;
        if concrete == Ty::Err {
            return Ty::Err;
        }
        if self.types.contains_join_handle(concrete) {
            self.err(diag(
                "a JoinHandle cannot be stored in an erased box",
                args[0].pos,
            ));
            return Ty::Err;
        }
        let trait_name = self.types.dyns[id].0.clone();
        let Some(methods) = self.object_methods(id, self.types.dyns[id].1) else {
            return Ty::Err;
        };
        if concrete == Ty::Unit || !self.implements(concrete, &trait_name) {
            self.err(diag(
                format!("{} does not implement {}", self.tn(concrete), trait_name),
                args[0].pos,
            ));
            return Ty::Err;
        }
        if !self.in_generic {
            let mut targets = Vec::new();
            for (name, _) in methods {
                let Some((target, _)) = self.method_target(concrete, Some(&trait_name), &name)
                else {
                    return Ty::Err;
                };
                targets.push(target);
            }
            let table = self
                .types
                .dyn_tables
                .iter()
                .position(|t| t.object == id && t.concrete == concrete)
                .unwrap_or_else(|| {
                    let table = self.types.dyn_tables.len();
                    self.types.dyn_tables.push(DynTable {
                        object: id,
                        concrete,
                        methods: targets,
                    });
                    table
                });
            e.k = EK::DynBox {
                table,
                value: Box::new(args.remove(0)),
            };
        }
        e.ty = box_ty;
        box_ty
    }

    fn dyn_call(&mut self, e: &mut Expr, id: usize) -> Ty {
        e.ty = Ty::Err;
        let Some(methods) = self.object_methods(id, self.types.dyns[id].1) else {
            return Ty::Err;
        };
        let EK::Method { recv, m, args } = &mut e.k else {
            unreachable!()
        };
        let Some((slot, (_, sig))) = methods
            .into_iter()
            .enumerate()
            .find(|(_, (name, _))| name == m)
        else {
            self.err(diag(
                format!("{} has no method '{}'", self.tn(Ty::Dyn(id)), m),
                e.pos,
            ));
            return Ty::Err;
        };
        if sig.params.len() != args.len() + 1 {
            self.err(diag(
                format!(
                    "method '{}' expects {} argument(s), got {}",
                    m,
                    sig.params.len() - 1,
                    args.len()
                ),
                e.pos,
            ));
            return Ty::Err;
        }
        let mut args = std::mem::take(args);
        args.insert(0, *std::mem::replace(recv, Box::new(s_var("$dyn", e.pos))));
        let pre = vec![None; args.len()];
        let ret = self.call_args(
            Sig {
                params: sig.params.clone(),
                ret: sig.ret,
            },
            &mut args,
            pre,
        );
        // A later argument expression can consume a root without being a bare `move`.
        for (arg, (mode, ty)) in args.iter().zip(&sig.params) {
            if *mode == Mode::Mut || (*mode == Mode::View && self.types.owned(*ty)) {
                self.check_moved(path(arg).0, arg.pos);
            }
        }
        e.k = EK::DynCall { slot, sig, args };
        e.ty = ret;
        self.protect_operands(e);
        ret
    }

    fn trait_header(&mut self, tr: &str, m: &str, self_ty: Ty) -> Option<FnHeader> {
        let tok = self
            .ps
            .traits
            .iter()
            .find(|t| t.name == tr)?
            .methods
            .iter()
            .find(|x| x.name == m)?
            .tok;
        self.with_ps(|ps| ps.parse_fn_at(tok, vec![], Some(self_ty), true))
            .ok()
            .map(|x| x.0)
    }

    /// Find method `m` for values of type `ty`: the function to call and its signature.
    /// Impl methods of built-in types are compiled the first time something calls them.
    fn method_target(&mut self, ty: Ty, only: Option<&str>, m: &str) -> Option<(String, Sig)> {
        if let Ty::Param(i) = ty {
            let bounds = self.types.params[i].bounds.clone();
            for b in bounds {
                if only.is_some_and(|o| o != b) {
                    continue;
                }
                if let Some(h) = self.trait_header(&b, m, ty) {
                    let name = format!("{}__{}__{}", b, self.types.mangle(ty), m);
                    let sig = Self::sig_of(&h);
                    self.sigs.insert(name.clone(), sig.clone());
                    return Some((name, sig));
                }
            }
            return None;
        }
        // inherent methods first, then trait impls
        let mut found: Option<(usize, usize, Vec<Ty>)> = None;
        for pass in 0..2 {
            for (ii, im) in self.ps.impls.iter().enumerate() {
                if (pass == 0) != im.trait_name.is_none() {
                    continue;
                }
                if only.is_some_and(|o| im.trait_name.as_deref() != Some(o)) {
                    continue;
                }
                let k = match im.methods.iter().position(|x| x.name == m) {
                    Some(k) => k,
                    None => continue,
                };
                if let Some(binds) = self.impl_binds(im, ty) {
                    found = Some((ii, k, binds));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let (ii, k, binds) = found?;
        let tok = self.ps.impls[ii].methods[k].tok;
        let tn = self.ps.impls[ii].trait_name.clone();
        let subst: Vec<(String, Ty)> = self.ps.impls[ii]
            .generics
            .iter()
            .map(|g| g.name.clone())
            .zip(binds.iter().copied())
            .collect();
        let generic_impl = !subst.is_empty();
        let name = impl_fn_name(&self.types, tn.as_deref(), ty, m);
        let lazy = if generic_impl {
            !self.impl_done.contains(&name)
        } else {
            !self.ps.impls[ii].methods[k].parsed
        };
        if lazy && !self.in_generic {
            match self.with_ps(|ps| ps.parse_fn_at(tok, subst.clone(), Some(ty), false)) {
                Ok((h, body)) => {
                    let f = Func {
                        name: name.clone(),
                        params: h.params,
                        ret: h.ret,
                        body: body.unwrap_or_default(),
                        pos: h.pos,
                    };
                    self.sigs.insert(
                        name.clone(),
                        Sig {
                            params: f.params.iter().map(|p| (p.mode, p.ty)).collect(),
                            ret: f.ret,
                        },
                    );
                    self.ps.impls[ii].methods[k].parsed = true;
                    self.impl_done.insert(name.clone());
                    self.queue.push((
                        f,
                        format!(
                            "the built-in {} for {}",
                            tn.as_deref().unwrap_or("impl"),
                            self.types.name(ty)
                        ),
                    ));
                }
                Err(d) => {
                    self.err(d);
                    return None;
                }
            }
        }
        let h = self
            .with_ps(|ps| ps.parse_fn_at(tok, subst, Some(ty), true))
            .ok()?
            .0;
        let sig = Self::sig_of(&h);
        self.sigs.insert(name.clone(), sig.clone());
        Some((name, sig))
    }

    /// Traits (by name) that declare a method called `m`.
    fn traits_with(&self, m: &str) -> Vec<String> {
        self.ps
            .traits
            .iter()
            .filter(|t| t.methods.iter().any(|x| x.name == m))
            .map(|t| t.name.clone())
            .collect()
    }

    fn missing_impl_help(&mut self, tr: &str, ty: Ty) -> String {
        let tyn = self.tn(ty);
        let methods: Vec<String> = self
            .ps
            .traits
            .iter()
            .find(|t| t.name == tr)
            .map(|t| t.methods.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default();
        let mut sigs = vec![];
        for m in methods {
            if let Some(h) = self.trait_header(tr, &m, ty) {
                sigs.push(format!("{} {{ .. }}", self.show_sig(&h)));
            }
        }
        if tr == "Copy" {
            format!("a {} holds owned data (a String or a List), so it is moved, never copied; `Copy` types are Int, Bool, Text, enums and structs of those", tyn)
        } else {
            format!(
                "help: add `impl {} for {} {{ {} }}`",
                tr,
                tyn,
                sigs.join(" ")
            )
        }
    }

    /// `x.m(args)` -> `Trait__Type__m(x, args)`.
    fn rewrite_method(&mut self, e: &mut Expr, rt: Ty) -> Result<(), ()> {
        let pos = e.pos;
        let (m, nargs) = match &e.k {
            EK::Method { m, args, .. } => (m.clone(), args.len()),
            _ => return Err(()),
        };
        let tyn = self.tn(rt);
        let mut target = self.method_target(rt, None, &m);
        if target.is_none() && m == "to_string" {
            // `x.to_string()` is `x.show()` for anything that implements Show
            target = self.method_target(rt, Some("Show"), "show");
        }
        let (name, sig) = match target {
            Some(x) => x,
            None if matches!(rt, Ty::List(_) | Ty::String | Ty::Char | Ty::Ptr(_)) => {
                if let Some(n) = self.unmet_bound_note(rt, &m) {
                    let mut d = diag(format!("{} has no method '{}' here", tyn, m), pos);
                    for l in n.split('\n') {
                        d = d.note(l.to_string());
                    }
                    self.err(d);
                    return Err(());
                }
                return Ok(()); // the built-in path explains
            }
            None => {
                let mut d = diag(format!("type {} has no method '{}'", tyn, m), pos);
                if let Some(n) = self.unmet_bound_note(rt, &m) {
                    for l in n.split('\n') {
                        d = d.note(l.to_string());
                    }
                    self.err(d);
                    return Err(());
                }
                let ts = self.traits_with(&m);
                if let Ty::Param(i) = rt {
                    let b = self.types.params[i].bounds.clone();
                    let have = if b.is_empty() {
                        "no bounds".to_string()
                    } else {
                        b.join(" + ")
                    };
                    d = d.note(format!("{} is a type parameter with {}: it only has the methods its bounds promise", tyn, have));
                    if let Some(t) = ts.first() {
                        d = d.note(format!(
                            "help: add the bound `{}: {}` to the function's type parameters",
                            tyn, t
                        ));
                    }
                } else if let Some(t) = ts.first() {
                    d = d.note(format!(
                        "help: `{}` is a method of trait {}: write `impl {} for {} {{ .. }}`",
                        m, t, t, tyn
                    ));
                } else if matches!(
                    rt,
                    Ty::Int | Ty::Bool | Ty::Text | Ty::Struct(_) | Ty::Enum(_)
                ) {
                    d = d.note("help: define it in an `impl` block, e.g. `impl Point { fn norm(view self) -> Int { .. } }`");
                }
                self.err(d);
                return Err(());
            }
        };
        if sig.params.len() != nargs + 1 {
            self.err(diag(
                format!(
                    "method '{}' of {} expects {} argument(s) besides the receiver but got {}",
                    m,
                    tyn,
                    sig.params.len() - 1,
                    nargs
                ),
                pos,
            ));
            return Err(());
        }
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        if let EK::Method { recv, args, .. } = k {
            let mut cargs = vec![*recv];
            cargs.extend(args);
            // `self` by value of an owned type consumes the receiver
            if sig.params[0].0 == Mode::Own && self.types.owned(rt) {
                if let EK::Var(n, _) = &cargs[0].k {
                    let (n, p, t) = (n.clone(), cargs[0].pos, cargs[0].ty);
                    cargs[0] = Expr {
                        k: EK::Move(n, 0),
                        pos: p,
                        ty: t,
                    };
                }
            }
            e.k = EK::Call(name, cargs);
        }
        Ok(())
    }

    /// `a == b` -> `Equal` call, `a < b` -> `Ordered` call compared with 0.
    /// An array or slice value (a place of such a type, or an array literal).
    fn is_seq_expr(&self, e: &Expr) -> bool {
        if matches!(e.k, EK::ArrayLit(_) | EK::ArrayFill(..)) {
            return true;
        }
        self.isp(e)
            && self.peek_ty(e).is_some_and(|t| {
                self.types.array_of(t).is_some() || self.types.slice_elem(t).is_some()
            })
    }
    /// `a == b` / `a < b` / `a.show()` on arrays and slices (element-wise, through prelude_fmt.yx).
    fn seq_rewrite(&mut self, e: &mut Expr) {
        let pos = e.pos;
        match &e.k {
            EK::Bin(op, a, _)
                if matches!(*op, "==" | "!=" | "<" | ">" | "<=" | ">=") && self.is_seq_expr(a) =>
            {
                let op = *op;
                if let EK::Bin(_, a, b) = &e.k {
                    let ta = self.peek_ty(a).filter(|t| self.types.array_of(*t).is_some());
                    let tb = self.peek_ty(b).filter(|t| self.types.array_of(*t).is_some());
                    if let (Some(ta), Some(tb)) = (ta, tb) {
                        if ta != tb {
                            let (na, nb) = (self.tn(ta), self.tn(tb));
                            self.err(diag(format!("cannot compare {} with {}", na, nb), pos)
                                .note("arrays are compared element by element and must have the same type; lend them as slices (`view [T]`) to compare different lengths"));
                        }
                    }
                }
                let EK::Bin(_, a, b) = std::mem::replace(&mut e.k, EK::Int(0)) else {
                    unreachable!()
                };
                let f = if matches!(op, "==" | "!=") {
                    "std__seq_eq"
                } else {
                    "std__seq_cmp"
                };
                let call = sx(EK::Call(f.into(), vec![*a, *b]), pos);
                e.k = match op {
                    "==" => call.k,
                    "!=" => EK::Un("!", Box::new(call)),
                    _ => EK::Bin(op, Box::new(call), Box::new(sx(EK::Int(0), pos))),
                };
            }
            EK::Method { recv, m, args }
                if ((m == "show" && args.is_empty())
                    || (matches!(m.as_str(), "eq" | "cmp") && args.len() == 1))
                    && self.is_seq_expr(recv) =>
            {
                let f = match m.as_str() {
                    "show" => "std__seq_show",
                    "eq" => "std__seq_eq",
                    _ => "std__seq_cmp",
                };
                let EK::Method { recv, args, .. } = std::mem::replace(&mut e.k, EK::Int(0)) else {
                    unreachable!()
                };
                let mut all = vec![*recv];
                all.extend(args);
                e.k = EK::Call(f.into(), all);
            }
            _ => {}
        }
    }
    fn rewrite_op(&mut self, e: &mut Expr, t: Ty) -> Result<(), ()> {
        let pos = e.pos;
        let op = match &e.k {
            EK::Bin(op, ..) => *op,
            _ => return Err(()),
        };
        let (tr, m) = if matches!(op, "==" | "!=") {
            ("Equal", "eq")
        } else {
            ("Ordered", "cmp")
        };
        let tyn = self.tn(t);
        if !self.implements(t, tr) {
            let help = self.missing_impl_help(tr, t);
            let mut d = diag(
                format!(
                    "cannot use `{}` on values of type {}: it does not implement {}",
                    op, tyn, tr
                ),
                pos,
            );
            if let Ty::Param(_) = t {
                d = d.note(format!(
                    "help: add the bound `{}: {}` to the function's type parameters",
                    tyn, tr
                ));
            } else {
                d = d.note(help);
            }
            self.err(d);
            return Err(());
        }
        let (name, _) = match self.method_target(t, Some(tr), m) {
            Some(x) => x,
            None => return Err(()),
        };
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        if let EK::Bin(_, a, b) = k {
            let call = Expr {
                k: EK::Call(name, vec![*a, *b]),
                pos,
                ty: Ty::Unit,
            };
            e.k = match op {
                "==" => call.k,
                "!=" => EK::Un("!", Box::new(call)),
                _ => EK::Bin(
                    op,
                    Box::new(call),
                    Box::new(Expr {
                        k: EK::Int(0),
                        pos,
                        ty: Ty::Unit,
                    }),
                ),
            };
        }
        Ok(())
    }

    fn struct_lit(
        &mut self,
        sid: usize,
        fields: &mut Vec<(String, Expr)>,
        pos: Pos,
        pre: Option<&[Ty]>,
    ) -> Ty {
        if pos.line < PRELUDE_LINE && self.types.opaque_struct(sid) {
            self.err(diag(
                format!(
                    "{} cannot be constructed with a struct literal",
                    self.tn(Ty::Struct(sid))
                ),
                pos,
            ));
            return Ty::Err;
        }
        let def = self.types.structs[sid].clone();
        let mut seen: HashSet<String> = HashSet::new();
        for (k, (fname, fe)) in fields.iter_mut().enumerate() {
            let fty = def.fields.iter().find(|(n, _)| n == fname).map(|x| x.1);
            let t = match pre {
                Some(p) => p[k],
                None => {
                    self.hint = fty;
                    self.expr(fe)
                }
            };
            match fty {
                None => self.err(diag(
                    format!("struct {} has no field '{}'", def.name, fname),
                    fe.pos,
                )),
                Some(fty) => {
                    self.expect(t, fty, fe.pos);
                    if !seen.insert(fname.clone()) {
                        self.err(diag(format!("field '{}' is given twice", fname), fe.pos));
                    }
                }
            }
        }
        let missing: Vec<&str> = def
            .fields
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !seen.contains(*n))
            .collect();
        if !missing.is_empty() {
            self.err(diag(
                format!("missing field(s) in {}: {}", def.name, missing.join(", ")),
                pos,
            ));
        }
        Ty::Struct(sid)
    }

    fn variant(
        &mut self,
        eid: usize,
        vname: &str,
        args: &mut Vec<Expr>,
        pos: Pos,
        pre: Option<&[Ty]>,
    ) -> (Ty, usize) {
        let def = self.types.enums[eid].clone();
        match def.variants.iter().position(|v| v.0 == vname) {
            None => {
                self.err(diag(
                    format!("enum {} has no variant '{}'", def.name, vname),
                    pos,
                ));
                (Ty::Err, 0)
            }
            Some(vi) => {
                let payload = def.variants[vi].1.clone();
                if payload.len() != args.len() {
                    self.err(diag(
                        format!(
                            "{}.{} expects {} value(s), got {}",
                            def.name,
                            vname,
                            payload.len(),
                            args.len()
                        ),
                        pos,
                    ));
                }
                for (k, (a, pt)) in args.iter_mut().zip(payload).enumerate() {
                    let t = match pre {
                        Some(p) => p[k],
                        None => {
                            self.hint = Some(pt);
                            self.expr(a)
                        }
                    };
                    self.expect(t, pt, a.pos);
                }
                (Ty::Enum(eid), vi)
            }
        }
    }

    /// `Pair { a: 1, b: "x" }`, `Maybe<Int>.None`, `Maybe.Some(5)`: pick the instantiation, then check as usual.
    fn generic_lit(&mut self, k: EK, pos: Pos, hint: Option<Ty>) -> (Ty, EK) {
        match k {
            EK::GenStruct {
                name,
                targs,
                mut fields,
            } => {
                let mut pre: Option<Vec<Ty>> = None;
                let targs = match targs {
                    Some(t) => Some(t),
                    None => {
                        // infer from the field values
                        let (tpl, pids) = match self.with_ps(|ps| ps.generic_template(&name)) {
                            Some(x) => x,
                            None => return (Ty::Err, EK::Int(0)),
                        };
                        let pids: Vec<usize> = pids
                            .iter()
                            .map(|t| if let Ty::Param(i) = t { *i } else { 0 })
                            .collect();
                        let tys: Vec<Ty> = fields.iter_mut().map(|(_, fe)| self.expr(fe)).collect();
                        let mut bind = vec![None; pids.len()];
                        if let Ty::Struct(sid) = tpl {
                            for ((fname, _), t) in fields.iter().zip(&tys) {
                                if let Some((_, pat)) = self.types.structs[sid]
                                    .fields
                                    .iter()
                                    .find(|(n, _)| n == fname)
                                {
                                    let _ = unify(&self.types, *pat, *t, &pids, &mut bind);
                                }
                            }
                        }
                        if tys.contains(&Ty::Err) {
                            return (Ty::Err, EK::Int(0));
                        }
                        pre = Some(tys);
                        if bind.iter().any(|b| b.is_none()) {
                            let ps = self.with_ps(|ps| ps.generic_type_params(&name));
                            let un: Vec<String> = ps
                                .iter()
                                .zip(&bind)
                                .filter(|(_, b)| b.is_none())
                                .map(|(n, _)| n.clone())
                                .collect();
                            self.err(
                                diag(
                                    format!(
                                        "cannot infer the type argument(s) {} of {}",
                                        un.join(", "),
                                        name
                                    ),
                                    pos,
                                )
                                .note(format!(
                                    "help: write them out: `{}<{}> {{ .. }}`",
                                    name,
                                    ps.join(", ")
                                )),
                            );
                            return (Ty::Err, EK::Int(0));
                        }
                        Some(bind.into_iter().map(|b| b.unwrap()).collect())
                    }
                };
                let ty = match self.with_ps(|ps| ps.generic_instance(&name, targs.unwrap(), pos)) {
                    Ok(t) => t,
                    Err(d) => {
                        self.err(d);
                        return (Ty::Err, EK::Int(0));
                    }
                };
                match ty {
                    Ty::Struct(sid) => {
                        let t = self.struct_lit(sid, &mut fields, pos, pre.as_deref());
                        (t, EK::StructLit { sid, fields })
                    }
                    _ => {
                        self.err(diag(format!("{} is not a struct", name), pos));
                        (Ty::Err, EK::Int(0))
                    }
                }
            }
            EK::GenVariant {
                name,
                targs,
                vname,
                mut args,
            } if !self.with_ps(|ps| ps.generic_is_enum(&name)) => {
                // `Map<String, Int>.new()`: a function of the instantiated struct, found in its impls
                let mut checked = vec![None; args.len()];
                let inferred_constructor = vname == "new"
                    && matches!(name.as_str(), "Shared" | "Locked" | "Atomic")
                    && args.len() == 1;
                let targs = match targs {
                    Some(t) => t,
                    None if inferred_constructor => {
                        let t = self.expr(&mut args[0]);
                        if t == Ty::Err {
                            return (Ty::Err, EK::Int(0));
                        }
                        checked[0] = Some(t);
                        vec![t]
                    }
                    None => {
                        let ps = self.with_ps(|ps| ps.generic_type_params(&name));
                        self.err(
                            diag(
                                format!(
                                    "{}.{}() needs the type arguments of {}",
                                    name, vname, name
                                ),
                                pos,
                            )
                            .note(format!(
                                "help: write them out: `{}<{}>.{}()`",
                                name,
                                ps.join(", "),
                                vname
                            )),
                        );
                        return (Ty::Err, EK::Int(0));
                    }
                };
                let ty = match self.with_ps(|ps| ps.generic_instance(&name, targs, pos)) {
                    Ok(t) => t,
                    Err(d) => {
                        self.err(d);
                        return (Ty::Err, EK::Int(0));
                    }
                };
                let tyn = self.tn(ty);
                if vname == "new" {
                    if let Some(payload) = self
                        .types
                        .shared_payload(ty)
                        .or_else(|| self.types.locked_payload(ty))
                    {
                        if !self.thread_transferable(payload, &mut HashSet::new()) {
                            self.err(diag(
                                format!(
                                    "{} cannot contain {} because it is not thread-transferable",
                                    tyn,
                                    self.tn(payload)
                                ),
                                pos,
                            ));
                            return (Ty::Err, EK::Int(0));
                        }
                    }
                }
                let (fname, sig) = match self.method_target(ty, None, &vname) {
                    Some(x) => x,
                    None => {
                        let mut d = diag(format!("{} has no function '{}'", tyn, vname), pos);
                        if let Some(n) = self.unmet_bound_note(ty, &vname) {
                            for l in n.split('\n') {
                                d = d.note(l.to_string());
                            }
                        }
                        self.err(d);
                        return (Ty::Err, EK::Int(0));
                    }
                };
                if sig.params.len() != args.len() {
                    self.err(diag(
                        format!(
                            "{}.{}() expects {} argument(s) but got {}",
                            tyn,
                            vname,
                            sig.params.len(),
                            args.len()
                        ),
                        pos,
                    ));
                    return (Ty::Err, EK::Int(0));
                }
                let t = self.call_args(sig, &mut args, checked);
                (t, EK::Call(fname, args))
            }
            EK::GenVariant {
                name,
                targs,
                vname,
                mut args,
            } => {
                let mut pre: Option<Vec<Ty>> = None;
                let mut targs = targs;
                if targs.is_none() {
                    // the expected type, if it is an instantiation of this enum
                    if let Some(Ty::Enum(h)) = hint {
                        if let Some((n, a)) = &self.types.enums[h].origin {
                            if *n == name {
                                targs = Some(a.clone());
                            }
                        }
                    }
                }
                let targs = match targs {
                    Some(t) => t,
                    None => {
                        let (tpl, pids) = match self.with_ps(|ps| ps.generic_template(&name)) {
                            Some(x) => x,
                            None => return (Ty::Err, EK::Int(0)),
                        };
                        let pids: Vec<usize> = pids
                            .iter()
                            .map(|t| if let Ty::Param(i) = t { *i } else { 0 })
                            .collect();
                        let tys: Vec<Ty> = args.iter_mut().map(|a| self.expr(a)).collect();
                        let mut bind = vec![None; pids.len()];
                        if let Ty::Enum(eid) = tpl {
                            if let Some((_, payload)) =
                                self.types.enums[eid].variants.iter().find(|v| v.0 == vname)
                            {
                                for (pat, t) in payload.iter().zip(&tys) {
                                    let _ = unify(&self.types, *pat, *t, &pids, &mut bind);
                                }
                            }
                        }
                        if tys.contains(&Ty::Err) {
                            return (Ty::Err, EK::Int(0));
                        }
                        pre = Some(tys);
                        if bind.iter().any(|b| b.is_none()) {
                            let ps = self.with_ps(|ps| ps.generic_type_params(&name));
                            let un: Vec<String> = ps
                                .iter()
                                .zip(&bind)
                                .filter(|(_, b)| b.is_none())
                                .map(|(n, _)| n.clone())
                                .collect();
                            self.err(
                                diag(
                                    format!(
                                        "cannot infer the type argument(s) {} of {}.{}",
                                        un.join(", "),
                                        name,
                                        vname
                                    ),
                                    pos,
                                )
                                .note("nothing here says what type it should hold")
                                .note(format!(
                                    "help: write them out, e.g. `{}<Int>.{}`",
                                    name, vname
                                )),
                            );
                            return (Ty::Err, EK::Int(0));
                        }
                        bind.into_iter().map(|b| b.unwrap()).collect()
                    }
                };
                let ty = match self.with_ps(|ps| ps.generic_instance(&name, targs, pos)) {
                    Ok(t) => t,
                    Err(d) => {
                        self.err(d);
                        return (Ty::Err, EK::Int(0));
                    }
                };
                match ty {
                    Ty::Enum(eid) => {
                        let (t, vid) = self.variant(eid, &vname, &mut args, pos, pre.as_deref());
                        (
                            t,
                            EK::Variant {
                                eid,
                                vname,
                                vid,
                                args,
                            },
                        )
                    }
                    _ => {
                        self.err(diag(format!("{} is not an enum", name), pos));
                        (Ty::Err, EK::Int(0))
                    }
                }
            }
            other => (Ty::Err, other),
        }
    }

    /// Call of a generic function: infer the type arguments from the arguments, check the bounds,
    /// then call the instantiation (created and queued for checking on first use).
    fn generic_call(&mut self, gi: usize, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        let mut g = self.ps.gfns[gi].clone();
        if self.broken.contains(&g.name) {
            return Ty::Err; // its own errors are already reported
        }
        if g.params.len() != args.len() {
            self.err(diag(
                format!(
                    "'{}' expects {} argument(s) but got {}",
                    g.name,
                    g.params.len(),
                    args.len()
                ),
                pos,
            ));
            return Ty::Err;
        }
        let spawning = g.name == "__spawn_owned";
        let locking = matches!(g.name.as_str(), "__locked_with" | "__locked_with_view");
        if locking
            && pos.line < PRELUDE_LINE
            && !matches!(args[1].k, EK::Closure { is_move: false, .. })
        {
            self.err(diag("with requires a borrowing closure literal", pos));
            return Ty::Err;
        }
        if spawning && !matches!(args[0].k, EK::Closure { worker: true, .. }) {
            self.err(diag("spawn requires a move closure literal", pos));
            return Ty::Err;
        }
        let mut pre: Vec<Option<Ty>> = vec![None; args.len()];
        // `"{expr}"` with an owned temporary (a String, a List): the formatter takes it instead of borrowing it
        let mut gi = gi;
        if g.name == "std__fmt_val" && args.len() == 2 && self.is_seq_expr(&args[1]) {
            let a = std::mem::replace(&mut args[1], sx(EK::Int(0), pos));
            let p = a.pos;
            args[1] = sx(EK::Call("std__seq_show".into(), vec![a]), p);
        }
        if g.name == "std__fmt_val" && args.len() == 2 && !self.isp(&args[1]) {
            // the text built so far comes first: `"{r.is_err()} {r.err()}"` reads `r` before moving it
            self.hint = Some(Ty::String);
            pre[0] = Some(self.expr(&mut args[0]));
            let t = self.expr(&mut args[1]);
            pre[1] = Some(t);
            if self.types.owned(t) {
                if let Some(o) = self.ps.gfns.iter().position(|x| x.name == "std__fmt_own") {
                    gi = o;
                    g = self.ps.gfns[o].clone();
                }
            }
        }
        let mut bind: Vec<Option<Ty>> = vec![None; g.pids.len()];
        let mut bad = false;
        // closure literals are checked after the other arguments: those fix the type parameters that the
        // closure's parameters need, and the closure's body then fixes the ones in its result type
        let mut order: Vec<usize> = (0..args.len())
            .filter(|k| !matches!(args[*k].k, EK::Closure { .. }))
            .collect();
        order.extend((0..args.len()).filter(|k| matches!(args[*k].k, EK::Closure { .. })));
        for k in order {
            let arg = &mut args[k];
            let (pmode, pat) = g.params[k];
            if matches!(pat, Ty::Fn(_)) {
                self.norm_fn_arg(arg);
            }
            let t = if let EK::Closure { is_move, .. } = &arg.k {
                let borrowing = pmode != Mode::Own && !*is_move;
                let exp = self.subst_known(pat, &g.pids, &bind);
                let free: Vec<usize> = g
                    .pids
                    .iter()
                    .zip(&bind)
                    .filter(|(_, b)| b.is_none())
                    .map(|(p, _)| *p)
                    .collect();
                self.free_pids = free;
                let t = self.closure_expr(arg, Some(exp), borrowing, spawning, locking);
                self.free_pids.clear();
                pre[k] = Some(t);
                t
            } else if let Some(t) = pre[k] {
                t
            } else if let EK::SliceOf(b, ..) = &arg.k {
                match self.peek_ty(b).and_then(|t| self.types.seq_elem(t)) {
                    Some(e) => self.types.intern_slice(e),
                    None => {
                        self.err(diag("a slice needs an array, a List or a slice variable (or field) before `[..]`", arg.pos));
                        return Ty::Err;
                    }
                }
            } else if self.isp(arg) {
                match self.peek_ty(arg) {
                    Some(t) => t,
                    // not a plain place (`mk().x`, a field of a Copy temporary): check it now; when that
                    // fails, `place` has reported why
                    None => match self.place(arg) {
                        Some(p) => {
                            pre[k] = Some(p.ty);
                            p.ty
                        }
                        None => return Ty::Err,
                    },
                }
            } else {
                if !self.types.has_param(pat) {
                    self.hint = Some(pat);
                }
                let t = self.expr(arg);
                pre[k] = Some(t);
                t
            };
            if let Err(prior) = unify(&self.types, pat, t, &g.pids, &mut bind) {
                bad = true;
                let (got, want) = (self.tn(t), self.tn(pat));
                let d = match (pat, prior) {
                    (Ty::Param(_), Some(b)) => diag(
                        format!("argument {} of '{}' is {}, but {} was already fixed to {} by an earlier argument", k + 1, g.name, got, want, self.tn(b)),
                        arg.pos,
                    )
                    .note(format!("every parameter declared as `{}` must receive the same type", want)),
                    _ => diag(format!("expected {} but this is {}", want, got), arg.pos),
                };
                self.err(d);
            }
        }
        if bad {
            return Ty::Err;
        }
        if args.iter().any(|a| a.ty == Ty::Err) {
            return Ty::Err;
        }
        if let Some(k) = bind.iter().position(|b| b.is_none()) {
            self.err(
                diag(
                    format!(
                        "cannot infer the type parameter {} of '{}'",
                        g.tparams[k].name, g.name
                    ),
                    pos,
                )
                .note(format!(
                    "no argument has a type that mentions {}",
                    g.tparams[k].name
                ))
                .note("help: give the function a parameter of that type"),
            );
            return Ty::Err;
        }
        let targs: Vec<Ty> = bind.iter().map(|b| b.unwrap()).collect();
        if g.name == "__parallel_list" && !self.thread_transferable(targs[0], &mut HashSet::new()) {
            self.err(diag(
                format!(
                    "parallel for cannot read list elements of type {}",
                    self.tn(targs[0])
                ),
                pos,
            ));
            return Ty::Err;
        }
        // bounds
        let mut ok = true;
        for (k, tp) in g.tparams.iter().enumerate() {
            for (b, _) in &tp.bounds {
                if !self.ps.traits.iter().any(|t| t.name == *b) {
                    continue; // reported once at the definition
                }
                if !self.implements(targs[k], b) {
                    ok = false;
                    let tyn = self.tn(targs[k]);
                    let help = self.missing_impl_help(b, targs[k]);
                    if g.name.starts_with("std__fmt_") {
                        let d = diag(format!("cannot put a {} inside `{{...}}` in a string: it does not implement Show", tyn), pos)
                            .note("`\"x = {x}\"` calls x.show() on every value, so its type needs `impl Show`")
                            .note(help);
                        self.err(d);
                        continue;
                    }
                    let mut d = diag(
                        format!(
                            "{} does not implement {}, which '{}' needs for {}",
                            tyn, b, g.name, tp.name
                        ),
                        pos,
                    )
                    .note(format!(
                        "'{}' is declared `fn {}<{}: {}>`",
                        g.name,
                        g.name,
                        tp.name,
                        tp.bounds
                            .iter()
                            .map(|x| x.0.as_str())
                            .collect::<Vec<_>>()
                            .join(" + ")
                    ));
                    d = d.note(help);
                    self.err(d);
                }
            }
        }
        if !ok {
            return Ty::Err;
        }
        let (iname, sig) = match self.instance(gi, targs, pos) {
            Some(x) => x,
            None => return Ty::Err,
        };
        // checking the arguments may check nested calls (`sh(mk().x)`), which consume `self.rename`:
        // set it only once they are done, or the nested call would take the instance name
        let r = self.call_args(sig, args, pre);
        self.rename = Some(iname);
        r
    }

    fn instance(&mut self, gi: usize, targs: Vec<Ty>, pos: Pos) -> Option<(String, Sig)> {
        let g = self.ps.gfns[gi].clone();
        let key = (g.name.clone(), targs.clone());
        if !self.in_generic {
            if let Some(x) = self.inst_fns.get(&key) {
                return Some(x.clone());
            }
        }
        let subst: Vec<(String, Ty)> = g
            .tparams
            .iter()
            .map(|p| p.name.clone())
            .zip(targs.iter().copied())
            .collect();
        let mang: Vec<String> = targs.iter().map(|t| self.types.mangle(*t)).collect();
        let name = format!("{}__{}", g.name, mang.join("__"));
        let header_only = self.in_generic;
        match self.with_ps(|ps| ps.parse_fn_at(g.tok, subst, None, header_only)) {
            Err(d) => {
                self.err(d);
                None
            }
            Ok((h, body)) => {
                let sig = Self::sig_of(&h);
                if header_only {
                    return Some((name, sig));
                }
                self.n_inst += 1;
                if self.n_inst > 400 {
                    self.err(
                        diag(format!("'{}' is instantiated over and over with ever-larger types", g.name), pos)
                            .note("help: a generic function that calls itself with a bigger type each time (e.g. List<T> inside List<T>) never ends"),
                    );
                    return None;
                }
                let shown: Vec<String> = g
                    .tparams
                    .iter()
                    .zip(&targs)
                    .map(|(p, t)| format!("{} = {}", p.name, self.tn(*t)))
                    .collect();
                self.sigs.insert(name.clone(), sig.clone());
                self.inst_fns.insert(key, (name.clone(), sig.clone()));
                let f = Func {
                    name: name.clone(),
                    params: h.params,
                    ret: h.ret,
                    body: body.unwrap_or_default(),
                    pos: h.pos,
                };
                self.queue
                    .push((f, format!("`{}` with {}", g.name, shown.join(", "))));
                Some((name, sig))
            }
        }
    }
}

/// `ParseError` -> `parse_error`, `String` -> `string` (names of the conversion functions `?` looks for).
fn snake_case(n: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in n.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
            prev_lower = false;
        }
    }
    out.trim_end_matches('_').to_string()
}

/// Does `t` hold a value of type `target` BY VALUE (through struct fields and enum payloads)? `List<T>` and
/// `Box<T>` are heap indirections and do not count: that is what makes recursive types possible.
fn contains_value(types: &Types, t: Ty, target: Ty, seen: &mut Vec<Ty>) -> bool {
    match t {
        Ty::Struct(i) => {
            if t == target {
                return true;
            }
            if seen.contains(&t) {
                return false;
            }
            seen.push(t);
            types.structs[i]
                .fields
                .iter()
                .any(|(_, f)| contains_value(types, *f, target, seen))
        }
        Ty::Enum(i) => {
            if t == target {
                return true;
            }
            if seen.contains(&t) {
                return false;
            }
            seen.push(t);
            types.enums[i]
                .variants
                .iter()
                .any(|(_, ps)| ps.iter().any(|p| contains_value(types, *p, target, seen)))
        }
        _ => false,
    }
}

pub fn check_program(p: &mut Program, ps: &mut Parser) -> Result<Sigs, Vec<Diag>> {
    let mut errs: Vec<Diag> = std::mem::take(&mut ps.inst_errs);
    let types = std::mem::take(&mut p.types);

    // 1. types that contain themselves by value (infinite size) — must be ruled out before asking `owned`
    for (i, st) in types.structs.iter().enumerate() {
        if st
            .fields
            .iter()
            .any(|(_, f)| contains_value(&types, *f, Ty::Struct(i), &mut vec![]))
        {
            errs.push(
                diag(format!("struct {} contains itself", st.name), st.pos)
                    .note("a value cannot hold itself directly: it would have infinite size")
                    .note("help: put it behind a heap indirection: a field of type `Box<Self>`, or a `List<Self>` for any number of children"),
            );
        }
    }
    for (i, en) in types.enums.iter().enumerate() {
        if en.variants.iter().any(|(_, ps)| {
            ps.iter()
                .any(|f| contains_value(&types, *f, Ty::Enum(i), &mut vec![]))
        }) {
            errs.push(
                diag(format!("enum {} contains itself", en.name), en.pos)
                    .note("a value cannot hold itself directly: it would have infinite size")
                    .note(format!(
                        "help: put the recursive part behind a heap indirection: `Box<{}>`",
                        en.name
                    )),
            );
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    // 3. functions
    let mut sigs: Sigs = HashMap::new();
    for f in &p.funcs {
        if sigs.contains_key(&f.name) && !f.name.contains("__") {
            errs.push(diag(
                format!("function '{}' is defined twice", f.name),
                f.pos,
            ));
        }
        sigs.insert(
            f.name.clone(),
            Sig {
                params: f.params.iter().map(|q| (q.mode, q.ty)).collect(),
                ret: f.ret,
            },
        );
    }
    for f in &p.externs {
        let c_compatible = |t: Ty| {
            matches!(
                t,
                Ty::Int | Ty::Char | Ty::Bool | Ty::Text | Ty::Ptr(_) | Ty::Unit | Ty::Num(_)
            )
        };
        for param in &f.params {
            if param.mode != Mode::Own || !c_compatible(param.ty) {
                errs.push(diag(format!("extern function '{}' has a parameter type that is not supported by the C ABI yet", f.name), f.pos)
                    .note("supported types are Int, Bool, Char, Text, Ptr<T>, Unit, and numeric types; borrowed parameters and aggregates are not supported yet"));
            }
        }
        if !c_compatible(f.ret) {
            errs.push(diag(
                format!(
                    "extern function '{}' has a return type that is not supported by the C ABI yet",
                    f.name
                ),
                f.pos,
            ));
        }
        if sigs.contains_key(&f.name) {
            errs.push(diag(
                format!("function '{}' is defined twice", f.name),
                f.pos,
            ));
        }
        sigs.insert(
            f.name.clone(),
            Sig {
                params: f.params.iter().map(|q| (q.mode, q.ty)).collect(),
                ret: f.ret,
            },
        );
    }
    for g in &ps.gfns {
        if sigs.contains_key(&g.name) {
            errs.push(diag(
                format!("function '{}' is defined twice", g.name),
                g.pos,
            ));
        }
    }
    match p.funcs.iter().find(|f| f.name == "main") {
        None => errs.push(diag(
            "missing `fn main()`",
            Pos {
                line: 1,
                col: 1,
                len: 1,
            },
        )),
        Some(m) if !m.params.is_empty() || m.ret != Ty::Unit => errs.push(diag(
            "`main` takes no parameters and returns nothing",
            m.pos,
        )),
        _ => {}
    }
    if ps.gfns.iter().any(|g| g.name == "main") {
        errs.push(diag(
            "`main` cannot be generic",
            ps.gfns.iter().find(|g| g.name == "main").unwrap().pos,
        ));
    }
    let mut ck = Checker {
        types,
        sigs: sigs.clone(),
        extern_names: p.externs.iter().map(|f| f.name.clone()).collect(),
        variadic_externs: p
            .externs
            .iter()
            .filter(|f| f.variadic)
            .map(|f| f.name.clone())
            .collect(),
        unsafe_depth: 0,
        vars: vec![],
        scopes: vec![],
        st: State::default(),
        errs,
        loop_ivl: HashMap::new(),
        init_ivl: HashMap::new(),
        explicit_range: HashSet::new(),
        div_contract: false,
        unstable: HashSet::new(),
        ev_escape: vec![],
        ev_assign: vec![],
        scan_assign: std::cell::Cell::new(true),
        alias: HashMap::new(),
        lazy_modes: HashMap::new(),
        unknown_calls: std::cell::RefCell::new(HashSet::new()),
        ret: Ty::Unit,
        worker_panics: ps.uses_workers(),
        ps,
        in_generic: false,
        queue: vec![],
        inst_fns: HashMap::new(),
        hint: None,
        rename: None,
        n_inst: 0,
        broken: HashSet::new(),
        dscopes: vec![],
        pre: vec![],
        ptr_op: None,
        arg_temps: vec![],
        reported: HashSet::new(),
        impl_done: HashSet::new(),
        pos_names: HashMap::new(),
        pos_vars: HashMap::new(),
        lost_facts: HashMap::new(),
        loops: vec![],
        next_loop: 0,
        hid: 0,
        in_defer: false,
        frozen: vec![],
        match_scruts: HashMap::new(),
        lifted: vec![],
        thunks: HashSet::new(),
        nclos: 0,
        unit_ret: false,
        closure_depth: 0,
        capture_ids: HashSet::new(),
        closure_borrow_pos: HashSet::new(),
        ret_free: false,
        free_pids: vec![],
        closure_loans: HashMap::new(),
    };
    ck.check_decls();
    // generic bodies: checked once with abstract type parameters, against their bounds only
    ck.in_generic = true;
    for f in p.templates.iter_mut() {
        let before = ck.errs.len();
        ck.func(f);
        if ck.errs.len() > before {
            ck.broken.insert(f.name.clone());
        }
    }
    ck.in_generic = false;
    for f in p.funcs.iter_mut() {
        ck.func(f);
    }
    // instantiations (they were created while checking the code above, and may create more)
    let mut done: Vec<Func> = vec![];
    while let Some((mut f, why)) = ck.queue.pop() {
        let before = ck.errs.len();
        ck.func(&mut f);
        for d in ck.errs[before..].iter_mut() {
            d.notes.push(format!("while checking {}", why));
        }
        done.push(f);
    }
    // Some object types first appear when a generic body is instantiated.
    for i in 0..ck.types.dyns.len() {
        ck.object_methods(i, ck.types.dyns[i].1);
    }
    // fixed-size arrays hold Copy values only (checked here: struct fields are known by now)
    let mut arrays: Vec<(usize, Ty)> = ck.types.arrays.iter().map(|(s, (e, _))| (*s, *e)).collect();
    arrays.sort_by_key(|a| a.0);
    for (sid, elem) in arrays {
        if ck.types.owned(elem) {
            let pos = ck.types.structs[sid].pos;
            let d = ck.array_copy_error(elem, pos);
            ck.err(d);
        }
    }
    ck.with_ps(|_| ());
    p.funcs.extend(done);
    p.funcs.extend(std::mem::take(&mut ck.lifted));
    p.types = std::mem::take(&mut ck.types);
    let sigs = std::mem::take(&mut ck.sigs);
    let mut errs = std::mem::take(&mut ck.errs);
    if errs.is_empty() {
        Ok(sigs)
    } else {
        errs.sort_by_key(|d| (d.pos.line, d.pos.col));
        errs.dedup_by(|a, b| a.pos == b.pos && a.msg == b.msg);
        Err(errs)
    }
}

impl<'a> Checker<'a> {
    /// Declarations: bounds name real traits, impls match their traits.
    fn check_decls(&mut self) {
        let tnames: Vec<String> = self.ps.traits.iter().map(|t| t.name.clone()).collect();
        let user_traits: Vec<String> = self
            .ps
            .traits
            .iter()
            .filter(|t| !t.prelude)
            .map(|t| t.name.clone())
            .collect();
        let known = || tnames.join(", ");
        for g in self.ps.gfns.clone() {
            for tp in &g.tparams {
                for (b, bp) in &tp.bounds {
                    if !tnames.contains(b) && b != crate::syntax::CONST_BOUND {
                        self.err(
                            diag(format!("unknown trait '{}' in the bound of {}", b, tp.name), *bp)
                                .note(format!("known traits: {}", known()))
                                .note("help: declare it with `trait Name { fn method(view self) -> Int }`"),
                        );
                    }
                }
            }
        }
        let _ = user_traits;
        let n = self.ps.impls.len();
        for ii in 0..n {
            let (tn, tpos, ty, ipos, prelude) = {
                let im = &self.ps.impls[ii];
                (
                    im.trait_name.clone(),
                    im.trait_pos,
                    im.self_ty,
                    im.pos,
                    im.prelude,
                )
            };
            if prelude {
                continue;
            }
            let tyn = self.tn(ty);
            if matches!(ty, Ty::List(_) | Ty::String) {
                self.err(
                    diag(
                        format!("cannot add methods to the built-in type {}", tyn),
                        ipos,
                    )
                    .note("help: wrap it in your own struct and implement the trait for that"),
                );
                continue;
            }
            if matches!(ty, Ty::Param(_) | Ty::Err) {
                continue;
            }
            let tn = match tn {
                Some(t) => t,
                None => continue,
            };
            if tn == "Copy" {
                self.err(
                    diag("`Copy` cannot be implemented by hand", tpos)
                        .note("a type is Copy automatically when it holds no String or List"),
                );
                continue;
            }
            let tr = match self.ps.traits.iter().find(|t| t.name == tn) {
                Some(t) => t.methods.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
                None => {
                    self.err(
                        diag(format!("unknown trait '{}'", tn), tpos)
                            .note(format!("known traits: {}", known()))
                            .note(format!("help: declare it first: `trait {} {{ fn method(view self) -> Int }}`", tn)),
                    );
                    continue;
                }
            };
            // the same impl twice
            if self.ps.impls[..ii]
                .iter()
                .any(|o| o.trait_name.as_deref() == Some(&tn) && o.self_ty == ty)
            {
                self.err(
                    diag(format!("{} is already implemented for {}", tn, tyn), ipos)
                        .note("each trait can be implemented once per type (built-in impls count)"),
                );
                continue;
            }
            let have: Vec<(String, usize, Pos)> = self.ps.impls[ii]
                .methods
                .iter()
                .map(|m| (m.name.clone(), m.tok, m.pos))
                .collect();
            for (mn, tok, mpos) in &have {
                if !tr.contains(mn) {
                    self.err(
                        diag(format!("'{}' is not a method of trait {}", mn, tn), *mpos)
                            .note(format!("{} has: {}", tn, tr.join(", ")))
                            .note("help: move it to a plain `impl Type { .. }` block"),
                    );
                    continue;
                }
                let want = self.trait_header(&tn, mn, ty);
                let got = self
                    .with_ps(|ps| ps.parse_fn_at(*tok, vec![], Some(ty), true))
                    .ok()
                    .map(|x| x.0);
                if let (Some(w), Some(g)) = (want, got) {
                    let same = w.ret == g.ret
                        && w.params.len() == g.params.len()
                        && w.params
                            .iter()
                            .zip(&g.params)
                            .all(|(a, b)| a.mode == b.mode && a.ty == b.ty);
                    if !same {
                        self.err(
                            diag(format!("method '{}' does not match the signature in trait {}", mn, tn), *mpos)
                                .note(format!("trait {} requires: {}", tn, self.show_sig(&w)))
                                .note(format!("this impl declares: {}", self.show_sig(&g)))
                                .note("help: parameter modes (`view`/`mut`), types and the return type must be identical"),
                        );
                    }
                }
            }
            let missing: Vec<&String> = tr
                .iter()
                .filter(|m| !have.iter().any(|h| &h.0 == *m))
                .collect();
            if !missing.is_empty() {
                let mut sigs = vec![];
                for m in &missing {
                    if let Some(h) = self.trait_header(&tn, m, ty) {
                        sigs.push(format!("{} {{ .. }}", self.show_sig(&h)));
                    }
                }
                self.err(
                    diag(
                        format!(
                            "impl {} for {} is missing: {}",
                            tn,
                            tyn,
                            missing
                                .iter()
                                .map(|m| m.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        ipos,
                    )
                    .note(format!("help: add {}", sigs.join(" and "))),
                );
            }
        }
    }
}
