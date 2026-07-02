//! `DotRegistry` — pluggable backing store for dot-path config.
//!
//! Implement this trait to connect any key-value store (flat file, IPFS,
//! database, or an in-memory map) to the evaluator.  The evaluator never
//! touches storage directly; all access goes through
//! [`SchemeCtx::eval_dot`][crate::SchemeCtx::eval_dot], which delegates CRUD
//! to whatever registry the host provides.
//!
//! # Examples
//!
//! ```
//! use ma_zscheme::registry::{DotRegistry, InMemoryRegistry};
//!
//! let mut reg = InMemoryRegistry::default();
//! reg.set("my.aliases.sky", "did:ma:abc");
//! assert_eq!(reg.resolve_alias("sky"), Some("did:ma:abc".to_string()));
//! ```
//!
//! # Extension points
//!
//! The trait has two provided methods with sensible defaults:
//!
//! - [`is_read_only`][DotRegistry::is_read_only] — override to protect keys
//!   (e.g. `EgoConfig` protects `.my.identity.*`).
//! - [`resolve_target`][DotRegistry::resolve_target] — override if your alias
//!   lookup differs from the standard `my.aliases.<name>` convention.
//!
//! Future registry implementations may include an IPFS-backed store
//! (e.g. `IpfsRegistry`) or a database-backed store.

use std::collections::HashMap;

// ── Trait ──────────────────────────────────────────────────────────────────

/// Pluggable dot-path key-value store.
///
/// Keys may be passed with or without a leading `.`; each implementation is
/// expected to normalise internally (see [`normalize_key`]).
pub trait DotRegistry {
    /// Return the value stored at `path`, or `None` if absent.
    fn get(&self, path: &str) -> Option<String>;

    /// Store `value` at `path`. Overwrites any existing value.
    fn set(&mut self, path: &str, value: &str);

    /// Delete the exact key at `path` **and** every key that has it as a
    /// dot-prefix (i.e. the whole subtree rooted at `path`).
    fn delete_subtree(&mut self, path: &str);

    /// List all `(key, value)` pairs whose key begins with `prefix` (exact
    /// match or `prefix.`-prefixed children). Keys in the returned pairs are
    /// normalised with a leading `.`.
    fn list(&self, prefix: &str) -> Vec<(String, String)>;

    /// Resolve an alias name (with or without leading `@`) to the stored DID.
    ///
    /// Default implementation looks up `my.aliases.<name>` via
    /// [`get`][Self::get].
    fn resolve_alias(&self, name: &str) -> Option<String> {
        let bare = name.trim_start_matches('@');
        self.get(&format!("my.aliases.{bare}"))
    }

    /// Whether this path is read-only (writes and deletes should be rejected).
    ///
    /// Default: all paths are writable. Override in stores that protect
    /// certain keys (e.g. `EgoConfig` protects `.my.identity.*`).
    fn is_read_only(&self, _path: &str) -> bool {
        false
    }

    /// Resolve an actor target such as `@alias#fragment` or
    /// `did:ma:…#fragment` to a full DID+fragment string.
    ///
    /// This provided implementation delegates to
    /// [`resolve_alias`][Self::resolve_alias]; you normally do not need to
    /// override it.
    fn resolve_target(&self, raw: &str) -> Result<String, String> {
        let raw = raw.trim_start_matches('@');
        if raw.starts_with("did:") {
            return Ok(raw.to_string());
        }
        if let Some((alias, fragment)) = raw.split_once('#') {
            if alias.is_empty() || fragment.is_empty() {
                return Err(format!("invalid target: {raw}"));
            }
            let did = self
                .resolve_alias(alias)
                .ok_or_else(|| format!("unknown alias: {alias}"))?;
            return Ok(format!("{did}#{fragment}"));
        }
        self.resolve_alias(raw)
            .ok_or_else(|| format!("unknown alias: {raw}"))
    }
}

// ── InMemoryRegistry ───────────────────────────────────────────────────────

/// Simple in-memory registry backed by a `HashMap`.
///
/// This is the default when no file or external store is configured.
/// Data is **not** persisted across process restarts.
#[derive(Clone, Default)]
pub struct InMemoryRegistry {
    data: HashMap<String, String>,
}

impl InMemoryRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DotRegistry for InMemoryRegistry {
    fn get(&self, path: &str) -> Option<String> {
        self.data.get(&normalize_key(path)).cloned()
    }

    fn set(&mut self, path: &str, value: &str) {
        self.data.insert(normalize_key(path), value.to_string());
    }

    fn delete_subtree(&mut self, path: &str) {
        let key = normalize_key(path);
        let prefix = format!("{key}.");
        self.data
            .retain(|k, _| k != &key && !k.starts_with(&prefix));
    }

    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        let key = normalize_key(prefix);
        let prefix_dot = format!("{key}.");
        let mut pairs: Vec<(String, String)> = self
            .data
            .iter()
            .filter(|(k, _)| k.as_str() == key || k.starts_with(&prefix_dot))
            .map(|(k, v)| (format!(".{k}"), v.clone()))
            .collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        pairs
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Strip leading `.` from a dot-path key for normalised internal storage.
pub fn normalize_key(path: &str) -> String {
    path.trim_start_matches('.').to_string()
}
