# ma-zscheme

Scheme evaluator for the [間 (ma) actor network](https://github.com/bahner/rust-ma-core).
A complete, async Scheme interpreter with ma-specific primitives for
dot-path config, actor messaging, and IPFS/CID loading.

Runs on both **native** (tokio `LocalSet`) and **WASM** (browser event loop)
targets without modification. The canonical host is
[zion](https://github.com/bahner/ma-zion), the browser-based ma actor workstation.

---

## Language overview

zscheme is a Lisp/Scheme dialect. Any expression wrapped in `(…)` is
evaluated before the surrounding command is dispatched. Results are spliced
back as strings.

```scheme
; Arithmetic and strings
(+ 1 2)                               ; → 3
(string-append "hello" " " "world")   ; → hello world

; Definitions persist for the session
(define (fib n)
  (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
(fib 10)                              ; → 55

; Inline substitution — result becomes part of the host command
(.my.aliases.sky)#room:enter ((.my.aliases.ms)#house:enter #room)
```

---

## Types

| Type | Examples | Notes |
|------|----------|-------|
| Integer | `42`, `-7` | 64-bit signed |
| Float | `3.14`, `-0.5` | 64-bit IEEE 754 |
| String | `"hello"`, `"did:ma:…"` | UTF-8 |
| Boolean | `#t`, `#f` | |
| Nil | `()`, `nil` | Empty list / null |
| List | `(1 2 3)` | Proper list |
| Lambda | `(lambda (x) x)` | Closure |

Fragment atoms such as `#room` and `#house:enter` are treated as strings.

---

## Special forms

`define`, `lambda` / `ʎ`, `let`, `let*`, `letrec`, `if`, `cond`, `begin`,
`and`, `or`, `when`, `unless`, `set!`, `quote` / `'`, `guard`, `apply`.

Named `let`:

```scheme
(let loop ((i 0) (acc '()))
  (if (= i 5)
      acc
      (loop (+ i 1) (cons i acc))))
```

`guard` (R7RS-small structured error handling):

```scheme
(guard (e
        ((string-contains e "not found") "default")
        (#t (error e)))
  (risky-operation))
```

---

## Core builtins

Arithmetic: `+` `-` `*` `/` `mod` `floor` `ceiling` `round` `truncate`

Comparison: `=` `<` `>` `<=` `>=` `equal?`

Lists: `list` `cons` `car` `cdr` `null?` `pair?`

Strings: `string-append` `string-length` `substring` `string-index`
`string-upcase` `string-downcase` `number->string` `string->number`

Type predicates: `string?` `number?` `boolean?` `procedure?`

I/O and control: `display` `write` `error` `assert`

Script loading: `(include path)` — evaluate all forms in `path.content`

---

## ma primitives

The evaluator recognises forms based on the head of a list expression.

### Dot-path (synchronous config access)

| Form | Meaning |
|------|---------|
| `(.my.path)` | get — returns the config value |
| `(.my.path: "v")` | set — writes config, returns `nil` |
| `(.my.path:)` | delete subtree, returns `nil` |

```scheme
(.my.aliases.sky)                     ; returns stored DID
(.my.config.colour.text)              ; returns colour string
(.my.config.k: "value")              ; sets a config key
```

### Actor RPC (asynchronous)

| Form | Meaning |
|------|---------|
| `(@alias#frag:verb arg…)` | expand alias → DID, send RPC, await reply |
| `(did:ma:abc#frag:verb arg…)` | send RPC directly to full DID-URL |

The `@` / `did:` syntax auto-unwraps replies: success returns the value,
failure raises a `SchemeErr`. Use `rpc-send` for explicit tuple handling.

```scheme
(@sky#house:enter #room)              ; → "ticket-xyz"
(rpc-send "@sky#house" ":enter" "#room")  ; → (:ok "ticket-xyz")
(ok? (rpc-send "@sky#ping" ":ping"))      ; → #t
```

### CID loading

A CID literal in function position fetches content from IPFS and evaluates
all top-level forms in the session environment.

```scheme
(<bafyXXX>)             ; fetch CID, eval all top-level forms
(<bafyXXX> arg1 arg2)   ; fetch CID, eval, then call result as lambda
```

Wrap with `guard` to handle fetch or parse failures:

```scheme
(guard (e (#t (display (string-append "load failed: " e))))
  (<bafyxxx>))
```

---

## Pipe threading

Inside a `(…)` expression, `|` threads a value through a chain of functions
(thread-first). An explicit `_` placeholder overrides placement.

```scheme
"hello" | string-upcase | (string-append " world")
; → "HELLO world"

(@sky#room:who | (search-by "hans") | length)
; count players named "hans"

(@sky#room:who | (take _ 5) | (join _ "\n"))
; explicit _ placeholder
```

---

## Send primitives

| Function | Description |
|----------|-------------|
| `(rpc-send target verb arg…)` | RPC call; returns `(:ok v)` / `(:error r)` / `(:timeout)` |
| `(msg-send target body)` | Plain-text inbox message; returns `(:ok msg-id)` |
| `(chat-send target text)` | Ephemeral chat message |
| `(emote-send target text)` | Emote message |

### Reply tuple helpers

| Function | Description |
|----------|-------------|
| `(ok? reply)` | True if first element is `":ok"` |
| `(err? reply)` | True if first element is `":error"` |
| `(ok-val reply)` | Second element of `(:ok value)` |
| `(err-msg reply)` | Second element of `(:error reason)` |

---

## Session environment

`(define …)` bindings persist across `eval_source` calls within a session
and are cleared on logout.

```scheme
(define (square x) (* x x))
(square 9)    ; → 81  (available for the entire session)
```

---

## Implementation

### Crate structure

| File | Contents |
|------|----------|
| `src/lib.rs` | Public API: `eval_source`, `init_session_env`, `reset_session_env`, `get_env` |
| `src/eval.rs` | Async evaluator: special forms, builtins, ma primitives, TCO loop |
| `src/host.rs` | `SchemeCtx` trait — host interface; `Ctx` type alias |
| `src/parser.rs` | S-expression lexer + parser → `SchemeExpr` AST |
| `src/value.rs` | `SchemeVal` enum + `Env` (lexically-scoped environment) |

### Notable properties

- **Proper tail-call optimisation (TCO)** — iterative `'tco` loop; deep
  recursion does not overflow the stack.
- **Named `let`** — `(let loop ((n 0)) …)` is fully supported.
- **No external parser dependencies** — the lexer and parser are pure Rust.

### Integrating into a host

Implement `SchemeCtx` for your host context, then call `eval_source`:

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

// Evaluate source
use std::rc::Rc;
use ma_zscheme::{eval_source, init_session_env};

let ctx: Ctx = Rc::new(MyCtx::new());
init_session_env();

let result = eval_source("(+ 1 2)", ctx).await?;
// result == SchemeVal::Int(3)
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
