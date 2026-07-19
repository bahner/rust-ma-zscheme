//! Session-environment serialisation.
//!
//! Serialises the bindings of an [`Env`] back to Scheme source — one
//! `(define …)` per binding — so a session can be saved to a file or
//! document and reloaded by evaluating it.

use crate::parser::SchemeExpr;
use crate::value::{Env, SchemeVal};

/// Serialise the environment's own bindings to reloadable Scheme source.
///
/// Bindings that cannot be represented as source (builtins, actor handles,
/// …) are skipped.
#[must_use]
pub fn dump_env_source(env: &Env) -> String {
    let mut pairs: Vec<(String, SchemeVal)> = env.own_bindings();
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut lines = vec![
        "; zscheme session image — reload by evaluating this file".to_string(),
        String::new(),
    ];
    for (name, val) in &pairs {
        if let Some(src) = val_to_define(name, val) {
            lines.push(src);
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

fn val_to_define(name: &str, val: &SchemeVal) -> Option<String> {
    match val {
        SchemeVal::Str(s) => Some(format!("(define {name} {s:?})")),
        SchemeVal::Int(n) => Some(format!("(define {name} {n})")),
        SchemeVal::Float(f) => Some(format!("(define {name} {f})")),
        SchemeVal::Bool(true) => Some(format!("(define {name} #t)")),
        SchemeVal::Bool(false) => Some(format!("(define {name} #f)")),
        SchemeVal::Nil => Some(format!("(define {name} ())")),
        SchemeVal::List(items) => {
            let elems: Vec<String> = items.iter().map(val_to_src).collect();
            Some(format!("(define {name} '({}))", elems.join(" ")))
        }
        SchemeVal::Lambda {
            params, rest, body, ..
        } => {
            let param_str = match rest {
                Some(r) if params.is_empty() => format!(". {r}"),
                Some(r) => format!("{} . {r}", params.join(" ")),
                None => params.join(" "),
            };
            let body_fmt = body.iter().fold(String::new(), |mut acc, e| {
                acc.push_str("\n  ");
                acc.push_str(&expr_to_src(e));
                acc
            });
            Some(format!("(define ({name} {param_str}){body_fmt})"))
        }
        _ => None,
    }
}

fn val_to_src(val: &SchemeVal) -> String {
    match val {
        SchemeVal::Str(s) => format!("{s:?}"),
        SchemeVal::Int(n) => n.to_string(),
        SchemeVal::Float(f) => f.to_string(),
        SchemeVal::Bool(true) => "#t".to_string(),
        SchemeVal::Bool(false) => "#f".to_string(),
        SchemeVal::Nil => "()".to_string(),
        SchemeVal::List(items) => format!(
            "({})",
            items.iter().map(val_to_src).collect::<Vec<_>>().join(" ")
        ),
        other => other.display(),
    }
}

fn expr_to_src(expr: &SchemeExpr) -> String {
    match expr {
        SchemeExpr::Nil => "()".to_string(),
        SchemeExpr::Str(s) => format!("{s:?}"),
        SchemeExpr::Atom(s) => s.clone(),
        SchemeExpr::List(fs) => format!(
            "({})",
            fs.iter().map(expr_to_src).collect::<Vec<_>>().join(" ")
        ),
    }
}
