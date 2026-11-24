// Function values and closures (SPEC.md, "Function values and closures").
//
// A closure literal is lifted by the checker into an ordinary function `closure__N(env, params..)` whose
// first parameter is its environment, so the backends only ever see plain functions plus four small
// constructs: `MkClosure` (build a function value), `CallFn` (call one), `AddrOf` and `Deref` (borrowed
// captures). The body reaches every captured variable through a `Bind` at its start, exactly like a
// `view`/`mut` parameter, so all the usual ownership rules apply inside it unchanged.
use super::*;

/// Names mentioned by a closure and free uses that must be supplied by its environment.
#[derive(Default)]
pub(super) struct Scan {
    pub used: Vec<String>,
    free: Vec<String>,
    scopes: Vec<HashSet<String>>,
    /// root of an assignment target (`n = ..`, `p.x = ..`)
    pub assigned: HashSet<String>,
    /// root of a method receiver or of a call argument (may be modified by a `mut` parameter)
    pub touched: HashSet<String>,
    pub methods: HashSet<String>,
}

fn root_name(e: &Expr) -> Option<String> {
    match &e.k {
        EK::Var(n, _) | EK::Move(n, _) => Some(n.clone()),
        EK::Field(b, _) => root_name(b),
        EK::Method { recv, m, .. } if m == "at" => root_name(recv),
        _ => None,
    }
}

impl Scan {
    fn use_(&mut self, n: &str) {
        if !self.used.iter().any(|u| u == n) {
            self.used.push(n.to_string());
        }
        if !self.bound(n) && !self.free.iter().any(|u| u == n) {
            self.free.push(n.to_string());
        }
    }
    fn bound(&self, n: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(n))
    }
    fn declare(&mut self, n: &str) {
        self.scopes.last_mut().unwrap().insert(n.to_string());
    }
    fn touch(&mut self, n: String) {
        if !self.bound(&n) {
            self.touched.insert(n);
        }
    }
    fn pat(&mut self, p: &Pat) {
        match p {
            Pat::Bind(n, sub, _) => {
                self.declare(n);
                if let Some(s) = sub {
                    self.pat(s);
                }
            }
            Pat::Variant(_, subs, _) | Pat::Tuple(subs, _) | Pat::Or(subs, _) => {
                subs.iter().for_each(|s| self.pat(s))
            }
            _ => {}
        }
    }
    fn unpack(&mut self, p: &UnpackPat) {
        match p {
            UnpackPat::Bind(n, ..) => {
                self.declare(n);
            }
            UnpackPat::Tuple(items, _) => items.iter().for_each(|i| self.unpack(i)),
        }
    }
    fn args(&mut self, args: &[Expr]) {
        for a in args {
            self.expr(a);
            if let Some(r) = root_name(a) {
                self.touch(r);
            }
        }
    }
    pub fn expr(&mut self, e: &Expr) {
        match &e.k {
            EK::Var(n, _) => self.use_(n),
            EK::Move(n, _) => {
                self.use_(n);
                self.touch(n.clone());
            }
            EK::Field(b, _)
            | EK::Un(_, b)
            | EK::Conv(b, _)
            | EK::Ascribe(b)
            | EK::IsVariant(b, _)
            | EK::Payload(b, ..)
            | EK::AddrOf(b, _)
            | EK::Deref(b) => self.expr(b),
            EK::AsElse(b, blk) => {
                self.expr(b);
                self.stmts(blk);
            }
            EK::Bin(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            EK::Call(n, args) => {
                self.use_(n);
                self.args(args);
            }
            EK::Static { args, .. } | EK::Variant { args, .. } | EK::GenVariant { args, .. } => {
                self.args(args)
            }
            EK::Method { recv, m, args } => {
                self.methods.insert(m.clone());
                self.expr(recv);
                if let Some(r) = root_name(recv) {
                    self.touch(r);
                }
                self.args(args);
            }
            EK::StructLit { fields, .. } | EK::GenStruct { fields, .. } => {
                fields.iter().for_each(|(_, f)| self.expr(f))
            }
            EK::Closure { params, body, .. } => {
                self.scopes.push(params.iter().map(|p| p.name.clone()).collect());
                self.stmts(body);
                self.scopes.pop();
            }
            EK::MkClosure { caps, .. } => caps.iter().for_each(|c| self.expr(c)),
            EK::DynBox { value, .. } => self.expr(value),
            EK::DynCall { args, .. } => self.args(args),
            EK::CallFn(c, args) => {
                self.expr(c);
                self.args(args);
            }
            EK::Sequence(stmts, result) => {
                self.stmts(stmts);
                self.expr(result);
            }
            EK::IfExpr(c, t, e) => {
                self.expr(c);
                self.stmts(t);
                self.stmts(e);
            }
            EK::MatchExpr(x, arms) => {
                self.expr(x);
                self.arms(arms);
            }
            EK::Uninit => {}
            EK::ArrayLit(items) => self.args(items),
            EK::ArrayFill(b, _) => self.expr(b),
            EK::SliceOf(b, lo, hi) => {
                self.expr(b);
                if let Some(r) = root_name(b) {
                    self.touch(r);
                }
                lo.iter().chain(hi).for_each(|a| self.expr(a));
            }
            EK::Int(_) | EK::Float(..) | EK::Char(_) | EK::Bool(_) | EK::Text(_) => {}
        }
    }
    pub fn stmts(&mut self, b: &[Stmt]) {
        self.scopes.push(HashSet::new());
        b.iter().for_each(|s| self.stmt(s));
        self.scopes.pop();
    }
    fn stmt(&mut self, s: &Stmt) {
        match &s.k {
            SK::Let(n, _, e, _) => {
                self.expr(e);
                self.declare(n);
            }
            SK::Assign(a, b) => {
                self.expr(b);
                self.expr(a);
                let targets: Vec<&Expr> = match &a.k {
                    EK::GenStruct { fields, .. } => fields.iter().map(|(_, f)| f).collect(),
                    _ => vec![a],
                };
                for t in targets {
                    if let Some(r) = root_name(t).filter(|r| !self.bound(r)) {
                        self.assigned.insert(r.clone());
                        self.touch(r);
                    }
                }
            }
            SK::ExprS(e) => self.expr(e),
            SK::If(c, t, el) => {
                self.expr(c);
                self.stmts(t);
                if let Some(e) = el {
                    self.stmts(e);
                }
            }
            SK::While(c, b, meta) => {
                self.expr(c);
                self.stmts(b);
                self.stmts(&meta.step);
            }
            SK::Break(..) | SK::Continue(..) => {}
            SK::Block(b, _) => self.stmts(b),
            SK::Unsafe(b) => self.stmts(b),
            SK::Unpack(p, e) => {
                self.expr(e);
                self.unpack(p);
            }
            SK::Bind(n, _, e, _) => {
                self.expr(e);
                self.declare(n);
            }
            SK::For(h, body) => {
                match &h.iter {
                    ForIter::Range(a, b, _) => {
                        self.expr(a);
                        self.expr(b);
                    }
                    ForIter::Expr(e) => {
                        self.expr(e);
                        if h.mutable {
                            if let Some(r) = root_name(e) {
                                self.touch(r);
                            }
                        }
                    }
                }
                if let Some(s) = &h.step {
                    self.expr(s);
                }
                self.scopes.push(HashSet::from([h.var.clone()]));
                self.stmts(body);
                self.scopes.pop();
            }
            SK::Return(e) => {
                if let Some(e) = e {
                    self.expr(e)
                }
            }
            SK::Match(e, arms) => {
                self.expr(e);
                self.arms(arms);
            }
            SK::Defer(s) => self.stmt(s),
        }
    }
    fn arms(&mut self, arms: &[Arm]) {
        for a in arms {
            self.scopes.push(HashSet::new());
            for (n, _) in &a.binds {
                self.declare(n);
            }
            if let Some(p) = &a.pat {
                self.pat(p);
            }
            if let Some(g) = &a.guard {
                self.expr(g);
            }
            self.stmts(&a.body);
            self.scopes.pop();
        }
    }
}

/// One captured variable.
struct Cap {
    name: String,
    outer: usize,
    ty: Ty,
    by_ref: bool,
    mode: Mode, // how the body sees it (View/Mut), like a parameter
}

fn complete_worker_returns(body: &mut [Stmt], done: usize) {
    for stmt in body {
        match &mut stmt.k {
            SK::Return(value) if value.is_none() => {
                *value = Some(worker_done_expr(done, stmt.pos));
            }
            SK::Block(body, _) | SK::While(_, body, _) => complete_worker_returns(body, done),
            SK::If(_, yes, no) => {
                complete_worker_returns(yes, done);
                if let Some(no) = no {
                    complete_worker_returns(no, done);
                }
            }
            SK::Match(_, arms) => {
                for arm in arms {
                    complete_worker_returns(&mut arm.body, done);
                }
            }
            _ => {}
        }
    }
}

fn worker_done_expr(sid: usize, pos: Pos) -> Expr {
    Expr {
        k: EK::StructLit {
            sid,
            fields: vec![],
        },
        ty: Ty::Struct(sid),
        pos,
    }
}

impl<'a> Checker<'a> {
    /// A type that mentions a type parameter of the generic function currently being called (still unknown).
    fn mentions_free(&self, t: Ty) -> bool {
        match t {
            Ty::Param(p) => self.free_pids.contains(&p),
            Ty::List(i) => self.mentions_free(self.types.lists[i]),
            Ty::Fn(i) => {
                let f = self.types.fns[i].clone();
                f.params.iter().any(|(_, t)| self.mentions_free(*t)) || self.mentions_free(f.ret)
            }
            // `Option<U>`, `Pair<T, U>`, tuples: an instance of a generic type mentions its type arguments
            Ty::Enum(i) => match self.types.enums[i].origin.clone() {
                Some((_, args)) => args.iter().any(|a| self.mentions_free(*a)),
                None => false,
            },
            Ty::Struct(i) => match self.types.structs[i].origin.clone() {
                Some((_, args)) => args.iter().any(|a| self.mentions_free(*a)),
                None => false,
            },
            _ => false,
        }
    }

    /// Note added to "cannot move/return/copy" errors about a borrowed function value.
    pub(super) fn fn_borrow_note(&self, ty: Ty) -> Option<&'static str> {
        if matches!(ty, Ty::Fn(_)) {
            Some("a `view fn` parameter may borrow local variables of the caller for the duration of the call, so it cannot be stored or returned; declare the parameter `fn(..)` (without `view`) to take ownership of the closure")
        } else {
            None
        }
    }

    /// The function item `name` used as a value: a function value whose code is a thunk `name__fnv(env, args..)`.
    pub(super) fn fn_item(&mut self, name: &str, pos: Pos) -> Option<(Ty, EK)> {
        if self.ps.gfns.iter().any(|g| g.name == name) {
            self.err(
                diag(format!("the generic function '{}' cannot be used as a value yet", name), pos)
                    .note("a function value has one concrete type, and a generic function has none until it is called")
                    .note(format!("help: wrap the call in a closure: `|x| {}(x)`", name)),
            );
            return None;
        }
        if !self.sigs.contains_key(name) && !self.in_generic {
            self.load_lazy(name);
        }
        let sig = self.sigs.get(name).cloned().or_else(|| {
            if self.in_generic {
                self.lazy_header(name)
            } else {
                None
            }
        })?;
        let ty = self.types.intern_fn(FnTy {
            params: sig.params.clone(),
            ret: sig.ret,
        });
        let thunk = format!("{}__fnv", name);
        if !self.in_generic && self.thunks.insert(thunk.clone()) {
            let base = 0;
            let mut params = vec![Param {
                name: "env".into(),
                mode: Mode::Own,
                ty: Ty::Int,
                id: base,
            }];
            let mut args = vec![];
            for (k, (m, t)) in sig.params.iter().enumerate() {
                let pn = format!("a{}", k);
                let id = base + 1 + k;
                params.push(Param {
                    name: pn.clone(),
                    mode: *m,
                    ty: *t,
                    id,
                });
                let ek = if *m == Mode::Own && self.types.owned(*t) {
                    EK::Move(pn, id)
                } else {
                    EK::Var(pn, id)
                };
                args.push(Expr { k: ek, pos, ty: *t });
            }
            let call = Expr {
                k: EK::Call(name.to_string(), args),
                pos,
                ty: sig.ret,
            };
            let body = vec![Stmt {
                k: if sig.ret == Ty::Unit {
                    SK::ExprS(call)
                } else {
                    SK::Return(Some(call))
                },
                pos,
            }];
            self.sigs.insert(
                thunk.clone(),
                Sig {
                    params: params.iter().map(|p| (p.mode, p.ty)).collect(),
                    ret: sig.ret,
                },
            );
            self.lifted.push(Func {
                name: thunk.clone(),
                params,
                ret: sig.ret,
                body,
                pos,
            });
        }
        Some((
            ty,
            EK::MkClosure {
                func: thunk,
                env: None,
                caps: vec![],
            },
        ))
    }

    /// `Var(name)` where `name` is a function and not a variable: rewrite it to a function value.
    pub(super) fn norm_fn_arg(&mut self, arg: &mut Expr) {
        if let EK::Var(n, _) = &arg.k {
            if self.lookup(n).is_none() && !self.types.consts.contains_key(n) {
                let n = n.clone();
                if self.sigs.contains_key(&n)
                    || self.ps.gfns.iter().any(|g| g.name == n)
                    || self.lazy_known(&n)
                {
                    if let Some((ty, k)) = self.fn_item(&n, arg.pos) {
                        arg.k = k;
                        arg.ty = ty;
                    }
                }
            }
        }
    }

    pub(super) fn lazy_known(&mut self, n: &str) -> bool {
        if self.sigs.contains_key(n) || self.in_generic {
            return self.sigs.contains_key(n) || self.lazy_header(n).is_some();
        }
        self.load_lazy(n);
        self.sigs.contains_key(n)
    }

    /// Is this argument a function value that has to be made (closure literal, function name, call result)
    /// before it can be lent to a `view fn` / `mut fn` parameter?
    pub(super) fn needs_fn_temp(&mut self, arg: &mut Expr, mode: Mode, pty: Ty) -> bool {
        if mode == Mode::Own || !matches!(pty, Ty::Fn(_)) {
            return false;
        }
        self.norm_fn_arg(arg);
        !is_place(arg)
    }

    /// Check a function-valued argument for a borrowing parameter and hoist it into a hidden variable
    /// (declared in the current scope, created right before the statement), which then is lent like any
    /// variable. A closure literal given here may borrow the caller's variables for the call.
    pub(super) fn hoist_fn_arg(&mut self, arg: &mut Expr, mode: Mode, pty: Ty) -> Option<Ty> {
        let _pos = arg.pos;
        let t = if matches!(arg.k, EK::Closure { is_move: false, .. }) {
            self.closure_expr(arg, Some(pty), true, false, false)
        } else {
            self.hint = Some(pty);
            self.expr(arg)
        };
        if t == Ty::Err {
            return None;
        }
        self.hoist_temp(arg, t, mode);
        Some(t)
    }

    /// Declare the hidden variable for an already checked function-valued argument and create it before the statement.
    pub(super) fn hoist_temp(&mut self, arg: &mut Expr, t: Ty, mode: Mode) {
        let pos = arg.pos;
        let mut loans: Vec<(usize, u8, Pos)> = vec![];
        if let EK::MkClosure { caps, .. } = &arg.k {
            for c in caps {
                if let EK::AddrOf(v, m) = &c.k {
                    loans.push((path(v).0, *m as u8, pos));
                    self.closure_borrow_pos.insert((pos.line, pos.col));
                }
            }
        }
        let name = format!("fn__{}", self.vars.len());
        let id = self.declare(&name, t, Mode::Own, mode == Mode::Mut);
        let old = std::mem::replace(
            arg,
            Expr {
                k: EK::Var(name.clone(), id),
                pos,
                ty: t,
            },
        );
        if !loans.is_empty() {
            self.closure_loans.insert(id, loans);
        }
        self.pre.push(Stmt {
            k: SK::Let(name, mode == Mode::Mut, old, id),
            pos,
        });
    }

    /// `f(args)` where `f` is a place of function type.
    pub(super) fn call_value(&mut self, callee: &mut Expr, args: &mut Vec<Expr>, pos: Pos) -> Ty {
        if !is_place(callee) {
            // the function value is a temporary (`make_adder(1)(2)`): keep it in a hidden variable for the call
            let t = self.expr(callee);
            if t == Ty::Err {
                for a in args.iter_mut() {
                    self.expr(a);
                }
                return Ty::Err;
            }
            self.hoist_temp(callee, t, Mode::Own);
        }
        let Some(p) = self.place(callee) else {
            for a in args.iter_mut() {
                self.expr(a);
            }
            return Ty::Err;
        };
        let Ty::Fn(fi) = p.ty else {
            if p.ty != Ty::Err {
                let (n, t) = (place_str(callee), self.tn(p.ty));
                self.err(diag(
                    format!("'{}' is a {}, not a function, so it cannot be called", n, t),
                    pos,
                ));
            }
            return Ty::Err;
        };
        let ft = self.types.fns[fi].clone();
        if ft.params.len() != args.len() {
            let shown = self.tn(p.ty);
            self.err(
                diag(
                    format!(
                        "this function value takes {} argument(s) but {} were given",
                        ft.params.len(),
                        args.len()
                    ),
                    pos,
                )
                .note(format!("its type is {}", shown)),
            );
            return ft.ret;
        }
        let pre = vec![None; args.len()];
        self.call_args(
            Sig {
                params: ft.params,
                ret: ft.ret,
            },
            args,
            pre,
        )
    }

    pub(super) fn thread_transferable(&self, ty: Ty, seen: &mut HashSet<Ty>) -> bool {
        if !seen.insert(ty) {
            return true;
        }
        match ty {
            Ty::Fn(_) | Ty::Dyn(_) => false,
            Ty::List(i) => self.thread_transferable(self.types.lists[i], seen),
            Ty::Box(i) => self.thread_transferable(self.types.boxes[i], seen),
            Ty::Struct(i) => {
                let def = &self.types.structs[i];
                let name = def.origin.as_ref().map_or(&def.name, |(name, _)| name);
                !self.types.thread_bound.contains(name)
                    && self.types.join_payload(i).is_none()
                    && def
                        .fields
                        .iter()
                        .all(|(_, t)| self.thread_transferable(*t, seen))
            }
            Ty::Enum(i) => {
                let def = &self.types.enums[i];
                let name = def.origin.as_ref().map_or(&def.name, |(name, _)| name);
                !self.types.thread_bound.contains(name)
                    && def.variants.iter().all(|(_, fields)| {
                        fields.iter().all(|t| self.thread_transferable(*t, seen))
                    })
            }
            _ => true,
        }
    }

    /// Check a closure literal against the expected function type `hint`. `borrowing`: the literal is the
    /// argument of a `view fn` / `mut fn` parameter, so it may borrow the variables it uses instead of
    /// capturing them by value.
    pub(super) fn closure_expr(
        &mut self,
        e: &mut Expr,
        hint: Option<Ty>,
        borrowing: bool,
        spawning: bool,
        locking: bool,
    ) -> Ty {
        let pos = e.pos;
        let k = std::mem::replace(&mut e.k, EK::Int(0));
        let EK::Closure {
            params,
            ret,
            mut body,
            is_move,
            expr_body,
            parallel,
            ..
        } = k
        else {
            unreachable!("closure_expr")
        };
        e.ty = Ty::Err;
        let exp: Option<FnTy> = match hint {
            Some(Ty::Fn(i)) => Some(self.types.fns[i].clone()),
            _ => None,
        };
        if let Some(x) = &exp {
            if x.params.len() != params.len() {
                let want = self.tn(hint.unwrap());
                self.err(
                    diag(format!("this closure takes {} parameter(s), but {} are expected here", params.len(), x.params.len()), pos)
                        .note(format!("the expected type is {}", want))
                        .note("help: list one name per parameter: `|a, b| ..`; use `_` for one you do not need"),
                );
                return Ty::Err;
            }
        }
        // ── parameters ──
        let mut ptys: Vec<(Mode, Ty)> = vec![];
        for (k, p) in params.iter().enumerate() {
            let want = exp.as_ref().map(|x| x.params[k]);
            let r = match (p.ty, want) {
                (Some(t), Some((m, wt))) => {
                    if t != wt && !self.mentions_free(wt) && wt != Ty::Err {
                        let (g, w) = (self.tn(t), self.tn(wt));
                        self.err(diag(
                            format!(
                                "parameter '{}' is declared {} but {} is expected here",
                                p.name, g, w
                            ),
                            p.pos,
                        ));
                    }
                    (p.mode.unwrap_or(m), t)
                }
                (Some(t), None) => (p.mode.unwrap_or(Mode::Own), t),
                (None, Some((m, wt))) if !self.mentions_free(wt) && wt != Ty::Err => (m, wt),
                _ => {
                    self.err(
                        diag(format!("cannot infer the type of the closure parameter '{}'", p.name), p.pos)
                            .note("a closure learns its parameter types from where it is used (an argument of a known function type)")
                            .note(format!("help: write the type: `|{}: Int| ..`", p.name)),
                    );
                    (Mode::Own, Ty::Err)
                }
            };
            ptys.push(r);
        }
        if ptys.iter().any(|(_, t)| *t == Ty::Err) {
            return Ty::Err;
        }
        let declared_ret: Option<Ty> = ret.or_else(|| {
            exp.as_ref()
                .map(|x| x.ret)
                .filter(|r| !self.mentions_free(*r))
        });

        // `|x| list.push(x)`: an expression body whose value is not needed is just a statement
        if expr_body && declared_ret == Some(Ty::Unit) {
            if let Some(Stmt {
                k: SK::Return(Some(_)),
                ..
            }) = body.first()
            {
                if let SK::Return(Some(e)) =
                    std::mem::replace(&mut body[0].k, SK::Block(vec![], false))
                {
                    body[0].k = SK::ExprS(e);
                }
            }
        }

        // ── captures ──
        let mut sc = Scan {
            scopes: vec![params.iter().map(|p| p.name.clone()).collect()],
            ..Scan::default()
        };
        sc.stmts(&body);
        if locking
            && (sc.used.iter().any(|n| {
                matches!(
                    n.as_str(),
                    "__spawn_owned"
                        | "__thread_wait"
                        | "__thread_start"
                        | "__futex_park"
                        | "__locked_with"
                        | "__locked_with_view"
                        | "__parallel_range"
                        | "__parallel_list"
                )
            }) || sc.methods.iter().any(|m| {
                matches!(
                    m.as_str(),
                    "with"
                        | "with_view"
                        | "join"
                        | "recv"
                        | "sleep_ms"
                        | "read_line"
                        | "read_file"
                        | "write_file"
                )
            }))
        {
            self.err(diag(
                "cannot wait or acquire a lock inside a with closure",
                pos,
            ));
            return Ty::Err;
        }
        let mut caps: Vec<Cap> = vec![];
        let mut bad = false;
        for n in sc.free {
            let Some(outer) = self.lookup(&n) else {
                continue;
            };
            let (ty, omut, omode) = (
                self.vars[outer].ty,
                self.vars[outer].mutable,
                self.vars[outer].mode,
            );
            if self.types.contains_join_handle(ty) {
                self.err(diag(
                    format!(
                        "a closure cannot capture '{}' because it contains a JoinHandle",
                        n
                    ),
                    pos,
                ));
                bad = true;
                continue;
            }
            if spawning && omode != Mode::Own {
                self.err(diag(
                    format!("spawn cannot capture '{}' because it is borrowed", n),
                    pos,
                ));
                bad = true;
                continue;
            }
            if (spawning || parallel) && !self.thread_transferable(ty, &mut HashSet::new()) {
                let name = self.tn(ty);
                let operation = if spawning { "spawn" } else { "parallel for" };
                self.err(diag(
                    format!("{} cannot transfer '{}' of type {}", operation, n, name),
                    pos,
                ));
                bad = true;
                continue;
            }
            let assigned = sc.assigned.contains(&n);
            let touched = sc.touched.contains(&n);
            if parallel {
                if assigned {
                    self.err(diag(
                        format!("parallel for cannot assign to captured variable '{}'", n),
                        pos,
                    ));
                    bad = true;
                }
                caps.push(Cap {
                    name: n,
                    outer,
                    ty,
                    by_ref: true,
                    mode: Mode::View,
                });
            } else if borrowing && !is_move {
                if assigned && !omut {
                    self.err(
                        diag(
                            format!(
                                "this closure assigns '{}', but '{}' cannot be modified",
                                n, n
                            ),
                            pos,
                        )
                        .note(if omode == Mode::View {
                            "it is only borrowed for reading (`view`)".to_string()
                        } else {
                            "it was declared with `let` (immutable)".to_string()
                        })
                        .note("help: declare it with `var` (or receive it as `mut`)"),
                    );
                    bad = true;
                }
                let m = if omut && (assigned || touched) {
                    Mode::Mut
                } else {
                    Mode::View
                };
                caps.push(Cap {
                    name: n,
                    outer,
                    ty,
                    by_ref: true,
                    mode: m,
                });
            } else {
                if self.types.owned(ty) && !is_move {
                    let tn = self.tn(ty);
                    self.err(
                        diag(format!("this closure uses '{}', which owns memory (a {}), so it has to be moved into the closure", n, tn), pos)
                            .note("a closure that is stored or returned owns what it captured, and an owned value has a single owner")
                            .note(format!("help: write `move |..| ..`: '{}' then belongs to the closure and cannot be used afterwards", n))
                            .note("help: or pass the closure directly to a function that takes a `view fn(..)` parameter: then it only borrows the variable for the call"),
                    );
                    bad = true;
                } else if assigned && !is_move {
                    self.err(
                        diag(format!("this closure assigns '{}', but a stored closure works on its own copy", n), pos)
                            .note(format!("'{}' is copied into the closure when it is created; changing it inside would not change the variable outside", n))
                            .note("help: write `move |..| ..` to say that the closure keeps its own state (a counter, an accumulator)")
                            .note("help: or pass the closure directly to a `view fn(..)` parameter: then it works on the variable itself during the call"),
                    );
                    bad = true;
                }
                // a spawned worker owns what it captured (owned values are moved in, scalars copied), so a nested
                // spawn may capture it again
                let mode = if spawning {
                    Mode::Own
                } else if is_move {
                    Mode::Mut
                } else {
                    Mode::View
                };
                caps.push(Cap {
                    name: n,
                    outer,
                    ty,
                    by_ref: false,
                    mode,
                });
            }
        }
        if bad {
            return Ty::Err;
        }

        // ── the outer side: what the closure takes from its surroundings, checked like ordinary uses ──
        let mut cap_exprs: Vec<Expr> = vec![];
        for c in &caps {
            let mut v = Expr {
                k: EK::Var(c.name.clone(), c.outer),
                pos,
                ty: c.ty,
            };
            if c.by_ref {
                if c.mode == Mode::Mut {
                    self.ev_escape.push(c.outer);
                }
                if self.place(&mut v).is_some() && c.mode == Mode::Mut {
                    self.frozen_check(&v, true, pos);
                }
                cap_exprs.push(Expr {
                    k: EK::AddrOf(Box::new(v), c.mode == Mode::Mut),
                    pos,
                    ty: Ty::Int,
                });
            } else {
                if self.types.owned(c.ty) {
                    v.k = EK::Move(c.name.clone(), c.outer);
                }
                let t = self.expr(&mut v);
                if t == Ty::Err {
                    return Ty::Err;
                }
                cap_exprs.push(v);
            }
        }

        // ── the environment type ──
        self.nclos += 1;
        let cid = self.nclos;
        let sid = if caps.is_empty() {
            None
        } else {
            let mut fields = vec![("env_drop".to_string(), Ty::Int)];
            for (k, c) in caps.iter().enumerate() {
                fields.push((format!("c{}", k), if c.by_ref { Ty::Int } else { c.ty }));
                if c.mode == Mode::Own {
                    fields.push((format!("live_c{k}"), Ty::Int));
                }
            }
            let generic = self.in_generic || fields.iter().any(|(_, t)| self.types.has_param(*t));
            self.types.structs.push(StructDef {
                name: "closure environment".into(),
                cname: format!("Env__{}", cid),
                fields,
                pos,
                origin: None,
                generic,
            });
            Some(self.types.structs.len() - 1)
        };
        let env_ty = sid.map_or(Ty::Int, Ty::Struct);

        // ── the body, in a context of its own ──
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        let saved_st = std::mem::take(&mut self.st);
        let saved_d = std::mem::take(&mut self.dscopes);
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_frozen = std::mem::take(&mut self.frozen);
        let saved_pre = std::mem::take(&mut self.pre);
        let saved_ret = (self.ret, self.ret_free);
        let saved_free = std::mem::take(&mut self.free_pids);
        let env_id = self.declare(
            "env",
            env_ty,
            if sid.is_some() { Mode::Mut } else { Mode::Own },
            sid.is_some(),
        );
        let mut inner: Vec<usize> = vec![];
        for c in &caps {
            let id = self.declare(&c.name, c.ty, c.mode, c.mode != Mode::View);
            inner.push(id);
        }
        let mut fparams = vec![Param {
            name: "env".into(),
            mode: if sid.is_some() { Mode::Mut } else { Mode::Own },
            ty: env_ty,
            id: env_id,
        }];
        for (p, (m, t)) in params.iter().zip(&ptys) {
            let id = self.declare(&p.name, *t, *m, *m != Mode::View);
            fparams.push(Param {
                name: p.name.clone(),
                mode: *m,
                ty: *t,
                id,
            });
        }
        self.ret = declared_ret.unwrap_or(Ty::Err);
        self.ret_free = declared_ret.is_none();
        for (id, capture) in inner.iter().zip(&caps) {
            if capture.mode != Mode::Own {
                self.capture_ids.insert(*id);
            }
        }
        self.closure_depth += 1;
        let saved_pending = std::mem::take(&mut self.pending_bare);
        self.block(&mut body);
        if self.pending_bare > 0 {
            let hint = if self.ret_free { None } else { Some(self.ret) };
            self.fix_bare_returns(&mut body, hint);
            if self.ret_free {
                // every return was a bare `None`: the inference error above says it; keep the closure quiet
                self.ret = Ty::Err;
                self.ret_free = false;
            }
        }
        self.pending_bare = saved_pending;
        self.closure_depth -= 1;
        let mut ret_ty = if self.ret_free { Ty::Unit } else { self.ret };
        if (spawning || locking) && ret_ty == Ty::Unit {
            let done = self
                .types
                .structs
                .iter()
                .position(|d| d.name == "ThreadDone" && d.pos.line >= PRELUDE_LINE)
                .expect("ThreadDone is defined in the prelude");
            complete_worker_returns(&mut body, done);
            if !always_returns(&body) {
                body.push(Stmt {
                    k: SK::Return(Some(worker_done_expr(done, pos))),
                    pos,
                });
            }
            ret_ty = Ty::Struct(done);
        }
        if ret_ty != Ty::Unit && ret_ty != Ty::Err && !always_returns(&body) {
            let n = self.tn(ret_ty);
            self.err(diag(
                format!("this closure must return a {} on every path", n),
                pos,
            ));
        }
        self.scopes = saved_scopes;
        self.st = saved_st;
        self.dscopes = saved_d;
        self.loops = saved_loops;
        self.frozen = saved_frozen;
        self.pre = saved_pre;
        self.ret = saved_ret.0;
        self.ret_free = saved_ret.1;
        self.free_pids = saved_free;

        if spawning && !self.thread_transferable(ret_ty, &mut HashSet::new()) {
            let name = self.tn(ret_ty);
            self.err(diag(
                format!("spawn cannot return a value of type {}", name),
                pos,
            ));
            return Ty::Err;
        }

        // Bind captures to their environment fields. A worker may consume its owned captures once.
        let mut binds: Vec<Stmt> = vec![];
        for (k, c) in caps.iter().enumerate() {
            let envv = Expr {
                k: EK::Var("env".into(), env_id),
                pos,
                ty: env_ty,
            };
            let fty = if c.by_ref { Ty::Int } else { c.ty };
            let field = Expr {
                k: EK::Field(Box::new(envv), format!("c{}", k)),
                pos,
                ty: fty,
            };
            let src = if c.by_ref {
                Expr {
                    k: EK::Deref(Box::new(field)),
                    pos,
                    ty: c.ty,
                }
            } else {
                field
            };
            binds.push(Stmt {
                k: SK::Bind(c.name.clone(), c.mode, src, inner[k]),
                pos,
            });
        }
        body.splice(0..0, binds);
        let name = format!("closure__{}", cid);
        let fty = self.types.intern_fn(FnTy {
            params: ptys,
            ret: ret_ty,
        });
        let sig = Sig {
            params: fparams.iter().map(|p| (p.mode, p.ty)).collect(),
            ret: ret_ty,
        };
        self.sigs.insert(name.clone(), sig);
        if !self.in_generic {
            self.lifted.push(Func {
                name: name.clone(),
                params: fparams,
                ret: ret_ty,
                body,
                pos,
            });
        }
        e.k = EK::MkClosure {
            func: name,
            env: sid,
            caps: cap_exprs,
        };
        e.ty = fty;
        fty
    }

    /// Replace the type parameters of the generic function being called that are already known.
    pub(super) fn subst_known(&mut self, t: Ty, pids: &[usize], bind: &[Option<Ty>]) -> Ty {
        match t {
            Ty::Param(p) => match pids.iter().position(|x| *x == p) {
                Some(k) => bind[k].unwrap_or(t),
                None => t,
            },
            Ty::List(i) => {
                let e = self.types.lists[i];
                let ne = self.subst_known(e, pids, bind);
                if ne == e {
                    t
                } else if self.types.slices.contains(&i) {
                    self.types.intern_slice(ne)
                } else {
                    self.types.intern_list(ne)
                }
            }
            Ty::Struct(_) if self.types.array_of(t).is_some() => {
                let (e, n) = self.types.array_of(t).unwrap();
                let ne = self.subst_known(e, pids, bind);
                let nn = if n >= crate::syntax::CONST_LEN {
                    match pids.iter().position(|x| *x == n - crate::syntax::CONST_LEN) {
                        Some(k) => match bind[k] {
                            Some(Ty::Range(v, _)) => v as usize,
                            _ => n,
                        },
                        None => n,
                    }
                } else {
                    n
                };
                if ne == e && nn == n {
                    t
                } else {
                    let pos = self.types.structs[match t {
                        Ty::Struct(s) => s,
                        _ => unreachable!(),
                    }]
                    .pos;
                    self.types.intern_array(ne, nn, pos)
                }
            }
            Ty::Fn(i) => {
                let f = self.types.fns[i].clone();
                let params = f
                    .params
                    .iter()
                    .map(|(m, t)| (*m, self.subst_known(*t, pids, bind)))
                    .collect();
                let ret = self.subst_known(f.ret, pids, bind);
                self.types.intern_fn(FnTy { params, ret })
            }
            _ => t,
        }
    }
}
