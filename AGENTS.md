# rust-ma-zscheme - Agent Notes

## Repository role

This workspace owns the reusable zscheme evaluator and host contracts. Platform I/O belongs behind `ma_zscheme::SchemeCtx`; evaluator code must not depend on Zion, the native CLI, Kubo, or browser APIs.

## Workspace layout

This is a **virtual workspace**: the root `Cargo.toml` is `[workspace]`-only
and has no `[package]`. Every published crate lives in a member directory:

| Crate | Directory | crates.io package |
|---|---|---|
| Scheme evaluator and host contracts | `ma-zscheme/` | `ma-zscheme` |
| YAML mapping extensions | `ma-zscheme-yaml/` | `ma-zscheme-yaml` |
| IPFS/IPLD helpers | `ma-zscheme-ipfs/` | `ma-zscheme-ipfs` |

The workspace root holds only the workspace manifest, `Cargo.lock`, and
top-level docs/scripts. It must never contain crate source: a root-level
`src/` or `[package]` is orphaned code that is never compiled, tested, or
published, but looks real to an agent. The evaluator and its Scheme builtins
(e.g. `ok?`, `ok-reply?`, `err?`, `ok-val`, `err-msg`) live in
`ma-zscheme/src/` — today the flat `ma-zscheme/src/eval.rs`. Before editing
any `.rs` file, confirm it sits under a member directory; a change anywhere
else silently misses the published crate.

> 2026-08-30: the `ok-reply?` builtin and the `eval/` module split were
> committed to a stray root `src/`, so published `ma-zscheme` 0.6.0 shipped
> without `ok-reply?` and consumers failed with `undefined: ok-reply?`. The
> stray directory was deleted (`368c2cf`) and the fix re-applied to
> `ma-zscheme/src/eval.rs` (`e3c30c9`, v0.6.1). Never recreate a root `src/`.

## Agent rules

- Write DRY, KISS code: avoid duplicated logic and prefer the simplest
  implementation that meets the requirement.
- Crate source lives only in member directories — see "Workspace layout".
  Never create or edit a root-level `src/`; it is orphaned code that is
  never compiled, tested, or published.

## Content access contract

Content references are literal data unless an explicit primitive consumes them:

- `(ipfs-get #/ipfs/<cid>)` returns `SchemeVal::Bytes`.
- `(ipfs-cat #/ipfs/<cid>)` returns UTF-8 text.
- `(ipfs-name-resolve #/ipns/<name>)` returns the current `/ipfs/<cid>` path without fetching its content.
- `(include #/ipfs/<cid>)` fetches and evaluates Scheme source.

`ipfs-get` and `ipfs-cat` may also consume supported IPNS/IPLD paths. Never make a bare path, parenthesised path, config setter, or command splice fetch content implicitly. Byte values are opaque: do not stringify or splice them into terminal commands.

## Local path syntax

Inside Scheme parentheses, source uses hash-dot syntax exclusively for local paths: `#.my.path`. Bare dot paths must remain rejected as Scheme list heads. This restriction does not alter a host application's terminal command grammar outside Scheme expressions. `SchemeCtx::eval_dot` receives the normalised path without the leading `#`.

## Host boundaries

- Keep `SchemeCtx::fetch_path`, `fetch_bytes`, and `resolve_ipns` platform-neutral.
- `SchemeCtx::random_bytes` is the entropy boundary for `(random)`.
  Hosts must use a cryptographically secure source and must fail rather than
  substitute deterministic or presentation-grade randomness.
- Hosts must preserve `SchemeVal::Bytes` as CBOR byte strings and string-keyed `SchemeVal::Map` values as CBOR maps.
- IPNS name resolution belongs in `ma_core::IpfsGatewayResolver`; hosts call it instead of implementing gateway selection or response parsing themselves.

## Multi-repository development

Consumers use published semver dependencies. Do not commit cross-repository path dependencies. Before matching patch releases are published, validate consumers with Cargo `--config patch.crates-io.<crate>.path=...`; keep lockfiles on registry sources afterwards.
