# ma-zscheme

Scheme evaluator for the [間 (ma) actor network](https://github.com/bahner/rust-ma-core).
Provides a complete, async, host-agnostic Scheme interpreter with ma-specific
primitives for dot-path config, actor messaging, and IPFS/CID loading.

Runs on both **native** (tokio `LocalSet`) and **WASM** (browser event loop)
targets without modification.

---

## Features

- **Proper tail-call optimisation (TCO)** — iterative `'tco` loop; deep
  recursion does not overflow the stack.
- **Named `let`** — `(let loop ((n 0)) …)` is supported.
- **`apply`** as a first-class procedure.
- **Pipe threading** — `val | (f arg) | g` syntax.
- **ma primitives**:
  - Dot-path access: `(.my.config.key)` / `(.my.path: "value")`
  - Actor calls: `(@alias#frag:verb arg)` / `(did:ma:…#frag:verb arg)`
  - CID loading: `(<bafyXXX>)` — fetches content from IPFS and evaluates it
- **Session environment** — `(define …)` bindings persist for the login
  session and are cleared on logout.
- **R7RS-small `guard`** — structured error handling.
- **No external parser dependencies** — the lexer and parser are pure Rust.

---

## Crate structure

| File | Contents |
|------|----------|
| `src/lib.rs` | Public API: `eval_source`, `init_session_env`, `reset_session_env`, `get_env` |
| `src/eval.rs` | Async evaluator: special forms, builtins, ma primitives, TCO loop |
| `src/host.rs` | `SchemeCtx` trait — host interface; `Ctx` type alias |
| `src/parser.rs` | S-expression lexer + parser → `SchemeExpr` AST |
| `src/value.rs` | `SchemeVal` enum + `Env` (lexically-scoped environment) |

---

## Usage

### 1. Implement `SchemeCtx`

```rust
use ma_zscheme::{Ctx, SchemeCtx, SchemeVal, SchemeErr};
use futures::{channel::oneshot, future::LocalBoxFuture};

struct MyCtx { /* … */ }

impl SchemeCtx for MyCtx {
    fn eval_dot(&self, command: &str) -> Result<SchemeVal, SchemeErr> {
        // handle .my.path, .my.path: value, .my.path!verb args
        todo!()
    }

    fn display_output(&self, text: &str) {
        println!("{text}");
    }

    fn resolve_target(&self, raw: &str) -> Result<String, String> {
        // expand @alias → did:ma:…
        todo!()
    }

    fn register_reply_sender(
        &self,
        msg_id: String,
        sender: oneshot::Sender<Result<String, String>>,
    ) {
        todo!()
    }

    fn fetch_cid<'a>(&'a self, cid: &'a str) -> LocalBoxFuture<'a, Result<String, String>> {
        todo!()
    }

    fn eval_actor<'a>(&'a self, cmd: &'a str) -> LocalBoxFuture<'a, Result<SchemeVal, SchemeErr>> {
        todo!()
    }

    fn send_rpc<'a>(
        &'a self,
        target: &'a str,
        verb: &'a str,
        args: &'a [String],
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        todo!()
    }

    fn send_text<'a>(
        &'a self,
        target: &'a str,
        body: &'a str,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        todo!()
    }
}
```

### 2. Evaluate source

```rust
use std::rc::Rc;
use ma_zscheme::{eval_source, init_session_env};

let ctx: Ctx = Rc::new(MyCtx::new());
init_session_env();

let result = eval_source("(+ 1 2)", ctx).await?;
// result == SchemeVal::Int(3)
```

---

## ma primitives

### Dot-path (synchronous config access)

| Form | Meaning |
|------|---------|
| `(.my.path)` | get — returns the config value as a string |
| `(.my.path: "v")` | set — writes config, returns `nil` |
| `(.my.path:)` | delete subtree, returns `nil` |
| `(.my.path!verb args…)` | meta-verb dispatch, returns `nil` |

### Actor calls (asynchronous RPC)

| Form | Meaning |
|------|---------|
| `(@alias#frag:verb arg…)` | expand alias → DID, send RPC, await reply |
| `(did:ma:abc#frag:verb arg…)` | send RPC directly to full DID-URL |

### CID loading

```scheme
(<bafyXXX>)             ; fetch CID, eval all top-level forms
(<bafyXXX> arg1 arg2)   ; fetch CID, eval, then call result as lambda
```

### Pipe threading

```scheme
"hello" | string-upcase | (string-append " world")
; => "HELLO world"
```

---

## Special forms

`define`, `lambda`, `let`, `let*`, `letrec`, `if`, `cond`, `begin`,
`and`, `or`, `when`, `unless`, `set!`, `quote`, `guard`, `apply`.

Named `let`:

```scheme
(let loop ((i 0) (acc '()))
  (if (= i 5)
      acc
      (loop (+ i 1) (cons i acc))))
```

`guard` (R7RS-small error handling):

```scheme
(guard (e
        ((string-contains e "not found") "default")
        (#t (error e)))
  (risky-operation))
```

---

## Session environment

`(define …)` bindings persist across `eval_source` calls within a session:

```rust
init_session_env();   // on login / script start
eval_source("(define x 42)", ctx.clone()).await?;
eval_source("(+ x 1)", ctx.clone()).await?;  // => 43
reset_session_env();  // on logout / script end
```

---

## Dependency

```toml
[dependencies]
ma-zscheme = "0.1"
```

---

## License

MIT
