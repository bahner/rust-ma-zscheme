//! Built-in procedure dispatch for the ma Scheme evaluator.
//!
//! `apply_builtin` routes to one of the category functions below.
//! Each category is a plain `fn` or `async fn` that handles a logical group
//! of procedures, keeping individual functions short and easy to navigate.

use futures::future::LocalBoxFuture;

use crate::host::Ctx;
use crate::value::{Env, SchemeVal};

use super::helpers::{
    arity, arity_min, compare_chain, err_tuple, int_arg, is_err_ack, is_ok_ack, is_ok_reply, list_arg,
    num_lt, num_pred, ok_tuple, one_float, str_arg, timeout_tuple,
};
use super::{apply, eval_source_in_env, is_link_value, SchemeErr};

// ── Builtin name set ──────────────────────────────────────────────────────

pub(super) fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        // apply
        "apply"
        // arithmetic
        | "+" | "-" | "*" | "/" | "mod" | "remainder" | "quotient"
        | "abs" | "max" | "min" | "floor" | "ceiling" | "round" | "truncate"
        | "even?" | "odd?" | "zero?" | "positive?" | "negative?"
        // comparison
        | "=" | "equal?" | "eqv?" | "eq?" | "<" | ">" | "<=" | ">=" | "not"
        // list
        | "list" | "cons" | "car" | "cdr" | "append" | "reverse" | "length"
        | "list-ref" | "null?" | "pair?" | "list?" | "cadr" | "caddr" | "cadddr"
        // string / type predicates
        | "string?" | "number?" | "boolean?" | "procedure?"
        | "string-append" | "string-length" | "substring"
        | "string-contains" | "string-index" | "string-upcase" | "string-downcase"
        | "number->string" | "string->number"
        // higher-order
        | "map" | "filter" | "for-each" | "fold" | "fold-left"
        // I/O and errors
        | "display" | "write" | "newline" | "error" | "assert"
        // ma actor primitives
        | "rpc-send" | "msg-send"
        // reply tuple helpers
        | "ok?" | "ok-reply?" | "err?" | "ok-val" | "err-msg"
        // misc
        | "use" | "include"
    )
}

// ── Routing dispatch ──────────────────────────────────────────────────────

pub(super) fn apply_builtin(
    name: String,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        match name.as_str() {
            "apply" => builtin_apply(args, ctx).await,

            "+" | "-" | "*" | "/" | "mod" | "remainder" | "quotient" | "abs" | "max" | "min"
            | "floor" | "ceiling" | "round" | "truncate" | "even?" | "odd?" | "zero?"
            | "positive?" | "negative?" => builtin_arithmetic(&name, args),

            "=" | "equal?" | "eqv?" | "eq?" | "<" | ">" | "<=" | ">=" | "not" => {
                builtin_comparison(&name, args)
            }

            "list" | "cons" | "car" | "cdr" | "append" | "reverse" | "length" | "list-ref"
            | "null?" | "pair?" | "list?" | "cadr" | "caddr" | "cadddr" => {
                builtin_list(&name, args)
            }

            "string?" | "number?" | "boolean?" | "procedure?" | "string-append"
            | "string-length" | "substring" | "string-contains" | "string-index"
            | "string-upcase" | "string-downcase" | "number->string" | "string->number" => {
                builtin_string(&name, &args)
            }

            "map" | "filter" | "for-each" | "fold" | "fold-left" => {
                builtin_higher_order(&name, args, ctx).await
            }

            "display" | "write" | "newline" | "error" | "assert" => builtin_io(&name, args, &ctx),

            "rpc-send" | "msg-send" | "ok?" | "ok-reply?" | "err?" | "ok-val" | "err-msg" | "use"
            | "include" => builtin_ma(&name, args, ctx).await,

            other => Err(SchemeErr::Undefined(other.to_string())),
        }
    })
}

// ── apply ─────────────────────────────────────────────────────────────────

async fn builtin_apply(args: Vec<SchemeVal>, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    arity_min("apply", &args, 2)?;
    let f = args[0].clone();
    // (apply f arg1 … list) — spread last arg as trailing positional args.
    let mut call_args: Vec<SchemeVal> = args[1..args.len() - 1].to_vec();
    match args.last().unwrap() {
        SchemeVal::List(v) => call_args.extend(v.iter().cloned()),
        SchemeVal::Nil => {}
        _ => {
            return Err(SchemeErr::Runtime(
                "apply: last argument must be a list".into(),
            ))
        }
    }
    apply(f, call_args, ctx).await
}

// ── Arithmetic ────────────────────────────────────────────────────────────

fn builtin_arithmetic(name: &str, args: Vec<SchemeVal>) -> Result<SchemeVal, SchemeErr> {
    match name {
        "+" => {
            let mut sum_i: i64 = 0;
            let mut sum_f: f64 = 0.0;
            let mut is_float = false;
            for a in &args {
                match a {
                    SchemeVal::Int(n) => sum_i += n,
                    SchemeVal::Float(f) => {
                        is_float = true;
                        sum_f += f;
                    }
                    _ => {
                        return Err(SchemeErr::Runtime(format!(
                            "+: not a number: {}",
                            a.display()
                        )))
                    }
                }
            }
            if is_float {
                Ok(SchemeVal::Float(sum_f + sum_i as f64))
            } else {
                Ok(SchemeVal::Int(sum_i))
            }
        }
        "-" => {
            arity_min("-", &args, 1)?;
            match &args[0] {
                SchemeVal::Int(n) if args.len() == 1 => Ok(SchemeVal::Int(-n)),
                SchemeVal::Float(f) if args.len() == 1 => Ok(SchemeVal::Float(-f)),
                SchemeVal::Int(first) => {
                    let mut r = *first;
                    for a in &args[1..] {
                        match a {
                            SchemeVal::Int(n) => r -= n,
                            _ => return Err(SchemeErr::Runtime("-: not an integer".into())),
                        }
                    }
                    Ok(SchemeVal::Int(r))
                }
                SchemeVal::Float(first) => {
                    let mut r = *first;
                    for a in &args[1..] {
                        match a {
                            SchemeVal::Float(f) => r -= f,
                            SchemeVal::Int(n) => r -= *n as f64,
                            _ => return Err(SchemeErr::Runtime("-: not a number".into())),
                        }
                    }
                    Ok(SchemeVal::Float(r))
                }
                _ => Err(SchemeErr::Runtime("-: not a number".into())),
            }
        }
        "*" => {
            let mut prod_i: i64 = 1;
            let mut prod_f: f64 = 1.0;
            let mut is_float = false;
            for a in &args {
                match a {
                    SchemeVal::Int(n) => prod_i *= n,
                    SchemeVal::Float(f) => {
                        is_float = true;
                        prod_f *= f;
                    }
                    _ => {
                        return Err(SchemeErr::Runtime(format!(
                            "*: not a number: {}",
                            a.display()
                        )))
                    }
                }
            }
            if is_float {
                Ok(SchemeVal::Float(prod_f * prod_i as f64))
            } else {
                Ok(SchemeVal::Int(prod_i))
            }
        }
        "/" => {
            arity("/", &args, 2)?;
            match (&args[0], &args[1]) {
                (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                    if *b == 0 {
                        return Err(SchemeErr::Runtime("division by zero".into()));
                    }
                    Ok(SchemeVal::Int(a / b))
                }
                (SchemeVal::Float(a), SchemeVal::Float(b)) => Ok(SchemeVal::Float(a / b)),
                (SchemeVal::Int(a), SchemeVal::Float(b)) => Ok(SchemeVal::Float(*a as f64 / b)),
                (SchemeVal::Float(a), SchemeVal::Int(b)) => Ok(SchemeVal::Float(a / *b as f64)),
                _ => Err(SchemeErr::Runtime("/: not numbers".into())),
            }
        }
        "mod" | "remainder" => {
            arity("mod", &args, 2)?;
            match (&args[0], &args[1]) {
                (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                    if *b == 0 {
                        return Err(SchemeErr::Runtime("modulo by zero".into()));
                    }
                    Ok(SchemeVal::Int(a % b))
                }
                _ => Err(SchemeErr::Runtime("mod: not integers".into())),
            }
        }
        "quotient" => {
            arity("quotient", &args, 2)?;
            match (&args[0], &args[1]) {
                (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                    if *b == 0 {
                        return Err(SchemeErr::Runtime("quotient: division by zero".into()));
                    }
                    Ok(SchemeVal::Int(a / b))
                }
                _ => Err(SchemeErr::Runtime("quotient: not integers".into())),
            }
        }
        "abs" => {
            arity("abs", &args, 1)?;
            match &args[0] {
                SchemeVal::Int(n) => Ok(SchemeVal::Int(n.abs())),
                SchemeVal::Float(f) => Ok(SchemeVal::Float(f.abs())),
                _ => Err(SchemeErr::Runtime("abs: not a number".into())),
            }
        }
        "max" => {
            arity_min("max", &args, 1)?;
            let mut m = args[0].clone();
            for a in &args[1..] {
                if num_lt(&m, a)? {
                    m = a.clone();
                }
            }
            Ok(m)
        }
        "min" => {
            arity_min("min", &args, 1)?;
            let mut m = args[0].clone();
            for a in &args[1..] {
                if num_lt(a, &m)? {
                    m = a.clone();
                }
            }
            Ok(m)
        }
        "floor" => one_float("floor", &args, f64::floor),
        "ceiling" => one_float("ceiling", &args, f64::ceil),
        "round" => one_float("round", &args, f64::round),
        "truncate" => one_float("truncate", &args, f64::trunc),
        "even?" => {
            arity("even?", &args, 1)?;
            match &args[0] {
                SchemeVal::Int(n) => Ok(SchemeVal::Bool(n % 2 == 0)),
                _ => Err(SchemeErr::Runtime("even?: not an integer".into())),
            }
        }
        "odd?" => {
            arity("odd?", &args, 1)?;
            match &args[0] {
                SchemeVal::Int(n) => Ok(SchemeVal::Bool(n % 2 != 0)),
                _ => Err(SchemeErr::Runtime("odd?: not an integer".into())),
            }
        }
        "zero?" => num_pred("zero?", &args, |i| i == 0, |f| f == 0.0),
        "positive?" => num_pred("positive?", &args, |i| i > 0, |f| f > 0.0),
        "negative?" => num_pred("negative?", &args, |i| i < 0, |f| f < 0.0),
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── Comparison ────────────────────────────────────────────────────────────

fn builtin_comparison(name: &str, args: Vec<SchemeVal>) -> Result<SchemeVal, SchemeErr> {
    match name {
        "=" | "equal?" | "eqv?" | "eq?" => {
            arity_min("=", &args, 2)?;
            let first = &args[0];
            for a in &args[1..] {
                if !super::helpers::scheme_equal(first, a) {
                    return Ok(SchemeVal::Bool(false));
                }
            }
            Ok(SchemeVal::Bool(true))
        }
        "<" => compare_chain(&args, num_lt),
        ">" => compare_chain(&args, |a, b| num_lt(b, a)),
        "<=" => compare_chain(&args, |a, b| num_lt(b, a).map(|r| !r)),
        ">=" => compare_chain(&args, |a, b| num_lt(a, b).map(|r| !r)),
        "not" => {
            arity("not", &args, 1)?;
            Ok(SchemeVal::Bool(!args[0].is_truthy()))
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── List operations ───────────────────────────────────────────────────────

fn builtin_list(name: &str, args: Vec<SchemeVal>) -> Result<SchemeVal, SchemeErr> {
    match name {
        "list" => Ok(SchemeVal::List(args)),
        "cons" => {
            arity("cons", &args, 2)?;
            match args[1].clone() {
                SchemeVal::List(mut v) => {
                    v.insert(0, args[0].clone());
                    Ok(SchemeVal::List(v))
                }
                SchemeVal::Nil => Ok(SchemeVal::List(vec![args[0].clone()])),
                b => Ok(SchemeVal::List(vec![args[0].clone(), b])),
            }
        }
        "car" => {
            arity("car", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) if !v.is_empty() => Ok(v[0].clone()),
                _ => Err(SchemeErr::Runtime("car: not a pair".into())),
            }
        }
        "cdr" => {
            arity("cdr", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) if v.len() > 1 => Ok(SchemeVal::List(v[1..].to_vec())),
                SchemeVal::List(_) => Ok(SchemeVal::Nil),
                _ => Err(SchemeErr::Runtime("cdr: not a pair".into())),
            }
        }
        "append" => {
            let mut result = Vec::new();
            for a in args {
                match a {
                    SchemeVal::List(v) => result.extend(v),
                    SchemeVal::Nil => {}
                    _ => return Err(SchemeErr::Runtime("append: not a list".into())),
                }
            }
            Ok(SchemeVal::List(result))
        }
        "reverse" => {
            arity("reverse", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) => Ok(SchemeVal::List(v.iter().rev().cloned().collect())),
                SchemeVal::Nil => Ok(SchemeVal::Nil),
                _ => Err(SchemeErr::Runtime("reverse: not a list".into())),
            }
        }
        "length" => {
            arity("length", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) => Ok(SchemeVal::Int(v.len() as i64)),
                SchemeVal::Nil => Ok(SchemeVal::Int(0)),
                _ => Err(SchemeErr::Runtime("length: not a list".into())),
            }
        }
        "list-ref" => {
            arity("list-ref", &args, 2)?;
            let SchemeVal::List(lst) = &args[0] else {
                return Err(SchemeErr::Runtime("list-ref: not a list".into()));
            };
            let SchemeVal::Int(n) = &args[1] else {
                return Err(SchemeErr::Runtime("list-ref: index not an integer".into()));
            };
            let idx = usize::try_from(*n).map_err(|_| {
                SchemeErr::Runtime("list-ref: index must be non-negative".into())
            })?;
            lst.get(idx)
                .cloned()
                .ok_or_else(|| SchemeErr::Runtime(format!("list-ref: index {idx} out of range")))
        }
        "null?" => {
            arity("null?", &args, 1)?;
            let is_null = matches!(&args[0], SchemeVal::Nil | SchemeVal::List(v) if v.is_empty());
            Ok(SchemeVal::Bool(is_null))
        }
        "pair?" => {
            arity("pair?", &args, 1)?;
            Ok(SchemeVal::Bool(
                matches!(&args[0], SchemeVal::List(v) if !v.is_empty()),
            ))
        }
        "list?" => {
            arity("list?", &args, 1)?;
            Ok(SchemeVal::Bool(matches!(
                &args[0],
                SchemeVal::List(_) | SchemeVal::Nil
            )))
        }
        "cadr" => {
            arity("cadr", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) if v.len() >= 2 => Ok(v[1].clone()),
                _ => Err(SchemeErr::Runtime("cadr: list too short".into())),
            }
        }
        "caddr" => {
            arity("caddr", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) if v.len() >= 3 => Ok(v[2].clone()),
                _ => Err(SchemeErr::Runtime("caddr: list too short".into())),
            }
        }
        "cadddr" => {
            arity("cadddr", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v) if v.len() >= 4 => Ok(v[3].clone()),
                _ => Err(SchemeErr::Runtime("cadddr: list too short".into())),
            }
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── String & type predicates ──────────────────────────────────────────────

fn builtin_string(name: &str, args: &[SchemeVal]) -> Result<SchemeVal, SchemeErr> {
    match name {
        "string?" => {
            arity("string?", args, 1)?;
            Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Str(_))))
        }
        "number?" => {
            arity("number?", args, 1)?;
            Ok(SchemeVal::Bool(matches!(
                &args[0],
                SchemeVal::Int(_) | SchemeVal::Float(_)
            )))
        }
        "boolean?" => {
            arity("boolean?", args, 1)?;
            Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Bool(_))))
        }
        "procedure?" => {
            arity("procedure?", args, 1)?;
            Ok(SchemeVal::Bool(matches!(
                &args[0],
                SchemeVal::Lambda { .. } | SchemeVal::Builtin(_)
            )))
        }
        "string-append" => {
            let mut s = String::new();
            for a in args {
                match a {
                    SchemeVal::Str(st) => s.push_str(st),
                    _ => {
                        return Err(SchemeErr::Runtime(
                            "string-append: not a string".into(),
                        ))
                    }
                }
            }
            Ok(SchemeVal::Str(s))
        }
        "string-length" => {
            arity("string-length", args, 1)?;
            match &args[0] {
                SchemeVal::Str(s) => Ok(SchemeVal::Int(s.len() as i64)),
                _ => Err(SchemeErr::Runtime("string-length: not a string".into())),
            }
        }
        "substring" => {
            arity("substring", args, 3)?;
            let s = match &args[0] {
                SchemeVal::Str(s) => s.clone(),
                _ => return Err(SchemeErr::Runtime("substring: not a string".into())),
            };
            let start = int_arg(&args[1], "substring")?;
            let end = int_arg(&args[2], "substring")?;
            Ok(SchemeVal::Str(s.get(start..end).unwrap_or("").to_string()))
        }
        "string-contains" => {
            arity("string-contains", args, 2)?;
            match (&args[0], &args[1]) {
                (SchemeVal::Str(hay), SchemeVal::Str(needle)) => {
                    Ok(SchemeVal::Bool(hay.contains(needle.as_str())))
                }
                _ => Err(SchemeErr::Runtime("string-contains: not strings".into())),
            }
        }
        "string-index" => {
            arity("string-index", args, 2)?;
            match (&args[0], &args[1]) {
                (SchemeVal::Str(hay), SchemeVal::Str(needle)) => {
                    Ok(match hay.find(needle.as_str()) {
                        Some(i) => SchemeVal::Int(i as i64),
                        None => SchemeVal::Bool(false),
                    })
                }
                _ => Err(SchemeErr::Runtime("string-index: not strings".into())),
            }
        }
        "string-upcase" => {
            arity("string-upcase", args, 1)?;
            match &args[0] {
                SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_uppercase())),
                _ => Err(SchemeErr::Runtime("string-upcase: not a string".into())),
            }
        }
        "string-downcase" => {
            arity("string-downcase", args, 1)?;
            match &args[0] {
                SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_lowercase())),
                _ => Err(SchemeErr::Runtime("string-downcase: not a string".into())),
            }
        }
        "number->string" => {
            arity("number->string", args, 1)?;
            match &args[0] {
                SchemeVal::Int(n) => Ok(SchemeVal::Str(n.to_string())),
                SchemeVal::Float(f) => Ok(SchemeVal::Str(f.to_string())),
                _ => Err(SchemeErr::Runtime("number->string: not a number".into())),
            }
        }
        "string->number" => {
            arity("string->number", args, 1)?;
            match &args[0] {
                SchemeVal::Str(s) => Ok(if let Ok(n) = s.parse::<i64>() {
                    SchemeVal::Int(n)
                } else if let Ok(f) = s.parse::<f64>() {
                    SchemeVal::Float(f)
                } else {
                    SchemeVal::Bool(false)
                }),
                _ => Err(SchemeErr::Runtime("string->number: not a string".into())),
            }
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── Higher-order list procedures ──────────────────────────────────────────

async fn builtin_higher_order(
    name: &str,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> Result<SchemeVal, SchemeErr> {
    match name {
        "map" => {
            arity_min("map", &args, 2)?;
            let f = args[0].clone();
            let lst = list_arg(&args[1], "map")?;
            let mut result = Vec::with_capacity(lst.len());
            for item in lst {
                result.push(apply(f.clone(), vec![item], ctx.clone()).await?);
            }
            Ok(SchemeVal::List(result))
        }
        "filter" => {
            arity("filter", &args, 2)?;
            let f = args[0].clone();
            let lst = list_arg(&args[1], "filter")?;
            let mut result = Vec::new();
            for item in lst {
                let keep = apply(f.clone(), vec![item.clone()], ctx.clone()).await?;
                if keep.is_truthy() {
                    result.push(item);
                }
            }
            Ok(SchemeVal::List(result))
        }
        "for-each" => {
            arity_min("for-each", &args, 2)?;
            let f = args[0].clone();
            let lst = list_arg(&args[1], "for-each")?;
            for item in lst {
                apply(f.clone(), vec![item], ctx.clone()).await?;
            }
            Ok(SchemeVal::Nil)
        }
        "fold" | "fold-left" => {
            arity("fold", &args, 3)?;
            let f = args[0].clone();
            let mut acc = args[1].clone();
            let lst = list_arg(&args[2], "fold")?;
            for item in lst {
                acc = apply(f.clone(), vec![item, acc], ctx.clone()).await?;
            }
            Ok(acc)
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── I/O and error forms ───────────────────────────────────────────────────

fn builtin_io(name: &str, args: Vec<SchemeVal>, ctx: &Ctx) -> Result<SchemeVal, SchemeErr> {
    match name {
        "display" | "write" => {
            let text = if name == "write" {
                args.iter().map(SchemeVal::repr).collect::<Vec<_>>().join(" ")
            } else {
                args.iter()
                    .map(SchemeVal::display)
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            ctx.display_output(&text);
            Ok(SchemeVal::Nil)
        }
        "newline" => {
            ctx.display_output("\n");
            Ok(SchemeVal::Nil)
        }
        "error" => {
            let msg = args
                .iter()
                .map(SchemeVal::display)
                .collect::<Vec<_>>()
                .join(" ");
            Err(SchemeErr::Runtime(msg))
        }
        "assert" => {
            arity("assert", &args, 1)?;
            if args[0].is_truthy() {
                Ok(args[0].clone())
            } else {
                Err(SchemeErr::Runtime("assertion failed".into()))
            }
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}

// ── ma actor primitives ───────────────────────────────────────────────────

async fn builtin_ma(
    name: &str,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> Result<SchemeVal, SchemeErr> {
    match name {
        "rpc-send" => {
            arity_min("rpc-send", &args, 2)?;
            let raw = str_arg(&args[0], "rpc-send")?;
            let verb = str_arg(&args[1], "rpc-send")?;
            let target = ctx.resolve_target(&raw).map_err(SchemeErr::MaError)?;
            let extra: Vec<String> =
                args[2..].iter().map(SchemeVal::to_splice_lossy).collect();
            let send_result = ctx.send_rpc(&target, &verb, &extra).await;
            match send_result {
                Err(e) => Ok(err_tuple(e)),
                Ok(msg_id) => {
                    let (sender, receiver) =
                        futures::channel::oneshot::channel::<Result<String, String>>();
                    ctx.register_reply_sender(msg_id, sender);
                    match receiver.await {
                        Ok(Ok(content)) => Ok(ok_tuple(content)),
                        Ok(Err(e)) => Ok(err_tuple(e)),
                        Err(_) => Ok(timeout_tuple()),
                    }
                }
            }
        }
        "msg-send" => {
            arity("msg-send", &args, 2)?;
            let raw = str_arg(&args[0], "msg-send")?;
            let body = str_arg(&args[1], "msg-send")?;
            let target = ctx.resolve_target(&raw).map_err(SchemeErr::MaError)?;
            match ctx.send_text(&target, &body).await {
                Ok(msg_id) => Ok(ok_tuple(msg_id)),
                Err(e) => Ok(err_tuple(e)),
            }
        }
        "ok?" => {
            arity("ok?", &args, 1)?;
            Ok(SchemeVal::Bool(is_ok_ack(&args[0])))
        }
        "ok-reply?" => {
            arity("ok-reply?", &args, 1)?;
            Ok(SchemeVal::Bool(is_ok_reply(&args[0])))
        }
        "err?" => {
            arity("err?", &args, 1)?;
            Ok(SchemeVal::Bool(is_err_ack(&args[0])))
        }
        "ok-val" => {
            arity("ok-val", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v)
                    if v.len() >= 2
                        && matches!(&v[0], SchemeVal::Str(s) if s == ":ok") =>
                {
                    Ok(v[1].clone())
                }
                _ => Err(SchemeErr::Runtime(
                    "ok-val: not an (:ok value) tuple".into(),
                )),
            }
        }
        "err-msg" => {
            arity("err-msg", &args, 1)?;
            match &args[0] {
                SchemeVal::List(v)
                    if v.len() >= 2
                        && matches!(&v[0], SchemeVal::Str(s) if s == ":error") =>
                {
                    Ok(v[1].clone())
                }
                _ => Err(SchemeErr::Runtime(
                    "err-msg: not an (:error reason) tuple".into(),
                )),
            }
        }
        "use" => Ok(SchemeVal::Nil), // no-op: host handles focus state
        "include" => {
            arity("include", &args, 1)?;
            let path = match &args[0] {
                SchemeVal::Str(s) => s.clone(),
                other => {
                    return Err(SchemeErr::Runtime(format!(
                        "include: expected a path string or CID, got {}",
                        other.display()
                    )))
                }
            };
            let content = if is_link_value(&path) {
                ctx.fetch_path(&path).await.map_err(SchemeErr::MaError)?
            } else if path.starts_with('/') {
                match ctx.eval_dot(&path)? {
                    SchemeVal::Str(s) => s,
                    _ => {
                        return Err(SchemeErr::MaError(format!(
                            "include: {path} is not a string value"
                        )))
                    }
                }
            } else {
                path.clone()
            };
            let env = crate::get_env();
            eval_source_in_env(&content, env, ctx).await
        }
        _ => Err(SchemeErr::Undefined(name.to_string())),
    }
}
