//! Dot-command parsing utilities.
//!
//! `DotOp` and the associated parse functions are the canonical way to
//! interpret local `.my`, `.ctx` path commands.  They are shared across all
//! `SchemeCtx` implementations so any host can call them without
//! re-implementing the grammar.

use crate::DotRegistry;

// ── DotOp ──────────────────────────────────────────────────────────────────

/// The operation encoded in a path command string.
#[derive(Debug, Clone)]
pub enum DotOp {
    /// `.my.path` — return the stored value or list children.
    Get,
    /// `.my.path: value` — store `value` at the path.
    Set(String),
    /// `.my.path:` — delete the subtree rooted at the path.
    Delete,
    /// `.my.path!verb [args]` — dispatch a side-effect verb.
    Meta { verb: String, args: String },
}

/// Parse a path command string into `(path, DotOp)`.
///
/// Formats:
/// - `.my.path`            → `Get`
/// - `.my.path: value`     → `Set("value")`
/// - `.my.path:`           → `Delete`
/// - `.my.path!verb args`  → `Meta { verb, args }`
///
/// Returns `None` if the input is empty after stripping the leading `/`.
///
/// # Examples
///
/// ```
/// use ma_zscheme::{parse_dot_command, DotOp};
///
/// let (path, op) = parse_dot_command(".my.i18n: nb").unwrap();
/// assert_eq!(path, "my/i18n");
/// assert!(matches!(op, DotOp::Set(v) if v == "nb"));
///
/// let (path, op) = parse_dot_command(".my.aliases.sky").unwrap();
/// assert_eq!(path, "my/aliases/sky");
/// assert!(matches!(op, DotOp::Get));
///
/// assert!(parse_dot_command("/my/aliases/sky").is_none());
/// assert!(parse_dot_command("my/aliases/sky").is_none());
/// ```
#[must_use]
pub fn parse_dot_command(command: &str) -> Option<(String, DotOp)> {
    let s = normalize_command_path(command.trim())?;

    // Verb dispatch: .path!verb [args]
    if let Some(bang_idx) = s.find('!') {
        let path = s[..bang_idx].to_string();
        let rest = s[bang_idx + 1..].trim();
        let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
        return Some((
            path,
            DotOp::Meta {
                verb: verb.to_string(),
                args: args.to_string(),
            },
        ));
    }

    // Setter / Delete: find the first colon (paths never contain colons).
    if let Some(colon_idx) = s.find(':') {
        let path = s[..colon_idx].to_string();
        let value = s[colon_idx + 1..].trim().to_string();
        if value.is_empty() {
            return Some((path, DotOp::Delete));
        }
        return Some((path, DotOp::Set(value)));
    }

    // Get
    Some((s.clone(), DotOp::Get))
}

fn normalize_command_path(command: &str) -> Option<String> {
    let command = command.strip_prefix('.')?;
    if command.is_empty() || command.starts_with('.') {
        return None;
    }
    Some(command.replace('.', "/"))
}

// ── Actor command parsing ──────────────────────────────────────────────────

/// Parse an actor command string into `(target_with_fragment, verb, args)`.
///
/// Accepted forms:
/// - `@alias#fragment:verb arg1 arg2`
/// - `@alias#fragment`
/// - `@alias:verb arg1`
/// - `did:ma:abc#fragment:verb arg1`
///
/// # Errors
///
/// Returns `Err` if the alias is unknown in `registry`.
pub fn parse_actor_command(
    cmd: &str,
    registry: &dyn DotRegistry,
) -> Result<(String, String, Vec<String>), String> {
    let (first_token, rest) = cmd.split_once(' ').map_or_else(
        || (cmd.to_string(), String::new()),
        |(a, b)| (a.to_string(), b.to_string()),
    );

    let resolved_first = if let Some(bare) = first_token.strip_prefix('@') {
        resolve_actor_head(bare, registry)?
    } else if first_token.starts_with("did:") {
        first_token.clone()
    } else {
        resolve_actor_head(&first_token, registry)?
    };

    let (target_with_frag, verb) = split_resolved_did_verb(&resolved_first);

    let args: Vec<String> = if rest.is_empty() {
        vec![]
    } else {
        rest.split_whitespace().map(ToString::to_string).collect()
    };

    Ok((target_with_frag, verb, args))
}

/// Resolve an actor head (`alias`, `alias#frag:verb`, etc.) to its full DID form.
fn resolve_actor_head(head: &str, registry: &dyn DotRegistry) -> Result<String, String> {
    if head.starts_with("did:") {
        return Ok(head.to_string());
    }
    let alias_end = head.find(['#', ':', '.']).unwrap_or(head.len());
    let alias = &head[..alias_end];
    let suffix = &head[alias_end..];

    let did = registry
        .resolve_alias(alias)
        .ok_or_else(|| format!("unknown alias: {alias}"))?;
    Ok(format!("{did}{suffix}"))
}

/// Split `did:ma:abc#frag:verb` or `did:ma:abc:verb` into `(target, verb)`.
fn split_resolved_did_verb(s: &str) -> (String, String) {
    if let Some(hash_idx) = s.find('#') {
        let frag_and_maybe_verb = &s[hash_idx + 1..];
        if let Some(colon_idx) = frag_and_maybe_verb.find(':') {
            let frag = &frag_and_maybe_verb[..colon_idx];
            let verb = &frag_and_maybe_verb[colon_idx + 1..];
            let target = format!("{}#{}", &s[..hash_idx], frag);
            return (target, verb.to_string());
        }
        return (s.to_string(), String::new());
    }
    // Look for verb after the 3rd colon in `did:ma:abc:verb`
    let mut colon_count = 0;
    for (i, ch) in s.char_indices() {
        if ch == ':' {
            colon_count += 1;
            if colon_count == 3 {
                let (did_part, verb) = s.split_at(i);
                let verb = &verb[1..];
                if verb.is_empty() {
                    return (did_part.to_string(), String::new());
                }
                return (did_part.to_string(), verb.to_string());
            }
        }
    }
    (s.to_string(), String::new())
}

// ── Link detection ─────────────────────────────────────────────────────────

/// Returns `true` if the string is a `did:ma:` DID or a `/ipfs/`, `/ipns/`,
/// `/ipld/` path.
///
/// # Examples
///
/// ```
/// use ma_zscheme::is_link_value;
///
/// assert!(is_link_value("did:ma:abc"));
/// assert!(is_link_value("/ipfs/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"));
/// assert!(!is_link_value("hello world"));
/// ```
#[must_use]
pub fn is_link_value(s: &str) -> bool {
    s.starts_with("did:ma:")
        || s.starts_with("/ipfs/")
        || s.starts_with("/ipns/")
        || s.starts_with("/ipld/")
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InMemoryRegistry;

    fn reg_with(name: &str, did: &str) -> InMemoryRegistry {
        let mut r = InMemoryRegistry::new();
        r.set(&format!("my/aliases/{name}"), did);
        r
    }

    // ── parse_dot_command ──────────────────────────────

    #[test]
    fn dot_get() {
        let (path, op) = parse_dot_command(".my.aliases.sky").unwrap();
        assert_eq!(path, "my/aliases/sky");
        assert!(matches!(op, DotOp::Get));
    }

    #[test]
    fn dot_get_requires_leading_dot() {
        assert!(parse_dot_command("my/i18n").is_none());
        assert!(parse_dot_command("/my/i18n").is_none());
    }

    #[test]
    fn dot_set() {
        let (path, op) = parse_dot_command(".my.i18n: nb").unwrap();
        assert_eq!(path, "my/i18n");
        assert!(matches!(op, DotOp::Set(v) if v == "nb"));
    }

    #[test]
    fn dot_set_value_trimmed() {
        let (_, op) = parse_dot_command(".my.i18n:   sv  ").unwrap();
        assert!(matches!(op, DotOp::Set(v) if v == "sv"));
    }

    #[test]
    fn dot_delete() {
        let (path, op) = parse_dot_command(".my.i18n:").unwrap();
        assert_eq!(path, "my/i18n");
        assert!(matches!(op, DotOp::Delete));
    }

    #[test]
    fn dot_meta_with_args() {
        let (path, op) = parse_dot_command(".my.inbox.0!reply hello world").unwrap();
        assert_eq!(path, "my/inbox/0");
        assert!(matches!(
            op,
            DotOp::Meta { ref verb, ref args }
            if verb == "reply" && args == "hello world"
        ));
    }

    #[test]
    fn dot_meta_no_args() {
        let (path, op) = parse_dot_command(".my.doc.foo!eval").unwrap();
        assert_eq!(path, "my/doc/foo");
        assert!(
            matches!(op, DotOp::Meta { ref verb, ref args } if verb == "eval" && args.is_empty())
        );
    }

    // ── is_link_value ─────────────────────────────────────────────────────

    #[test]
    fn link_did_ma() {
        assert!(is_link_value("did:ma:abc"));
    }

    #[test]
    fn link_ipfs() {
        assert!(is_link_value(
            "/ipfs/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
    }

    #[test]
    fn link_ipns() {
        assert!(is_link_value(
            "/ipns/k51qzi5uqu5dgeb1kdz9fqvzhx2rmpe3fjb0k4jvpxvbn4bcnrfkfeoo9wisze"
        ));
    }

    #[test]
    fn link_ipld() {
        assert!(is_link_value(
            "/ipld/bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
    }

    #[test]
    fn not_link_bare_cid() {
        assert!(!is_link_value(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
    }

    #[test]
    fn not_link_plain_text() {
        assert!(!is_link_value("hello world"));
    }

    #[test]
    fn not_link_http_url() {
        assert!(!is_link_value("https://example.com"));
    }

    // ── parse_actor_command ─────────────────────────────────────────────

    #[test]
    fn actor_alias_frag_verb_args() {
        let reg = reg_with("sky", "did:ma:abc");
        let (target, verb, args) = parse_actor_command("@sky#house:enter ticket", &reg).unwrap();
        assert_eq!(target, "did:ma:abc#house");
        assert_eq!(verb, "enter");
        assert_eq!(args, vec!["ticket"]);
    }

    #[test]
    fn actor_full_did_frag_verb() {
        let reg = InMemoryRegistry::new();
        let (target, verb, args) = parse_actor_command("did:ma:abc#room:ping", &reg).unwrap();
        assert_eq!(target, "did:ma:abc#room");
        assert_eq!(verb, "ping");
        assert!(args.is_empty());
    }

    #[test]
    fn actor_alias_no_frag() {
        let reg = reg_with("sky", "did:ma:abc");
        let (target, verb, _) = parse_actor_command("@sky:ping", &reg).unwrap();
        assert_eq!(target, "did:ma:abc");
        assert_eq!(verb, "ping");
    }

    #[test]
    fn actor_multiple_args() {
        let reg = reg_with("ms", "did:ma:def");
        let (_, _, args) = parse_actor_command("@ms#room:enter one two three", &reg).unwrap();
        assert_eq!(args, vec!["one", "two", "three"]);
    }

    #[test]
    fn actor_unknown_alias_is_err() {
        let reg = InMemoryRegistry::new();
        assert!(parse_actor_command("@nobody#frag:verb", &reg).is_err());
    }
}
