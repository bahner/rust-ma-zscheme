/// Async Scheme evaluator for the ma actor network.
///
/// Platform-agnostic: runs on both native (`tokio` `LocalSet`) and WASM (browser
/// event loop via `gloo_timers`) using `LocalBoxFuture`.
/// Host-specific behaviour is abstracted through `crate::host::SchemeCtx`.
use std::collections::BTreeMap;

use futures::future::LocalBoxFuture;

use crate::host::Ctx;
use crate::parser::{parse_expr, tokenize, SchemeExpr};
use crate::value::{Env, SchemeVal};

// ── Link-value check ───────────────────────────────────────────────────────

/// True if `s` looks like an IPFS CID or a `did:ma:` DID.
/// True if `s` is a `did:ma:` DID or a `/ipfs/…`, `/ipns/…`, `/ipld/…` path.
/// Used to decide whether a path or `` `include` `` argument should be
/// fetched remotely rather than read from local `/my` / `/ctx` config.
#[must_use]
pub fn is_link_value(s: &str) -> bool {
    s.starts_with("did:ma:")
        || s.starts_with("/ipfs/")
        || s.starts_with("/ipns/")
        || s.starts_with("/ipld/")
}

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SchemeErr {
    Arity {
        name: String,
        expected: usize,
        got: usize,
    },
    Runtime(String),
    Undefined(String),
    #[allow(dead_code)]
    ParseError(String),
    MaError(String),
}

impl std::fmt::Display for SchemeErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemeErr::Arity {
                name,
                expected,
                got,
            } => {
                write!(f, "{name}: expected {expected} args, got {got}")
            }
            SchemeErr::Runtime(s) => write!(f, "{s}"),
            SchemeErr::Undefined(s) => write!(f, "undefined: {s}"),
            SchemeErr::ParseError(s) => write!(f, "parse error: {s}"),
            SchemeErr::MaError(s) => write!(f, "ma: {s}"),
        }
    }
}

// ── Entry points ───────────────────────────────────────────────────────────

/// Evaluate a Scheme source string.
pub fn eval_str(
    source: &str,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    let source = source.to_string();
    Box::pin(async move {
        let tokens = tokenize(&source).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        let (expr, _) = parse_expr(&tokens, 0).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        eval(expr, env, ctx).await
    })
}

/// Evaluate a parsed `SchemeExpr`.
pub fn eval(
    expr: SchemeExpr,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move { eval_inner(expr, env, ctx).await })
}

// ── Multi-expression evaluator ───────────────────────────────────────────────

/// Evaluate all top-level Scheme expressions in `source` within `env`.
/// Used by `include` and by the public `eval_source` in `crate::lib`.
pub(crate) async fn eval_source_in_env(
    source: &str,
    env: Env,
    ctx: Ctx,
) -> Result<SchemeVal, SchemeErr> {
    let tokens = tokenize(source).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
    let mut pos = 0;
    let mut last = SchemeVal::Nil;
    while pos < tokens.len() {
        let (expr, next) =
            parse_expr(&tokens, pos).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        last = eval(expr, env.clone(), ctx.clone()).await?;
        pos = next;
    }
    Ok(last)
}

// ── Core evaluator ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
async fn eval_inner(mut expr: SchemeExpr, mut env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    'tco: loop {
        match expr {
            SchemeExpr::Nil => return Ok(SchemeVal::Nil),
            SchemeExpr::Str(s) => return Ok(SchemeVal::Str(s)),
            SchemeExpr::Atom(s) => {
                // ma path atom in value position: #/my/…, #/ctx/…,
                // #/ipfs/…, #/ipns/…, #/ipld/… — `#/` avoids colliding with
                // the `/` division builtin.
                if let Some(rest) = s.strip_prefix("#/") {
                    let path = format!("/{rest}");
                    return if is_link_value(&path) {
                        ctx.fetch_path(&path)
                            .await
                            .map(SchemeVal::Str)
                            .map_err(SchemeErr::MaError)
                    } else {
                        ctx.eval_dot(&path)
                    };
                }
                return eval_atom(&s, &env);
            }
            SchemeExpr::List(forms) => {
                if forms.is_empty() {
                    return Ok(SchemeVal::Nil);
                }

                // ── Special forms ─────────────────────────────────────────────
                if let SchemeExpr::Atom(head) = &forms[0] {
                    match head.as_str() {
                        "define" => return eval_define(forms, env, ctx).await,
                        "lambda" | "ʎ" => return eval_lambda(&forms, env),

                        "let" => {
                            if forms.len() < 3 {
                                return Err(SchemeErr::Runtime(
                                    "let: expected bindings and body".to_string(),
                                ));
                            }
                            // Named let: (let name ((var init) …) body…)
                            if let SchemeExpr::Atom(loop_name) = &forms[1] {
                                if loop_name != "nil" && loop_name != "()" {
                                    if forms.len() < 4 {
                                        return Err(SchemeErr::Runtime(
                                            "named let: expected name, bindings, and body"
                                                .to_string(),
                                        ));
                                    }
                                    let bindings = match &forms[2] {
                                        SchemeExpr::List(b) => b.clone(),
                                        _ => {
                                            return Err(SchemeErr::Runtime(
                                                "named let: bindings must be a list".to_string(),
                                            ))
                                        }
                                    };
                                    let params: Vec<String> = bindings
                                        .iter()
                                        .map(|b| match b {
                                            SchemeExpr::List(parts) if !parts.is_empty() => {
                                                atom_name(&parts[0], "named let")
                                            }
                                            _ => Err(SchemeErr::Runtime(
                                                "named let: malformed binding".to_string(),
                                            )),
                                        })
                                        .collect::<Result<_, _>>()?;
                                    let inits: Vec<SchemeExpr> = bindings
                                        .iter()
                                        .map(|b| match b {
                                            SchemeExpr::List(parts) if parts.len() >= 2 => {
                                                Ok(parts[1].clone())
                                            }
                                            _ => Err(SchemeErr::Runtime(
                                                "named let: malformed binding".to_string(),
                                            )),
                                        })
                                        .collect::<Result<_, _>>()?;
                                    let body = forms[3..].to_vec();
                                    let (params, rest) = extract_rest_param(params);

                                    // loop_env contains the loop name itself (letrec semantics)
                                    let loop_env = Env::extend(&env);
                                    let lambda = SchemeVal::Lambda {
                                        params: params.clone(),
                                        rest: rest.clone(),
                                        body: body.clone(),
                                        env: loop_env.clone(),
                                    };
                                    loop_env.define(loop_name.clone(), lambda);

                                    // Evaluate inits in the outer env
                                    let mut init_vals = Vec::with_capacity(inits.len());
                                    for init in &inits {
                                        init_vals.push(
                                            eval(init.clone(), env.clone(), ctx.clone()).await?,
                                        );
                                    }

                                    // Bind params and TCO into body
                                    let call_env = Env::extend(&loop_env);
                                    let min = params.len();
                                    if rest.is_none() && init_vals.len() != min {
                                        return Err(SchemeErr::Arity {
                                            name: loop_name.clone(),
                                            expected: min,
                                            got: init_vals.len(),
                                        });
                                    }
                                    if init_vals.len() < min {
                                        return Err(SchemeErr::Arity {
                                            name: loop_name.clone(),
                                            expected: min,
                                            got: init_vals.len(),
                                        });
                                    }
                                    for (p, a) in params.iter().zip(init_vals.iter()) {
                                        call_env.define(p.clone(), a.clone());
                                    }
                                    if let Some(rest_name) = rest {
                                        call_env.define(
                                            rest_name,
                                            SchemeVal::List(init_vals[min..].to_vec()),
                                        );
                                    }
                                    if body.is_empty() {
                                        return Ok(SchemeVal::Nil);
                                    }
                                    for f in &body[..body.len() - 1] {
                                        eval(f.clone(), call_env.clone(), ctx.clone()).await?;
                                    }
                                    expr = body.last().unwrap().clone();
                                    env = call_env;
                                    continue 'tco;
                                }
                            }
                            // Regular let: (let ((var init) …) body…)
                            let bindings = match &forms[1] {
                                SchemeExpr::List(b) => b.clone(),
                                _ => {
                                    return Err(SchemeErr::Runtime(
                                        "let: bindings must be a list".to_string(),
                                    ))
                                }
                            };
                            let new_env = Env::extend(&env);
                            for binding in &bindings {
                                let (name, val_expr) = let_binding(binding, "let")?;
                                let val = eval(val_expr, env.clone(), ctx.clone()).await?;
                                new_env.define(name, val);
                            }
                            let body = &forms[2..];
                            if body.is_empty() {
                                return Ok(SchemeVal::Nil);
                            }
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), new_env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            env = new_env;
                            continue 'tco;
                        }

                        "let*" => {
                            if forms.len() < 3 {
                                return Err(SchemeErr::Runtime(
                                    "let*: expected bindings and body".to_string(),
                                ));
                            }
                            let bindings = match &forms[1] {
                                SchemeExpr::List(b) => b.clone(),
                                _ => {
                                    return Err(SchemeErr::Runtime(
                                        "let*: bindings must be a list".to_string(),
                                    ))
                                }
                            };
                            let new_env = Env::extend(&env);
                            for binding in &bindings {
                                let (name, val_expr) = let_binding(binding, "let*")?;
                                let val = eval(val_expr, new_env.clone(), ctx.clone()).await?;
                                new_env.define(name, val);
                            }
                            let body = &forms[2..];
                            if body.is_empty() {
                                return Ok(SchemeVal::Nil);
                            }
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), new_env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            env = new_env;
                            continue 'tco;
                        }

                        "letrec" => {
                            if forms.len() < 3 {
                                return Err(SchemeErr::Runtime(
                                    "letrec: expected bindings and body".to_string(),
                                ));
                            }
                            let bindings = match &forms[1] {
                                SchemeExpr::List(b) => b.clone(),
                                _ => {
                                    return Err(SchemeErr::Runtime(
                                        "letrec: bindings must be a list".to_string(),
                                    ))
                                }
                            };
                            let new_env = Env::extend(&env);
                            for binding in &bindings {
                                let (name, _) = let_binding(binding, "letrec")?;
                                new_env.define(name, SchemeVal::Nil);
                            }
                            for binding in &bindings {
                                let (name, val_expr) = let_binding(binding, "letrec")?;
                                let val = eval(val_expr, new_env.clone(), ctx.clone()).await?;
                                new_env.define(name, val);
                            }
                            let body = &forms[2..];
                            if body.is_empty() {
                                return Ok(SchemeVal::Nil);
                            }
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), new_env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            env = new_env;
                            continue 'tco;
                        }

                        "if" => {
                            if forms.len() < 3 || forms.len() > 4 {
                                return Err(SchemeErr::Runtime(
                                    "if: expected (if cond then) or (if cond then else)"
                                        .to_string(),
                                ));
                            }
                            let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
                            expr = if cond.is_truthy() {
                                forms[2].clone()
                            } else if forms.len() == 4 {
                                forms[3].clone()
                            } else {
                                return Ok(SchemeVal::Nil);
                            };
                            continue 'tco;
                        }

                        "cond" => {
                            let mut matched: Option<Vec<SchemeExpr>> = None;
                            for clause in &forms[1..] {
                                if let SchemeExpr::List(parts) = clause {
                                    if parts.is_empty() {
                                        continue;
                                    }
                                    if let SchemeExpr::Atom(kw) = &parts[0] {
                                        if kw == "else" {
                                            matched = Some(parts[1..].to_vec());
                                            break;
                                        }
                                    }
                                    let test =
                                        eval(parts[0].clone(), env.clone(), ctx.clone()).await?;
                                    if test.is_truthy() {
                                        if parts.len() == 1 {
                                            return Ok(test);
                                        }
                                        matched = Some(parts[1..].to_vec());
                                        break;
                                    }
                                }
                            }
                            match matched {
                                None => return Ok(SchemeVal::Nil),
                                Some(b) if b.is_empty() => return Ok(SchemeVal::Nil),
                                Some(b) => {
                                    for f in &b[..b.len() - 1] {
                                        eval(f.clone(), env.clone(), ctx.clone()).await?;
                                    }
                                    expr = b.last().unwrap().clone();
                                    continue 'tco;
                                }
                            }
                        }

                        "begin" => {
                            let body = &forms[1..];
                            if body.is_empty() {
                                return Ok(SchemeVal::Nil);
                            }
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            continue 'tco;
                        }

                        "quote" => {
                            if forms.len() != 2 {
                                return Err(SchemeErr::Runtime(
                                    "quote: expected exactly one argument".to_string(),
                                ));
                            }
                            return Ok(expr_to_val(&forms[1]));
                        }

                        "and" => {
                            let args = &forms[1..];
                            if args.is_empty() {
                                return Ok(SchemeVal::Bool(true));
                            }
                            for f in &args[..args.len() - 1] {
                                let v = eval(f.clone(), env.clone(), ctx.clone()).await?;
                                if !v.is_truthy() {
                                    return Ok(SchemeVal::Bool(false));
                                }
                            }
                            expr = args.last().unwrap().clone();
                            continue 'tco;
                        }

                        "or" => {
                            let args = &forms[1..];
                            if args.is_empty() {
                                return Ok(SchemeVal::Bool(false));
                            }
                            for f in &args[..args.len() - 1] {
                                let v = eval(f.clone(), env.clone(), ctx.clone()).await?;
                                if v.is_truthy() {
                                    return Ok(v);
                                }
                            }
                            expr = args.last().unwrap().clone();
                            continue 'tco;
                        }

                        "when" => {
                            if forms.len() < 3 {
                                return Err(SchemeErr::Runtime(
                                    "when: expected condition + body".to_string(),
                                ));
                            }
                            let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
                            if !cond.is_truthy() {
                                return Ok(SchemeVal::Nil);
                            }
                            let body = &forms[2..];
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            continue 'tco;
                        }

                        "unless" => {
                            if forms.len() < 3 {
                                return Err(SchemeErr::Runtime(
                                    "unless: expected condition + body".to_string(),
                                ));
                            }
                            let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
                            if cond.is_truthy() {
                                return Ok(SchemeVal::Nil);
                            }
                            let body = &forms[2..];
                            for f in &body[..body.len() - 1] {
                                eval(f.clone(), env.clone(), ctx.clone()).await?;
                            }
                            expr = body.last().unwrap().clone();
                            continue 'tco;
                        }

                        "set!" => {
                            if forms.len() != 3 {
                                return Err(SchemeErr::Runtime(
                                    "set!: expected symbol and value".to_string(),
                                ));
                            }
                            let name = atom_name(&forms[1], "set!")?;
                            let val = eval(forms[2].clone(), env.clone(), ctx).await?;
                            env.set_existing(&name, val)
                                .ok_or(SchemeErr::Undefined(name))?;
                            return Ok(SchemeVal::Nil);
                        }

                        "guard" => return eval_guard(forms, env, ctx).await,

                        _ => {}
                    }
                }

                // ── Pipe threading: (val | (f arg) | g) ───────────────────────
                if forms
                    .iter()
                    .skip(1)
                    .any(|f| matches!(f, SchemeExpr::Atom(s) if s == "|"))
                {
                    return eval_pipe(forms, env, ctx).await;
                }

                // ── ma path in head position (#/my, #/ctx, #/ipfs, #/ipns, #/ipld) ──
                if let SchemeExpr::Atom(head) = &forms[0] {
                    if let Some(rest) = head.strip_prefix("#/") {
                        let path = format!("/{rest}");
                        if forms.len() == 1 {
                            if is_link_value(&path) {
                                return ctx
                                    .fetch_path(&path)
                                    .await
                                    .map(SchemeVal::Str)
                                    .map_err(SchemeErr::MaError);
                            }
                            let val = ctx.eval_dot(&path)?;
                            if let SchemeVal::Str(ref s) = val {
                                if s.trim_start().starts_with('(') {
                                    let tokens = tokenize(s)
                                        .map_err(|e| SchemeErr::ParseError(e.to_string()))?;
                                    let (e, _) = parse_expr(&tokens, 0)
                                        .map_err(|e| SchemeErr::ParseError(e.to_string()))?;
                                    expr = e;
                                    continue 'tco;
                                }
                            }
                            return Ok(val);
                        }
                        if is_link_value(&path) {
                            return Err(SchemeErr::Runtime(format!(
                                "{path} is read-only and does not accept arguments"
                            )));
                        }
                        let mapath = SchemeVal::MaPath(path);
                        let mut args = Vec::with_capacity(forms.len() - 1);
                        for form in &forms[1..] {
                            args.push(eval(form.clone(), env.clone(), ctx.clone()).await?);
                        }
                        return apply(mapath, args, ctx).await;
                    }
                }

                // ── Application ────────────────────────────────────────────────
                let head_val = eval(forms[0].clone(), env.clone(), ctx.clone()).await?;

                let mut args = Vec::with_capacity(forms.len() - 1);
                for form in &forms[1..] {
                    args.push(eval(form.clone(), env.clone(), ctx.clone()).await?);
                }

                // TCO for direct lambda application
                match head_val {
                    SchemeVal::Lambda {
                        params,
                        rest,
                        body,
                        env: lambda_env,
                    } => {
                        let new_env = Env::extend(&lambda_env);
                        let min = params.len();
                        if rest.is_none() && args.len() != min {
                            return Err(SchemeErr::Arity {
                                name: "#<lambda>".to_string(),
                                expected: min,
                                got: args.len(),
                            });
                        }
                        if args.len() < min {
                            return Err(SchemeErr::Arity {
                                name: "#<lambda>".to_string(),
                                expected: min,
                                got: args.len(),
                            });
                        }
                        for (p, a) in params.iter().zip(args.iter()) {
                            new_env.define(p.clone(), a.clone());
                        }
                        if let Some(rest_name) = rest {
                            new_env.define(rest_name, SchemeVal::List(args[min..].to_vec()));
                        }
                        if body.is_empty() {
                            return Ok(SchemeVal::Nil);
                        }
                        for f in &body[..body.len() - 1] {
                            eval(f.clone(), new_env.clone(), ctx.clone()).await?;
                        }
                        expr = body.last().unwrap().clone();
                        env = new_env;
                    }
                    other => return apply(other, args, ctx).await,
                }
            }
        }
    }
}
// ── Atom evaluation ────────────────────────────────────────────────────────

fn eval_atom(s: &str, env: &Env) -> Result<SchemeVal, SchemeErr> {
    if let Some(v) = env.get(s) {
        return Ok(v);
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(SchemeVal::Int(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(SchemeVal::Float(f));
    }
    if s == "#t" || s == "true" {
        return Ok(SchemeVal::Bool(true));
    }
    if s == "#f" || s == "false" {
        return Ok(SchemeVal::Bool(false));
    }
    if s == "nil" || s == "()" {
        return Ok(SchemeVal::Nil);
    }
    // ma fragment atoms like `#room`, `#house:enter` — treat as strings.
    // (`#/…` path atoms are intercepted earlier in `eval_inner` and never
    // reach this fallback.)
    if s.starts_with('#') {
        return Ok(SchemeVal::Str(s.to_string()));
    }
    if s.starts_with('@') {
        return Ok(SchemeVal::MaActor(s.to_string()));
    }
    if is_builtin(s) {
        return Ok(SchemeVal::Builtin(s.to_string()));
    }
    Err(SchemeErr::Undefined(s.to_string()))
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "apply"
            | "+"
            | "-"
            | "*"
            | "/"
            | "mod"
            | "remainder"
            | "quotient"
            | "="
            | "<"
            | ">"
            | "<="
            | ">="
            | "equal?"
            | "eqv?"
            | "eq?"
            | "not"
            | "list"
            | "cons"
            | "car"
            | "cdr"
            | "append"
            | "reverse"
            | "length"
            | "list-ref"
            | "null?"
            | "pair?"
            | "map?"
            | "make-map"
            | "map-ref"
            | "map-set"
            | "map-delete"
            | "map-has-key?"
            | "map-keys"
            | "map-values"
            | "map->alist"
            | "alist->map"
            | "string?"
            | "number?"
            | "boolean?"
            | "procedure?"
            | "string-append"
            | "string-length"
            | "substring"
            | "string-contains"
            | "string-index"
            | "string-upcase"
            | "string-downcase"
            | "number->string"
            | "string->number"
            | "abs"
            | "max"
            | "min"
            | "floor"
            | "ceiling"
            | "round"
            | "truncate"
            | "even?"
            | "odd?"
            | "zero?"
            | "positive?"
            | "negative?"
            | "map"
            | "filter"
            | "for-each"
            | "fold"
            | "fold-left"
            | "display"
            | "write"
            | "newline"
            | "error"
            | "assert"
            | "list?"
            | "rpc-send"
            | "msg-send"
            | "ok?"
            | "err?"
            | "ok-val"
            | "err-msg"
            | "use"
            | "include"
            | "cadr"
            | "caddr"
            | "cadddr"
    )
}

// ── Special forms ──────────────────────────────────────────────────────────

async fn eval_define(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "define: expected name and value".to_string(),
        ));
    }
    match &forms[1] {
        SchemeExpr::Atom(name) => {
            let val = eval(forms[2].clone(), env.clone(), ctx).await?;
            env.define(name.clone(), val);
            Ok(SchemeVal::Nil)
        }
        SchemeExpr::List(params) => {
            if params.is_empty() {
                return Err(SchemeErr::Runtime(
                    "define: function form missing name".to_string(),
                ));
            }
            let name = atom_name(&params[0], "define")?;
            let param_names = params[1..]
                .iter()
                .map(|p| atom_name(p, "define"))
                .collect::<Result<Vec<_>, _>>()?;
            let (param_names, rest) = extract_rest_param(param_names);
            let body = forms[2..].to_vec();
            env.define(
                name,
                SchemeVal::Lambda {
                    params: param_names,
                    rest,
                    body,
                    env: env.clone(),
                },
            );
            Ok(SchemeVal::Nil)
        }
        _ => Err(SchemeErr::Runtime(
            "define: first arg must be a symbol or list".to_string(),
        )),
    }
}

fn eval_lambda(forms: &[SchemeExpr], env: Env) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "lambda: expected parameter list and body".to_string(),
        ));
    }
    let param_names = match &forms[1] {
        SchemeExpr::List(ps) => ps
            .iter()
            .map(|p| atom_name(p, "lambda"))
            .collect::<Result<Vec<_>, _>>()?,
        SchemeExpr::Nil => vec![],
        SchemeExpr::Atom(s) if s == "()" || s == "nil" => vec![],
        _ => {
            return Err(SchemeErr::Runtime(
                "lambda: parameter list must be a list".to_string(),
            ))
        }
    };
    let (params, rest) = extract_rest_param(param_names);
    Ok(SchemeVal::Lambda {
        params,
        rest,
        body: forms[2..].to_vec(),
        env,
    })
}

// ── Guard form ────────────────────────────────────────────────────────────

async fn eval_guard(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    // (guard (var (test expr…) …) body…)
    // R7RS-style error guard.  The body is evaluated; if it raises an error,
    // `var` is bound to the error message string and each clause's test is
    // tried in order.  The first truthy test's expression (or the test value
    // itself when no expression follows) is returned.  If no clause matches
    // the error is re-raised.  If the body succeeds its value is returned.
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "guard: expected (var clauses…) + body".to_string(),
        ));
    }
    let spec = match &forms[1] {
        SchemeExpr::List(s) => s.clone(),
        _ => {
            return Err(SchemeErr::Runtime(
                "guard: first argument must be (var clause…)".to_string(),
            ))
        }
    };
    if spec.is_empty() {
        return Err(SchemeErr::Runtime(
            "guard: missing variable name".to_string(),
        ));
    }
    let var_name = atom_name(&spec[0], "guard")?;
    let clauses = spec[1..].to_vec();
    let body = forms[2..].to_vec();

    match eval_begin(&body, env.clone(), ctx.clone()).await {
        Ok(v) => Ok(v),
        Err(err) => {
            let guard_env = Env::extend(&env);
            guard_env.define(var_name, SchemeVal::Str(err.to_string()));
            for clause in &clauses {
                let parts = match clause {
                    SchemeExpr::List(p) if !p.is_empty() => p,
                    _ => return Err(SchemeErr::Runtime("guard: malformed clause".to_string())),
                };
                let test_val = eval(parts[0].clone(), guard_env.clone(), ctx.clone()).await?;
                if test_val.is_truthy() {
                    return if parts.len() == 1 {
                        Ok(test_val)
                    } else {
                        eval_begin(&parts[1..], guard_env, ctx).await
                    };
                }
            }
            // No clause matched — re-raise.
            Err(err)
        }
    }
}

// ── Pipe threading ────────────────────────────────────────────────────────

fn eval_pipe(
    forms: Vec<SchemeExpr>,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        let mut stages: Vec<Vec<SchemeExpr>> = vec![vec![]];
        for form in forms {
            if matches!(&form, SchemeExpr::Atom(s) if s == "|") {
                stages.push(vec![]);
            } else {
                stages.last_mut().unwrap().push(form);
            }
        }

        let first = stages.remove(0);
        let mut acc = if first.len() == 1 {
            eval(first[0].clone(), env.clone(), ctx.clone()).await?
        } else {
            let h = eval(first[0].clone(), env.clone(), ctx.clone()).await?;
            let mut a = Vec::with_capacity(first.len() - 1);
            for f in &first[1..] {
                a.push(eval(f.clone(), env.clone(), ctx.clone()).await?);
            }
            apply(h, a, ctx.clone()).await?
        };

        for stage in stages {
            if stage.is_empty() {
                continue;
            }
            let stage_env = Env::extend(&env);
            stage_env.define("_", acc.clone());

            let has_placeholder = stage
                .iter()
                .any(|f| matches!(f, SchemeExpr::Atom(s) if s == "_"));

            let f = eval(stage[0].clone(), stage_env.clone(), ctx.clone()).await?;

            let mut args = if has_placeholder {
                Vec::with_capacity(stage.len() - 1)
            } else {
                vec![acc]
            };
            for form in &stage[1..] {
                args.push(eval(form.clone(), stage_env.clone(), ctx.clone()).await?);
            }

            acc = apply(f, args, ctx.clone()).await?;
        }

        Ok(acc)
    })
}

fn eval_begin(
    forms: &[SchemeExpr],
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    let forms = forms.to_owned();
    Box::pin(async move {
        if forms.is_empty() {
            return Ok(SchemeVal::Nil);
        }
        let last = forms.len() - 1;
        for form in &forms[..last] {
            eval(form.clone(), env.clone(), ctx.clone()).await?;
        }
        eval(forms[last].clone(), env, ctx).await
    })
}

// ── Application ────────────────────────────────────────────────────────────

fn apply(
    func: SchemeVal,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        match func {
            SchemeVal::Lambda {
                params,
                rest,
                body,
                env,
            } => {
                let new_env = Env::extend(&env);
                let min = params.len();
                if rest.is_none() && args.len() != min {
                    return Err(SchemeErr::Arity {
                        name: "#<lambda>".to_string(),
                        expected: min,
                        got: args.len(),
                    });
                }
                if args.len() < min {
                    return Err(SchemeErr::Arity {
                        name: "#<lambda>".to_string(),
                        expected: min,
                        got: args.len(),
                    });
                }
                for (p, a) in params.iter().zip(args.iter()) {
                    new_env.define(p.clone(), a.clone());
                }
                if let Some(rest_name) = rest {
                    new_env.define(rest_name, SchemeVal::List(args[min..].to_vec()));
                }
                eval_begin(&body, new_env, ctx).await
            }

            SchemeVal::Builtin(name) => apply_builtin(name, args, ctx).await,

            SchemeVal::MaPath(path) => {
                let command = if args.len() == 1 && args[0].to_splice_lossy().is_empty() {
                    let clean = path.trim_end_matches(':');
                    format!("{clean}:")
                } else if args.is_empty() {
                    path.clone()
                } else {
                    let args_str = args
                        .iter()
                        .map(SchemeVal::to_splice_lossy)
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{path} {args_str}")
                };
                ctx.eval_dot(&command)
            }

            SchemeVal::MaActor(actor) => ctx.eval_actor_with_vals(&actor, &args).await,

            // DID string in function position.
            SchemeVal::Str(ref s) if s.starts_with("did:") => {
                ctx.eval_actor_with_vals(s, &args).await
            }

            other => Err(SchemeErr::Runtime(format!(
                "not a procedure: {}",
                other.display()
            ))),
        }
    })
}

// ── Builtins ───────────────────────────────────────────────────────────────

#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]
fn apply_builtin(
    name: String,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        match name.as_str() {
            "apply" => {
                arity_min("apply", &args, 2)?;
                let f = args[0].clone();
                // (apply f arg1 ... list) — spread last arg as trailing fixed args
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
            "=" | "equal?" | "eqv?" | "eq?" => {
                arity_min("=", &args, 2)?;
                let first = &args[0];
                for a in &args[1..] {
                    if !scheme_equal(first, a) {
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
                lst.get(idx).cloned().ok_or_else(|| {
                    SchemeErr::Runtime(format!("list-ref: index {idx} out of range"))
                })
            }
            "null?" => {
                arity("null?", &args, 1)?;
                let is_null = match &args[0] {
                    SchemeVal::Nil => true,
                    SchemeVal::List(v) if v.is_empty() => true,
                    _ => false,
                };
                Ok(SchemeVal::Bool(is_null))
            }
            "pair?" => {
                arity("pair?", &args, 1)?;
                Ok(SchemeVal::Bool(
                    matches!(&args[0], SchemeVal::List(v) if !v.is_empty()),
                ))
            }
            "map?" => {
                arity("map?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Map(_))))
            }
            "make-map" => {
                if args.len() % 2 != 0 {
                    return Err(SchemeErr::Runtime(format!(
                        "make-map: expected an even number of key/value arguments, got {}",
                        args.len()
                    )));
                }
                let mut map = BTreeMap::new();
                for pair in args.chunks(2) {
                    let key = str_arg(&pair[0], "make-map")?;
                    map.insert(key, pair[1].clone());
                }
                Ok(SchemeVal::Map(map))
            }
            "map-ref" => {
                if !(args.len() == 2 || args.len() == 3) {
                    return Err(SchemeErr::Runtime(format!(
                        "map-ref: expected 2 or 3 arguments, got {}",
                        args.len()
                    )));
                }
                let map = map_arg(&args[0], "map-ref")?;
                let key = str_arg(&args[1], "map-ref")?;
                Ok(map
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| args.get(2).cloned().unwrap_or(SchemeVal::Bool(false))))
            }
            "map-set" => {
                arity("map-set", &args, 3)?;
                let mut map = map_arg(&args[0], "map-set")?;
                let key = str_arg(&args[1], "map-set")?;
                map.insert(key, args[2].clone());
                Ok(SchemeVal::Map(map))
            }
            "map-delete" => {
                arity("map-delete", &args, 2)?;
                let mut map = map_arg(&args[0], "map-delete")?;
                let key = str_arg(&args[1], "map-delete")?;
                map.remove(&key);
                Ok(SchemeVal::Map(map))
            }
            "map-has-key?" => {
                arity("map-has-key?", &args, 2)?;
                let map = map_arg(&args[0], "map-has-key?")?;
                let key = str_arg(&args[1], "map-has-key?")?;
                Ok(SchemeVal::Bool(map.contains_key(&key)))
            }
            "map-keys" => {
                arity("map-keys", &args, 1)?;
                let map = map_arg(&args[0], "map-keys")?;
                Ok(SchemeVal::List(
                    map.keys().cloned().map(SchemeVal::Str).collect(),
                ))
            }
            "map-values" => {
                arity("map-values", &args, 1)?;
                let map = map_arg(&args[0], "map-values")?;
                Ok(SchemeVal::List(map.values().cloned().collect()))
            }
            "map->alist" => {
                arity("map->alist", &args, 1)?;
                let map = map_arg(&args[0], "map->alist")?;
                Ok(SchemeVal::List(
                    map.into_iter()
                        .map(|(key, value)| SchemeVal::List(vec![SchemeVal::Str(key), value]))
                        .collect(),
                ))
            }
            "alist->map" => {
                arity("alist->map", &args, 1)?;
                let entries = list_arg(&args[0], "alist->map")?;
                let mut map = BTreeMap::new();
                for entry in entries {
                    let SchemeVal::List(pair) = entry else {
                        return Err(SchemeErr::Runtime(
                            "alist->map: entries must be key/value lists".into(),
                        ));
                    };
                    if pair.len() != 2 {
                        return Err(SchemeErr::Runtime(
                            "alist->map: entries must have exactly two elements".into(),
                        ));
                    }
                    let key = str_arg(&pair[0], "alist->map")?;
                    map.insert(key, pair[1].clone());
                }
                Ok(SchemeVal::Map(map))
            }
            "string?" => {
                arity("string?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Str(_))))
            }
            "number?" => {
                arity("number?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(
                    &args[0],
                    SchemeVal::Int(_) | SchemeVal::Float(_)
                )))
            }
            "boolean?" => {
                arity("boolean?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Bool(_))))
            }
            "procedure?" => {
                arity("procedure?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(
                    &args[0],
                    SchemeVal::Lambda { .. } | SchemeVal::Builtin(_)
                )))
            }
            "string-append" => {
                let mut s = String::new();
                for a in &args {
                    match a {
                        SchemeVal::Str(st) => s.push_str(st),
                        _ => return Err(SchemeErr::Runtime("string-append: not a string".into())),
                    }
                }
                Ok(SchemeVal::Str(s))
            }
            "string-length" => {
                arity("string-length", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Int(s.len() as i64)),
                    _ => Err(SchemeErr::Runtime("string-length: not a string".into())),
                }
            }
            "substring" => {
                arity("substring", &args, 3)?;
                let s = match &args[0] {
                    SchemeVal::Str(s) => s.clone(),
                    _ => return Err(SchemeErr::Runtime("substring: not a string".into())),
                };
                let start = int_arg(&args[1], "substring")?;
                let end = int_arg(&args[2], "substring")?;
                Ok(SchemeVal::Str(s.get(start..end).unwrap_or("").to_string()))
            }
            "string-contains" => {
                arity("string-contains", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Str(hay), SchemeVal::Str(needle)) => {
                        Ok(SchemeVal::Bool(hay.contains(needle.as_str())))
                    }
                    _ => Err(SchemeErr::Runtime("string-contains: not strings".into())),
                }
            }
            "string-index" => {
                arity("string-index", &args, 2)?;
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
            "string-upcase" => {
                arity("string-upcase", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_uppercase())),
                    _ => Err(SchemeErr::Runtime("string-upcase: not a string".into())),
                }
            }
            "string-downcase" => {
                arity("string-downcase", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_lowercase())),
                    _ => Err(SchemeErr::Runtime("string-downcase: not a string".into())),
                }
            }
            "number->string" => {
                arity("number->string", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) => Ok(SchemeVal::Str(n.to_string())),
                    SchemeVal::Float(f) => Ok(SchemeVal::Str(f.to_string())),
                    _ => Err(SchemeErr::Runtime("number->string: not a number".into())),
                }
            }
            "string->number" => {
                arity("string->number", &args, 1)?;
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
            // I/O — write to stdout
            "display" | "write" => {
                let text = if name == "write" {
                    args.iter()
                        .map(SchemeVal::repr)
                        .collect::<Vec<_>>()
                        .join(" ")
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
            "list?" => {
                arity("list?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(
                    &args[0],
                    SchemeVal::List(_) | SchemeVal::Nil
                )))
            }
            // ── ma send primitives ────────────────────────────────────────
            "rpc-send" => {
                arity_min("rpc-send", &args, 2)?;
                let raw = str_arg(&args[0], "rpc-send")?;
                let verb = str_arg(&args[1], "rpc-send")?;
                let target = ctx.resolve_target(&raw).map_err(SchemeErr::MaError)?;
                let send_result = ctx.send_rpc(&target, &verb, &args[2..]).await;
                match send_result {
                    Err(e) => Ok(err_tuple(e)),
                    Ok(msg_id) => {
                        let (sender, receiver) =
                            futures::channel::oneshot::channel::<Result<SchemeVal, String>>();
                        ctx.register_reply_sender(msg_id, sender);
                        match receiver.await {
                            Ok(Ok(val)) => Ok(SchemeVal::List(vec![
                                SchemeVal::Str(":ok".to_string()),
                                val,
                            ])),
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
            // ── Reply tuple helpers ────────────────────────────────────────
            "ok?" => {
                arity("ok?", &args, 1)?;
                Ok(SchemeVal::Bool(is_ok_tuple(&args[0])))
            }
            "err?" => {
                arity("err?", &args, 1)?;
                Ok(SchemeVal::Bool(is_err_tuple(&args[0])))
            }
            "ok-val" => {
                arity("ok-val", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v)
                        if v.len() >= 2 && matches!(&v[0], SchemeVal::Str(s) if s == ":ok") =>
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
                        if v.len() >= 2 && matches!(&v[0], SchemeVal::Str(s) if s == ":error") =>
                    {
                        Ok(v[1].clone())
                    }
                    _ => Err(SchemeErr::Runtime(
                        "err-msg: not an (:error reason) tuple".into(),
                    )),
                }
            }
            "use" => {
                // Focus mode: no-op in the evaluator; host handles UI state.
                Ok(SchemeVal::Nil)
            }
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
            other => Err(SchemeErr::Undefined(other.to_string())),
        }
    })
}

// ── Helper utilities ───────────────────────────────────────────────────────

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
        (SchemeVal::Map(x), SchemeVal::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| scheme_equal(value, other)))
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

fn map_arg(v: &SchemeVal, name: &str) -> Result<BTreeMap<String, SchemeVal>, SchemeErr> {
    match v {
        SchemeVal::Map(map) => Ok(map.clone()),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a map"))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SchemeCtx;
    use crate::value::Env;
    use futures::{channel::oneshot, future::LocalBoxFuture};
    use std::rc::Rc;

    // ── Minimal host ──────────────────────────────────────────────────────

    /// A no-op `SchemeCtx` for unit tests.
    /// Dot-paths return `Nil`; actor calls and IPFS fetches return errors.
    struct TestCtx;

    impl SchemeCtx for TestCtx {
        fn eval_dot(&self, _command: &str) -> Result<SchemeVal, SchemeErr> {
            Ok(SchemeVal::Nil)
        }
        fn display_output(&self, _text: &str) {}
        fn resolve_target(&self, raw: &str) -> Result<String, String> {
            Ok(raw.to_string())
        }
        fn register_reply_sender(
            &self,
            _id: String,
            _tx: oneshot::Sender<Result<SchemeVal, String>>,
        ) {
        }
        fn fetch_path<'a>(&'a self, _path: &'a str) -> LocalBoxFuture<'a, Result<String, String>> {
            Box::pin(async { Err("no IPFS in tests".to_string()) })
        }
        fn eval_actor<'a>(
            &'a self,
            _cmd: &'a str,
        ) -> LocalBoxFuture<'a, Result<SchemeVal, SchemeErr>> {
            Box::pin(async { Err(SchemeErr::Runtime("no actors in tests".into())) })
        }
        fn eval_actor_with_vals<'a>(
            &'a self,
            _actor: &'a str,
            _args: &'a [SchemeVal],
        ) -> LocalBoxFuture<'a, Result<SchemeVal, SchemeErr>> {
            Box::pin(async { Err(SchemeErr::Runtime("no actors in tests".into())) })
        }
        fn send_rpc<'a>(
            &'a self,
            _target: &'a str,
            _verb: &'a str,
            _args: &'a [SchemeVal],
        ) -> LocalBoxFuture<'a, Result<String, String>> {
            Box::pin(async { Err("no RPC in tests".to_string()) })
        }
        fn send_text<'a>(
            &'a self,
            _target: &'a str,
            _body: &'a str,
        ) -> LocalBoxFuture<'a, Result<String, String>> {
            Box::pin(async { Err("no send_text in tests".to_string()) })
        }
    }

    fn ctx() -> Ctx {
        Rc::new(TestCtx)
    }

    /// Evaluate all top-level forms in `src` and return the last value.
    fn run(src: &str) -> SchemeVal {
        let env = Env::new_root();
        futures::executor::block_on(eval_source_in_env(src, env, ctx())).unwrap()
    }

    /// Like `run` but returns the `Result` so error cases can be asserted.
    fn run_res(src: &str) -> Result<SchemeVal, SchemeErr> {
        let env = Env::new_root();
        futures::executor::block_on(eval_source_in_env(src, env, ctx()))
    }

    // ── is_link_value ─────────────────────────────────────────────────────

    /// ```
    /// # use ma_zscheme::eval::is_link_value;
    /// assert!(is_link_value("/ipfs/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"));
    /// assert!(is_link_value("did:ma:12D3KooWBmAwcd4PJNJvfV89HwE48nwkRmAgo8Vy3uQEyNNHBox2"));
    /// assert!(!is_link_value("hello"));
    /// assert!(!is_link_value(""));
    /// ```
    #[test]
    fn is_link_value_recognises_cids_and_dids() {
        assert!(is_link_value(
            "/ipfs/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
        assert!(is_link_value(
            "/ipfs/bafkreigh2akiscaildcqabab4efnxqfos5zqz2o3qcaz4x6gclz3a47bk4"
        ));
        assert!(is_link_value(
            "/ipns/k51qzi5uqu5dgeb1kdz9fqvzhx2rmpe3fjb0k4jvpxvbn4bcnrfkfeoo9wisze"
        ));
        assert!(is_link_value(
            "did:ma:12D3KooWBmAwcd4PJNJvfV89HwE48nwkRmAgo8Vy3uQEyNNHBox2"
        ));
        assert!(!is_link_value("hello"));
        assert!(!is_link_value(""));
        assert!(!is_link_value("http://example.com"));
        assert!(!is_link_value(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
    }

    // ── Atoms & literals ──────────────────────────────────────────────────

    #[test]
    fn literal_integer() {
        assert!(matches!(run("42"), SchemeVal::Int(42)));
    }

    #[test]
    fn literal_float() {
        assert!(matches!(run("1.5"), SchemeVal::Float(f) if (f - 1.5).abs() < 1e-10));
    }

    #[test]
    fn literal_bool_true() {
        assert!(matches!(run("#t"), SchemeVal::Bool(true)));
    }

    #[test]
    fn literal_bool_false() {
        assert!(matches!(run("#f"), SchemeVal::Bool(false)));
    }

    #[test]
    fn literal_string() {
        assert!(matches!(run("\"hello\""), SchemeVal::Str(s) if s == "hello"));
    }

    #[test]
    fn empty_list_is_nil() {
        assert!(matches!(run("()"), SchemeVal::Nil));
    }

    // ── Arithmetic ────────────────────────────────────────────────────────

    #[test]
    fn add_integers() {
        assert!(matches!(run("(+ 1 2)"), SchemeVal::Int(3)));
    }

    #[test]
    fn add_multiple() {
        assert!(matches!(run("(+ 1 2 3 4)"), SchemeVal::Int(10)));
    }

    #[test]
    fn subtract() {
        assert!(matches!(run("(- 10 3)"), SchemeVal::Int(7)));
    }

    #[test]
    fn multiply() {
        assert!(matches!(run("(* 3 4)"), SchemeVal::Int(12)));
    }

    #[test]
    fn divide_exact() {
        assert!(matches!(run("(/ 10 2)"), SchemeVal::Int(5)));
    }

    #[test]
    fn divide_by_zero_is_err() {
        assert!(run_res("(/ 1 0)").is_err());
    }

    #[test]
    fn modulo() {
        assert!(matches!(run("(mod 10 3)"), SchemeVal::Int(1)));
    }

    #[test]
    fn negate() {
        assert!(matches!(run("(- 5)"), SchemeVal::Int(-5)));
    }

    // ── Comparisons ───────────────────────────────────────────────────────

    #[test]
    fn equal_integers() {
        assert!(matches!(run("(= 2 2)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(= 2 3)"), SchemeVal::Bool(false)));
    }

    #[test]
    fn less_than() {
        assert!(matches!(run("(< 1 2)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(< 2 1)"), SchemeVal::Bool(false)));
    }

    #[test]
    fn greater_than() {
        assert!(matches!(run("(> 3 2)"), SchemeVal::Bool(true)));
    }

    #[test]
    fn chain_comparison() {
        assert!(matches!(run("(< 1 2 3)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(< 1 3 2)"), SchemeVal::Bool(false)));
    }

    // ── Boolean ops ───────────────────────────────────────────────────────

    #[test]
    fn not_false() {
        assert!(matches!(run("(not #f)"), SchemeVal::Bool(true)));
    }

    #[test]
    fn not_truthy() {
        assert!(matches!(run("(not 42)"), SchemeVal::Bool(false)));
    }

    #[test]
    fn and_short_circuits() {
        assert!(matches!(run("(and #t #t)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(and #t #f)"), SchemeVal::Bool(false)));
        assert!(matches!(run("(and #f (/ 1 0))"), SchemeVal::Bool(false)));
    }

    #[test]
    fn or_short_circuits() {
        assert!(matches!(run("(or #f #t)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(or #t (/ 1 0))"), SchemeVal::Bool(true)));
    }

    // ── Control flow ─────────────────────────────────────────────────────

    #[test]
    fn if_true_branch() {
        assert!(matches!(run("(if #t 1 2)"), SchemeVal::Int(1)));
    }

    #[test]
    fn if_false_branch() {
        assert!(matches!(run("(if #f 1 2)"), SchemeVal::Int(2)));
    }

    #[test]
    fn cond_first_matching() {
        assert!(matches!(
            run("(cond (#f 0) (#t 1) (else 2))"),
            SchemeVal::Int(1)
        ));
    }

    #[test]
    fn cond_else_fallthrough() {
        assert!(matches!(run("(cond (#f 0) (else 9))"), SchemeVal::Int(9)));
    }

    #[test]
    fn when_true_evaluates_body() {
        assert!(matches!(run("(when #t 42)"), SchemeVal::Int(42)));
    }

    #[test]
    fn when_false_returns_nil() {
        assert!(matches!(run("(when #f 42)"), SchemeVal::Nil));
    }

    #[test]
    fn unless_false_evaluates_body() {
        assert!(matches!(run("(unless #f 7)"), SchemeVal::Int(7)));
    }

    #[test]
    fn begin_returns_last() {
        assert!(matches!(run("(begin 1 2 3)"), SchemeVal::Int(3)));
    }

    // ── Define & lambda ───────────────────────────────────────────────────

    #[test]
    fn define_and_use() {
        assert!(matches!(run("(define x 10) x"), SchemeVal::Int(10)));
    }

    #[test]
    fn define_function_and_call() {
        assert!(matches!(
            run("(define (square n) (* n n)) (square 5)"),
            SchemeVal::Int(25)
        ));
    }

    #[test]
    fn lambda_closure() {
        assert!(matches!(
            run("(define (adder n) (lambda (x) (+ x n))) ((adder 3) 4)"),
            SchemeVal::Int(7)
        ));
    }

    #[test]
    fn varargs_rest_param() {
        assert!(matches!(
            run("(define (sum . ns) (fold + 0 ns)) (sum 1 2 3)"),
            SchemeVal::Int(6)
        ));
    }

    // ── let forms ─────────────────────────────────────────────────────────

    #[test]
    fn let_binds_locally() {
        assert!(matches!(run("(let ((x 5)) x)"), SchemeVal::Int(5)));
    }

    #[test]
    fn let_star_sequential_binding() {
        assert!(matches!(
            run("(let* ((x 1) (y (+ x 1))) y)"),
            SchemeVal::Int(2)
        ));
    }

    #[test]
    fn letrec_mutual_recursion() {
        let src = "(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
                            (odd?  (lambda (n) (if (= n 0) #f (even? (- n 1))))))
                    (even? 4))";
        assert!(matches!(run(src), SchemeVal::Bool(true)));
    }

    #[test]
    fn named_let_loop() {
        assert!(matches!(
            run("(let loop ((i 0) (acc 0)) (if (= i 5) acc (loop (+ i 1) (+ acc i))))"),
            SchemeVal::Int(10)
        ));
    }

    // ── Tail-call optimisation ────────────────────────────────────────────

    #[test]
    fn tco_deep_recursion_does_not_overflow() {
        // 1 000 000 iterations — would overflow without TCO
        let src = "(define (count n) (if (= n 0) #t (count (- n 1)))) (count 1000000)";
        assert!(matches!(run(src), SchemeVal::Bool(true)));
    }

    // ── List operations ───────────────────────────────────────────────────

    #[test]
    fn list_cons_car_cdr() {
        assert!(matches!(
            run("(car (cons 1 (list 2 3)))"),
            SchemeVal::Int(1)
        ));
        assert!(matches!(run("(car (cdr (list 1 2 3)))"), SchemeVal::Int(2)));
    }

    #[test]
    fn list_length() {
        assert!(matches!(run("(length (list 1 2 3))"), SchemeVal::Int(3)));
        assert!(matches!(run("(length '())"), SchemeVal::Int(0)));
    }

    #[test]
    fn list_ref() {
        assert!(matches!(
            run("(list-ref (list 10 20 30) 1)"),
            SchemeVal::Int(20)
        ));
    }

    #[test]
    fn list_ref_negative_index_is_err() {
        assert!(run_res("(list-ref (list 1 2 3) -1)").is_err());
    }

    #[test]
    fn append_lists() {
        let v = run("(append (list 1 2) (list 3 4))");
        assert!(matches!(v, SchemeVal::List(xs) if xs.len() == 4));
    }

    #[test]
    fn reverse_list() {
        let v = run("(reverse (list 1 2 3))");
        if let SchemeVal::List(xs) = v {
            assert!(matches!(xs[0], SchemeVal::Int(3)));
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn map_doubles() {
        let v = run("(map (lambda (x) (* x 2)) (list 1 2 3))");
        if let SchemeVal::List(xs) = v {
            assert!(matches!(xs[1], SchemeVal::Int(4)));
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn filter_evens() {
        let v = run("(filter (lambda (x) (= (mod x 2) 0)) (list 1 2 3 4))");
        if let SchemeVal::List(xs) = v {
            assert_eq!(xs.len(), 2);
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn fold_sum() {
        assert!(matches!(
            run("(fold + 0 (list 1 2 3 4))"),
            SchemeVal::Int(10)
        ));
    }

    // ── Map operations ────────────────────────────────────────────────────

    #[test]
    fn map_builtins() {
        assert!(matches!(run("(map? (make-map))"), SchemeVal::Bool(true)));
        assert!(matches!(
            run(r#"(map-ref (make-map "a" 1 "b" 2) "a")"#),
            SchemeVal::Int(1)
        ));
        assert!(matches!(
            run(r#"(map-ref (make-map) "missing" "fallback")"#),
            SchemeVal::Str(s) if s == "fallback"
        ));
        assert!(matches!(
            run(r#"(map-has-key? (make-map "a" 1) "a")"#),
            SchemeVal::Bool(true)
        ));
        assert!(matches!(
            run(r#"(map-ref (map-set (make-map "a" 1) "a" 9) "a")"#),
            SchemeVal::Int(9)
        ));
        assert!(matches!(
            run(r#"(map-has-key? (map-delete (make-map "a" 1) "a") "a")"#),
            SchemeVal::Bool(false)
        ));
        assert!(matches!(
            run(r#"(map-ref (alist->map (map->alist (make-map "a" 1))) "a")"#),
            SchemeVal::Int(1)
        ));
        assert!(matches!(
            run(r#"(map-ref (make-map "a" 1 "a" 2) "a")"#),
            SchemeVal::Int(2)
        ));
    }

    #[test]
    fn map_keys_and_values_are_deterministic() {
        assert!(matches!(
            run(r#"(map-keys (make-map "b" 2 "a" 1))"#),
            SchemeVal::List(xs)
                if matches!(&xs[..], [SchemeVal::Str(a), SchemeVal::Str(b)] if a == "a" && b == "b")
        ));
        assert!(matches!(
            run(r#"(map-values (make-map "b" 2 "a" 1))"#),
            SchemeVal::List(xs)
                if matches!(&xs[..], [SchemeVal::Int(1), SchemeVal::Int(2)])
        ));
    }

    // ── String operations ─────────────────────────────────────────────────

    #[test]
    fn string_append() {
        assert!(matches!(
            run("(string-append \"hello\" \" \" \"world\")"),
            SchemeVal::Str(s) if s == "hello world"
        ));
    }

    #[test]
    fn string_length() {
        assert!(matches!(
            run("(string-length \"hello\")"),
            SchemeVal::Int(5)
        ));
    }

    #[test]
    fn substring() {
        assert!(matches!(
            run("(substring \"hello\" 1 3)"),
            SchemeVal::Str(s) if s == "el"
        ));
    }

    #[test]
    fn string_contains() {
        assert!(matches!(
            run("(string-contains \"foobar\" \"oba\")"),
            SchemeVal::Bool(true)
        ));
        assert!(matches!(
            run("(string-contains \"foobar\" \"xyz\")"),
            SchemeVal::Bool(false)
        ));
    }

    #[test]
    fn string_upcase_downcase() {
        assert!(matches!(
            run("(string-upcase \"hello\")"),
            SchemeVal::Str(s) if s == "HELLO"
        ));
        assert!(matches!(
            run("(string-downcase \"WORLD\")"),
            SchemeVal::Str(s) if s == "world"
        ));
    }

    // ── apply builtin ─────────────────────────────────────────────────────

    #[test]
    fn apply_builtin_fn() {
        assert!(matches!(run("(apply + (list 1 2 3))"), SchemeVal::Int(6)));
    }

    #[test]
    fn apply_with_leading_args() {
        assert!(matches!(run("(apply + 1 2 (list 3))"), SchemeVal::Int(6)));
    }

    // ── quote ─────────────────────────────────────────────────────────────

    #[test]
    fn quote_atom() {
        assert!(matches!(run("(quote foo)"), SchemeVal::Str(s) if s == "foo"));
    }

    #[test]
    fn quote_shorthand() {
        assert!(matches!(run("'bar"), SchemeVal::Str(s) if s == "bar"));
    }

    // ── guard form ────────────────────────────────────────────────────────

    #[test]
    fn guard_catches_error() {
        let src = r#"(guard (e (#t "caught")) (error "boom"))"#;
        assert!(matches!(run(src), SchemeVal::Str(s) if s == "caught"));
    }

    #[test]
    fn guard_passes_through_on_no_error() {
        assert!(matches!(run("(guard (e (#t 0)) 42)"), SchemeVal::Int(42)));
    }

    // ── set! ─────────────────────────────────────────────────────────────

    #[test]
    fn set_bang_mutates_binding() {
        assert!(matches!(
            run("(define x 1) (set! x 99) x"),
            SchemeVal::Int(99)
        ));
    }

    // ── predicates ───────────────────────────────────────────────────────

    #[test]
    fn null_pred() {
        assert!(matches!(run("(null? '())"), SchemeVal::Bool(true)));
        assert!(matches!(run("(null? (list 1))"), SchemeVal::Bool(false)));
    }

    #[test]
    fn pair_pred() {
        assert!(matches!(run("(pair? (list 1 2))"), SchemeVal::Bool(true)));
        assert!(matches!(run("(pair? '())"), SchemeVal::Bool(false)));
    }

    #[test]
    fn string_pred() {
        assert!(matches!(run("(string? \"hi\")"), SchemeVal::Bool(true)));
        assert!(matches!(run("(string? 42)"), SchemeVal::Bool(false)));
    }

    #[test]
    fn number_pred() {
        assert!(matches!(run("(number? 3)"), SchemeVal::Bool(true)));
        assert!(matches!(run("(number? \"x\")"), SchemeVal::Bool(false)));
    }

    // ── error propagation ─────────────────────────────────────────────────

    #[test]
    fn undefined_symbol_is_err() {
        assert!(run_res("undefined-variable").is_err());
    }

    #[test]
    fn arity_mismatch_is_err() {
        assert!(run_res("(car)").is_err());
    }
}
