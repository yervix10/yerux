// Lexical identities in source order, before checker desugaring introduces temporaries.
use super::*;

#[derive(Default)]
pub(super) struct Lexical {
    bindings: Vec<(String, usize, usize)>, // name, id, closure depth
    next_binding: usize,
    closures: Vec<usize>,
    next_closure: usize,
    captures: HashSet<(usize, usize)>,
    pub(super) rows: Vec<String>,
}

impl Lexical {
    pub(super) fn function(&mut self, params: impl IntoIterator<Item = String>, body: &[Stmt]) {
        let mark = self.bindings.len();
        for n in params {
            self.bind(&n);
        }
        self.block(body);
        self.bindings.truncate(mark);
    }

    fn bind(&mut self, name: &str) {
        let id = self.next_binding;
        self.next_binding += 1;
        self.rows.push(format!("bind:{id}:{name}"));
        self.bindings.push((name.into(), id, self.closures.len()));
    }

    fn reference(&mut self, name: &str, kind: &str) {
        let Some((_, id, depth)) = self.bindings.iter().rev().find(|(n, _, _)| n == name) else {
            return; // a function, constant or other name owned by a later lookup
        };
        self.rows.push(format!("use:{kind}:{id}:{name}"));
        for &closure in &self.closures[*depth..] {
            if self.captures.insert((closure, *id)) {
                self.rows.push(format!("capture:{closure}:{id}:{name}"));
            }
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        let mark = self.bindings.len();
        for stmt in stmts {
            self.stmt(stmt);
        }
        self.bindings.truncate(mark);
    }

    fn unpack(&mut self, pat: &UnpackPat) {
        match pat {
            UnpackPat::Bind(n, ..) if n != "_" => self.bind(n),
            UnpackPat::Tuple(items, _) => items.iter().for_each(|p| self.unpack(p)),
            _ => {}
        }
    }

    fn pattern(&mut self, p: &Pat) {
        match p {
            Pat::Bind(n, inner, _) => {
                self.bind(n);
                if let Some(inner) = inner {
                    self.pattern(inner);
                }
            }
            Pat::Variant(_, items, _) | Pat::Tuple(items, _) => {
                items.iter().for_each(|p| self.pattern(p));
            }
            Pat::Or(items, _) => {
                if let Some(first) = items.first() {
                    self.pattern(first); // the checker verifies all alternatives bind the same names
                }
            }
            _ => {}
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.k {
            SK::Let(n, _, e, _) | SK::Bind(n, _, e, _) => {
                self.expr(e);
                self.bind(n);
            }
            SK::Unpack(p, e) => {
                self.expr(e);
                self.unpack(p);
            }
            SK::Assign(l, r) => {
                self.expr(l);
                self.expr(r);
            }
            SK::ExprS(e) | SK::Return(Some(e)) => self.expr(e),
            SK::If(e, yes, no) => {
                self.expr(e);
                self.block(yes);
                if let Some(no) = no {
                    self.block(no);
                }
            }
            SK::While(e, body, _) => {
                self.expr(e);
                self.block(body);
            }
            SK::For(head, body) => {
                match &head.iter {
                    ForIter::Range(lo, hi, _) => {
                        self.expr(lo);
                        self.expr(hi);
                    }
                    ForIter::Expr(e) => self.expr(e),
                }
                if let Some(step) = &head.step {
                    self.expr(step);
                }
                let mark = self.bindings.len();
                self.bind(&head.var);
                self.block(body);
                self.bindings.truncate(mark);
            }
            SK::Match(e, arms) => self.arms(e, arms),
            SK::Block(body, _) | SK::Unsafe(body) => self.block(body),
            SK::Defer(stmt) => self.stmt(stmt),
            SK::Break(..) | SK::Continue(..) | SK::Return(None) => {}
        }
    }

    pub(super) fn expr(&mut self, e: &Expr) {
        match &e.k {
            EK::Var(n, _) => self.reference(n, "read"),
            EK::Move(n, _) => self.reference(n, "move"),
            EK::Call(n, args) => {
                self.reference(n, "call");
                self.args(args);
            }
            EK::Closure { params, body, .. } => {
                let id = self.next_closure;
                self.next_closure += 1;
                self.rows.push(format!("closure:{id}"));
                self.closures.push(id);
                self.function(params.iter().map(|p| p.name.clone()), body);
                self.closures.pop();
            }
            EK::IfExpr(c, yes, no) => {
                self.expr(c);
                self.block(yes);
                self.block(no);
            }
            EK::MatchExpr(e, arms) => self.arms(e, arms),
            EK::AsElse(e, body) => {
                self.expr(e);
                self.block(body);
            }
            EK::Ascribe(e) | EK::Field(e, _) | EK::Un(_, e) | EK::ArrayFill(e, _) => self.expr(e),
            EK::Bin(_, l, r) => {
                self.expr(l);
                self.expr(r);
            }
            EK::Method { recv, args, .. } | EK::CallFn(recv, args) => {
                self.expr(recv);
                self.args(args);
            }
            EK::Static { args, .. } | EK::GenVariant { args, .. } | EK::ArrayLit(args) => {
                self.args(args);
            }
            EK::GenStruct { fields, .. } => fields.iter().for_each(|(_, e)| self.expr(e)),
            EK::SliceOf(e, lo, hi) => {
                self.expr(e);
                for bound in lo.iter().chain(hi) {
                    self.expr(bound);
                }
            }
            EK::Int(_) | EK::Float(..) | EK::Text(_) | EK::Char(_) | EK::Bool(_) => {}
            _ => unreachable!("resolved expression in source lexical pass"),
        }
    }

    fn arms(&mut self, e: &Expr, arms: &[Arm]) {
        self.expr(e);
        for arm in arms {
            let mark = self.bindings.len();
            if let Some(p) = &arm.pat {
                self.pattern(p);
            }
            if let Some(guard) = &arm.guard {
                self.expr(guard);
            }
            self.block(&arm.body);
            self.bindings.truncate(mark);
        }
    }

    fn args(&mut self, args: &[Expr]) {
        args.iter().for_each(|arg| self.expr(arg));
    }
}
