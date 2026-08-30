]633;E;cat <<'HEADER'\x0apub mod builtins\x3b\x0apub mod helpers\x3b\x0a\x0ause builtins::{apply_builtin, is_builtin}\x3b\x0ause helpers::{atom_name, expr_to_val, extract_rest_param, let_binding}\x3b\x0a\x0aHEADER\x0a;9130a89e-1ad8-4976-9372-aedd084aaf9c]633;Cpub mod builtins;
pub mod helpers;

use builtins::{apply_builtin, is_builtin};
use helpers::{atom_name, expr_to_val, extract_rest_param, let_binding};

/// Async Scheme evaluator for the ma actor network.
///
/// Platform-agnostic: runs on both native (`tokio` `LocalSet`) and WASM (browser
/// event loop via `gloo_timers`) using `LocalBoxFuture`.
/// Host-specific behaviour is abstracted through `crate::host::SchemeCtx`.
use futures::future::LocalBoxFuture;

use crate::host::Ctx;
use crate::parser::{parse_expr, tokenize, SchemeExpr};
use crate::value::{Env, SchemeVal};

// ── Link-value check ───────────────────────────────────────────────────────

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
    // ma fragment atoms like `#room`, `#room:look` — treat as strings.
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


// ── Submodule re-exports ───────────────────────────────────────────────────
pub use helpers::{err_tuple, is_err_ack, is_ok_ack, is_ok_reply, ok_tuple, timeout_tuple};

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
        fn register_reply_sender(&self, _id: String, _tx: oneshot::Sender<Result<String, String>>) {
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
        fn send_rpc<'a>(
            &'a self,
            _target: &'a str,
            _verb: &'a str,
            _args: &'a [String],
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
        assert!(is_link_value("/ipns/k51qzi5uqu5dgeb1kdz9fqvzhx2rmpe3fjb0k4jvpxvbn4bcnrfkfeoo9wisze"));
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

    // ── Reply predicates ──────────────────────────────────────────────────

    #[test]
    fn ok_predicate_is_true_only_for_the_bare_ack() {
        // ok? is the bare :ok ack; the (:ok payload) tuple is ok-reply?'s job.
        assert!(matches!(run(r#"(ok? ":ok")"#), SchemeVal::Bool(true)));
        assert!(matches!(run(r#"(ok? (list ":ok" "prop updated"))"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(ok? "prop updated")"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(ok? (list ":error" "nope"))"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(ok? ())"#), SchemeVal::Bool(false)));
    }

    #[test]
    fn ok_reply_predicate_is_true_only_for_the_ok_tuple() {
        assert!(matches!(
            run(r#"(ok-reply? (list ":ok" "prop updated"))"#),
            SchemeVal::Bool(true)
        ));
        assert!(matches!(run(r#"(ok-reply? ":ok")"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(ok-reply? "prop updated")"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(ok-reply? (list ":error" "nope"))"#), SchemeVal::Bool(false)));
    }

    #[test]
    fn err_predicate_is_true_only_for_the_bare_error() {
        assert!(matches!(run(r#"(err? ":error")"#), SchemeVal::Bool(true)));
        assert!(matches!(run(r#"(err? (list ":error" "nope"))"#), SchemeVal::Bool(false)));
        assert!(matches!(run(r#"(err? ":ok")"#), SchemeVal::Bool(false)));
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
