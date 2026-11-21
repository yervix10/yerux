// Binding types from the production checker, before compile-time evaluation.
use crate::syntax::{lex, Program, Stmt, Tok, Ty, Types, PRELUDE_LINE, SK};
use std::collections::HashMap;

type Bindings = HashMap<(usize, usize), String>;

pub fn binding_types(program: &Program, source: &str) -> Vec<String> {
    // Record the source bindings independently of the checker's synthetic lets
    // (temporaries, closure environments and desugared loop variables).
    let tokens = lex(source).unwrap_or_else(|_| unreachable!("entry source was already lexed"));
    let declarations: Bindings = tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].t, &pair[1].t) {
            (Tok::Id(word), Tok::Id(name)) if word == "let" || word == "var" => {
                let pos = pair[0].pos;
                Some(((pos.line, pos.col), name.clone()))
            }
            _ => None,
        })
        .collect();
    let mut rows = Vec::new();
    for function in program.funcs.iter().chain(&program.templates) {
        if function.pos.line < PRELUDE_LINE {
            block(&function.body, &program.types, &declarations, &mut rows);
        }
    }
    rows.sort_unstable();
    rows
}

fn block(body: &[Stmt], types: &Types, declarations: &Bindings, rows: &mut Vec<String>) {
    for stmt in body {
        match &stmt.k {
            SK::Let(name, _, init, _)
                if declarations.get(&(stmt.pos.line, stmt.pos.col)) == Some(name) =>
            {
                // The basic source checker does not perform flow or interval analysis.
                let ty = if matches!(init.ty, Ty::Range(..)) {
                    Ty::Int
                } else {
                    init.ty
                };
                rows.push(format!("type:{name}:{}", types.name(ty)));
            }
            SK::If(_, yes, no) => {
                block(yes, types, declarations, rows);
                if let Some(no) = no {
                    block(no, types, declarations, rows);
                }
            }
            SK::Block(body, _) | SK::While(_, body, _) | SK::Unsafe(body) => {
                block(body, types, declarations, rows);
            }
            SK::Match(_, arms) => {
                for arm in arms {
                    block(&arm.body, types, declarations, rows);
                }
            }
            SK::Defer(stmt) => block(std::slice::from_ref(stmt), types, declarations, rows),
            _ => {}
        }
    }
}
