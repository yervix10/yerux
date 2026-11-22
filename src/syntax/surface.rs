// Source AST before module loading, name resolution and constant folding.
use super::{lexical::Lexical, *};

#[derive(Default)]
pub(super) struct Surface {
    // In source mode, Ty::Param indexes a serialized type and array lengths are one-based
    // handles. This preserves their spelling without registering or resolving compiler types.
    types: Vec<String>,
    lengths: Vec<String>,
    structs: HashSet<String>,
    imports: HashSet<String>,
    lexical: Option<Lexical>,
}

fn node(tag: &str, fields: impl IntoIterator<Item = String>) -> String {
    format!(
        "{tag}({})",
        fields.into_iter().collect::<Vec<_>>().join(",")
    )
}

fn list(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}

fn optional<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map(f).unwrap_or_else(|| "_".into())
}

fn name(s: &str) -> String {
    dump_hex(s)
}

pub fn dump(src: &str) -> PR<String> {
    source(src, false)
}

pub fn dump_locals(src: &str) -> PR<String> {
    source(src, true)
}

fn source(src: &str, lexical: bool) -> PR<String> {
    let mut p = Parser::new_raw(lex(src)?);
    let structs =
        p.t.windows(2)
            .filter_map(|pair| match (&pair[0].t, &pair[1].t) {
                (Tok::Id(k), Tok::Id(n)) if k == "struct" => Some(n.clone()),
                _ => None,
            })
            .collect();
    p.surface = Some(Surface {
        structs,
        lexical: lexical.then(Lexical::default),
        ..Surface::default()
    });
    let mut items = Vec::new();
    while !matches!(p.peek(), Tok::Eof) {
        items.push(p.source_item()?);
    }
    if let Some(lexical) = p.surface.unwrap().lexical {
        Ok(lexical.rows.join("\n"))
    } else {
        Ok(node("File", [list(items)]))
    }
}

impl Parser {
    fn source_eat_kw(&mut self, word: &str) -> bool {
        if self.is_kw(word) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn source_path(&mut self) -> PR<String> {
        let mut path = self.ident()?.0;
        self.source_path_tail(&mut path)?;
        Ok(path)
    }

    pub(super) fn source_path_tail(&mut self, path: &mut String) -> PR<()> {
        while self.is_sym(".") && matches!(self.peek_at(1), Tok::Id(n) if !KW.contains(&n.as_str()))
        {
            self.bump();
            path.push('.');
            path.push_str(&self.ident()?.0);
        }
        Ok(())
    }

    fn source_list<T>(
        &mut self,
        close: &str,
        mut parse: impl FnMut(&mut Self) -> PR<T>,
    ) -> PR<Vec<T>> {
        let mut values = Vec::new();
        while !self.is_sym(close) {
            values.push(parse(self)?);
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(close)?;
        Ok(values)
    }

    fn source_mode(&mut self) -> Mode {
        if self.source_eat_kw("view") {
            Mode::View
        } else if self.source_eat_kw("mut") {
            Mode::Mut
        } else {
            Mode::Own
        }
    }

    fn source_string(&mut self) -> PR<String> {
        match self.bump() {
            Token { t: Tok::Str(s), .. } => Ok(name(&s)),
            t => Err(diag("expected a string", t.pos)),
        }
    }

    pub(super) fn source_length(&mut self) -> PR<i64> {
        let length = match self.peek() {
            Tok::Num(n) => node("Int", [n.to_string()]),
            Tok::Id(n) if !KW.contains(&n.as_str()) => node("Var", [name(n)]),
            _ => return Err(diag("expected an array length", self.pos())),
        };
        self.bump();
        let lengths = &mut self.surface.as_mut().unwrap().lengths;
        lengths.push(length);
        Ok(lengths.len() as i64)
    }

    pub(super) fn source_type(&mut self) -> PR<Ty> {
        let text = if self.source_eat_kw("fn") {
            self.expect_sym("(")?;
            let params = self.source_list(")", |p| {
                let mode = p.source_mode();
                let ty = p.ty()?;
                Ok(node(
                    "TypeParam",
                    [mode_name(mode).into(), p.source_ty_text(ty)],
                ))
            })?;
            node("FnType", [list(params), self.source_ret()?])
        } else if self.source_eat_kw("dyn") {
            node("DynType", [name(&self.source_path()?)])
        } else if self.eat_sym("[") {
            let ty = self.ty()?;
            let ty = self.source_ty_text(ty);
            let length = if self.eat_sym(";") {
                let id = self.source_length()? as usize;
                self.source_length_text(id)
            } else {
                "_".into()
            };
            self.expect_sym("]")?;
            node("BracketType", [ty, length])
        } else if self.eat_sym("(") {
            let types = self.source_list(")", |p| p.ty())?;
            node("TupleType", [self.source_types(&types)])
        } else {
            let path = self.source_path()?;
            let args = self.source_type_args(&path)?;
            node("Type", [name(&path), self.source_types(&args)])
        };
        let types = &mut self.surface.as_mut().unwrap().types;
        types.push(text);
        Ok(Ty::Param(types.len() - 1))
    }

    fn source_type_args(&mut self, path: &str) -> PR<Vec<Ty>> {
        if !self.eat_sym("<") {
            return Ok(vec![]);
        }
        if path == "Int" {
            let bound = |p: &mut Self| -> PR<String> {
                let neg = p.eat_sym("-");
                let value = match p.bump() {
                    Token { t: Tok::Num(n), .. } => n.to_string(),
                    Token {
                        t: Tok::NumT(n, ""),
                        ..
                    } => n.to_string(),
                    t => return Err(diag("expected an integer bound", t.pos)),
                };
                Ok(if neg { format!("-{value}") } else { value })
            };
            let low = if self.is_sym("..") {
                "_".into()
            } else {
                bound(self)?
            };
            self.expect_sym("..")?;
            let high = if self.is_sym(">") {
                "_".into()
            } else {
                bound(self)?
            };
            self.expect_sym(">")?;
            let types = &mut self.surface.as_mut().unwrap().types;
            types.push(node("Range", [low, high]));
            return Ok(vec![Ty::Param(types.len() - 1)]);
        }
        self.source_list(">", |p| p.ty())
    }

    fn source_ty_text(&self, ty: Ty) -> String {
        match ty {
            Ty::Param(id) => self.surface.as_ref().unwrap().types[id].clone(),
            _ => unreachable!("surface AST type"),
        }
    }

    fn source_length_text(&self, id: usize) -> String {
        self.surface.as_ref().unwrap().lengths[id - 1].clone()
    }

    fn source_types(&self, types: &[Ty]) -> String {
        list(types.iter().map(|&t| self.source_ty_text(t)))
    }

    fn source_ret(&mut self) -> PR<String> {
        if self.eat_sym("->") {
            let ty = self.ty()?;
            Ok(self.source_ty_text(ty))
        } else {
            Ok("_".into())
        }
    }

    fn source_generics(&mut self) -> PR<String> {
        if !self.eat_sym("<") {
            return Ok(list([]));
        }
        let params = self.source_list(">", |p| {
            let constant = p.is_kw("const") && matches!(p.peek_at(1), Tok::Id(_));
            if constant {
                p.bump();
            }
            let n = name(&p.ident()?.0);
            if constant {
                p.expect_sym(":")?;
                let ty = p.ty()?;
                return Ok(node("ConstParam", [n, p.source_ty_text(ty)]));
            }
            let mut bounds = Vec::new();
            if p.eat_sym(":") {
                loop {
                    bounds.push(name(&p.source_path()?));
                    if !p.eat_sym("+") {
                        break;
                    }
                }
            }
            Ok(node("Generic", [n, list(bounds)]))
        })?;
        Ok(list(params))
    }

    fn source_fn(&mut self, body: bool, method: bool, external: bool) -> PR<String> {
        if !self.source_eat_kw("fn") {
            return Err(diag("expected a function", self.pos()));
        }
        let n = name(&self.ident()?.0);
        let generics = self.source_generics()?;
        self.expect_sym("(")?;
        let mut first = true;
        let mut param_names = Vec::new();
        let params = self.source_list(")", |p| {
            if external && !first && p.eat_sym("..") {
                p.expect_sym(".")?;
                if !p.is_sym(")") {
                    return Err(diag("variadic parameter must be last", p.pos()));
                }
                return Ok(node("Variadic", []));
            }
            let at = usize::from(p.is_kw("view") || p.is_kw("mut"));
            let receiver = first
                && method
                && matches!(p.peek_at(at), Tok::Id(w) if w == "self")
                && matches!(p.peek_at(at + 1), Tok::Sym("," | ")"));
            first = false;
            if receiver {
                let mode = p.source_mode();
                p.bump();
                param_names.push("self".into());
                return Ok(node("Self", [mode_name(mode).into()]));
            }
            let param = p.ident()?.0;
            let n = name(&param);
            param_names.push(param);
            p.expect_sym(":")?;
            let mode = p.source_mode();
            let ty = p.ty()?;
            Ok(node(
                "Param",
                [n, mode_name(mode).into(), p.source_ty_text(ty)],
            ))
        })?;
        let ret = self.source_ret()?;
        let block = if body {
            let block = self.block()?;
            if let Some(lexical) = &mut self.surface.as_mut().unwrap().lexical {
                lexical.function(param_names, &block);
            }
            self.source_block(&block)
        } else {
            self.eat_sym(";");
            "_".into()
        };
        Ok(node("Fn", [n, generics, list(params), ret, block]))
    }

    fn source_item(&mut self) -> PR<String> {
        let public = self.source_eat_kw("pub");
        let attribute = if self.eat_sym("@") {
            name(&self.ident()?.0)
        } else {
            "_".into()
        };
        let item = if self.is_kw("fn") {
            self.source_fn(true, false, false)?
        } else if self.source_eat_kw("import") {
            let path = self.source_path()?;
            let alias = if self.source_eat_kw("as") {
                Some(self.ident()?.0)
            } else {
                None
            };
            let local = alias
                .as_deref()
                .unwrap_or_else(|| path.rsplit('.').next().unwrap());
            self.surface.as_mut().unwrap().imports.insert(local.into());
            self.eat_sym(";");
            node("Import", [name(&path), optional(alias.as_deref(), name)])
        } else if self.source_eat_kw("test") {
            let n = self.source_string()?;
            let body = self.block()?;
            if let Some(lexical) = &mut self.surface.as_mut().unwrap().lexical {
                lexical.function([], &body);
            }
            node("Test", [n, self.source_block(&body)])
        } else if self.source_eat_kw("link") {
            node("Link", [self.source_string()?])
        } else if self.source_eat_kw("extern") {
            let abi = self.source_string()?;
            self.expect_sym("{")?;
            let mut funcs = Vec::new();
            while !self.is_sym("}") {
                funcs.push(self.source_fn(false, false, true)?);
            }
            self.bump();
            node("Extern", [abi, list(funcs)])
        } else if self.source_eat_kw("const") {
            let n = name(&self.ident()?.0);
            let ty = if self.eat_sym(":") {
                let ty = self.ty()?;
                self.source_ty_text(ty)
            } else {
                "_".into()
            };
            self.expect_sym("=")?;
            let value = self.expr()?;
            if let Some(lexical) = &mut self.surface.as_mut().unwrap().lexical {
                lexical.expr(&value);
            }
            self.eat_sym(";");
            node("Const", [n, ty, self.source_expr(&value)])
        } else if self.source_eat_kw("type") {
            let n = name(&self.ident()?.0);
            self.expect_sym("=")?;
            let ty = self.ty()?;
            self.eat_sym(";");
            node("Alias", [n, self.source_ty_text(ty)])
        } else if self.is_kw("struct") || self.is_kw("enum") {
            let is_struct = self.source_eat_kw("struct");
            if !is_struct {
                self.bump();
            }
            let n = name(&self.ident()?.0);
            let generics = self.source_generics()?;
            self.expect_sym("{")?;
            let entries = self.source_list("}", |p| {
                let n = name(&p.ident()?.0);
                if is_struct {
                    p.expect_sym(":")?;
                    let ty = p.ty()?;
                    Ok(node("FieldDef", [n, p.source_ty_text(ty)]))
                } else {
                    let payload = if p.eat_sym("(") {
                        p.source_list(")", |p| p.ty())?
                    } else {
                        vec![]
                    };
                    Ok(node("VariantDef", [n, p.source_types(&payload)]))
                }
            })?;
            node(
                if is_struct { "Struct" } else { "Enum" },
                [n, generics, list(entries)],
            )
        } else if self.source_eat_kw("trait") {
            let n = name(&self.ident()?.0);
            self.expect_sym("{")?;
            let mut methods = Vec::new();
            while !self.is_sym("}") {
                methods.push(self.source_fn(false, true, false)?);
            }
            self.bump();
            node("Trait", [n, list(methods)])
        } else if self.source_eat_kw("impl") {
            let generics = self.source_generics()?;
            let ty = self.ty()?;
            let (trait_ty, target) = if self.source_eat_kw("for") {
                (self.source_ty_text(ty), self.ty()?)
            } else {
                ("_".into(), ty)
            };
            self.expect_sym("{")?;
            let mut methods = Vec::new();
            while !self.is_sym("}") {
                methods.push(self.source_fn(true, true, false)?);
            }
            self.bump();
            node(
                "Impl",
                [
                    generics,
                    trait_ty,
                    self.source_ty_text(target),
                    list(methods),
                ],
            )
        } else {
            return Err(diag("expected a declaration", self.pos()));
        };
        Ok(node("Item", [public.to_string(), attribute, item]))
    }

    fn source_struct_name(&self, path: &str) -> bool {
        let surface = self.surface.as_ref().unwrap();
        match path.split_once('.') {
            None => surface.structs.contains(path),
            // A qualified constructor can only have one module prefix. Its declaration lives
            // in another file, so use the source type-name convention for empty literals.
            Some((alias, ty)) => {
                surface.imports.contains(alias)
                    && !ty.contains('.')
                    && ty.starts_with(|c: char| c.is_ascii_uppercase())
            }
        }
    }

    pub(super) fn source_after_name(&mut self, first: String, pos: Pos) -> PR<Expr> {
        let start = self.i;
        let mut path = first.clone();
        self.source_path_tail(&mut path)?;
        let angle = self.i;
        let args = if self.is_sym("<") {
            let type_count = self.surface.as_ref().unwrap().types.len();
            match self.source_type_args(&path) {
                Ok(args) if self.is_sym(".") || self.is_sym("{") => Some(args),
                _ => {
                    self.i = angle;
                    self.surface.as_mut().unwrap().types.truncate(type_count);
                    None
                }
            }
        } else {
            None
        };
        let has_fields = matches!(self.peek_at(1), Tok::Id(n) if !KW.contains(&n.as_str()))
            && *self.peek_at(2) == Tok::Sym(":")
            && !matches!(self.peek_at(3), Tok::Id(n) if n == "while" || n == "for");
        let empty_literal =
            *self.peek_at(1) == Tok::Sym("}") && (args.is_some() || self.source_struct_name(&path));
        if self.is_sym("{") && (has_fields || empty_literal) {
            let (fields, end) = self.struct_lit_fields()?;
            return Ok(mk(
                EK::GenStruct {
                    name: path,
                    targs: args,
                    fields,
                },
                span(pos, end),
            ));
        }
        if let Some(args) = args {
            self.expect_sym(".")?;
            let (vname, vp) = self.ident()?;
            let (values, end) = if self.is_sym("(") {
                self.args()?
            } else {
                (vec![], vp)
            };
            return Ok(mk(
                EK::GenVariant {
                    name: path,
                    targs: Some(args),
                    vname,
                    args: values,
                },
                span(pos, end),
            ));
        }
        self.i = start;
        if self.is_sym("(") {
            let (args, end) = self.args()?;
            return Ok(mk(EK::Call(first, args), span(pos, end)));
        }
        Ok(mk(EK::Var(first, 0), pos))
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Own => "Own",
        Mode::View => "View",
        Mode::Mut => "Mut",
    }
}

fn source_pat(pat: &Pat) -> String {
    match pat {
        Pat::Wild(_) => node("Wild", []),
        Pat::Bind(n, p, _) => node("Bind", [name(n), optional(p.as_deref(), source_pat)]),
        Pat::Int(n, _) => node("Int", [n.to_string()]),
        Pat::Char(n, _) => node("Char", [n.to_string()]),
        Pat::Bool(b, _) => node("Bool", [b.to_string()]),
        Pat::Range(lo, hi, ch, _) => node(
            "PatRange",
            [
                ch.to_string(),
                optional(*lo, |x| x.to_string()),
                optional(*hi, |x| x.to_string()),
            ],
        ),
        Pat::Variant(n, args, _) => node("Variant", [name(n), list(args.iter().map(source_pat))]),
        Pat::Tuple(items, _) => node("TuplePat", [list(items.iter().map(source_pat))]),
        Pat::Or(items, _) => node("Or", [list(items.iter().map(source_pat))]),
    }
}

fn source_unpack(pat: &UnpackPat) -> String {
    match pat {
        UnpackPat::Bind(n, mutable, _, _) => node("UnpackBind", [name(n), mutable.to_string()]),
        UnpackPat::Tuple(items, _) => node("UnpackTuple", [list(items.iter().map(source_unpack))]),
    }
}

impl Parser {
    fn source_block(&self, stmts: &[Stmt]) -> String {
        node("Block", [list(stmts.iter().map(|s| self.source_stmt(s)))])
    }

    fn source_stmt(&self, stmt: &Stmt) -> String {
        let expr = |x: &Expr| self.source_expr(x);
        let block = |b: &Vec<Stmt>| self.source_block(b);
        let label = |l: &Option<(String, Pos)>| optional(l.as_ref(), |(n, _)| name(n));
        match &stmt.k {
            SK::Let(n, mutable, value, _) => {
                node("Let", [name(n), mutable.to_string(), expr(value)])
            }
            SK::Unpack(p, x) => node("Unpack", [source_unpack(p), expr(x)]),
            SK::Assign(l, r) => node("Assign", [expr(l), expr(r)]),
            SK::ExprS(e) => node("Expr", [expr(e)]),
            SK::If(c, t, e) => node("If", [expr(c), block(t), optional(e.as_ref(), block)]),
            SK::While(c, body, lp) => node("While", [label(&lp.label), expr(c), block(body)]),
            SK::For(head, body) => {
                let iter = match &head.iter {
                    ForIter::Range(lo, hi, incl) => {
                        node("IterRange", [expr(lo), expr(hi), incl.to_string()])
                    }
                    ForIter::Expr(e) => expr(e),
                };
                node(
                    "For",
                    [
                        label(&head.label),
                        name(&head.var),
                        head.mutable.to_string(),
                        iter,
                        optional(head.step.as_ref(), expr),
                        block(body),
                    ],
                )
            }
            SK::Break(l, _) => node("Break", [label(l)]),
            SK::Continue(l, _) => node("Continue", [label(l)]),
            SK::Block(b, _) => node("Scope", [block(b)]),
            SK::Unsafe(b) => node("Unsafe", [block(b)]),
            SK::Return(e) => node("Return", [optional(e.as_ref(), expr)]),
            SK::Match(e, arms) => node(
                "Match",
                [
                    expr(e),
                    list(arms.iter().map(|a| {
                        node(
                            "Arm",
                            [
                                source_pat(a.pat.as_ref().expect("surface match pattern")),
                                optional(a.guard.as_ref(), expr),
                                block(&a.body),
                            ],
                        )
                    })),
                ],
            ),
            SK::Defer(s) => node("Defer", [self.source_stmt(s)]),
            SK::Bind(..) => unreachable!("checker-only statement"),
        }
    }

    fn source_expr(&self, e: &Expr) -> String {
        let expr = |x: &Expr| self.source_expr(x);
        let args = |items: &[Expr]| list(items.iter().map(expr));
        match &e.k {
            EK::Ascribe(x) => node("Ascribe", [self.source_ty_text(e.ty), expr(x)]),
            EK::AsElse(x, b) => node(
                "Cast",
                [expr(x), self.source_ty_text(e.ty), self.source_block(b)],
            ),
            EK::Static { ty, args, .. } => node(
                "Cast",
                [expr(&args[0]), self.source_ty_text(*ty), "_".into()],
            ),
            EK::Int(n) => {
                if matches!(e.ty, Ty::Int | Ty::Num(_)) {
                    node("NumT", [(*n as u64).to_string(), numeric::sfx(e.ty).into()])
                } else if e.ty == Ty::Err {
                    node("NumT", [(*n as u64).to_string(), String::new()])
                } else {
                    node("Int", [n.to_string()])
                }
            }
            EK::Float(_, text) => node("Float", [name(text), numeric::sfx(e.ty).into()]),
            EK::Char(n) => node("Char", [n.to_string()]),
            EK::Bool(b) => node("Bool", [b.to_string()]),
            EK::Text(s) => node("Text", [name(s)]),
            EK::Var(n, _) => node("Var", [name(n)]),
            EK::Move(n, _) => node("Move", [name(n)]),
            EK::Field(x, n) => node("Field", [expr(x), name(n)]),
            EK::Un(op, x) => node("Un", [name(op), expr(x)]),
            EK::Bin(op, l, r) => node("Bin", [name(op), expr(l), expr(r)]),
            EK::Call(n, a) => node("Call", [name(n), args(a)]),
            EK::Method { recv, m, args: a } => node("Method", [expr(recv), name(m), args(a)]),
            EK::GenStruct {
                name: n,
                targs,
                fields,
            } => node(
                "StructLit",
                [
                    name(n),
                    optional(targs.as_deref(), |a| self.source_types(a)),
                    list(
                        fields
                            .iter()
                            .map(|(f, e)| node("FieldInit", [name(f), expr(e)])),
                    ),
                ],
            ),
            EK::GenVariant {
                name: n,
                targs,
                vname,
                args: a,
            } => node(
                "GenericMember",
                [
                    name(n),
                    self.source_types(targs.as_ref().expect("surface generic arguments")),
                    name(vname),
                    args(a),
                ],
            ),
            EK::Closure {
                params,
                ret,
                body,
                is_move,
                expr_body,
                parallel,
                worker,
            } => node(
                "Closure",
                [
                    is_move.to_string(),
                    expr_body.to_string(),
                    parallel.to_string(),
                    worker.to_string(),
                    list(params.iter().map(|p| {
                        node(
                            "Param",
                            [
                                name(&p.name),
                                mode_name(p.mode.unwrap_or(Mode::Own)).into(),
                                optional(p.ty, |ty| {
                                    if *parallel && ty == Ty::Int {
                                        node("Type", [name("Int"), list([])])
                                    } else {
                                        self.source_ty_text(ty)
                                    }
                                }),
                            ],
                        )
                    })),
                    optional(*ret, |ty| self.source_ty_text(ty)),
                    self.source_block(body),
                ],
            ),
            EK::CallFn(f, a) => node("CallFn", [expr(f), args(a)]),
            EK::ArrayLit(items) => node("Array", [args(items)]),
            EK::ArrayFill(item, len) => {
                node("ArrayFill", [expr(item), self.source_length_text(*len)])
            }
            EK::SliceOf(x, lo, hi) => node(
                "Slice",
                [
                    expr(x),
                    optional(lo.as_deref(), expr),
                    optional(hi.as_deref(), expr),
                ],
            ),
            _ => unreachable!("resolved expression in surface AST"),
        }
    }
}
