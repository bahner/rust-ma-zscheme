//! YAML/file-backed [`DotRegistry`] for ma-zscheme.
//!
//! `SchemeConfig` stores `/`-path key-value pairs in a YAML file under
//! `$XDG_CONFIG_HOME/ma/zscheme-data.yaml`.  It is the default persistent
//! backend for the `zscheme` CLI.
//!
//! # Example
//!
//! ```no_run
//! use ma_zscheme_yaml::SchemeConfig;
//! use ma_zscheme::DotRegistry;
//!
//! let path = SchemeConfig::default_path().unwrap();
//! let mut cfg = SchemeConfig::load(&path);
//! cfg.set("my/aliases/sky", "did:ma:abc");
//! cfg.save(&path).unwrap();
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ma_zscheme::DotRegistry;

// ── SchemeConfig ───────────────────────────────────────────────────────────

/// Flat key-value configuration store backed by a YAML file.
///
/// Keys are stored without a leading `/` (e.g. `"my/aliases/sky"`).
#[derive(Clone, Default)]
pub struct SchemeConfig {
    data: HashMap<String, String>,
}

impl SchemeConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from a YAML file. Returns an empty config if the file is absent.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::new();
        };
        let map: HashMap<String, String> = serde_yaml::from_str(&text).unwrap_or_default();
        Self { data: map }
    }

    /// Persist to a YAML file, creating parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the directory cannot be created or the file cannot be written.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_yaml::to_string(&self.data)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Return the default data file path: `$XDG_CONFIG_HOME/ma/zscheme-data.yaml`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the home/config directory cannot be determined.
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
        Ok(base.config_dir().join("ma").join("zscheme-data.yaml"))
    }

    /// Get a value at `path` (leading `/` optional).
    #[must_use]
    pub fn get_str(&self, path: &str) -> Option<String> {
        self.data.get(&normalize_key(path)).cloned()
    }

    /// Set a value at `path`.
    pub fn set(&mut self, path: &str, value: &str) {
        self.data.insert(normalize_key(path), value.to_string());
    }

    /// Delete the subtree rooted at `path`.
    pub fn delete_subtree(&mut self, path: &str) {
        let key = normalize_key(path);
        let prefix = format!("{key}/");
        self.data
            .retain(|k, _| k != &key && !k.starts_with(&prefix));
    }

    /// List all `(key, value)` pairs that are direct or indirect children of `path`.
    #[must_use]
    pub fn list(&self, path: &str) -> Vec<(String, String)> {
        let key = normalize_key(path);
        let prefix = format!("{key}/");
        let mut pairs: Vec<(String, String)> = self
            .data
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix) || *k == &key)
            .map(|(k, v)| (format!("/{k}"), v.clone()))
            .collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        pairs
    }

    /// Resolve an alias name to its stored DID.
    #[must_use]
    pub fn resolve_alias(&self, name: &str) -> Option<String> {
        let bare = name.trim_start_matches('@');
        let key = format!("my/aliases/{bare}");
        self.data.get(&key).cloned()
    }
}

// ── DotRegistry impl ───────────────────────────────────────────────────────

impl DotRegistry for SchemeConfig {
    fn get(&self, path: &str) -> Option<String> {
        self.get_str(path)
    }

    fn set(&mut self, path: &str, value: &str) {
        SchemeConfig::set(self, path, value);
    }

    fn delete_subtree(&mut self, path: &str) {
        SchemeConfig::delete_subtree(self, path);
    }

    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        SchemeConfig::list(self, prefix)
    }

    fn resolve_alias(&self, name: &str) -> Option<String> {
        SchemeConfig::resolve_alias(self, name)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Strip leading `/` from a path key for internal storage.
fn normalize_key(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ma_zscheme::DotRegistry;

    // ── get_str / set ─────────────────────────────────────────────────────

    #[test]
    fn get_str_missing_returns_none() {
        assert!(SchemeConfig::new().get_str("my/nonexistent").is_none());
    }

    #[test]
    fn set_and_get_str() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/i18n", "nb");
        assert_eq!(cfg.get_str("my/i18n"), Some("nb".into()));
    }

    #[test]
    fn get_str_leading_slash_stripped() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/i18n", "nb");
        assert_eq!(cfg.get_str("/my/i18n"), Some("nb".into()));
    }

    #[test]
    fn set_with_leading_slash() {
        let mut cfg = SchemeConfig::new();
        cfg.set("/my/i18n", "nb");
        assert_eq!(cfg.get_str("my/i18n"), Some("nb".into()));
    }

    // ── DotRegistry::get / set ────────────────────────────────────────────

    #[test]
    fn trait_get_and_set() {
        let mut cfg = SchemeConfig::new();
        DotRegistry::set(&mut cfg, "/my/i18n", "sv");
        assert_eq!(DotRegistry::get(&cfg, "my/i18n"), Some("sv".into()));
        assert_eq!(DotRegistry::get(&cfg, "/my/i18n"), Some("sv".into()));
    }

    // ── delete_subtree ────────────────────────────────────────────────────

    #[test]
    fn delete_subtree_removes_children() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/sky", "did:ma:abc");
        cfg.set("my/aliases/ms", "did:ma:def");
        cfg.set("my/i18n", "nb");
        DotRegistry::delete_subtree(&mut cfg, "my/aliases");
        assert!(cfg.get_str("my/aliases/sky").is_none());
        assert!(cfg.get_str("my/aliases/ms").is_none());
        assert_eq!(cfg.get_str("my/i18n"), Some("nb".into())); // untouched
    }

    #[test]
    fn delete_exact_leaf() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/i18n", "nb");
        DotRegistry::delete_subtree(&mut cfg, "/my/i18n");
        assert!(cfg.get_str("my/i18n").is_none());
    }

    #[test]
    fn delete_absent_is_noop() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/i18n", "nb");
        DotRegistry::delete_subtree(&mut cfg, "my/nonexistent");
        assert_eq!(cfg.get_str("my/i18n"), Some("nb".into()));
    }

    // ── list ──────────────────────────────────────────────────────────────

    #[test]
    fn list_sorted_with_leading_slash() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/z", "did:ma:z");
        cfg.set("my/aliases/a", "did:ma:a");
        let pairs = DotRegistry::list(&cfg, "my/aliases");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "/my/aliases/a");
        assert_eq!(pairs[1].0, "/my/aliases/z");
    }

    #[test]
    fn list_empty_when_no_match() {
        assert!(DotRegistry::list(&SchemeConfig::new(), "my/nonexistent").is_empty());
    }

    // ── resolve_alias ─────────────────────────────────────────────────────

    #[test]
    fn resolve_alias_found() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/sky", "did:ma:abc");
        assert_eq!(
            DotRegistry::resolve_alias(&cfg, "sky"),
            Some("did:ma:abc".into())
        );
    }

    #[test]
    fn resolve_alias_at_prefix_stripped() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/sky", "did:ma:abc");
        assert_eq!(
            DotRegistry::resolve_alias(&cfg, "@sky"),
            Some("did:ma:abc".into())
        );
    }

    #[test]
    fn resolve_alias_missing_returns_none() {
        assert!(DotRegistry::resolve_alias(&SchemeConfig::new(), "nobody").is_none());
    }

    // ── resolve_target (provided) ─────────────────────────────────────────

    #[test]
    fn resolve_target_did_passthrough() {
        let cfg = SchemeConfig::new();
        assert_eq!(
            DotRegistry::resolve_target(&cfg, "did:ma:abc#room"),
            Ok("did:ma:abc#room".into())
        );
    }

    #[test]
    fn resolve_target_alias_fragment() {
        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/sky", "did:ma:abc");
        assert_eq!(
            DotRegistry::resolve_target(&cfg, "@sky#room"),
            Ok("did:ma:abc#room".into())
        );
    }

    #[test]
    fn resolve_target_unknown_is_err() {
        assert!(DotRegistry::resolve_target(&SchemeConfig::new(), "@nobody").is_err());
    }

    // ── save / load roundtrip ─────────────────────────────────────────────

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");

        let mut cfg = SchemeConfig::new();
        cfg.set("my/aliases/sky", "did:ma:abc");
        cfg.set("my/i18n", "nb");
        cfg.save(&path).unwrap();

        let loaded = SchemeConfig::load(&path);
        assert_eq!(loaded.get_str("my/aliases/sky"), Some("did:ma:abc".into()));
        assert_eq!(loaded.get_str("my/i18n"), Some("nb".into()));
    }

    #[test]
    fn load_absent_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.yaml");
        let cfg = SchemeConfig::load(&path);
        assert!(cfg.get_str("my/anything").is_none());
    }
}
