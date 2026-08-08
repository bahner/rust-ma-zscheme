//! Small utility functions shared by the evaluator and builtins.

use crate::parser::SchemeExpr;
use crate::value::SchemeVal;

use super::SchemeErr;

fn arity(name: &str, args: &[SchemeVal], n: usize) -> Result<(), SchemeErr> {
    if args.len() == n {
        Ok(())
    } else {
        Err(SchemeErr::Arity {
            name: name.to_string(),
            expected: n,
            got: args.len(),
        })
    }
}

fn arity_min(name: &str, args: &[SchemeVal], min: usize) -> Result<(), SchemeErr> {
    if args.len() < min {
        Err(SchemeErr::Arity {
            name: name.to_string(),
            expected: min,
            got: args.len(),
        })
    } else {
        Ok(())
    }
}

#[allow(clippy::float_cmp, clippy::cast_precision_loss)]
fn scheme_equal(a: &SchemeVal, b: &SchemeVal) -> bool {
    match (a, b) {
        (SchemeVal::Int(x), SchemeVal::Int(y)) => x == y,
        (SchemeVal::Float(x), SchemeVal::Float(y)) => x == y,
        (SchemeVal::Int(x), SchemeVal::Float(y)) => (*x as f64) == *y,
        (SchemeVal::Float(x), SchemeVal::Int(y)) => *x == (*y as f64),
        (SchemeVal::Str(x), SchemeVal::Str(y)) => x == y,
        (SchemeVal::Bool(x), SchemeVal::Bool(y)) => x == y,
        (SchemeVal::Nil, SchemeVal::Nil) => true,
        (SchemeVal::List(x), SchemeVal::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| scheme_equal(a, b))
        }
        _ => false,
    }
}

#[allow(clippy::cast_precision_loss)]
fn num_lt(a: &SchemeVal, b: &SchemeVal) -> Result<bool, SchemeErr> {
    match (a, b) {
        (SchemeVal::Int(x), SchemeVal::Int(y)) => Ok(x < y),
        (SchemeVal::Float(x), SchemeVal::Float(y)) => Ok(x < y),
        (SchemeVal::Int(x), SchemeVal::Float(y)) => Ok((*x as f64) < *y),
        (SchemeVal::Float(x), SchemeVal::Int(y)) => Ok(*x < (*y as f64)),
        _ => Err(SchemeErr::Runtime(format!(
            "comparison: not numbers: {} and {}",
            a.display(),
            b.display()
        ))),
    }
}

fn compare_chain(
    args: &[SchemeVal],
    cmp: impl Fn(&SchemeVal, &SchemeVal) -> Result<bool, SchemeErr>,
) -> Result<SchemeVal, SchemeErr> {
    if args.len() < 2 {
        return Err(SchemeErr::Arity {
            name: "comparison".to_string(),
            expected: 2,
            got: args.len(),
        });
    }
    for pair in args.windows(2) {
        if !cmp(&pair[0], &pair[1])? {
            return Ok(SchemeVal::Bool(false));
        }
    }
    Ok(SchemeVal::Bool(true))
}

fn one_float(name: &str, args: &[SchemeVal], f: fn(f64) -> f64) -> Result<SchemeVal, SchemeErr> {
    arity(name, args, 1)?;
    match &args[0] {
        SchemeVal::Int(n) => Ok(SchemeVal::Int(*n)),
        SchemeVal::Float(v) => Ok(SchemeVal::Float(f(*v))),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a number"))),
    }
}

fn num_pred(
    name: &str,
    args: &[SchemeVal],
    int_pred: fn(i64) -> bool,
    float_pred: fn(f64) -> bool,
) -> Result<SchemeVal, SchemeErr> {
    arity(name, args, 1)?;
    match &args[0] {
        SchemeVal::Int(n) => Ok(SchemeVal::Bool(int_pred(*n))),
        SchemeVal::Float(f) => Ok(SchemeVal::Bool(float_pred(*f))),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a number"))),
    }
}

fn list_arg(v: &SchemeVal, name: &str) -> Result<Vec<SchemeVal>, SchemeErr> {
    match v {
        SchemeVal::List(items) => Ok(items.clone()),
        SchemeVal::Nil => Ok(vec![]),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a list"))),
    }
}

fn int_arg(v: &SchemeVal, name: &str) -> Result<usize, SchemeErr> {
    match v {
        SchemeVal::Int(n) => usize::try_from(*n)
            .map_err(|_| SchemeErr::Runtime(format!("{name}: index must be non-negative"))),
        _ => Err(SchemeErr::Runtime(format!("{name}: index not an integer"))),
    }
}

fn atom_name(expr: &SchemeExpr, ctx: &str) -> Result<String, SchemeErr> {
    match expr {
        SchemeExpr::Atom(n) => Ok(n.clone()),
        _ => Err(SchemeErr::Runtime(format!(
            "{ctx}: expected a symbol, got {expr:?}"
        ))),
    }
}

fn let_binding(binding: &SchemeExpr, ctx: &str) -> Result<(String, SchemeExpr), SchemeErr> {
    match binding {
        SchemeExpr::List(pair) if pair.len() == 2 => {
            let name = atom_name(&pair[0], ctx)?;
            Ok((name, pair[1].clone()))
        }
        _ => Err(SchemeErr::Runtime(format!(
            "{ctx}: each binding must be (name value)"
        ))),
    }
}

fn extract_rest_param(mut params: Vec<String>) -> (Vec<String>, Option<String>) {
    if let Some(dot_pos) = params.iter().position(|p| p == ".") {
        if dot_pos + 1 < params.len() {
            let rest = params.remove(dot_pos + 1);
            params.remove(dot_pos);
            return (params, Some(rest));
        }
    }
    (params, None)
}

fn expr_to_val(expr: &SchemeExpr) -> SchemeVal {
    match expr {
        SchemeExpr::Nil => SchemeVal::Nil,
        SchemeExpr::Str(s) | SchemeExpr::Atom(s) => SchemeVal::Str(s.clone()),
        SchemeExpr::List(forms) => SchemeVal::List(forms.iter().map(expr_to_val).collect()),
    }
}

// ── Reply tuple constructors ───────────────────────────────────────────────

fn ok_tuple(value: impl Into<String>) -> SchemeVal {
    SchemeVal::List(vec![
        SchemeVal::Str(":ok".to_string()),
        SchemeVal::Str(value.into()),
    ])
}

fn err_tuple(reason: impl Into<String>) -> SchemeVal {
    SchemeVal::List(vec![
        SchemeVal::Str(":error".to_string()),
        SchemeVal::Str(reason.into()),
    ])
}

fn timeout_tuple() -> SchemeVal {
    SchemeVal::List(vec![SchemeVal::Str(":timeout".to_string())])
}

fn is_ok_tuple(v: &SchemeVal) -> bool {
    matches!(v, SchemeVal::List(items)
        if matches!(items.first(), Some(SchemeVal::Str(s)) if s == ":ok"))
}

fn is_err_tuple(v: &SchemeVal) -> bool {
    matches!(v, SchemeVal::List(items)
        if matches!(items.first(), Some(SchemeVal::Str(s)) if s == ":error"))
}

fn str_arg(v: &SchemeVal, name: &str) -> Result<String, SchemeErr> {
    match v {
        SchemeVal::Str(s) => Ok(s.clone()),
        other => Err(SchemeErr::Runtime(format!(
            "{name}: expected a string, got {}",
            other.display()
        ))),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

