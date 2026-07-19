//! IPFS-backed [`DotRegistry`] for ma-zscheme.
//!
//! # Status
//!
//! This crate is a skeleton. Implementations are stubbed with `todo!()`.
//!
//! A future implementation will store path values as IPFS DAG-CBOR
//! objects, enabling fully decentralised, content-addressed configuration.
//! Changes will be published to IPNS so the registry survives across
//! sessions without a local file.

use std::collections::HashMap;

use ma_zscheme::DotRegistry;

// ── IpfsRegistry ──────────────────────────────────────────────────────────

/// IPFS-backed registry.
///
/// Values are fetched from and stored to IPFS via Kubo or a gateway.
/// The in-memory `cache` holds values for the current session.
///
/// # Not yet implemented
///
/// All mutating methods currently `todo!()`.
pub struct IpfsRegistry {
    /// In-memory session cache.
    cache: HashMap<String, String>,
    /// Kubo RPC base URL, e.g. `http://127.0.0.1:5001`.
    kubo_url: String,
    /// IPFS gateway fallback URL, e.g. `https://dweb.link`.
    gateway_url: String,
}

impl IpfsRegistry {
    /// Create a new registry pointing at `kubo_url` with `gateway_url` as
    /// fallback for reads.
    pub fn new(kubo_url: impl Into<String>, gateway_url: impl Into<String>) -> Self {
        Self {
            cache: HashMap::new(),
            kubo_url: kubo_url.into(),
            gateway_url: gateway_url.into(),
        }
    }

    /// Kubo RPC URL this registry is configured to use.
    #[must_use]
    pub fn kubo_url(&self) -> &str {
        &self.kubo_url
    }

    /// IPFS gateway URL used as read fallback.
    #[must_use]
    pub fn gateway_url(&self) -> &str {
        &self.gateway_url
    }
}

impl DotRegistry for IpfsRegistry {
    fn get(&self, path: &str) -> Option<String> {
        // TODO: fetch from IPFS DAG by deriving CID from the IPNS root + path.
        self.cache.get(path.trim_start_matches('/')).cloned()
    }

    fn set(&mut self, _path: &str, _value: &str) {
        todo!("IpfsRegistry::set — publish updated DAG-CBOR node to IPFS/IPNS")
    }

    fn delete_subtree(&mut self, _path: &str) {
        todo!("IpfsRegistry::delete_subtree — remove subtree and republish")
    }

    fn list(&self, _prefix: &str) -> Vec<(String, String)> {
        todo!("IpfsRegistry::list — enumerate DAG children under prefix")
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ma_zscheme::DotRegistry;

    #[test]
    fn new_compiles_and_accessors_work() {
        let reg = IpfsRegistry::new("http://127.0.0.1:5001", "https://dweb.link");
        assert_eq!(reg.kubo_url(), "http://127.0.0.1:5001");
        assert_eq!(reg.gateway_url(), "https://dweb.link");
    }

    #[test]
    fn get_on_empty_cache_returns_none() {
        let reg = IpfsRegistry::new("http://127.0.0.1:5001", "https://dweb.link");
        assert!(reg.get("my.anything").is_none());
    }

    #[test]
    #[should_panic(expected = "not yet implemented")]
    fn set_is_unimplemented() {
        let mut reg = IpfsRegistry::new("http://127.0.0.1:5001", "https://dweb.link");
        reg.set("my.i18n", "nb");
    }

    #[test]
    #[should_panic(expected = "not yet implemented")]
    fn list_is_unimplemented() {
        let reg = IpfsRegistry::new("http://127.0.0.1:5001", "https://dweb.link");
        let _ = reg.list("my.aliases");
    }
}
