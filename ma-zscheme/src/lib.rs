//! ma-zscheme — Scheme evaluator for the ma actor network.
//!
//! ## Overview
//!
//! Provides a complete Scheme evaluator with:
//! - Proper tail-call optimisation (TCO) via an explicit `'tco` loop
//! - Named `let` (`let loop …`)
//! - `apply` as a first-class procedure
//! - ma-specific primitives: local config paths (`.my.path`, `.ctx.path`),
//!   actor calls (`@alias` / `did:ma:…`), and remote fetch paths
//!   (`#/ipfs/…`, `#/ipns/…`, `#/ipld/…`)
//! - Pipe threading (`val | (f arg) | g`)
//!
//! ## Usage
//!
//! Implement [`SchemeCtx`] for your host context, then call [`eval_source`]:
//!
//! ```ignore
//! use ma_zscheme::{eval_source, init_session_env, SchemeCtx, Ctx};
//!
//! let ctx: Ctx = Rc::new(MyCtx::new());
//! init_session_env();
//! let result = eval_source("(+ 1 2)", ctx).await?;
//! ```

pub mod dot;
pub mod dump;
pub mod eval;
pub mod host;
pub mod parser;
pub mod registry;
pub mod value;

pub use dot::{is_link_value, parse_actor_command, parse_dot_command, DotOp};
pub use dump::dump_env_source;
pub use eval::{eval, eval_str, SchemeErr};
pub use host::{Ctx, SchemeCtx};
pub use registry::{DotRegistry, InMemoryRegistry};
pub use value::{Env, SchemeVal};

use std::cell::RefCell;

// ── Session environment ────────────────────────────────────────────────────

thread_local! {
    static SESSION_ENV: RefCell<Option<Env>> = const { RefCell::new(None) };
}

/// Initialise a fresh session environment (call on login / script start).
pub fn init_session_env() {
    SESSION_ENV.with(|e| *e.borrow_mut() = Some(Env::new_root()));
}

/// Clear the session environment (call on logout / script end).
pub fn reset_session_env() {
    SESSION_ENV.with(|e| *e.borrow_mut() = None);
}

/// Return the current session environment, creating one lazily if needed.
///
/// # Panics
///
/// Panics if the thread-local `SESSION_ENV` has already been destroyed
/// (i.e., during thread shutdown after `reset_session_env` was not called).
#[must_use]
pub fn get_env() -> Env {
    SESSION_ENV.with(|e| {
        let mut inner = e.borrow_mut();
        if inner.is_none() {
            *inner = Some(Env::new_root());
        }
        inner.as_ref().unwrap().clone()
    })
}

// ── Public evaluation API ──────────────────────────────────────────────────

/// Evaluate all top-level Scheme expressions in `source` in the session
/// environment and return the value of the last expression.
///
/// # Errors
///
/// Returns `Err` if parsing fails or any evaluated expression raises an error.
pub async fn eval_source(source: &str, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    eval::eval_source_in_env(source, get_env(), ctx).await
}

/// Evaluate all top-level Scheme expressions in `source` in the given
/// environment (instead of the shared session environment) and return the
/// value of the last expression.
///
/// # Errors
///
/// Returns `Err` if parsing fails or any evaluated expression raises an error.
pub async fn eval_source_in(source: &str, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    eval::eval_source_in_env(source, env, ctx).await
}
