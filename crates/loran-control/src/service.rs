use crate::error::Result as AdminResult;
use crate::metrics::collect_system_metrics;
use crate::realm::realm_to_proto;
use chrono::Utc;
use issuer::realm_profile::{ProfileChange, ProfileError, RealmProfiles};
use loran_proto::NodeAdminService;
use loran_proto::{
    ConfigOverrideEntry as ProtoConfigOverrideEntry, ConfigType, CreateRealmRequest,
    CreateRealmResponse, DeleteConfigOverrideRequest, DeleteConfigOverrideResponse,
    GetConfigRequest, GetConfigResponse, GetNodeInfoRequest, GetNodeInfoResponse, GetRealmRequest,
    GetRealmResponse, ListConfigOverridesRequest, ListConfigOverridesResponse, ListRealmsRequest,
    ListRealmsResponse, RealmInfo, RetireRealmRequest, RetireRealmResponse, ServiceStatus,
    SetConfigOverrideRequest, SetConfigOverrideResponse, ShutdownRequest, ShutdownResponse,
    SystemMetrics, UpdateConfigRequest, UpdateConfigResponse, UpdateRealmRequest,
    UpdateRealmResponse,
};
use platform::ServiceCollector;
use platform::config::LoranConfig;
use platform::config::config_store::{ConfigOverride, ConfigOverrideStore};
use platform::config::registry;
use platform::config::resolver::{self, ResolvedField};
use platform::realm::membership::{self, BY_OPERATOR, IssuedInvite};
use platform::realm::{Realm, RealmError, RealmProfileRecord, RealmStatus};
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};

/// A created Realm: the response and its owner invite, returned once.
pub type CreatedRealm = (CreateRealmResponse, Option<IssuedInvite>);
type MetricsFuture = Pin<Box<dyn Future<Output = AdminResult<SystemMetrics>> + Send>>;
type MetricsProvider = Arc<dyn Fn() -> MetricsFuture + Send + Sync>;
type ShutdownFuture = Pin<Box<dyn Future<Output = AdminResult<()>> + Send>>;
type ShutdownHandler =
    Arc<dyn Fn(bool, Option<i32>, Option<String>) -> ShutdownFuture + Send + Sync>;
type ReloadFuture = Pin<Box<dyn Future<Output = AdminResult<bool>> + Send>>;
type ReloadHandler = Arc<dyn Fn() -> ReloadFuture + Send + Sync>;
type GrpcResult<T> = std::result::Result<T, Status>;

/// Placeholder written in place of secret values returned to the Admin UI.
/// A save request that echoes this placeholder back is treated as "unchanged"
/// and the original on-disk value is preserved.
const SECRET_MASK: &str = "***";

#[derive(Hash, Eq, PartialEq, Clone)]
struct ConfigKey {
    config_type: i32,
    key: String,
}

/// AdminApiService — canonical business-logic layer for node administration.
///
/// Both the REST/BFF (admin_api.rs) and the gRPC NodeAdminService trait call
/// into the `*_direct()` methods defined here.  REST handlers should be thin
/// wrappers that parse the request and format the JSON response.
#[derive(Clone)]
pub struct AdminApiService {
    node_id: String,
    name: String,
    location_tag: String,
    version: String,
    config_store: Arc<RwLock<HashMap<ConfigKey, String>>>,
    /// SQLite-backed config override store (L2 dynamic overrides).
    /// Present when running in AdminUi head mode.
    override_store: Option<Arc<ConfigOverrideStore>>,
    metrics_provider: MetricsProvider,
    shutdown_handler: Option<ShutdownHandler>,
    reload_handler: Option<ReloadHandler>,
    service_collector: ServiceCollector,
    started_at: Instant,
    /// Runtime config snapshot (set in AdminUi head mode).
    running_config: Option<LoranConfig>,
    /// Raw TOML content for L1 detection (resolver).
    toml_content: Option<String>,
    /// Path to config.toml on disk.
    config_path: Option<PathBuf>,
    /// Shared issuer (present when Issuer is enabled on this node). The same
    /// Arc backs the public /issuer router, so rotation here affects live traffic.
    aid_issuer: Option<Arc<issuer::AIdIssuer>>,
    /// Signs Realm Profiles when this Loran is the Actr Facility Provider for its own
    /// domain.
    realm_profiles: Option<Arc<RealmProfiles>>,
}

impl AdminApiService {
    /// Create a new control gRPC service instance.
    pub fn new(
        node_id: impl Into<String>,
        name: impl Into<String>,
        location_tag: impl Into<String>,
        version: impl Into<String>,
        service_collector: ServiceCollector,
    ) -> AdminResult<Self> {
        Ok(Self {
            node_id: node_id.into(),
            name: name.into(),
            location_tag: location_tag.into(),
            version: version.into(),
            config_store: Arc::new(RwLock::new(HashMap::new())),
            override_store: None,
            metrics_provider: Arc::new(|| Box::pin(async { collect_system_metrics().await })),
            shutdown_handler: None,
            reload_handler: None,
            service_collector,
            started_at: Instant::now(),
            running_config: None,
            toml_content: None,
            config_path: None,
            aid_issuer: None,
            realm_profiles: None,
        })
    }

    /// Override the metrics provider used by GetNodeInfo.
    pub fn with_metrics_provider<F, Fut>(mut self, provider: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AdminResult<SystemMetrics>> + Send + 'static,
    {
        self.metrics_provider = Arc::new(move || {
            let fut = provider();
            Box::pin(fut)
        });
        self
    }

    /// Attach a shutdown handler invoked when Shutdown is accepted.
    pub fn with_shutdown_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(bool, Option<i32>, Option<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AdminResult<()>> + Send + 'static,
    {
        self.shutdown_handler = Some(Arc::new(move |graceful, timeout, reason| {
            let fut = handler(graceful, timeout, reason);
            Box::pin(fut)
        }));
        self
    }

    /// Attach a SQLite-backed config override store for L2 dynamic overrides.
    pub fn with_override_store(mut self, store: Arc<ConfigOverrideStore>) -> Self {
        self.override_store = Some(store);
        self
    }

    /// Attach a reload handler invoked when reload is requested.
    pub fn with_reload_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AdminResult<bool>> + Send + 'static,
    {
        self.reload_handler = Some(Arc::new(move || {
            let fut = handler();
            Box::pin(fut)
        }));
        self
    }

    /// Attach the runtime config snapshot (AdminUi head mode).
    pub fn with_running_config(mut self, config: LoranConfig) -> Self {
        self.running_config = Some(config);
        self
    }

    /// Attach the raw TOML content for L1 detection.
    pub fn with_toml_content(mut self, content: String) -> Self {
        self.toml_content = Some(content);
        self
    }

    /// Attach the config file path.
    pub fn with_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    /// Attach the shared issuer so key rotation can be triggered through
    /// the control plane. Must be the same issuer that backs the public /issuer
    /// router; callers construct it once per router build.
    pub fn with_aid_issuer(mut self, issuer: Arc<issuer::AIdIssuer>) -> Self {
        self.aid_issuer = Some(issuer);
        self
    }

    /// Get the override store reference.
    /// Attach the Realm Profile signer of this Actr Facility Provider.
    pub fn with_realm_profiles(mut self, profiles: Arc<RealmProfiles>) -> Self {
        self.realm_profiles = Some(profiles);
        self
    }

    /// Realm Profiles of `realm`'s domain are signed here.
    fn signs_profiles_for(&self, realm: &str) -> Option<&Arc<RealmProfiles>> {
        self.realm_profiles
            .as_ref()
            .filter(|profiles| profiles.holds_key_for(realm))
    }

    pub fn override_store(&self) -> Option<&Arc<ConfigOverrideStore>> {
        self.override_store.as_ref()
    }

    fn build_config_key(config_type: ConfigType, key: String) -> ConfigKey {
        ConfigKey {
            config_type: config_type as i32,
            key,
        }
    }

    async fn load_realm(&self, realm: &str) -> GrpcResult<Realm> {
        let found = Realm::get(realm)
            .await
            .map_err(|e| Status::internal(format!("Failed to load realm: {e}")))?;

        found.ok_or_else(|| Status::not_found(format!("Realm not found: {realm}")))
    }

    async fn collect_metrics(&self) -> GrpcResult<SystemMetrics> {
        (self.metrics_provider)()
            .await
            .map_err(|e| Status::internal(format!("Failed to collect metrics: {e}")))
    }

    /// Collect current service statuses for this node.
    ///
    /// Returns all service statuses from the service registry.
    pub async fn service_statuses(&self) -> Vec<ServiceStatus> {
        self.service_collector.all_statuses().await
    }

    // ── Helper: get overrides list ──────────────────────────────────

    async fn overrides_list(&self) -> Vec<ConfigOverride> {
        if let Some(store) = &self.override_store {
            store.list_all().await.unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn toml_content_ref(&self) -> &str {
        self.toml_content.as_deref().unwrap_or("")
    }

    fn require_running_config(&self) -> GrpcResult<&LoranConfig> {
        self.running_config
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Running config not available"))
    }

    // ── Direct methods (transport-agnostic) ─────────────────────────

    /// Resolve the effective node name (L2 override → L1 config → running).
    pub async fn resolve_node_name(&self) -> String {
        let overrides = self.overrides_list().await;
        let fields = resolver::resolve_for_service("platform", self.toml_content_ref(), &overrides);

        fields
            .iter()
            .find(|f| f.key == "name")
            .map(|f| f.effective_value.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                self.running_config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| self.name.clone())
            })
    }

    /// Health check info.
    pub async fn health_info(&self) -> HealthInfo {
        let node = self.resolve_node_name().await;
        HealthInfo {
            status: "healthy".to_string(),
            node,
            version: self.version.clone(),
        }
    }

    /// List L2 dynamic config overrides.
    pub async fn list_overrides_direct(&self) -> GrpcResult<Vec<ConfigOverride>> {
        let store = self
            .override_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Override store not available"))?;
        store
            .list_all()
            .await
            .map_err(|e| Status::internal(format!("Failed to list overrides: {e}")))
    }

    /// Set a dynamic config override.
    pub async fn set_override_direct(&self, key: &str, value: &str, by: &str) -> GrpcResult<()> {
        let store = self
            .override_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Override store not available"))?;
        store
            .set(key, value, by)
            .await
            .map_err(|e| Status::invalid_argument(e.to_string()))
    }

    /// Delete a dynamic config override.
    pub async fn delete_override_direct(&self, key: &str) -> GrpcResult<bool> {
        let store = self
            .override_store
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Override store not available"))?;
        store
            .delete(key)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete override: {e}")))
    }

    /// Get platform detail with resolved config fields.
    pub async fn get_platform_detail_direct(&self) -> GrpcResult<PlatformDetail> {
        let cfg = self.require_running_config()?;

        let http_bind = cfg.bind.http.as_ref().map(|h| {
            serde_json::json!({
                "ip": h.ip, "port": h.port,
                "domain_name": h.domain_name, "advertised_ip": h.advertised_ip,
                "advertised_port": h.effective_advertised_port(),
                "cert": h.cert, "key": h.key,
                "is_tls": h.is_tls(),
            })
        });

        let config = serde_json::json!({
            "enable": cfg.enable,
            "name": cfg.name,
            "env": cfg.env,
            "location_tag": cfg.location_tag,
            "sqlite_path": cfg.sqlite_path.display().to_string(),
            "bind": {
                "http": http_bind,
                "ice": {
                    "ip": cfg.bind.ice.ip.to_string(),
                    "port": cfg.bind.ice.port,
                    "advertised_ip": cfg.bind.ice.advertised_ip.to_string(),
                    "advertised_port": cfg.bind.ice.advertised_port,
                },
            },
            "turn": {
                "relay_port_range": cfg.turn.relay_port_range,
            },
            "recording": {
                "service_name": cfg.recording.service_name,
            },
            "control": {
                "head": cfg.control.head,
                "admin_ui": {
                    "session_expiry_secs": cfg.control.admin_ui.session_expiry_secs,
                },
            },
        });

        let overrides = self.overrides_list().await;
        let config_fields =
            resolver::resolve_for_service("platform", self.toml_content_ref(), &overrides);

        Ok(PlatformDetail {
            config,
            config_fields,
        })
    }

    /// Get service detail with resolved config fields.
    pub async fn get_service_detail_direct(&self, name: &str) -> GrpcResult<ServiceDetail> {
        let type_id = service_name_to_type_id(name)
            .ok_or_else(|| Status::not_found(format!("Unknown service: {name}")))?;

        let cfg = self.require_running_config()?;

        let enabled = match name {
            "stun" => cfg.is_stun_enabled(),
            "turn" => cfg.is_turn_enabled(),
            "signaling" => cfg.is_signaling_enabled(),
            "issuer" => cfg.is_issuer_enabled(),
            "signer" => cfg.is_signer_enabled(),
            _ => false,
        };

        let statuses = self.service_statuses().await;
        let status = statuses.iter().find(|s| s.r#type == type_id).map(|s| {
            serde_json::json!({
                "name": s.name,
                "type": s.r#type,
                "is_healthy": s.is_healthy,
                "active_connections": s.active_connections,
                "total_requests": s.total_requests,
                "failed_requests": s.failed_requests,
                "average_latency_ms": s.average_latency_ms,
                "url": s.url,
                "port": s.port,
                "domain": s.domain,
            })
        });

        let config: Option<JsonValue> = match name {
            "stun" => Some(serde_json::json!({
                "bind_ip": cfg.bind.ice.ip,
                "bind_port": cfg.bind.ice.port,
                "advertised_ip": cfg.bind.ice.advertised_ip,
                "advertised_port": cfg.bind.ice.advertised_port,
            })),
            "turn" => Some(serde_json::json!({
                "bind_ip": cfg.bind.ice.ip,
                "bind_port": cfg.bind.ice.port,
                "advertised_ip": cfg.bind.ice.advertised_ip,
                "advertised_port": cfg.bind.ice.advertised_port,
                "relay_port_range": cfg.turn.relay_port_range,
                "realm": cfg.turn.realm,
            })),
            "signaling" => cfg.services.signaling.as_ref().map(|s| {
                serde_json::json!({
                    "ws_path": s.server.ws_path,
                    "rate_limit": {
                        "connection": {
                            "enabled": s.server.rate_limit.connection.enabled,
                            "per_minute": s.server.rate_limit.connection.per_minute,
                            "burst_size": s.server.rate_limit.connection.burst_size,
                            "max_concurrent_per_ip": s.server.rate_limit.connection.max_concurrent_per_ip,
                        },
                        "message": {
                            "enabled": s.server.rate_limit.message.enabled,
                            "per_second": s.server.rate_limit.message.per_second,
                            "burst_size": s.server.rate_limit.message.burst_size,
                        }
                    },
                    "dependencies": {
                        "signer": s.dependencies.signer.as_ref().map(|k| serde_json::json!({"endpoint": k.endpoint})),
                        "issuer": s.dependencies.issuer.as_ref().map(|a| serde_json::json!({"endpoint": a.endpoint})),
                    }
                })
            }),
            "issuer" => cfg.services.issuer.as_ref().map(|a| {
                serde_json::json!({
                    "token_ttl_secs": a.server.token_ttl_secs,
                    "signaling_heartbeat_interval_secs": a.server.signaling_heartbeat_interval_secs,
                    "dependencies": {
                        "signer": a.dependencies.signer.as_ref().map(|k| serde_json::json!({"endpoint": k.endpoint})),
                    }
                })
            }),
            "signer" => cfg.services.signer.as_ref().map(|k| {
                serde_json::json!({
                    "storage_backend": format!("{:?}", k.storage.backend),
                })
            }),
            _ => None,
        };

        let config_fields = if !name.is_empty() {
            let overrides = self.overrides_list().await;
            Some(resolver::resolve_for_service(
                name,
                self.toml_content_ref(),
                &overrides,
            ))
        } else {
            None
        };

        Ok(ServiceDetail {
            enabled,
            status,
            config,
            config_fields,
        })
    }

    /// Get config registry (all field definitions).
    pub fn get_registry_direct(&self) -> &'static [registry::ConfigFieldDef] {
        registry::all_fields()
    }

    /// Read config file from disk.
    pub async fn get_config_file_direct(&self) -> GrpcResult<ConfigFileContent> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Config path not available"))?;
        let path_display = path.display().to_string();
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| Status::internal(format!("Failed to read config file: {e}")))?;
        // Mask secret fields so the raw config.toml never leaks credentials to
        // the Admin UI. A subsequent save that echoes the mask back is treated
        // as "unchanged" (see save_config_file_direct).
        Ok(ConfigFileContent {
            content: mask_secrets_toml(&content),
            path: path_display,
        })
    }

    /// Write config file to disk (with TOML validation).
    ///
    /// `superv_managed` reflects whether the realm is supervisor-managed (Admin
    /// UI read-only for the control plane). When true, any change to the
    /// `control.*` section is rejected so a config write + reload cannot
    /// re-enable the gRPC API and bypass the realm-write lock.
    pub async fn save_config_file_direct(
        &self,
        content: &str,
        superv_managed: bool,
    ) -> GrpcResult<()> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Config path not available"))?;

        // Read the current on-disk content so masked secret placeholders can be
        // restored to their real values. The Admin UI never receives real
        // secrets, so an untouched secret field arrives back as the mask.
        // A missing file is fine (first save: nothing to restore), but any
        // other read failure must abort the save -- proceeding with an empty
        // original would make restore_masked_secrets a no-op and persist the
        // `***` mask over the real secrets, destroying them.
        let original = match tokio::fs::read_to_string(path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(Status::internal(format!(
                    "Failed to read existing config file for secret restoration: {e}"
                )));
            }
        };

        // Restore masked secrets left untouched by the caller before parsing or
        // writing, so the mask never lands in the real config file.
        let content = restore_masked_secrets(content, &original);

        // Parse as TOML to validate syntax
        let new_config = LoranConfig::from_toml(&content)
            .map_err(|e| Status::invalid_argument(format!("Invalid TOML: {e}")))?;

        // Supervisor-managed: the control plane is off-limits to the local Admin
        // UI. Reject any change to control.* (compared after secret restoration
        // so an untouched shared_secret does not count as a change).
        if superv_managed && control_section_changed(&new_config, &original) {
            return Err(Status::permission_denied(
                "control.* section is read-only while the realm is supervisor-managed",
            ));
        }

        // Validate config semantics
        if let Err(errors) = new_config.validate() {
            let non_warnings: Vec<_> = errors
                .iter()
                .filter(|e| !e.starts_with("Warning:"))
                .cloned()
                .collect();
            if !non_warnings.is_empty() {
                return Err(Status::invalid_argument(format!(
                    "Validation failed: {}",
                    non_warnings.join("; ")
                )));
            }
        }

        tokio::fs::write(path, content.as_bytes())
            .await
            .map_err(|e| Status::internal(format!("Failed to write: {e}")))?;

        platform::recording::info!("Config file saved via admin API");
        Ok(())
    }

    /// Trigger reload (SIGHUP).
    pub async fn reload_direct(&self) -> GrpcResult<bool> {
        platform::recording::info!("Reload requested via admin API, sending SIGHUP to self");
        #[cfg(unix)]
        {
            let pid = std::process::id() as i32;
            // SAFETY: sending a signal to our own process is safe
            let ret = unsafe { nix::libc::kill(pid, nix::libc::SIGHUP) };
            if ret == 0 {
                Ok(true)
            } else {
                Err(Status::internal("Failed to send SIGHUP"))
            }
        }
        #[cfg(not(unix))]
        {
            Err(Status::unimplemented(
                "Reload not supported on this platform",
            ))
        }
    }

    /// Query Signer keys from the SQLite database.
    pub async fn get_signer_keys_direct(&self) -> GrpcResult<SignerKeysResult> {
        let cfg = self.require_running_config()?;
        let db_path = cfg.sqlite_path.join("signer_keys.db");
        if !db_path.exists() {
            return Ok(SignerKeysResult {
                keys: vec![],
                total_count: 0,
            });
        }

        let url = format!("sqlite:{}?mode=ro", db_path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .map_err(|e| Status::internal(format!("DB connect error: {e}")))?
            .read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| Status::internal(format!("DB pool error: {e}")))?;

        let rows = sqlx::query_as::<_, (i64, i32, i64, bool)>(
            "SELECT key_id, length(public_key) as pk_size, created_at, secret_key = '' AS retired \
             FROM keys ORDER BY created_at DESC LIMIT 5",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| Status::internal(format!("Query error: {e}")))?;

        let total_count = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM keys")
            .fetch_one(&pool)
            .await
            .map(|r| r.0)
            .unwrap_or(0);

        let keys = rows
            .iter()
            .map(|(key_id, pk_size, created_at, retired)| KeyInfo {
                key_id: *key_id,
                pk_size: *pk_size,
                created_at: *created_at,
                retired: *retired,
            })
            .collect();

        Ok(SignerKeysResult { keys, total_count })
    }

    /// Get node info including metrics and service statuses.
    pub async fn node_info_direct(&self) -> GrpcResult<GetNodeInfoResponse> {
        platform::recording::debug!("GetNodeInfo request received");

        let uptime_secs = self.started_at.elapsed().as_secs() as i64;
        let metrics = self.collect_metrics().await?;
        let services = self.service_statuses().await;

        Ok(GetNodeInfoResponse {
            success: true,
            error_message: None,
            node_id: self.node_id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            location_tag: self.location_tag.clone(),
            uptime_secs,
            current_metrics: Some(metrics),
            services,
        })
    }

    /// List all realms.
    pub async fn list_realms_direct(&self) -> GrpcResult<ListRealmsResponse> {
        platform::recording::debug!("ListRealms request received");

        let realms = Realm::get_all()
            .await
            .map_err(|e| Status::internal(format!("Failed to load realm list: {e}")))?;

        let realms_info: Vec<RealmInfo> = realms.iter().map(realm_to_proto).collect();
        let total_count = realms_info.len() as u32;

        Ok(ListRealmsResponse {
            success: true,
            error_message: None,
            realms: realms_info,
            next_page_token: None,
            total_count,
        })
    }

    fn realm_config(&self) -> platform::config::RealmConfig {
        self.running_config
            .as_ref()
            .map(|config| config.realm.clone())
            .unwrap_or_default()
    }

    fn max_realms(&self) -> Option<u32> {
        self.realm_config().max_realms
    }

    async fn create_local_realm_internal(
        &self,
        req: CreateRealmRequest,
        profile: ProfileChange,
    ) -> GrpcResult<CreatedRealm> {
        platform::recording::info!("Create local realm request received: realm={}", req.realm);
        let failed = |message: String| {
            Ok((
                CreateRealmResponse {
                    success: false,
                    error_message: Some(message),
                    realm: None,
                    owner_invite: None,
                },
                None,
            ))
        };

        // A Realm under a domain whose Realm signing key Loran holds starts
        // pending and becomes active with its first Loran-signed Profile.
        let signer = self.signs_profiles_for(&req.realm);
        match signer {
            Some(profiles) => {
                if profile.package.is_none() {
                    return failed(format!(
                        "Realms under {} need package {{href, sha256}} for their Realm Profile",
                        profiles.provider_id()
                    ));
                }
                if profiles.signing_keys().is_none() {
                    return Err(Status::unavailable("Realm signing key is not ready"));
                }
            }
            // Other domains' key holders create their Realms with Realm
            // management requests.
            None if self.realm_profiles.is_some() => {
                return failed(format!(
                    "this Actr Facility Provider creates Realms only under {}",
                    self.realm_profiles
                        .as_ref()
                        .map(|profiles| profiles.provider_id())
                        .unwrap_or_default()
                ));
            }
            None if profile.package.is_some() || profile.display_name.is_some() => {
                return failed(
                    "package and display_name belong to a Loran-signed Realm Profile, \
                     which only Realms under the Provider domain have"
                        .to_string(),
                );
            }
            None => {}
        }

        let realm = Realm {
            name: req.realm,
            status: if signer.is_some() {
                RealmStatus::Pending
            } else {
                RealmStatus::Active
            },
            enabled: req.enabled,
            expires_at: (req.expires_at > 0).then_some(req.expires_at),
            // Removed if the process stops before the first Profile is signed.
            pending_expires_at: signer.is_some().then(|| {
                chrono::Utc::now().timestamp() as u64 + self.realm_config().pending_ttl_secs()
            }),
            ..Default::default()
        };

        let mut realm = match realm.insert(self.max_realms()).await {
            Ok(r) => r,
            Err(err) => return failed(format!("Failed to create realm: {err}")),
        };
        if let Some(profiles) = signer {
            realm = match profiles.issue(&realm.name, profile).await {
                Ok(realm) => realm,
                Err(err) => {
                    // Never served, so the name is free again.
                    if let Err(discard) = Realm::discard_pending(&realm.name).await {
                        platform::recording::warn!(
                            "Pending realm {} not discarded: {discard}",
                            realm.name
                        );
                    }
                    return failed(format!("Realm Profile not signed: {err}"));
                }
            };
        }
        // A Realm created active gets its issuer key now; the others got it
        // with their first Realm Profile draft. Issuance creates a missing one.
        if signer.is_none()
            && let Some(issuer) = &self.aid_issuer
            && let Err(err) = issuer.realm_key(&realm.name).await
        {
            platform::recording::warn!("Realm {} has no issuer key yet: {err}", realm.name);
        }

        // The owner invite comes back with the creation, once; the operator
        // hands it to the owner.
        let owner_invite = match membership::issue_owner_invite(&realm.name, BY_OPERATOR).await {
            Ok(invite) => Some(invite),
            Err(err) => {
                platform::recording::warn!("Realm {} has no owner invite: {err}", realm.name);
                None
            }
        };

        platform::recording::info!("Realm created: {}", realm.name);

        Ok((
            CreateRealmResponse {
                success: true,
                error_message: None,
                realm: Some(realm_to_proto(&realm)),
                owner_invite: owner_invite.as_ref().map(|invite| invite.invite.clone()),
            },
            owner_invite,
        ))
    }

    async fn create_managed_realm_internal(
        &self,
        req: CreateRealmRequest,
    ) -> GrpcResult<CreateRealmResponse> {
        platform::recording::info!("Create managed realm request received: realm={}", req.realm);

        // As Actr Facility Provider, Loran creates Realms only under its Provider
        // domain, with a Realm Profile package that this request lacks.
        if self.realm_profiles.is_some() && Realm::get(&req.realm).await.ok().flatten().is_none() {
            return Ok(CreateRealmResponse {
                success: false,
                error_message: Some(
                    "a new Realm needs a Realm Profile: create Realms under the Provider \
                     domain through the admin API; other domains use Realm management requests"
                        .to_string(),
                ),
                realm: None,
                owner_invite: None,
            });
        }

        match Realm::upsert_managed(
            req.realm,
            req.enabled,
            (req.expires_at > 0).then_some(req.expires_at),
            self.max_realms(),
        )
        .await
        {
            Ok((realm, created)) => {
                // A new Realm's owner invite comes back once.
                let owner_invite = if created {
                    match membership::issue_owner_invite(&realm.name, BY_OPERATOR).await {
                        Ok(invite) => Some(invite.invite),
                        Err(err) => {
                            platform::recording::warn!(
                                "Realm {} has no owner invite: {err}",
                                realm.name
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                Ok(CreateRealmResponse {
                    success: true,
                    error_message: None,
                    realm: Some(realm_to_proto(&realm)),
                    owner_invite,
                })
            }
            Err(err) => Ok(CreateRealmResponse {
                success: false,
                error_message: Some(format!("Failed to upsert managed realm: {err}")),
                realm: None,
                owner_invite: None,
            }),
        }
    }

    /// Create a realm and return its owner invite once.
    ///
    /// `profile` carries the package and display name of the first Realm
    /// Profile, required and allowed only for Realms under the Provider
    /// domain. As Actr Facility Provider, Loran creates no Realm elsewhere.
    pub async fn create_realm_with_invite_direct(
        &self,
        req: CreateRealmRequest,
        profile: ProfileChange,
    ) -> GrpcResult<CreatedRealm> {
        self.create_local_realm_internal(req, profile).await
    }

    /// Create or update a superv-managed realm (gRPC path).
    pub async fn create_realm_direct(
        &self,
        req: CreateRealmRequest,
    ) -> GrpcResult<CreateRealmResponse> {
        self.create_managed_realm_internal(req).await
    }

    /// Get a single realm by name.
    pub async fn get_realm_direct(&self, realm: &str) -> GrpcResult<GetRealmResponse> {
        platform::recording::debug!("GetRealm request received: realm={}", realm);

        match self.load_realm(realm).await {
            Ok(realm) => Ok(GetRealmResponse {
                success: true,
                error_message: None,
                realm: Some(realm_to_proto(&realm)),
            }),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(GetRealmResponse {
                success: false,
                error_message: Some(status.message().to_string()),
                realm: None,
            }),
            Err(e) => Err(e),
        }
    }

    /// Update an existing realm.
    pub async fn update_realm_direct(
        &self,
        req: UpdateRealmRequest,
    ) -> GrpcResult<UpdateRealmResponse> {
        let mut realm = match self.load_realm(&req.realm).await {
            Ok(r) => r,
            Err(status) if status.code() == tonic::Code::NotFound => {
                return Ok(UpdateRealmResponse {
                    success: false,
                    error_message: Some(status.message().to_string()),
                    realm: None,
                });
            }
            Err(e) => return Err(e),
        };

        if let Some(enabled) = req.enabled {
            realm.enabled = enabled;
        }
        if let Some(expires_at) = req.expires_at {
            realm.expires_at = (expires_at > 0).then_some(expires_at);
        }
        if let Err(err) = realm.save().await {
            return Ok(UpdateRealmResponse {
                success: false,
                error_message: Some(format!("Failed to update realm: {err}")),
                realm: None,
            });
        }

        Ok(UpdateRealmResponse {
            success: true,
            error_message: None,
            realm: Some(realm_to_proto(&realm)),
        })
    }

    /// Retire a realm by name. The retired record stays as a tombstone.
    ///
    /// Under the Provider domain Loran signs the retired Realm Profile. A
    /// Realm with a Profile signed by another domain's key holder retires
    /// only when that holder publishes a retired Profile; the operator can
    /// suspend it instead.
    pub async fn retire_realm_direct(&self, realm: &str) -> GrpcResult<RetireRealmResponse> {
        platform::recording::info!("RetireRealm request received: realm={}", realm);
        let retired = match self.signs_profiles_for(realm) {
            Some(profiles) => profiles
                .issue(
                    realm,
                    ProfileChange {
                        retire: true,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|err| match err {
                    ProfileError::Realm(err) => err,
                    err => RealmError::ValidationError(err.to_string()),
                }),
            None => match RealmProfileRecord::get(realm).await {
                Ok(Some(_)) => Err(RealmError::ValidationError(
                    "only the Realm signing key holder retires this Realm, by publishing a \
                     retired Realm Profile; suspend it instead"
                        .to_string(),
                )),
                Ok(None) => Realm::retire(realm).await,
                Err(err) => Err(err),
            },
        };
        match retired {
            Ok(realm) => Ok(RetireRealmResponse {
                success: true,
                error_message: None,
                realm: Some(realm_to_proto(&realm)),
            }),
            Err(RealmError::NotFound) => {
                Err(Status::not_found(format!("Realm not found: {realm}")))
            }
            Err(err) => Ok(RetireRealmResponse {
                success: false,
                error_message: Some(format!("Failed to retire realm: {err}")),
                realm: None,
            }),
        }
    }

    /// Issue a new owner invite for an active Realm (owner recovery),
    /// revoking its unused owner invites. Only for Realms under the Provider
    /// domain and Realms without a Realm Profile; the Realm signing key
    /// holder recovers the others (`FailedPrecondition`).
    pub async fn issue_owner_invite_direct(&self, realm: &str) -> GrpcResult<IssuedInvite> {
        let has_profile = RealmProfileRecord::get(realm)
            .await
            .map_err(|e| Status::internal(format!("Failed to load Realm Profile: {e}")))?
            .is_some();
        if has_profile && self.signs_profiles_for(realm).is_none() {
            return Err(Status::failed_precondition(
                "the Realm signing key holder issues this Realm's owner invites",
            ));
        }
        membership::issue_owner_invite(realm, BY_OPERATOR)
            .await
            .map_err(|err| match err.code {
                membership::Code::NotFound => Status::not_found(err.message),
                membership::Code::Conflict => Status::invalid_argument(err.message),
                _ => Status::internal(format!("Failed to issue an owner invite: {err}")),
            })
    }

    /// Members, clients and open invites of `realm`, read-only.
    pub async fn list_realm_members_direct(&self, realm: &str) -> GrpcResult<JsonValue> {
        self.load_realm(realm).await?;
        membership::list_members(realm, None)
            .await
            .map_err(|err| Status::internal(format!("Failed to list members: {err}")))
    }

    /// Start rotating `realm`'s issuer key (`realm-profile.md` section 6.1);
    /// returns the new key id. Only when this Loran is an Actr Facility Provider.
    pub async fn rotate_realm_issuer_key_direct(&self, realm: &str) -> GrpcResult<u32> {
        let Some(profiles) = &self.realm_profiles else {
            return Err(Status::failed_precondition(
                "this Loran is not an Actr Facility Provider",
            ));
        };
        profiles
            .rotate_issuer_key(realm)
            .await
            .map_err(|err| match err {
                ProfileError::Realm(RealmError::NotFound) => {
                    Status::not_found(format!("Realm not found: {realm}"))
                }
                ProfileError::Realm(RealmError::Conflict(message))
                | ProfileError::Invalid(message) => Status::invalid_argument(message),
                err => Status::internal(format!("Failed to rotate the issuer key: {err}")),
            })
    }

    /// The accepted Realm Profile document of `realm`, as signed.
    pub async fn get_realm_profile_direct(&self, realm: &str) -> GrpcResult<Option<String>> {
        RealmProfileRecord::get(realm)
            .await
            .map(|record| record.map(|record| record.document))
            .map_err(|e| Status::internal(format!("Failed to load Realm Profile: {e}")))
    }

    /// Get a config value.
    pub async fn get_config_direct(
        &self,
        config_type: ConfigType,
        config_key: String,
    ) -> GrpcResult<GetConfigResponse> {
        let key = Self::build_config_key(config_type, config_key);
        let store = self.config_store.read().await;

        if let Some(value) = store.get(&key) {
            // Never expose secret values through the config read path.
            let config_value = if is_secret_config_key(&key.key) {
                SECRET_MASK.to_string()
            } else {
                value.clone()
            };
            Ok(GetConfigResponse {
                success: true,
                error_message: None,
                config_value: Some(config_value),
            })
        } else {
            Ok(GetConfigResponse {
                success: false,
                error_message: Some("Config not found".to_string()),
                config_value: None,
            })
        }
    }

    /// Update a config value.
    pub async fn update_config_direct(
        &self,
        config_type: ConfigType,
        config_key: String,
        config_value: String,
    ) -> GrpcResult<UpdateConfigResponse> {
        let key = Self::build_config_key(config_type, config_key);
        let mut store = self.config_store.write().await;
        let old_value = store.insert(key, config_value);

        Ok(UpdateConfigResponse {
            success: true,
            error_message: None,
            old_value,
        })
    }

    /// Request node shutdown.
    pub async fn shutdown_direct(
        &self,
        graceful: bool,
        timeout_secs: Option<i32>,
        reason: Option<String>,
    ) -> GrpcResult<ShutdownResponse> {
        if let Some(handler) = &self.shutdown_handler {
            if let Err(e) = handler(graceful, timeout_secs, reason.clone()).await {
                return Ok(ShutdownResponse {
                    accepted: false,
                    error_message: Some(format!("Shutdown handler failed: {e}")),
                    estimated_shutdown_time: None,
                });
            }
        } else {
            platform::recording::warn!("Shutdown requested but no handler registered");
        }

        let estimated = if graceful {
            timeout_secs.map(|v| Utc::now().timestamp() + v as i64)
        } else {
            Some(Utc::now().timestamp())
        };

        Ok(ShutdownResponse {
            accepted: true,
            error_message: None,
            estimated_shutdown_time: estimated,
        })
    }
}

// ── Response structs (transport-agnostic, Serialize) ─────────────

/// Health check response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthInfo {
    pub status: String,
    pub node: String,
    pub version: String,
}

/// Platform detail with resolved config fields.
#[derive(Debug, Clone, Serialize)]
pub struct PlatformDetail {
    pub config: JsonValue,
    pub config_fields: Vec<ResolvedField>,
}

/// Service detail with resolved config fields.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceDetail {
    pub enabled: bool,
    pub status: Option<JsonValue>,
    pub config: Option<JsonValue>,
    pub config_fields: Option<Vec<ResolvedField>>,
}

/// Config file content with path.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigFileContent {
    pub content: String,
    pub path: String,
}

/// A Signer key.
#[derive(Debug, Clone, Serialize)]
pub struct KeyInfo {
    pub key_id: i64,
    pub pk_size: i32,
    pub created_at: i64,
    /// The private key is destroyed; the key never signs again.
    pub retired: bool,
}

/// Signer keys query result.
#[derive(Debug, Clone, Serialize)]
pub struct SignerKeysResult {
    pub keys: Vec<KeyInfo>,
    pub total_count: i64,
}

/// Map service name string to the ResourceType integer used in ServiceStatus.type
fn service_name_to_type_id(name: &str) -> Option<i32> {
    match name {
        "stun" => Some(1),
        "turn" => Some(2),
        "signaling" => Some(3),
        "issuer" => Some(4),
        "signer" => Some(5),
        _ => None,
    }
}

// ── Secret masking helpers (ADM-02) ─────────────────────────────

/// Extract the section name from a TOML section-header line (`[a.b]` or
/// `[[a.b]]`), returning the dotted path inside the brackets.
fn toml_section_of(trimmed_line: &str) -> String {
    let start = trimmed_line.find('[').map(|i| i + 1).unwrap_or(0);
    let end = trimmed_line.rfind(']').unwrap_or(trimmed_line.len());
    if start <= end {
        trimmed_line[start..end]
            .trim_matches(|c| c == '[' || c == ']')
            .trim()
            .to_string()
    } else {
        String::new()
    }
}

/// Decide whether a `key = value` line within `section` holds a secret that
/// must be masked in the raw config.toml.
fn is_secret_field(section: &str, key: &str) -> bool {
    match key {
        "loran_shared_key" | "turn_secret" | "renewal_token_secret" => true,
        "shared_secret" | "password_hash" => section.contains("control"),
        "kek" => section.contains("signer"),
        other => other.starts_with("kek_") && section.contains("signer"),
    }
}

/// Decide whether a config key (possibly a dotted path) addresses a secret
/// value in the get_config read path.
fn is_secret_config_key(key: &str) -> bool {
    let last = key.rsplit(['.', '/']).next().unwrap_or(key);
    matches!(
        last,
        "loran_shared_key"
            | "shared_secret"
            | "password_hash"
            | "turn_secret"
            | "renewal_token_secret"
            | "kek"
    ) || last.starts_with("kek_")
}

/// Replace the value of every secret field in a raw config.toml with the mask
/// placeholder, preserving the key structure and everything else verbatim.
fn mask_secrets_toml(content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 16);
    let mut section = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            section = toml_section_of(trimmed);
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !trimmed.starts_with('#') {
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim();
                if is_secret_field(&section, key) {
                    out.push_str(&line[..=eq]);
                    out.push_str(" \"");
                    out.push_str(SECRET_MASK);
                    out.push('"');
                    out.push('\n');
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') {
        out.pop();
    }
    out
}

/// Collect the raw right-hand side of every secret field, keyed by
/// (section, key), from a raw config.toml.
fn collect_secret_values(content: &str) -> HashMap<(String, String), String> {
    let mut map = HashMap::new();
    let mut section = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            section = toml_section_of(trimmed);
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_string();
            if is_secret_field(&section, &key) {
                map.insert((section.clone(), key), line[eq + 1..].to_string());
            }
        }
    }
    map
}

/// Restore secret fields left as the mask placeholder in `new_content` to their
/// original values taken from `original_content`. Fields the caller actually
/// changed (value differs from the mask) are left untouched.
fn restore_masked_secrets(new_content: &str, original_content: &str) -> String {
    let originals = collect_secret_values(original_content);
    let mut out = String::with_capacity(new_content.len());
    let mut section = String::new();
    for line in new_content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            section = toml_section_of(trimmed);
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !trimmed.starts_with('#') {
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim();
                if is_secret_field(&section, key) {
                    let value = line[eq + 1..].trim().trim_matches('"');
                    if value == SECRET_MASK {
                        if let Some(orig_rhs) = originals.get(&(section.clone(), key.to_string())) {
                            out.push_str(&line[..=eq]);
                            out.push_str(orig_rhs);
                            out.push('\n');
                            continue;
                        }
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !new_content.ends_with('\n') {
        out.pop();
    }
    out
}

/// Compare the `control.*` section of a parsed config against the original
/// on-disk content. Returns true if it changed (or cannot be verified).
fn control_section_changed(new_config: &LoranConfig, original_content: &str) -> bool {
    let old_config = match LoranConfig::from_toml(original_content) {
        Ok(cfg) => cfg,
        // Cannot parse the previous file: fail safe by treating it as a change.
        Err(_) => return true,
    };
    match (
        serde_json::to_value(&new_config.control),
        serde_json::to_value(&old_config.control),
    ) {
        (Ok(new_control), Ok(old_control)) => new_control != old_control,
        // Serialization failure: fail safe.
        _ => true,
    }
}

#[tonic::async_trait]
impl NodeAdminService for AdminApiService {
    async fn update_config(
        &self,
        request: Request<UpdateConfigRequest>,
    ) -> GrpcResult<Response<UpdateConfigResponse>> {
        let req = request.into_inner();
        self.update_config_direct(req.config_type(), req.config_key, req.config_value)
            .await
            .map(Response::new)
    }

    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> GrpcResult<Response<GetConfigResponse>> {
        let req = request.into_inner();
        self.get_config_direct(req.config_type(), req.config_key)
            .await
            .map(Response::new)
    }

    async fn create_realm(
        &self,
        request: Request<CreateRealmRequest>,
    ) -> GrpcResult<Response<CreateRealmResponse>> {
        self.create_realm_direct(request.into_inner())
            .await
            .map(Response::new)
    }

    async fn get_realm(
        &self,
        request: Request<GetRealmRequest>,
    ) -> GrpcResult<Response<GetRealmResponse>> {
        let req = request.into_inner();
        self.get_realm_direct(&req.realm).await.map(Response::new)
    }

    async fn update_realm(
        &self,
        request: Request<UpdateRealmRequest>,
    ) -> GrpcResult<Response<UpdateRealmResponse>> {
        self.update_realm_direct(request.into_inner())
            .await
            .map(Response::new)
    }

    async fn retire_realm(
        &self,
        request: Request<RetireRealmRequest>,
    ) -> GrpcResult<Response<RetireRealmResponse>> {
        let req = request.into_inner();
        self.retire_realm_direct(&req.realm)
            .await
            .map(Response::new)
    }

    async fn list_realms(
        &self,
        request: Request<ListRealmsRequest>,
    ) -> GrpcResult<Response<ListRealmsResponse>> {
        let _req = request.into_inner();
        self.list_realms_direct().await.map(Response::new)
    }

    async fn get_node_info(
        &self,
        request: Request<GetNodeInfoRequest>,
    ) -> GrpcResult<Response<GetNodeInfoResponse>> {
        let _req = request.into_inner();
        self.node_info_direct().await.map(Response::new)
    }

    async fn shutdown(
        &self,
        request: Request<ShutdownRequest>,
    ) -> GrpcResult<Response<ShutdownResponse>> {
        let req = request.into_inner();
        self.shutdown_direct(req.graceful, req.timeout_secs, req.reason)
            .await
            .map(Response::new)
    }

    async fn list_config_overrides(
        &self,
        _request: Request<ListConfigOverridesRequest>,
    ) -> GrpcResult<Response<ListConfigOverridesResponse>> {
        match self.list_overrides_direct().await {
            Ok(overrides) => {
                let entries = overrides
                    .into_iter()
                    .map(|o| ProtoConfigOverrideEntry {
                        key_path: o.key_path,
                        value: o.value,
                        updated_at: o.updated_at,
                        updated_by: o.updated_by,
                    })
                    .collect();
                Ok(Response::new(ListConfigOverridesResponse {
                    success: true,
                    error_message: None,
                    overrides: entries,
                }))
            }
            Err(e) => Ok(Response::new(ListConfigOverridesResponse {
                success: false,
                error_message: Some(e.message().to_string()),
                overrides: vec![],
            })),
        }
    }

    async fn set_config_override(
        &self,
        request: Request<SetConfigOverrideRequest>,
    ) -> GrpcResult<Response<SetConfigOverrideResponse>> {
        let req = request.into_inner();
        let by = req.updated_by.as_deref().unwrap_or("admin");
        match self.set_override_direct(&req.key, &req.value, by).await {
            Ok(()) => Ok(Response::new(SetConfigOverrideResponse {
                success: true,
                error_message: None,
            })),
            Err(e) => Ok(Response::new(SetConfigOverrideResponse {
                success: false,
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn delete_config_override(
        &self,
        request: Request<DeleteConfigOverrideRequest>,
    ) -> GrpcResult<Response<DeleteConfigOverrideResponse>> {
        let req = request.into_inner();
        match self.delete_override_direct(&req.key).await {
            Ok(deleted) => Ok(Response::new(DeleteConfigOverrideResponse {
                success: true,
                error_message: None,
                deleted,
            })),
            Err(e) => Ok(Response::new(DeleteConfigOverrideResponse {
                success: false,
                error_message: Some(e.message().to_string()),
                deleted: false,
            })),
        }
    }
}
