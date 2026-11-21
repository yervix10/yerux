// Binding types from the production checker, before compile-time evaluation.
use crate::syntax::{lex, Program, Stmt, Tok, Ty, Types, PRELUDE_LINE, SK};
use std::collections::{HashMap, HashSet};

type Bindings = HashMap<(usize, usize), String>;
/// (line, column of the `let`/`var`, name, type)
type Rows = Vec<(usize, usize, String, String)>;

pub fn binding_types(program: &Program, source: &str) -> Vec<String> {
    let mut rows: Vec<String> = typed_bindings(program, source)
        .into_iter()
        .map(|(_, _, name, ty)| format!("type:{name}:{ty}"))
        .collect();
    rows.sort_unstable();
    rows
}

/// The checked type of every source `let`/`var` of the entry file, keyed by the position of the keyword.
pub fn typed_bindings(program: &Program, source: &str) -> Rows {
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
    // A source snapshot includes each generic definition once. Its instantiated
    // copies share its source position, while the template preserves parameter types.
    let template_positions: HashSet<_> = program
        .templates
        .iter()
        .map(|f| (f.pos.line, f.pos.col))
        .collect();
    for function in program.templates.iter().chain(
        program
            .funcs
            .iter()
            .filter(|f| !template_positions.contains(&(f.pos.line, f.pos.col))),
    ) {
        if function.pos.line < PRELUDE_LINE {
            block(&function.body, &program.types, &declarations, &mut rows);
        }
    }
    rows
}

fn block(body: &[Stmt], types: &Types, declarations: &Bindings, rows: &mut Rows) {
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
                rows.push((stmt.pos.line, stmt.pos.col, name.clone(), types.name(ty)));
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
