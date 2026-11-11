//! Unified configuration management system
//!
//! This module is the "single source of truth" for Loran auxiliary service configuration.
//! All configuration item definitions, documentation, and default values are managed here uniformly.

pub mod authority;
pub mod bind;
pub mod config_store;
pub mod control;
pub mod issuer;
pub mod packages;
pub mod realm;
pub mod registry;
pub mod relay;
pub mod resolver;
pub mod services;
pub mod signaling;
pub mod signer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub mod turn;

pub use crate::config::authority::AuthorityConfig;
pub use crate::config::bind::BindConfig;
pub use crate::config::control::{AdminLoginLimits, AdminUiConfig, ControlConfig, ControlHead};
pub use crate::config::issuer::IssuerServiceConfig;
pub use crate::config::packages::PackagesConfig;
pub use crate::config::realm::RealmConfig;
pub use crate::config::relay::{RelayAuthMode, RelayConfig};
pub use crate::config::services::ServicesConfig;
pub use crate::config::signaling::SignalingConfig;
pub use crate::config::turn::TurnConfig;
use ::signer::storage::StorageBackend;
use base64::Engine as _;
use rand::{RngCore, rngs::OsRng};
use std::path::{Path, PathBuf};
use url::Url;

fn generate_ephemeral_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn looks_like_placeholder_secret(secret: &str) -> bool {
    let normalized = secret.to_ascii_lowercase();
    [
        "default",
        "change",
        "example",
        "replace",
        "changeme",
        "your-",
        "<generate",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

/// Minimum number of distinct bytes a secret must contain.
///
/// A secret that clears a length gate but is built from very few distinct bytes
/// (for example `"aaaa...a"` or a base64 blob of all-zero bytes) is trivially
/// guessable. This floor rejects the most degenerate cases while staying well
/// below the distinct-byte count of any real hex/base64 random secret (hex uses
/// up to 16 symbols, base64 up to 64), so legitimate values are never flagged.
const MIN_SECRET_DISTINCT_BYTES: usize = 8;

/// Approximate entropy check: true when the secret carries too few distinct
/// bytes to be a credible random value.
fn looks_like_low_entropy_secret(secret: &str) -> bool {
    secret
        .bytes()
        .collect::<std::collections::BTreeSet<u8>>()
        .len()
        < MIN_SECRET_DISTINCT_BYTES
}

/// Apply the shared secret-hygiene gates to a named secret: reject documented
/// placeholder markers, enforce a minimum length, and reject degenerate
/// low-entropy values. One error is pushed per failing gate.
fn validate_secret_hygiene(field: &str, secret: &str, min_len: usize, errors: &mut Vec<String>) {
    if looks_like_placeholder_secret(secret) {
        errors.push(format!(
            "{field} must not use a documented placeholder or example value"
        ));
    }
    if secret.len() < min_len {
        errors.push(format!("{field} must be at least {min_len} characters"));
    }
    if looks_like_low_entropy_secret(secret) {
        errors.push(format!(
            "{field} has too few distinct characters; use a high-entropy random value"
        ));
    }
}

/// CFG-06: collect every secret value that is actually present (non-empty) in
/// the config, paired with its config-field path.
///
/// Used for cross-field reuse detection: no two distinct secret fields may hold
/// the same value, because sharing a secret means compromising one credential
/// compromises the others. Empty / unset secrets are ignored. Password *hashes*
/// (for example `control.admin_ui.password_hash`) are intentionally excluded:
/// they are meant to be stored and are not raw secrets.
fn collect_present_secrets(config: &LoranConfig) -> Vec<(&'static str, &str)> {
    let mut secrets: Vec<(&'static str, &str)> = Vec::new();

    if !config.loran_shared_key.is_empty() {
        secrets.push(("loran_shared_key", config.loran_shared_key.as_str()));
    }
    if !config.turn.turn_secret.is_empty() {
        secrets.push(("turn.turn_secret", config.turn.turn_secret.as_str()));
    }
    if let Some(signer) = config.services.signer.as_ref() {
        if let Some(kek) = signer.kek.as_deref() {
            if !kek.is_empty() {
                secrets.push(("services.signer.kek", kek));
            }
        }
    }
    if let Some(issuer) = config.services.issuer.as_ref() {
        let renewal = issuer.server.renewal_token_secret.as_str();
        if !renewal.is_empty() {
            secrets.push(("services.issuer.server.renewal_token_secret", renewal));
        }
    }
    if !config.control.grpc_api.shared_secret.is_empty() {
        secrets.push((
            "control.grpc_api.shared_secret",
            config.control.grpc_api.shared_secret.as_str(),
        ));
    }

    secrets
}

/// CFG-06: reject reuse of one secret value across two or more distinct secret
/// fields. Reuse is a hard error in production and a warning in development so
/// local configs still boot (the runtime ignores `Warning:`-prefixed entries).
/// The colliding field paths are named so operators can fix them.
fn validate_secret_reuse(config: &LoranConfig, errors: &mut Vec<String>) {
    let secrets = collect_present_secrets(config);
    let mut by_value: std::collections::BTreeMap<&str, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for &(field, value) in &secrets {
        by_value.entry(value).or_default().push(field);
    }
    for (_value, mut fields) in by_value {
        if fields.len() >= 2 {
            fields.sort_unstable();
            let msg = format!(
                "secret reuse detected: {} share the same value; each secret must use an independent random value",
                fields.join(", ")
            );
            if config.env == "prod" {
                errors.push(msg);
            } else {
                errors.push(format!("Warning: {msg}"));
            }
        }
    }
}

/// Validate that an outbound dependency endpoint uses HTTPS in production.
///
/// Only `bind.http` (the inbound listener) is forced to TLS elsewhere; outbound
/// dependency calls must be checked separately (CFG-05). Loopback targets
/// (127.0.0.1, ::1, localhost) are exempt because that traffic never leaves the
/// host. When the caller carries an explicit `enable_tls` switch, an `https://`
/// endpoint must agree with it.
fn validate_prod_endpoint_tls(
    field: &str,
    endpoint: &str,
    enable_tls: Option<bool>,
    errors: &mut Vec<String>,
) {
    let parsed = match Url::parse(endpoint) {
        Ok(parsed) => parsed,
        Err(error) => {
            errors.push(format!(
                "Invalid endpoint URL in {field}: {endpoint} (parse error: {error})"
            ));
            return;
        }
    };

    // Loopback targets are exempt: intra-host traffic never crosses the network.
    let is_loopback = match parsed.host_str() {
        Some(host) => {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .map(|ip| ip.is_loopback())
                    .unwrap_or(false)
        }
        None => false,
    };
    if is_loopback {
        return;
    }

    if parsed.scheme() != "https" {
        errors.push(format!(
            "Production {field} must use https:// for non-loopback endpoints, got {endpoint}"
        ));
    } else if enable_tls == Some(false) {
        errors.push(format!(
            "Production {field} points at an https:// endpoint but enable_tls is false; set enable_tls = true"
        ));
    }
}

/// Main configuration struct for Loran auxiliary services
///
/// This is the core configuration of the system, containing all service configuration information.
/// The configuration file uses TOML format and supports fully type-safe loading.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LoranConfig {
    /// Service enable flags (bitmask) - Primary switch for business services
    ///
    /// This is the primary control mechanism for enabling business services.
    /// `control` has independent endpoint switches, not controlled by this bitmask.
    ///
    /// Bit positions (business services only):
    /// - Bit 0 (1): Signaling service
    /// - Bit 1 (2): STUN service
    /// - Bit 2 (4): TURN service
    /// - Bit 3 (8): Issuer (Actor Identity Service)
    /// - Bit 4 (16): Signer
    /// - Bit 5 (32): WebSocket relay
    ///
    /// Examples:
    /// - `enable = 63` enables all services; 31 preserves the original five-service mask
    /// - `enable = 6` enables STUN + TURN (2+4=6)
    /// - `enable = 7` enables Signaling + STUN + TURN (1+2+4=7)
    #[serde(default = "default_enable")]
    pub enable: u8,

    /// Server instance name
    ///
    /// Used to identify different server instances and distinguish nodes in cluster deployments.
    /// A meaningful naming convention is recommended, e.g. loran-01, loran-prod-east-1.
    pub name: String,

    /// Runtime environment identifier
    ///
    /// Specifies the current runtime environment, which affects security policies and default behavior:
    /// - "dev": Development environment; allows HTTP, relaxed certificate checks
    /// - "prod": Production environment; enforces HTTPS, strict security checks
    /// - "test": Test environment; used for automated testing
    pub env: String,

    /// Runtime user (optional)
    ///
    /// Specifies the system user the service runs as. The service drops to this user
    /// after binding ports to improve security. Leave empty to keep the current user.
    pub user: Option<String>,

    /// Runtime user group (optional)
    ///
    /// Specifies the system user group the service runs as. Used together with the `user` setting.
    pub group: Option<String>,

    /// PID file path (optional)
    ///
    /// File path used to store the process ID. System management tools can use this file
    /// to monitor and manage the service process.
    pub pid: Option<String>,

    /// Network bind configuration
    ///
    /// Defines bind addresses and port configuration for various network services.
    pub bind: BindConfig,

    /// Public Actr authority discovery.
    #[serde(default)]
    pub authority: AuthorityConfig,

    /// Realm hosting limits.
    #[serde(default)]
    pub realm: RealmConfig,

    /// TURN service-specific configuration
    ///
    /// Dedicated configuration for the TURN relay service, including public address, port range, authentication realm, etc.
    #[serde(default)]
    pub turn: TurnConfig,

    /// Multiplexed WebSocket relay, enabled with bit 5 (32).
    #[serde(default)]
    pub relay: RelayConfig,

    /// Location tag
    ///
    /// Used to identify the server's geographic location or logical grouping for operations management and monitoring.
    /// e.g. us-west-1, office-beijing, edge-node-01
    pub location_tag: String,

    /// Optional control plane configuration
    ///
    /// Endpoint switches are independent of the `enable` bitmask.
    #[serde(default)]
    pub control: ControlConfig,

    /// Service configuration collection
    ///
    /// Contains configuration for all business services. Each service can independently configure its own parameters and dependencies.
    /// Uses a service-level configuration structure to achieve high cohesion and low coupling.
    #[serde(default)]
    pub services: ServicesConfig,

    /// SQLite database file storage directory path
    ///
    /// Specifies the directory path used to store all SQLite database files.
    /// The main database file is stored as `{sqlite_path}/loran.db`.
    /// Includes Realm info, access control lists, nonce cache, etc.
    #[serde(
        default,
        serialize_with = "serialize_pathbuf",
        deserialize_with = "deserialize_pathbuf"
    )]
    pub sqlite_path: PathBuf,

    /// Loran internal service communication shared secret
    ///
    /// Used for internal communication authentication between Loran services, such as between Issuer and Signer.
    /// This is a system-level internal authentication key, used only for inter-service communication, not for external business.
    ///
    /// Notes:
    /// - This key is for Loran internal services only
    /// - Not for Realm business or external API access
    /// - A strong random key should be used in production environments
    /// - Configure it with the `loran_shared_key` field
    #[serde(default)]
    pub loran_shared_key: String,

    /// Recording pipeline configuration (logs + tracing)
    ///
    /// Merges log and OpenTelemetry tracing configuration into a unified `recording` section for easy management.
    #[serde(default)]
    pub recording: RecordingConfig,

    /// Monitoring endpoints configuration
    ///
    /// Controls authentication for `/health` and `/metrics` endpoints.
    /// Supports IP whitelist (CIDR) and/or HTTP Basic Auth (htpasswd file).
    /// When both are configured, a request matching either is allowed (OR logic).
    /// Per-service overrides replace the global config for that service.
    #[serde(default)]
    pub monitoring: MonitoringConfig,
}

/// Recording pipeline configuration (logs + tracing)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecordingConfig {
    /// Global recording sink URI (single sink)
    ///
    /// Serves as the default sink for all channels; can be overridden by [recording.<channel>].
    ///
    /// Supported:
    /// - file://...
    /// - otlp+http://...
    /// - otlp+grpc://...
    #[serde(default)]
    pub sink: Option<String>,

    /// OTLP reporting service.name
    #[serde(default = "default_recording_service_name")]
    pub service_name: String,

    /// Observability channel configuration
    #[serde(default = "default_observability_channel")]
    pub observability: RecordingChannelConfig,
    /// Audit channel configuration
    #[serde(default = "default_audit_channel")]
    pub audit: RecordingChannelConfig,
    /// Security channel configuration
    #[serde(default = "default_security_channel")]
    pub security: RecordingChannelConfig,
    /// Operations channel configuration
    #[serde(default = "default_operations_channel")]
    pub operations: RecordingChannelConfig,

    /// OTLP TLS CA certificate path (PEM). When set, traces and logs exporters
    /// use TLS with this CA to verify the collector.
    #[serde(default)]
    pub ca_cert_path: Option<String>,

    /// Enable OTLP logs export (same OTLP endpoint as traces; the collector
    /// routes log records to the logs backend).
    #[serde(default)]
    pub logs_enabled: Option<bool>,
}

/// Single-channel configuration: semantic filter + sink override
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecordingChannelConfig {
    /// Channel semantic filter
    ///
    /// Each channel has its own filter vocabulary:
    /// - observability: off / digest / detailed / full
    /// - audit: off / mutations / all
    /// - security: off / critical / high / medium / all
    /// - operations: off / lifecycle / detailed
    #[serde(default)]
    pub filter: String,

    /// Recording sink URI (single sink)
    ///
    /// Supported:
    /// - file://...
    /// - otlp+http://...
    /// - otlp+grpc://...
    #[serde(default)]
    pub sink: Option<String>,
}

fn default_audit_channel() -> RecordingChannelConfig {
    RecordingChannelConfig {
        filter: "mutations".to_string(),
        sink: None,
    }
}

fn default_security_channel() -> RecordingChannelConfig {
    RecordingChannelConfig {
        filter: "all".to_string(),
        sink: None,
    }
}

fn default_operations_channel() -> RecordingChannelConfig {
    RecordingChannelConfig {
        filter: "lifecycle".to_string(),
        sink: None,
    }
}

fn default_observability_channel() -> RecordingChannelConfig {
    RecordingChannelConfig {
        filter: "digest".to_string(),
        sink: None,
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            sink: None,
            service_name: default_recording_service_name(),
            observability: default_observability_channel(),
            audit: default_audit_channel(),
            security: default_security_channel(),
            operations: default_operations_channel(),
            ca_cert_path: None,
            logs_enabled: None,
        }
    }
}

fn normalized_optional_uri(uri: &Option<String>) -> Option<String> {
    uri.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn any_recording_file_sink(recording: &RecordingConfig) -> bool {
    [
        &recording.sink,
        &recording.observability.sink,
        &recording.audit.sink,
        &recording.security.sink,
        &recording.operations.sink,
    ]
    .into_iter()
    .filter_map(normalized_optional_uri)
    .any(|sink| sink.starts_with("file://"))
}

fn validate_recording_sink_field(field: &str, value: &Option<String>, errors: &mut Vec<String>) {
    let Some(uri) = normalized_optional_uri(value) else {
        return;
    };

    let parsed = match Url::parse(&uri) {
        Ok(parsed) => parsed,
        Err(error) => {
            errors.push(format!(
                "Invalid URI in {field}: {uri} (parse error: {error})"
            ));
            return;
        }
    };

    match parsed.scheme() {
        "file" => {
            if parsed.to_file_path().is_err() {
                errors.push(format!(
                    "Invalid file URI in {field}: {uri} (cannot convert to local file path)"
                ));
            }
        }
        "otlp+http" | "otlp+grpc" => {
            if parsed.host_str().is_none() {
                errors.push(format!(
                    "Invalid OTLP URI in {field}: {uri} (host is required)"
                ));
            }
        }
        _ => {
            errors.push(format!(
                "Invalid URI scheme in {field}: expected file://, otlp+http:// or otlp+grpc://, got {}",
                parsed.scheme()
            ));
        }
    }
}

/// Monitoring endpoint auth configuration.
///
/// Both `allowed_ips` and `htpasswd_file` can be used simultaneously.
/// When both are configured, a request matching either is allowed (OR logic).
/// When neither is configured, endpoints are publicly accessible.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MonitoringConfig {
    /// IP whitelist in CIDR notation (e.g. "10.0.0.0/8", "127.0.0.1").
    /// A bare IP without prefix length implies /32 (IPv4) or /128 (IPv6).
    #[serde(default)]
    pub allowed_ips: Vec<String>,

    /// Path to an htpasswd-format file for HTTP Basic Auth.
    /// Each line: `username:password` (plaintext).
    #[serde(default)]
    pub htpasswd_file: String,

    /// Per-service overrides. When a service key is present here,
    /// it completely replaces the global config for that service.
    #[serde(default)]
    pub services: std::collections::HashMap<String, MonitoringServiceOverride>,
}

/// Per-service monitoring auth override.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MonitoringServiceOverride {
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub htpasswd_file: String,
}

impl MonitoringConfig {
    /// Returns true if no auth is configured (globally).
    pub fn is_open(&self) -> bool {
        self.allowed_ips.is_empty() && self.htpasswd_file.is_empty()
    }

    /// Get the effective auth config for a given service.
    /// Per-service override replaces global when present.
    pub fn effective_for(&self, service: &str) -> (&[String], &str) {
        if let Some(ovr) = self.services.get(service) {
            (ovr.allowed_ips.as_slice(), ovr.htpasswd_file.as_str())
        } else {
            (self.allowed_ips.as_slice(), self.htpasswd_file.as_str())
        }
    }

    /// Returns true if no auth is configured for the given service.
    pub fn is_open_for(&self, service: &str) -> bool {
        let (ips, htpasswd) = self.effective_for(service);
        ips.is_empty() && htpasswd.is_empty()
    }
}

fn default_enable() -> u8 {
    31 // Signaling(1) + STUN(2) + TURN(4) + Issuer(8) + Signer(16)
}

fn default_recording_service_name() -> String {
    "loran".to_string()
}

fn serialize_pathbuf<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    path.display().to_string().serialize(serializer)
}

fn deserialize_pathbuf<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(PathBuf::from(s))
}

impl Default for LoranConfig {
    fn default() -> Self {
        Self {
            enable: default_enable(), // Default: enable STUN + TURN
            name: "loran-default".to_string(),
            env: "dev".to_string(),
            user: None,
            group: None,
            pid: Some("logs/loran.pid".to_string()),
            bind: BindConfig::default(),
            authority: AuthorityConfig::default(),
            realm: RealmConfig::default(),
            turn: TurnConfig::default(),
            relay: RelayConfig::default(),
            location_tag: "default-location".to_string(),
            control: ControlConfig::default(),
            services: ServicesConfig::default(),
            sqlite_path: PathBuf::from("database"),
            loran_shared_key: generate_ephemeral_secret(),
            recording: RecordingConfig::default(),
            monitoring: MonitoringConfig::default(),
        }
    }
}

// Service enable flag bit constants
pub const ENABLE_SIGNALING: u8 = 0b00001;
pub const ENABLE_STUN: u8 = 0b00010;
pub const ENABLE_TURN: u8 = 0b00100;
pub const ENABLE_ISSUER: u8 = 0b01000;
pub const ENABLE_SIGNER: u8 = 0b10000;
pub const ENABLE_RELAY: u8 = 0b100000;

impl LoranConfig {
    /// HTTP business services and control endpoints require an HTTP listener.
    pub fn requires_http(&self) -> bool {
        self.is_signaling_enabled()
            || self.is_issuer_enabled()
            || self.is_signer_enabled()
            || self.is_relay_enabled()
            || self.admin_ui_enabled()
            || self.grpc_api_enabled()
    }

    /// The HTTP service stack uses persistent storage; UDP STUN/TURN does not.
    pub fn requires_database(&self) -> bool {
        if self.relay.auth == RelayAuthMode::Static
            && !self.is_signaling_enabled()
            && !self.is_issuer_enabled()
            && !self.is_signer_enabled()
            && !self.admin_ui_enabled()
            && !self.grpc_api_enabled()
            && !self.authority.enabled
            && self.services.packages.is_none()
            && (self.is_relay_enabled() || self.authority.uri.is_none())
        {
            return false;
        }
        self.bind.http.is_some() || self.requires_http()
    }

    /// Check if the signaling service is enabled
    ///
    /// Service is enabled if the ENABLE_SIGNALING bit is set in the enable bitmask.
    pub fn is_signaling_enabled(&self) -> bool {
        self.enable & ENABLE_SIGNALING != 0
    }

    /// Check whether the STUN service is enabled
    pub fn is_stun_enabled(&self) -> bool {
        self.enable & ENABLE_STUN != 0
    }

    /// Check whether the TURN service is enabled
    pub fn is_turn_enabled(&self) -> bool {
        self.enable & ENABLE_TURN != 0
    }

    /// Realm-scoped TURN credentials need both persistent state and an Actr
    /// authority. Independent REST and static credentials do not.
    pub fn uses_realm_turn_auth(&self) -> bool {
        self.is_turn_enabled()
            && self.turn.users.is_empty()
            && self.requires_database()
            && (self.is_issuer_enabled()
                || self.is_signaling_enabled()
                || self.authority.uri.is_some())
    }

    pub fn is_relay_enabled(&self) -> bool {
        self.enable & ENABLE_RELAY != 0
    }

    /// Check if the AIS (AId Issue Service) identity service is enabled
    ///
    /// Service is enabled if the ENABLE_ISSUER bit is set in the enable bitmask.
    pub fn is_issuer_enabled(&self) -> bool {
        self.enable & ENABLE_ISSUER != 0
    }

    /// Check whether the Signer key service is enabled
    ///
    /// Service is enabled if the ENABLE_SIGNER bit is set in the enable bitmask.
    pub fn is_signer_enabled(&self) -> bool {
        self.enable & ENABLE_SIGNER != 0
    }

    /// Resolve the bind IP for the dedicated Signer gRPC listener (SGN-02).
    ///
    /// Uses `bind.signer.ip` when `[bind.signer]` is configured; otherwise
    /// defaults to loopback (`127.0.0.1`), matching the `expose = "loopback"`
    /// default.
    pub fn signer_bind_ip(&self) -> String {
        self.bind
            .signer
            .as_ref()
            .map(|s| s.ip.clone())
            .unwrap_or_else(|| "127.0.0.1".to_string())
    }

    /// Resolve the port for the dedicated Signer gRPC listener (SGN-02).
    ///
    /// Uses `bind.signer.port` when `[bind.signer]` is configured; otherwise
    /// defaults to `bind.http.port + 1`.
    pub fn signer_bind_port(&self) -> u16 {
        if let Some(ref signer) = self.bind.signer {
            return signer.port;
        }
        let http_port = self.bind.http.as_ref().map(|h| h.port).unwrap_or(8080);
        http_port.saturating_add(1)
    }

    /// Whether the dedicated Signer listener terminates TLS.
    ///
    /// The Signer listener serves TLS with mutual client-certificate
    /// verification exactly when `services.signer.mtls_enabled()` is true;
    /// otherwise it serves plaintext h2c for loopback callers.
    pub fn signer_listener_tls(&self) -> bool {
        self.services
            .signer
            .as_ref()
            .is_some_and(|s| s.mtls_enabled())
    }

    /// Domain name used for Signer TLS certificate verification by co-located
    /// clients. Prefers `bind.signer.domain_name`, falling back to the shared
    /// `bind.http.domain_name`.
    pub fn signer_tls_domain(&self) -> String {
        if let Some(ref signer) = self.bind.signer {
            return signer.domain_name.clone();
        }
        self.bind
            .http
            .as_ref()
            .map(|h| h.domain_name.clone())
            .unwrap_or_else(|| "localhost".to_string())
    }

    /// Endpoint that co-located clients (Issuer, Signaling) should dial to reach
    /// the local Signer gRPC surface on its dedicated listener.
    ///
    /// For the loopback plaintext default this is
    /// `http://127.0.0.1:{signer_port}`; when the listener terminates TLS it is
    /// `https://{domain}:{signer_port}`.
    pub fn signer_client_endpoint(&self) -> String {
        let port = self.signer_bind_port();
        if self.signer_listener_tls() {
            format!("https://{}:{}", self.signer_tls_domain(), port)
        } else {
            format!("http://127.0.0.1:{port}")
        }
    }

    /// Check whether ICE services (STUN or TURN) are enabled
    pub fn is_ice_enabled(&self) -> bool {
        self.is_stun_enabled() || self.is_turn_enabled()
    }

    /// Current control-plane head type
    pub fn control_head(&self) -> ControlHead {
        if self.control.grpc_api_enabled() && !self.control.admin_ui_enabled() {
            ControlHead::GrpcApi
        } else {
            ControlHead::AdminUi
        }
    }

    /// Whether Admin UI is enabled.
    pub fn admin_ui_enabled(&self) -> bool {
        self.control.admin_ui_enabled()
    }

    /// Whether the NodeAdminService gRPC API is enabled.
    pub fn grpc_api_enabled(&self) -> bool {
        self.control.grpc_api_enabled()
    }

    /// Whether superv takeover mode is active.
    pub fn superv_managed(&self) -> bool {
        self.control.superv_managed()
    }

    /// Get the PID file path, using the default value if not configured
    pub fn get_pid_path(&self) -> Option<String> {
        self.pid.clone().or_else(|| {
            // If pid is not configured, use the default value logs/loran.pid
            Some("logs/loran.pid".to_string())
        })
    }

    /// Get the Loran internal service communication shared secret
    ///
    /// This key is used for authenticated communication between Loran internal services,
    /// such as service calls between Issuer and Signer.
    ///
    /// Note: This key is for internal service communication only and must not be used for external business.
    pub fn get_loran_shared_key(&self) -> &str {
        &self.loran_shared_key
    }

    /// Returns a reference to the recording pipeline configuration
    pub fn recording_config(&self) -> &RecordingConfig {
        &self.recording
    }

    /// Returns the OTLP service.name
    pub fn recording_service_name(&self) -> &str {
        &self.recording.service_name
    }

    /// Get the composite log/trace filter level, preferring RUST_LOG.
    ///
    /// With semantic filters, channel targets are set to TRACE so that
    /// all events reach the recording layer (which applies its own gate).
    /// The base level stays `info` to suppress third-party noise.
    pub fn get_filter_level(&self) -> String {
        if let Ok(rust_log) = std::env::var("RUST_LOG") {
            let trimmed = rust_log.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }

        "info,loran::observability=trace,loran::audit=trace,loran::security=trace,loran::operations=trace".to_string()
    }

    /// Load configuration from a file
    pub fn from_file<P: AsRef<std::path::Path>>(
        path: P,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path_ref = path.as_ref();

        // Check if file exists
        if !path_ref.exists() {
            return Err(format!("Configuration file does not exist: {path_ref:?}").into());
        }

        // Check if path is a file, not a directory
        if !path_ref.is_file() {
            return Err(format!("Path is not a valid file: {path_ref:?}").into());
        }

        // Read file content
        let content = std::fs::read_to_string(path_ref)?;

        // Parse TOML content
        let config: LoranConfig = toml::from_str(&content)?;

        Ok(config)
    }

    /// Load configuration from a TOML string
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Serialize configuration to a TOML string
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Validate configuration
    ///
    /// Checks the validity of all configuration items, including:
    /// - Whether required fields are present
    /// - Whether value ranges are reasonable
    /// - Whether file paths are valid
    /// - Whether service configurations are consistent
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Validate bitmask value range (0-63, 6 bits)
        if self.enable > 63 {
            errors.push(format!(
                "Invalid enable bitmask value: {}. Must be between 0 and 63 (6 bits)",
                self.enable
            ));
        }

        // Validate instance name
        if self.name.trim().is_empty() {
            errors.push("Instance name cannot be empty".to_string());
        }

        // Validate environment
        if !["dev", "prod", "test"].contains(&self.env.as_str()) {
            errors.push(format!(
                "Invalid environment '{}', must be one of: dev, prod, test",
                self.env
            ));
        }

        if self.requires_http() && self.bind.http.is_none() {
            errors.push("Enabled HTTP services or control endpoints require bind.http".to_string());
        }

        if let Err(error) = self
            .relay
            .validate(self.is_relay_enabled(), self.env == "prod")
        {
            errors.push(error);
        }

        // Validate each channel's semantic filter (empty string = use channel default, skip validation)
        {
            let check = |name: &str, value: &str, valid: &[&str]| -> Option<String> {
                if value.is_empty() || valid.contains(&value) {
                    None
                } else {
                    Some(format!(
                        "Invalid recording.{name}.filter '{value}', must be one of: {}",
                        valid.join(", ")
                    ))
                }
            };
            if let Some(e) = check(
                "observability",
                &self.recording.observability.filter,
                &["off", "digest", "detailed", "full"],
            ) {
                errors.push(e);
            }
            if let Some(e) = check(
                "audit",
                &self.recording.audit.filter,
                &["off", "mutations", "all"],
            ) {
                errors.push(e);
            }
            if let Some(e) = check(
                "security",
                &self.recording.security.filter,
                &["off", "critical", "high", "medium", "all"],
            ) {
                errors.push(e);
            }
            if let Some(e) = check(
                "operations",
                &self.recording.operations.filter,
                &["off", "lifecycle", "detailed"],
            ) {
                errors.push(e);
            }
        }

        if self.recording.service_name.trim().is_empty() {
            errors.push("recording.service_name cannot be empty".to_string());
        }

        // Validate the new URI recording sink configuration (single sink, global + per-channel)
        validate_recording_sink_field("recording.sink", &self.recording.sink, &mut errors);
        validate_recording_sink_field(
            "recording.observability.sink",
            &self.recording.observability.sink,
            &mut errors,
        );
        validate_recording_sink_field(
            "recording.audit.sink",
            &self.recording.audit.sink,
            &mut errors,
        );
        validate_recording_sink_field(
            "recording.security.sink",
            &self.recording.security.sink,
            &mut errors,
        );
        validate_recording_sink_field(
            "recording.operations.sink",
            &self.recording.operations.sink,
            &mut errors,
        );

        if self.requires_database() {
            validate_secret_hygiene("loran_shared_key", &self.loran_shared_key, 32, &mut errors);
            if self
                .sqlite_path
                .to_str()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            {
                errors.push("SQLite database path cannot be empty".to_string());
            }
        }
        if self.is_issuer_enabled() || (self.is_turn_enabled() && self.turn.users.is_empty()) {
            validate_secret_hygiene("turn.turn_secret", &self.turn.turn_secret, 32, &mut errors);
        }
        if (self.is_issuer_enabled() || self.is_signaling_enabled() || self.authority.uri.is_some())
            && let Err(error) = self.authority.provider_id()
        {
            errors.push(error);
        }

        // Validate ICE configuration (if STUN or TURN is enabled)
        if self.is_ice_enabled() {
            if self.bind.ice.advertised_ip.trim().is_empty() {
                errors.push(
                    "bind.ice.advertised_ip is required when ICE services are enabled".to_string(),
                );
            }
            // Validate advertised_ip format
            if self
                .bind
                .ice
                .advertised_ip
                .parse::<std::net::IpAddr>()
                .is_err()
            {
                errors.push(format!(
                    "Invalid bind.ice.advertised_ip '{}', must be a valid IP address",
                    self.bind.ice.advertised_ip
                ));
            }
        }

        if self.is_turn_enabled() {
            if let Err(error) = self.turn.relay_ports() {
                errors.push(error);
            }
            if self.turn.users.is_empty() && self.turn.turn_secret.trim().is_empty() {
                errors.push("TURN shared secret or static users are required".to_string());
            }
            if self
                .turn
                .users
                .iter()
                .any(|(user, password)| user.is_empty() || password.is_empty())
            {
                errors.push("TURN static usernames and passwords must not be empty".to_string());
            }
        }

        // Validate TURN configuration (if enabled)
        if self.is_turn_enabled() && self.turn.realm.trim().is_empty() {
            errors.push("TURN realm is required when TURN is enabled".to_string());
        }
        if self.is_turn_enabled()
            && let Err(error) = self.turn.relay_port_bounds()
        {
            errors.push(error);
        }

        // Validate Signer configuration (if enabled)
        if self.is_signer_enabled() {
            if let Some(ref signer_cfg) = self.services.signer {
                if self.env == "prod" && signer_cfg.get_kek_source().is_none() {
                    errors.push(
                        "Production Signer requires kek_file, kek_env, or kek; plaintext private-key storage is forbidden"
                            .to_string(),
                    );
                }
                // Inline KEK material (Direct form) is a secret: apply the same
                // placeholder / length / entropy hygiene as other secrets. A KEK
                // encodes 32 bytes (44 base64 or 64 hex chars), so 32 is a safe
                // minimum-length floor that never rejects a well-formed value.
                // Messages spell out "KEK" so operators (and startup logs) tie
                // the failure back to the key-encryption key.
                if let Some(ref kek) = signer_cfg.kek {
                    if looks_like_placeholder_secret(kek) {
                        errors.push(
                            "services.signer.kek (inline KEK) must not use a documented placeholder or example value".to_string(),
                        );
                    }
                    if kek.len() < 32 {
                        errors.push(
                            "services.signer.kek (inline KEK) must be at least 32 characters"
                                .to_string(),
                        );
                    }
                    if looks_like_low_entropy_secret(kek) {
                        errors.push(
                            "services.signer.kek (inline KEK) has too few distinct characters; use a high-entropy random value".to_string(),
                        );
                    }
                }
                // Validate storage configuration.
                match signer_cfg.storage.backend {
                    StorageBackend::Sqlite => {}
                    StorageBackend::Postgres => {
                        if signer_cfg.storage.postgres.is_none() {
                            errors.push(
                                "Signer is configured to use PostgreSQL but postgres config is missing"
                                    .to_string(),
                            );
                        }
                    }
                }

                // SGN-05: the Postgres storage backend is out of scope for the
                // v1 Signer surface (v1 is scoped to SQLite per the accepted
                // design). Selecting it in prod is a hard, fail-closed error.
                if self.env == "prod" && signer_cfg.storage.backend == StorageBackend::Postgres {
                    errors.push(
                        "services.signer.storage.backend = \"postgres\" is not supported in v1; \
                         Signer must use the sqlite backend in prod"
                            .to_string(),
                    );
                }

                // SGN-02: fail-closed exposure gate. Serving the Signer gRPC
                // surface on the public listener in prod without mTLS is
                // rejected. `env` is owned here and threaded into the per-service
                // policy check on the live SignerServiceConfig.
                if let Err(error) = signer_cfg.validate_exposure(&self.env) {
                    errors.push(error);
                }

                // SGN-02: a bind/expose mismatch must not silently expose the
                // oracle. Even if `expose` is left at the loopback default, an
                // explicit non-loopback `bind.signer.ip` (including wildcard
                // addresses such as 0.0.0.0 / ::) effectively publishes the
                // Signer gRPC surface. Treat that as public: in prod it requires
                // mTLS, otherwise fail closed.
                if self.env == "prod"
                    && let Some(ref signer_bind) = self.bind.signer
                    && let Ok(ip) = signer_bind.ip.parse::<std::net::IpAddr>()
                    && !ip.is_loopback()
                    && !signer_cfg.mtls_enabled()
                {
                    errors.push(format!(
                        "bind.signer.ip '{}' is non-loopback (public) in prod but Signer mTLS \
                         is not enabled: set services.signer.require_mtls = true together with \
                         mtls_ca_cert, mtls_cert and mtls_key, or bind the Signer to a loopback \
                         address",
                        signer_bind.ip
                    ));
                }
            } else {
                // Signer bitmask is set but services.signer configuration is missing
                errors.push(
                    "Signer service is enabled (ENABLE_SIGNER bit is set) but services.signer configuration is missing".to_string(),
                );
            }
        }

        // Validate Issuer configuration (if enabled)
        if self.is_issuer_enabled() {
            // Check whether Signer configuration is available (explicit config or auto default)
            if let Some(ref issuer) = self.services.issuer {
                if issuer.server.worker_id > 31 {
                    errors.push(
                        "services.issuer.server.worker_id must be in the range 0..=31".to_string(),
                    );
                }
                if issuer.get_signer_client_config(self).is_none() {
                    errors.push(
                        "Issuer service is enabled but no Signer available: \
                        either configure services.issuer.dependencies.signer or enable local Signer service"
                            .to_string(),
                    );
                }
                // Validate renewal_token_secret configuration.
                if issuer.server.renewal_token_secret.is_empty() {
                    errors.push(
                        "services.issuer.server.renewal_token_secret is required".to_string(),
                    );
                } else {
                    // Placeholder check runs on the pre-decode string so example
                    // markers are caught even when the value is not valid base64.
                    if looks_like_placeholder_secret(&issuer.server.renewal_token_secret) {
                        errors.push(
                            "services.issuer.server.renewal_token_secret must not use a documented placeholder or example value"
                                .to_string(),
                        );
                    }
                    // CFG-06: entropy gate. The base64 / decoded-length check
                    // below still accepts a degenerate all-same-byte value (for
                    // example a base64 blob of zero bytes), so add the shared
                    // low-entropy gate on the raw string. It is a hard error in
                    // prod and a warning in dev, so shipped low-entropy example
                    // values still let local development boot.
                    if looks_like_low_entropy_secret(&issuer.server.renewal_token_secret) {
                        let msg = "services.issuer.server.renewal_token_secret has too few distinct characters; use a high-entropy random value";
                        if self.env == "prod" {
                            errors.push(msg.to_string());
                        } else {
                            errors.push(format!("Warning: {msg}"));
                        }
                    }
                    match base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &issuer.server.renewal_token_secret,
                    ) {
                        Ok(decoded) if decoded.len() >= 32 => {}
                        Ok(decoded) => {
                            errors.push(format!(
                                "services.issuer.server.renewal_token_secret must decode to \
                                 at least 32 bytes, got {} byte(s)",
                                decoded.len()
                            ));
                        }
                        Err(e) => {
                            errors.push(format!(
                                "services.issuer.server.renewal_token_secret is not valid \
                                 base64: {e}"
                            ));
                        }
                    }
                }
            } else {
                // Issuer bitmask is set but services.issuer configuration is missing
                errors.push(
                    "Issuer service is enabled (ENABLE_ISSUER bit is set) but services.issuer configuration is missing".to_string(),
                );
            }
        }

        // Validate Signaling configuration (if enabled)
        if self.is_signaling_enabled() {
            if let Some(ref signaling) = self.services.signaling {
                if signaling.dependencies.signer.is_none()
                    && !(self.is_signer_enabled() && self.services.signer.is_some())
                {
                    errors.push(
                    "Signaling dependencies.signer is not configured; enable Signer (bitmask + services.signer) to use local defaults"
                        .to_string(),
                );
                }

                if signaling.dependencies.issuer.is_none()
                    && !(self.is_issuer_enabled() && self.services.issuer.is_some())
                {
                    errors.push(
                    "Signaling dependencies.issuer is not configured; enable Issuer (bitmask + services.issuer) to use local defaults"
                        .to_string(),
                );
                }
            } else {
                errors.push(
                    "Signaling service is enabled (ENABLE_SIGNALING bit is set) but services.signaling configuration is missing"
                        .to_string(),
                );
            }
        }

        // Production environment additional checks
        if self.env == "prod" {
            // An HTTP listener in production must use HTTPS. UDP-only services
            // do not require an unrelated HTTP/TLS listener.
            if let Some(ref http) = self.bind.http {
                if !http.is_tls() {
                    errors.push(
                        "Production environment should enable HTTPS (configure cert and key in bind.http)".to_string(),
                    );
                }
            }

            // Production should configure at least one file:// recording sink.
            if !any_recording_file_sink(&self.recording) {
                errors.push("Warning: Production environment should configure recording.sink (file://...) or channel-specific recording.<channel>.sink".to_string());
            }

            // Monitoring endpoints (/health, /metrics) expose the node name,
            // version and full Prometheus metrics. In production they must not be
            // anonymous: require an IP allowlist or htpasswd, or force the
            // operator to make the exposure explicit.
            if self.bind.http.is_some()
                && self.monitoring.allowed_ips.is_empty()
                && self.monitoring.htpasswd_file.is_empty()
            {
                errors.push(
                    "Production environment requires monitoring authentication: set monitoring.allowed_ips or monitoring.htpasswd_file".to_string(),
                );
            }

            // CFG-05: outbound dependency endpoints must use HTTPS in prod.
            // bind.http (inbound) is forced to TLS above; these are the explicit
            // client endpoints this node dials. Loopback targets are exempt, and
            // the auto-discovered local-service fallbacks already point at
            // 127.0.0.1, so only operator-configured remote endpoints are gated.
            if let Some(ref issuer) = self.services.issuer {
                if let Some(ref signer) = issuer.dependencies.signer {
                    validate_prod_endpoint_tls(
                        "services.issuer.dependencies.signer.endpoint",
                        &signer.endpoint,
                        Some(signer.enable_tls),
                        &mut errors,
                    );
                }
            }
            if let Some(ref signaling) = self.services.signaling {
                if let Some(ref signer) = signaling.dependencies.signer {
                    validate_prod_endpoint_tls(
                        "services.signaling.dependencies.signer.endpoint",
                        &signer.endpoint,
                        Some(signer.enable_tls),
                        &mut errors,
                    );
                }
                if let Some(ref issuer) = signaling.dependencies.issuer {
                    validate_prod_endpoint_tls(
                        "services.signaling.dependencies.issuer.endpoint",
                        &issuer.endpoint,
                        None,
                        &mut errors,
                    );
                }
            }

            // CFG-07: production intends to run as root and drop privileges after
            // binding ports. validate() cannot observe the runtime uid, so this
            // is a warning rather than a hard error: without `user`, a root-run
            // daemon would keep root for its whole lifetime. The hard fail-closed
            // path lives in ProcessManager::drop_privileges (non-root + drop
            // configured => Err).
            if self.user.is_none() {
                errors.push(
                    "Warning: Production environment should set `user` (and optionally `group`) so the daemon drops root privileges after binding ports".to_string(),
                );
            }
        }

        if let Err(e) = self.control.validate() {
            errors.push(format!("Control configuration error: {e}"));
        }

        // CFG-06: the gRPC control shared_secret is a secret. Length (>=64) and
        // hex-format are enforced in ControlConfig::validate; add the placeholder
        // and entropy gates here (only meaningful when grpc_api is enabled and a
        // value is present).
        if self.control.grpc_api_enabled() && !self.control.grpc_api.shared_secret.is_empty() {
            let secret = &self.control.grpc_api.shared_secret;
            if looks_like_placeholder_secret(secret) {
                errors.push(
                    "control.grpc_api.shared_secret must not use a documented placeholder or example value".to_string(),
                );
            }
            if looks_like_low_entropy_secret(secret) {
                errors.push(
                    "control.grpc_api.shared_secret has too few distinct characters; use a high-entropy random value".to_string(),
                );
            }
        }

        // Argon2id parameter floor for the admin password hash (OWASP minimums:
        // m>=19456 KiB, t>=2, p>=1). A weak hash is trivially crackable if the
        // config/DB leaks. Hard error in production; a warning in dev so weak
        // local hashes still boot. (Algorithm/format are validated in control.)
        if !self.control.admin_ui.password_hash.is_empty() {
            if let Ok(parsed) = argon2::PasswordHash::new(&self.control.admin_ui.password_hash) {
                if let Ok(params) = argon2::Params::try_from(&parsed) {
                    if params.m_cost() < 19456 || params.t_cost() < 2 || params.p_cost() < 1 {
                        let msg = "control.admin_ui.password_hash uses Argon2id parameters below the OWASP minimum (m>=19456, t>=2, p>=1)";
                        if self.env == "prod" {
                            errors.push(msg.to_string());
                        } else {
                            errors.push(format!("Warning: {msg}"));
                        }
                    }
                }
            }
        }

        // CFG-06: mutual non-reuse across every present secret field. Runs last
        // so it sees the full set of secrets regardless of which services are
        // enabled. Hard error in prod, warning in dev.
        validate_secret_reuse(self, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn standalone_relay_needs_http_but_not_business_storage() {
        let mut config = super::LoranConfig::from_toml(include_str!(
            "../../../../loran/config/relay.example.toml"
        ))
        .unwrap();
        assert!(config.is_relay_enabled());
        assert!(config.requires_http());
        assert!(!config.requires_database());
        assert!(config.loran_shared_key.is_empty());
        assert!(config.validate().is_ok(), "{:?}", config.validate());
        let mut disabled = config.clone();
        disabled.enable = 0;
        assert!(!disabled.requires_database());
        assert!(disabled.validate().is_ok(), "{:?}", disabled.validate());
        disabled.authority.enabled = true;
        assert!(disabled.requires_database());
        disabled.authority.enabled = false;
        disabled.authority.uri = Some("actr://provider.example".into());
        assert!(disabled.requires_database());
        config.bind.http = None;
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("require bind.http"))
        );
    }

    #[test]
    fn relay_config_rejects_ambiguous_auth_and_insecure_advertisement() {
        let mut config = super::LoranConfig::from_toml(include_str!(
            "../../../../loran/config/relay.example.toml"
        ))
        .unwrap();
        config.relay.static_credentials[1].endpoint =
            config.relay.static_credentials[0].endpoint.clone();
        assert!(config.validate().is_err());
        config.relay.static_credentials.pop();
        config.relay.public_url = Some("ws://example.org/relay/ws".into());
        assert!(config.relay.validate(true, true).is_err());
        config.relay.public_url = Some("wss://example.org/relay/ws".into());
        assert!(config.relay.validate(true, true).is_ok());
        config.relay.auth = super::RelayAuthMode::Aid;
        assert!(config.requires_database());
        assert!(config.relay.validate(true, true).is_err());
    }
    #[test]
    fn udp_only_configuration_needs_no_control_plane_or_storage() {
        let example = include_str!("../../../../loran/config/ice.example.toml");
        let mut config = super::LoranConfig::from_toml(example).unwrap();
        for enable in [2, 4, 6] {
            config.enable = enable;
            assert!(config.bind.http.is_none());
            assert!(!config.requires_database());
            assert!(config.loran_shared_key.is_empty());
            assert!(config.sqlite_path.as_os_str().is_empty());
            let errors = config.validate().err().unwrap_or_default();
            assert!(
                errors.iter().all(|error| error.starts_with("Warning:")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn independent_rest_turn_does_not_require_a_realm_authority() {
        let mut config = super::LoranConfig::from_toml(include_str!(
            "../../../../loran/config/ice.example.toml"
        ))
        .unwrap();
        config.env = "dev".into();
        config.turn.users.clear();
        config.turn.turn_secret = "qQ3ry54gn6iDjz02rBGn3YRmxZv8bqsf1qB9Gcb2".into();
        assert!(!config.uses_realm_turn_auth());
        assert!(config.validate().is_ok());

        // An HTTP monitoring listener does not turn external REST
        // credentials into Realm-scoped credentials.
        config.bind.http = Some(Default::default());
        config.sqlite_path = "database".into();
        config.loran_shared_key = "b2xISzUdwmiURo7wKFL6K9tbkRHhTfM2E83r".into();
        assert!(config.requires_database());
        assert!(!config.uses_realm_turn_auth());
        assert!(config.validate().is_ok());

        config.authority.uri = Some("actr://provider.example".into());
        assert!(config.uses_realm_turn_auth());
        config.turn.users.insert("alice".into(), "secret".into());
        assert!(!config.uses_realm_turn_auth());
    }

    #[test]
    fn enabled_http_services_still_require_their_listener() {
        let example = include_str!("../../../../loran/config/ice.example.toml");
        let initial = super::LoranConfig::from_toml(example).unwrap();
        for enable in [1, 8, 16] {
            let mut config = initial.clone();
            config.enable = enable;
            assert!(config.requires_database());
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|error| error.contains("require bind.http"))
            );
        }
        let mut config = initial;
        config.control.admin_ui.enabled = true;
        assert!(config.requires_database());
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("require bind.http"))
        );
    }

    use super::*;
    use ::signer::SignerServiceConfig;

    fn valid_issuer_server_config() -> issuer::IssuerServerConfig {
        issuer::IssuerServerConfig {
            // High-entropy base64 secret decoding to 32 bytes; distinct from the
            // other test secrets so it passes the CFG-06 entropy and reuse gates.
            renewal_token_secret: "b2x+ISzUdwmiUR+Oe7w/KF+L6K9tbkRHhTfM2E83r/k=".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn example_config_loads_and_validates() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../loran/config/loran.example.toml");
        let cfg = LoranConfig::from_file(&path).expect("example config should parse");
        let errors = cfg
            .validate()
            .expect_err("example must fail until operators replace secret placeholders");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("loran_shared_key"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("turn.turn_secret"))
        );
        assert!(errors.iter().any(|error| error.contains("password_hash")));
        assert!(
            cfg.admin_ui_enabled(),
            "example should have admin_ui enabled"
        );
        assert!(
            !cfg.grpc_api_enabled(),
            "example should have grpc_api disabled by default"
        );
    }

    #[test]
    fn rust_example_config_is_valid_for_local_development() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/actr/rust/loran-config.example.toml");
        let cfg = LoranConfig::from_file(&path).expect("Rust example config should parse");
        // The dev example ships low-entropy / reused placeholder secrets, which
        // now surface as `Warning:`-prefixed entries. The runtime ignores those
        // (see loran-server main/manager), so the config still boots for local
        // development: assert only that it produces no hard (non-warning) error.
        if let Err(errors) = cfg.validate() {
            let hard: Vec<&String> = errors
                .iter()
                .filter(|error| !error.starts_with("Warning:"))
                .collect();
            assert!(
                hard.is_empty(),
                "Rust example config should have no hard validation errors, got: {hard:?}"
            );
        }
        assert_eq!(cfg.services.issuer.as_ref().unwrap().server.worker_id, 0);
    }

    #[test]
    fn test_default_config() {
        let config = LoranConfig::default();
        assert_eq!(config.enable, 31); // Default: original five services enabled
        assert_eq!(config.name, "loran-default");
        assert_eq!(config.env, "dev");
        assert!(config.is_signaling_enabled());
        assert!(config.is_stun_enabled());
        assert!(config.is_turn_enabled());
        assert!(config.is_issuer_enabled());
        assert!(config.is_signer_enabled());
        assert_eq!(config.recording.service_name, "loran");
        assert!(config.recording.sink.is_none());
        assert_eq!(config.recording.observability.filter, "digest");
        assert_eq!(config.recording.audit.filter, "mutations");
        assert_eq!(config.recording.security.filter, "all");
        assert_eq!(config.recording.operations.filter, "lifecycle");
    }

    #[test]
    fn test_recording_global_plus_spec_deserialize() {
        let mut config = LoranConfig::default();
        config.recording.sink = Some("file:///tmp/loran.log".to_string());
        config.recording.audit.sink = Some("otlp+http://127.0.0.1:4318/v1/logs".to_string());
        config.recording.security.sink = Some("otlp+grpc://127.0.0.1:4317".to_string());

        let toml = config.to_toml().expect("config should serialize");
        let parsed = LoranConfig::from_toml(&toml).expect("config should deserialize");
        assert_eq!(
            parsed.recording.sink.as_deref(),
            Some("file:///tmp/loran.log")
        );
        assert_eq!(
            parsed.recording.audit.sink.as_deref(),
            Some("otlp+http://127.0.0.1:4318/v1/logs")
        );
        assert_eq!(
            parsed.recording.security.sink.as_deref(),
            Some("otlp+grpc://127.0.0.1:4317")
        );
    }

    #[test]
    fn test_recording_sink_validation_rejects_invalid_scheme() {
        let mut config = LoranConfig::default();
        config.recording.sink = Some("http://127.0.0.1:4317".to_string());

        let errors = config.validate().expect_err("validation should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Invalid URI scheme in recording.sink")),
            "expected invalid scheme error, got: {errors:?}"
        );
    }

    #[test]
    fn test_recording_sink_validation_accepts_supported_schemes() {
        let mut config = LoranConfig {
            enable: 0,
            ..Default::default()
        };
        config.control.admin_ui.password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E".to_string();
        config.recording.sink = Some("file:///tmp/loran.log".to_string());
        config.recording.audit.sink = Some("otlp+http://127.0.0.1:4318/v1/logs".to_string());
        config.recording.security.sink = Some("otlp+grpc://127.0.0.1:4317".to_string());

        assert!(
            config.validate().is_ok(),
            "supported sink schemes should pass validation"
        );
    }

    #[test]
    fn test_recording_sink_validation_rejects_otlp_uri_without_host() {
        let mut config = LoranConfig::default();
        config.recording.sink = Some("otlp+grpc:///v1/traces".to_string());

        let errors = config.validate().expect_err("validation should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Invalid OTLP URI in recording.sink")),
            "expected missing-host error, got: {errors:?}"
        );
    }

    #[test]
    fn test_recording_sink_validation_rejects_invalid_file_uri() {
        let mut config = LoranConfig::default();
        config.recording.sink = Some("file://relative-path.log".to_string());

        let errors = config.validate().expect_err("validation should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Invalid file URI in recording.sink")),
            "expected invalid file-uri error, got: {errors:?}"
        );
    }

    #[test]
    fn test_toml_serialization() {
        let config = LoranConfig::default();
        let toml_str = config.to_toml().unwrap();
        assert!(toml_str.contains("enable = 31")); // original five services
        assert!(toml_str.contains("name = \"loran-default\""));
        assert!(toml_str.contains("loran_shared_key = \""));

        let parsed_config = LoranConfig::from_toml(&toml_str).unwrap();
        assert_eq!(parsed_config.enable, config.enable);
        assert_eq!(parsed_config.name, config.name);
        assert_eq!(parsed_config.loran_shared_key, config.loran_shared_key);
    }

    #[test]
    fn test_loran_shared_key() {
        let config = LoranConfig::default();
        assert!(config.get_loran_shared_key().len() >= 32);
        assert!(!looks_like_placeholder_secret(
            config.get_loran_shared_key()
        ));

        // Test custom shared key
        let mut custom_config = config;
        custom_config.loran_shared_key = "custom-shared-key".to_string();
        assert_eq!(custom_config.get_loran_shared_key(), "custom-shared-key");
    }

    #[test]
    fn test_service_flags() {
        let mut config = LoranConfig {
            enable: 0,
            ..LoranConfig::default()
        };

        // Test Signaling service: bitmask-only control
        // Case 1: Bitmask not set, service not enabled
        config.enable = 0;
        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies::default(),
        });
        assert!(!config.is_signaling_enabled());

        // Case 2: Bitmask set -> enabled (regardless of services.* config)
        config.enable = ENABLE_SIGNALING;
        assert!(config.is_signaling_enabled());

        // Case 3: Bitmask set, with services.* config -> still enabled
        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies::default(),
        });
        assert!(config.is_signaling_enabled());

        // Case 4: Bitmask set, no services.* config -> enabled
        config.services.signaling = None;
        assert!(config.is_signaling_enabled());

        // Test Issuer service: bitmask-only control
        config.enable = 0;
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies::default(),
        });
        assert!(!config.is_issuer_enabled());

        // Case 2: Bitmask set -> enabled (regardless of services.* config)
        config.enable = ENABLE_ISSUER;
        assert!(config.is_issuer_enabled());

        // Case 3: Bitmask set, with services.* config -> still enabled
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies::default(),
        });
        assert!(config.is_issuer_enabled());

        // Case 4: Bitmask set, no services.* config -> enabled
        config.services.issuer = None;
        assert!(config.is_issuer_enabled());

        // Test Signer service: bitmask-only control
        config.enable = 0;
        config.services.signer = Some(SignerServiceConfig {
            ..Default::default()
        });
        assert!(!config.is_signer_enabled());

        // Case 2: Bitmask set -> enabled (regardless of services.* config)
        config.enable = ENABLE_SIGNER;
        assert!(config.is_signer_enabled());

        // Case 3: Bitmask set, with services.* config -> still enabled
        config.services.signer = Some(SignerServiceConfig {
            ..Default::default()
        });
        assert!(config.is_signer_enabled());

        // Case 4: Bitmask set, no services.* config -> enabled
        config.services.signer = None;
        assert!(config.is_signer_enabled());

        // Test ICE services (STUN/TURN use bitmask only)
        config.enable = ENABLE_STUN;
        assert!(config.is_stun_enabled());
        assert!(config.is_ice_enabled());

        config.enable = ENABLE_TURN;
        assert!(config.is_turn_enabled());
        assert!(config.is_ice_enabled());
    }

    #[test]
    fn test_issuer_auto_signer_config() {
        let mut config = LoranConfig {
            enable: ENABLE_SIGNER | ENABLE_ISSUER,
            ..LoranConfig::default()
        };

        // Scenario 1: Local Signer enabled, Issuer does not configure a Signer client, should auto-use local Signer
        config.enable = ENABLE_SIGNER | ENABLE_ISSUER; // Enable both Signer and Issuer via bitmask
        config.services.signer = Some(SignerServiceConfig {
            ..Default::default()
        });

        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies { signer: None }, // Signer not configured
        });

        // Should be able to get the auto-generated Signer configuration
        let signer_config = config
            .services
            .issuer
            .as_ref()
            .unwrap()
            .get_signer_client_config(&config);
        assert!(signer_config.is_some());
        let signer_config = signer_config.unwrap();
        // SGN-02: Signer gRPC runs on its own dedicated listener, defaulting to
        // the HTTP port + 1 (8080 + 1 = 8081) when [bind.signer] is omitted.
        assert_eq!(signer_config.endpoint, "http://127.0.0.1:8081");

        // Scenario 2: Explicitly configured Signer client, should use the explicit configuration
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies {
                signer: Some(crate::config::signer::SignerClientConfig {
                    endpoint: "http://remote-signer:8080".to_string(),
                    timeout_seconds: 10,
                    enable_tls: false,
                    tls_domain: None,
                    ca_cert: None,
                    client_cert: None,
                    client_key: None,
                }),
            },
        });

        let signer_config = config
            .services
            .issuer
            .as_ref()
            .unwrap()
            .get_signer_client_config(&config);
        assert!(signer_config.is_some());
        let signer_config = signer_config.unwrap();
        assert_eq!(signer_config.endpoint, "http://remote-signer:8080"); // Uses explicit configuration

        // Scenario 3: No local Signer and no explicit configuration, should return None
        config.services.signer = None;
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies { signer: None },
        });
        config.enable = ENABLE_ISSUER; // Enable Issuer via bitmask

        let signer_config = config
            .services
            .issuer
            .as_ref()
            .unwrap()
            .get_signer_client_config(&config);
        assert!(signer_config.is_none());
    }

    #[test]
    fn test_signaling_auto_signer_config() {
        let mut config = LoranConfig {
            enable: ENABLE_SIGNER | ENABLE_SIGNALING,
            ..LoranConfig::default()
        };

        // Enable local Signer and Signaling
        config.enable = ENABLE_SIGNER | ENABLE_SIGNALING; // Enable both via bitmask
        config.services.signer = Some(SignerServiceConfig {
            ..Default::default()
        });

        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies {
                signer: None,
                issuer: None,
            },
        });

        // Signaling should be able to get the auto-generated Signer configuration
        let signer_config = config
            .services
            .signaling
            .as_ref()
            .unwrap()
            .get_signer_client_config(&config);
        assert!(signer_config.is_some());
        let signer_config = signer_config.unwrap();
        // SGN-02: Signer gRPC runs on its own dedicated listener, defaulting to
        // the HTTP port + 1 (8080 + 1 = 8081) when [bind.signer] is omitted.
        assert_eq!(signer_config.endpoint, "http://127.0.0.1:8081");
    }

    #[test]
    fn test_validate_bitmask_consistency() {
        let mut config = LoranConfig {
            enable: ENABLE_ISSUER,
            ..LoranConfig::default()
        };

        // Case 1: Issuer bitmask set but services.issuer config missing - should warn
        config.enable = ENABLE_ISSUER;
        config.services.issuer = None;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains(
            "Issuer service is enabled (ENABLE_ISSUER bit is set) but services.issuer configuration is missing"
        )));

        // Case 2: Signer bitmask set but services.signer config missing - should warn
        config.enable = ENABLE_SIGNER;
        config.services.issuer = None;
        config.services.signer = None;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| {
            e.contains("Signer service is enabled (ENABLE_SIGNER bit is set) but services.signer configuration is missing")
        }));

        // Case 3: Signaling bitmask set but services.signaling config missing - should error
        config.enable = ENABLE_SIGNALING;
        config.services.signaling = None;
        config.services.signer = None;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| {
            e.contains("Signaling service is enabled (ENABLE_SIGNALING bit is set) but services.signaling configuration is missing")
        }));

        // Case 4: Signaling enabled with config present - should pass (if other validations pass)
        // Note: Unlike Issuer, Signaling validation does not check Signer availability
        config.enable = ENABLE_SIGNALING;
        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies {
                signer: None,
                issuer: None,
            },
        });
        config.services.signer = None; // No local Signer

        let result = config.validate();
        // Signaling with config should not have bitmask consistency errors
        // (may have other validation errors like sqlite_path, etc.)
        if let Err(errors) = result {
            assert!(
                !errors
                    .iter()
                    .any(|e| e.contains("Signaling service is enabled (ENABLE_SIGNALING bit is set) but services.signaling configuration is missing"))
            );
            assert!(
                !errors
                    .iter()
                    .any(|e| e.contains("bit is not set in enable bitmask"))
            );
        }

        // Case 5: Bitmask set and services.* config present - should pass (if other validations pass)
        config.enable = ENABLE_ISSUER | ENABLE_SIGNER;
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies {
                signer: Some(crate::config::signer::SignerClientConfig {
                    endpoint: "http://127.0.0.1:8080".to_string(),
                    timeout_seconds: 10,
                    enable_tls: false,
                    tls_domain: None,
                    ca_cert: None,
                    client_cert: None,
                    client_key: None,
                }),
            },
        });
        config.services.signer = Some(SignerServiceConfig {
            storage: ::signer::storage::StorageConfig {
                backend: ::signer::storage::StorageBackend::Sqlite,
                sqlite: Some(::signer::storage::SqliteConfig::default()),
                postgres: None,
            },
            kek: None,
            kek_env: None,
            kek_file: None,
            ..Default::default()
        });

        // Should not have bitmask consistency errors (may have other validation errors)
        let result = config.validate();
        if let Err(errors) = result {
            assert!(
                !errors
                    .iter()
                    .any(|e| e.contains("bit is not set in enable bitmask"))
            );
        }
    }

    #[test]
    fn test_signaling_dependencies_require_local_services_when_missing_clients() {
        let mut config = LoranConfig {
            enable: ENABLE_SIGNALING,
            ..LoranConfig::default()
        };
        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies::default(),
        });
        config.services.signer = None;
        config.services.issuer = None;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Signaling dependencies.signer is not configured"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Signaling dependencies.issuer is not configured"))
        );
    }

    #[test]
    fn test_signaling_dependencies_accept_local_services_when_enabled() {
        let mut config = LoranConfig {
            enable: ENABLE_SIGNALING | ENABLE_SIGNER | ENABLE_ISSUER,
            ..LoranConfig::default()
        };
        config.control.admin_ui.password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E".to_string();
        config.turn.turn_secret = "test-turn-secret-32-bytes-minimum".to_string();
        config.services.signaling = Some(SignalingConfig {
            server: signaling::SignalingServerConfig::default(),
            dependencies: signaling::SignalingDependencies::default(),
        });
        config.services.signer = Some(SignerServiceConfig {
            ..Default::default()
        });
        config.services.issuer = Some(IssuerServiceConfig {
            server: valid_issuer_server_config(),
            dependencies: issuer::IssuerDependencies::default(),
        });

        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("authority.uri")),
            "{errors:?}"
        );

        config.authority.uri = Some("actr://provider.example".to_string());
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn production_signer_requires_private_key_encryption() {
        let mut config = LoranConfig {
            env: "prod".to_string(),
            enable: ENABLE_SIGNER,
            ..LoranConfig::default()
        };
        config.control.admin_ui.password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E".to_string();
        config.services.signer = Some(SignerServiceConfig::default());

        let errors = config
            .validate()
            .expect_err("production Signer must fail closed without a KEK");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("plaintext private-key storage is forbidden"))
        );

        config.services.signer.as_mut().unwrap().kek_env = Some("LORAN_SIGNER_KEK".to_string());
        let remaining_errors = config.validate().err().unwrap_or_default();
        assert!(
            !remaining_errors
                .iter()
                .any(|error| error.contains("plaintext private-key storage is forbidden"))
        );
    }
}
