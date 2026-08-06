# rust-ma-zscheme - Agent Notes

## Repository role

This workspace owns the reusable zscheme evaluator and host contracts. Platform I/O belongs behind `ma_zscheme::SchemeCtx`; evaluator code must not depend on Zion, the native CLI, Kubo, or browser APIs.

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
- Hosts must preserve `SchemeVal::Bytes` as CBOR byte strings and string-keyed `SchemeVal::Map` values as CBOR maps.
- IPNS name resolution belongs in `ma_core::IpfsGatewayResolver`; hosts call it instead of implementing gateway selection or response parsing themselves.

## Multi-repository development

Consumers use published semver dependencies. Do not commit cross-repository path dependencies. Before matching patch releases are published, validate consumers with Cargo `--config patch.crates-io.<crate>.path=...`; keep lockfiles on registry sources afterwards.
