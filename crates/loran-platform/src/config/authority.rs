//! Public Actr authority deployment configuration.

use actr_protocol::parse_authority_member;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct AuthorityConfig {
    /// Enables `/.well-known/actr` and authority-owned bootstrap endpoints.
    /// Package hosting is enabled independently under `services.packages`.
    #[serde(default)]
    pub enabled: bool,

    /// Canonical public authority URI, `actr://<domain>` without a port,
    /// for example `actr://example.com`. Its domain is the Actr Facility Provider ID.
    #[serde(default)]
    pub uri: Option<String>,

    /// Realm link path prefixes published as the descriptor `links` member,
    /// for example `["/i/"]`. Empty publishes no `links`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

impl AuthorityConfig {
    /// Actr Facility Provider ID: the canonical DNS domain of `uri`.
    pub fn provider_id(&self) -> Result<String, String> {
        let uri = self
            .uri
            .as_deref()
            .ok_or("authority.uri is required: its domain is the Actr Facility Provider ID")?;
        parse_authority_member(uri)
            .map(|uri| uri.domain().to_string())
            .map_err(|error| format!("authority.uri is not a valid authority URI: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(uri: Option<&str>) -> AuthorityConfig {
        AuthorityConfig {
            uri: uri.map(str::to_string),
            ..AuthorityConfig::default()
        }
    }

    #[test]
    fn provider_id_is_the_canonical_authority_domain() {
        assert_eq!(
            config(Some("actr://Provider.Example")).provider_id(),
            Ok("provider.example".to_string())
        );
        assert!(config(None).provider_id().is_err());
        for uri in [
            "actr://provider.example:443",
            "actr://provider.example/",
            "https://provider.example",
            "actr://abcdefghijklmnopqrstuvwxyz.actr.alt",
        ] {
            assert!(config(Some(uri)).provider_id().is_err(), "{uri}");
        }
    }
}
