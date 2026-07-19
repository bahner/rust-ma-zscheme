/// `SchemeCtx` — host interface for the ma-zscheme evaluator.
///
/// Implement this trait on your platform-specific context type to give the
/// evaluator access to config, transport, and I/O.  The trait uses
/// `LocalBoxFuture` for all async methods so it is compatible with both
/// `dyn Trait` dynamic dispatch and both native (tokio `LocalSet`) and WASM
/// (browser event loop) runtimes.
use std::rc::Rc;

use futures::{channel::oneshot, future::LocalBoxFuture};

use crate::eval::SchemeErr;
use crate::value::SchemeVal;

/// Host interface threaded through every recursive eval call.
pub trait SchemeCtx {
    // ── Synchronous methods ───────────────────────────────────────────────

    /// Evaluate a ma local config path and return the result as a `SchemeVal`.
    ///
    /// Handles get (`/my/path`), set (`/my/path: value`),
    /// delete (`/my/path:`), and meta-verbs (`/my/path!verb args`).
    /// Only ever called for local roots (`/my`, `/ctx`) — `/ipfs`, `/ipns`,
    /// `/ipld` are routed to [`fetch_path`][Self::fetch_path] instead.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the path command is invalid or the host refuses it.
    fn eval_dot(&self, command: &str) -> Result<SchemeVal, SchemeErr>;

    /// Write `text` to the host output channel (terminal line, browser span, …).
    fn display_output(&self, text: &str);

    /// Resolve an actor target (`@alias` or bare DID) to its full DID form.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the alias is unknown or the input is not a valid DID.
    fn resolve_target(&self, raw: &str) -> Result<String, String>;

    /// Register a oneshot `sender` so the poll loop can deliver the RPC reply
    /// for the message identified by `msg_id`.
    fn register_reply_sender(
        &self,
        msg_id: String,
        sender: oneshot::Sender<Result<SchemeVal, String>>,
    );

    // ── Asynchronous methods ──────────────────────────────────────────────

    /// Fetch the UTF-8 text content of a `/ipfs/<cid>`, `/ipns/<key>`,
    /// `/ipld/<cid>` path, or a bare `did:ma:` DID.
    fn fetch_path<'a>(&'a self, path: &'a str) -> LocalBoxFuture<'a, Result<String, String>>;

    /// Dispatch a fully-formed ma actor command and await the reply.
    ///
    /// `cmd` is a raw command string such as `@alias#frag:verb arg` or
    /// `did:ma:…#frag:verb arg`.
    fn eval_actor<'a>(&'a self, cmd: &'a str) -> LocalBoxFuture<'a, Result<SchemeVal, SchemeErr>>;

    /// Send an RPC message to `target` and return the message id for reply
    /// correlation via `register_reply_sender`.
    fn send_rpc<'a>(
        &'a self,
        target: &'a str,
        verb: &'a str,
        args: &'a [String],
    ) -> LocalBoxFuture<'a, Result<String, String>>;

    /// Send a plain-text inbox message (fire-and-forget) and return the
    /// message id.
    fn send_text<'a>(
        &'a self,
        target: &'a str,
        body: &'a str,
    ) -> LocalBoxFuture<'a, Result<String, String>>;
}

/// Reference-counted host context threaded through evaluation.
/// `Rc` (not `Arc`) because both tokio `LocalSet` and WASM are single-threaded.
pub type Ctx = Rc<dyn SchemeCtx>;
