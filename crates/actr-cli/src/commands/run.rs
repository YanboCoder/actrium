//! Run command implementation - Execute .actr packages

use crate::commands::runtime_state::{
    RuntimeRecord, RuntimeStateStore, absolutize_from_cwd, log_path_for_wid, resolve_hyper_dir,
};
use crate::core::{Command, CommandContext, CommandResult, ComponentType};
use crate::error::{ActrCliError, Result};
use async_trait::async_trait;
use chrono::Utc;
use clap::Args;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

const DETACHED_READY_TIMEOUT: Duration = Duration::from_secs(10);
const DETACHED_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Default filename for the runtime config file (`actr.toml`).
const DEFAULT_RUNTIME_CONFIG: &str = "actr.toml";

/// Resolve `path` against `base`: returns `path` if absolute, otherwise `base.join(path)`.
fn resolve_against(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[derive(Args)]
pub struct RunCommand {
    /// Runtime configuration file (defaults to ./actr.toml if not specified)
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Hyper data directory
    #[arg(long = "hyper-dir", value_name = "DIR")]
    pub hyper_dir: Option<PathBuf>,

    /// Run in detached mode (background)
    #[arg(short = 'd', long = "detach")]
    pub detach: bool,

    /// Internal flag used when the detached child re-executes this command.
    #[arg(long = "internal-detached-child", hide = true)]
    pub internal_detached_child: bool,

    /// Internal: WID passed from parent to detached child (or from start/restart for reuse).
    #[arg(long = "internal-wid", hide = true)]
    pub internal_wid: Option<String>,
    /// Run as a web server (serves static files + runtime config for browser-based actors)
    #[arg(long = "web")]
    pub web: bool,

    /// Override web server port (default from config or 8080)
    #[arg(long = "port", requires = "web")]
    pub port: Option<u16>,
}

#[async_trait]
impl Command for RunCommand {
    async fn execute(&self, _ctx: &CommandContext) -> anyhow::Result<CommandResult> {
        // The run command only supports packaged workloads via runtime config.
        if self.web {
            self.execute_web_mode().await?;
        } else {
            self.execute_package_mode().await?;
        }
        Ok(CommandResult::Success(String::new()))
    }

    fn required_components(&self) -> Vec<ComponentType> {
        vec![]
    }

    fn name(&self) -> &str {
        "run"
    }

    fn description(&self) -> &str {
        "Run a packaged workload"
    }
}

impl RunCommand {
    async fn execute_package_mode(&self) -> Result<()> {
        use actr_hyper::{WorkloadPackage, init_observability};

        info!("🚀 Starting packaged workload");

        // Resolve runtime config path: use the provided path or default to ./actr.toml.
        let config_path = self
            .config
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNTIME_CONFIG));

        // Check if the runtime config file exists.
        if !config_path.exists() {
            return Err(ActrCliError::command_error(format!(
                "Runtime config file not found: {}\n\n\
                 Create a runtime config file or specify one with -c/--config.",
                config_path.display()
            )));
        }

        let config_path = absolutize_from_cwd(&config_path)?;
        let hyper_dir = resolve_hyper_dir(Some(&config_path), self.hyper_dir.as_deref())?;

        if self.detach && !self.internal_detached_child {
            return self.spawn_detached_child(&config_path).await;
        }

        let detached_runtime = if self.internal_detached_child {
            Some(self.prepare_detached_child(&config_path).await?)
        } else {
            None
        };

        // 1. Resolve package path from the runtime config.
        let package_path = self.resolve_package_path(&config_path).await?;
        info!("📦 Loading package: {}", package_path.display());

        // 2. Load package bytes
        let package_bytes = tokio::fs::read(&package_path).await.map_err(|e| {
            ActrCliError::command_error(format!("Failed to read package file: {}", e))
        })?;
        let package = WorkloadPackage::new(package_bytes);

        // 3. Parse package manifest
        let manifest = package.manifest().map_err(|e| {
            ActrCliError::command_error(format!("Failed to parse package manifest: {}", e))
        })?;
        let package_info = self.build_package_info(&manifest);

        // 4. Load runtime configuration
        let config = actr_config::ConfigParser::from_runtime_file(&config_path, package_info)?;

        info!("📡 Signaling server: {}", config.signaling_url.as_str());
        info!("🔐 Trust anchors: {} configured", config.trust.len());

        // 6. Initialize observability
        let _obs_guard = init_observability(&config.observability).map_err(|e| {
            ActrCliError::command_error(format!("Failed to initialize observability: {}", e))
        })?;

        // 7. Initialize Hyper
        let hyper = self.init_hyper(&config, &package_path, &hyper_dir).await?;
        info!("✅ Hyper initialized");

        // 8. Node typestate chain: from_hyper → attach → register → start
        let issuer_endpoint = config.issuer_endpoint.clone();
        let attached = actr_hyper::Node::from_hyper(hyper, config.clone())
            .attach(&package)
            .await
            .map_err(|e| ActrCliError::command_error(format!("Failed to attach package: {}", e)))?;
        info!("✅ Package attached");

        let registered = attached.register(&issuer_endpoint).await.map_err(|e| {
            ActrCliError::command_error(format!(
                "Failed to register with Issuer at {}.\n\n\
                 Possible causes:\n\
                 - Issuer server is not running\n\
                 - Incorrect [issuer_endpoint] url in the runtime config\n\
                 - Network connectivity issues\n\n\
                 Error: {}",
                issuer_endpoint, e
            ))
        })?;
        info!("✅ Issuer registration successful");

        let actr_ref = registered
            .start()
            .await
            .map_err(|e| ActrCliError::command_error(format!("Failed to start ActrNode: {}", e)))?;
        info!("✅ ActrNode started");

        if let Some(runtime) = detached_runtime.as_ref() {
            self.write_runtime_record(runtime, &actr_ref).await?;
            info!("📝 Detached runtime state recorded");
        }

        self.run_foreground(actr_ref, detached_runtime.as_ref())
            .await?;

        Ok(())
    }

    async fn run_foreground(
        &self,
        actr_ref: actr_hyper::ActrRef,
        detached_runtime: Option<&DetachedRuntimeContext>,
    ) -> Result<()> {
        info!("📡 Running in foreground mode (Ctrl+C to stop)");

        // Block and wait for Ctrl+C
        actr_ref
            .wait_for_ctrl_c_and_shutdown()
            .await
            .map_err(|e| ActrCliError::command_error(format!("Shutdown error: {}", e)))?;

        if let Some(runtime) = detached_runtime {
            runtime
                .runtime_store
                .mark_stopped_by_wid(&runtime.wid, Utc::now())
                .await?;
        }

        info!("👋 Shutdown complete");
        Ok(())
    }

    async fn resolve_package_path(&self, config_path: &Path) -> Result<PathBuf> {
        // Load runtime config to get the packaged workload path.
        let config_content = tokio::fs::read_to_string(config_path).await?;
        let raw_config: actr_config::RuntimeRawConfig = toml::from_str(&config_content)
            .map_err(|e| ActrCliError::command_error(format!("Failed to parse config: {}", e)))?;

        if let Some(package_config) = raw_config.package {
            if let Some(path) = package_config.path {
                let base = config_path.parent().unwrap_or(Path::new("."));
                return Ok(resolve_against(base, &path));
            }
        }

        Err(ActrCliError::command_error(format!(
            "Package path not specified in runtime config: {}\n\n\
             Add the packaged workload path to your config:\n\
             [package]\n\
             path = \"dist/service.actr\"",
            config_path.display()
        )))
    }

    fn build_package_info(
        &self,
        manifest: &actr_pack::RuntimePackageManifest,
    ) -> actr_config::PackageInfo {
        let actr_type = manifest.actor_type();
        let (description, license) = match manifest {
            actr_pack::RuntimePackageManifest::V2(_) => (None, None),
            actr_pack::RuntimePackageManifest::Legacy(manifest) => (
                manifest.metadata.description.clone(),
                manifest.metadata.license.clone(),
            ),
        };
        actr_config::PackageInfo {
            name: actr_type.name.clone(),
            actr_type,
            description,
            authors: vec![],
            license,
        }
    }

    async fn init_hyper(
        &self,
        config: &actr_config::RuntimeConfig,
        package_path: &Path,
        hyper_dir: &Path,
    ) -> Result<actr_hyper::Hyper> {
        use actr_hyper::{
            ChainTrust, Hyper, HyperConfig, RegistryTrust, StaticTrust, TrustProvider,
        };
        use std::sync::Arc;

        // Honour `[hyper].storage_root` from the runtime config (already resolved
        // to an absolute path by the parser); otherwise HyperConfig defaults to
        // `<hyper_dir>/storage`.
        let apply_storage_root = |cfg: HyperConfig| match &config.storage_root {
            Some(root) => cfg.with_storage_root(root),
            None => cfg,
        };

        if config.trust.is_empty() {
            // Fallback: when no `[[trust]]` anchors are configured, auto-load
            // the package's sidecar `public-key.json` as a StaticTrust anchor.
            // Lets `actr init` scaffolds "just work" without boilerplate.
            let public_key = self.load_public_key(package_path).await?;
            let trust: Arc<dyn TrustProvider> =
                Arc::new(StaticTrust::new(public_key).map_err(|e| {
                    ActrCliError::command_error(format!("Invalid public key: {}", e))
                })?);
            return Hyper::new(apply_storage_root(HyperConfig::new(hyper_dir, trust)))
                .await
                .map_err(|e| {
                    ActrCliError::command_error(format!("Failed to initialize Hyper: {}", e))
                });
        }

        let mut providers: Vec<Arc<dyn TrustProvider>> = Vec::with_capacity(config.trust.len());
        for anchor in &config.trust {
            let p: Arc<dyn TrustProvider> = match anchor {
                actr_config::TrustAnchor::Static {
                    pubkey_file,
                    pubkey_b64,
                } => {
                    let key_bytes = self.load_static_pubkey(pubkey_file, pubkey_b64).await?;
                    Arc::new(StaticTrust::new(key_bytes).map_err(|e| {
                        ActrCliError::command_error(format!("Invalid static pubkey: {}", e))
                    })?)
                }
                actr_config::TrustAnchor::Registry { endpoint } => {
                    let base = endpoint.trim_end_matches("/issuer").to_string();
                    Arc::new(RegistryTrust::new(base))
                }
            };
            providers.push(p);
        }

        let trust: Arc<dyn TrustProvider> = if providers.len() == 1 {
            providers.into_iter().next().unwrap()
        } else {
            Arc::new(ChainTrust::new(providers))
        };

        Hyper::new(apply_storage_root(HyperConfig::new(hyper_dir, trust)))
            .await
            .map_err(|e| ActrCliError::command_error(format!("Failed to initialize Hyper: {}", e)))
    }

    async fn load_static_pubkey(
        &self,
        pubkey_file: &Option<PathBuf>,
        pubkey_b64: &Option<String>,
    ) -> Result<Vec<u8>> {
        use base64::Engine;
        if let Some(b64) = pubkey_b64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    ActrCliError::command_error(format!("Invalid base64 pubkey: {}", e))
                })?;
            if bytes.len() != 32 {
                return Err(ActrCliError::command_error(format!(
                    "pubkey_b64 must decode to 32 bytes, got {}",
                    bytes.len()
                )));
            }
            return Ok(bytes);
        }
        let path = pubkey_file.as_deref().ok_or_else(|| {
            ActrCliError::command_error(
                "Static trust anchor requires either `pubkey_file` or `pubkey_b64`".to_string(),
            )
        })?;
        parse_pubkey_json(path).await
    }
}

async fn parse_pubkey_json(path: &Path) -> Result<Vec<u8>> {
    if !path.exists() {
        return Err(ActrCliError::command_error(format!(
            "pubkey_file not found: {}",
            path.display()
        )));
    }
    let content = tokio::fs::read_to_string(path).await?;
    let json: serde_json::Value = serde_json::from_str(&content)?;
    let b64 = json["public_key"].as_str().ok_or_else(|| {
        ActrCliError::command_error(format!("{}: missing `public_key` field", path.display()))
    })?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| ActrCliError::command_error(format!("Invalid base64 pubkey: {}", e)))?;
    if bytes.len() != 32 {
        return Err(ActrCliError::command_error(format!(
            "{}: public_key must decode to 32 bytes, got {}",
            path.display(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

impl RunCommand {
    async fn load_public_key(&self, package_path: &Path) -> Result<Vec<u8>> {
        let package_dir = package_path.parent().unwrap_or(Path::new("."));
        let key_path = package_dir.join("public-key.json");

        if !key_path.exists() {
            return Err(ActrCliError::command_error(format!(
                "Public key not found for static trust anchor.\n\n\
                 Expected location: {}\n\n\
                 Either place public-key.json next to the .actr package, or\n\
                 configure explicit trust anchors in actr.toml:\n\n\
                 [[trust]]\n\
                 kind = \"static\"\n\
                 pubkey_file = \"public-key.json\"\n\n\
                 # or\n\
                 [[trust]]\n\
                 kind = \"registry\"\n\
                 endpoint = \"http://localhost:8081/issuer\"",
                key_path.display()
            )));
        }

        let key_content = tokio::fs::read_to_string(&key_path).await?;
        let key_json: serde_json::Value = serde_json::from_str(&key_content)?;

        let key_base64 = key_json["public_key"].as_str().ok_or_else(|| {
            ActrCliError::command_error(
                "Invalid public-key.json format: missing 'public_key' field".to_string(),
            )
        })?;

        use base64::Engine;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(key_base64)
            .map_err(|e| {
                ActrCliError::command_error(format!("Invalid base64 in public key: {}", e))
            })?;

        if key_bytes.len() != 32 {
            return Err(ActrCliError::command_error(format!(
                "Invalid public key size: expected 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        Ok(key_bytes)
    }

    #[cfg(unix)]
    async fn prepare_detached_child(&self, config_path: &Path) -> Result<DetachedRuntimeContext> {
        use nix::unistd::setsid;
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        let wid = self.internal_wid.clone().ok_or_else(|| {
            ActrCliError::command_error("--internal-wid is required for detached child".to_string())
        })?;

        let hyper_dir = resolve_hyper_dir(Some(config_path), self.hyper_dir.as_deref())?;
        let runtime_store = RuntimeStateStore::new(hyper_dir);
        runtime_store.ensure_layout().await?;
        setsid().map_err(|e| {
            ActrCliError::command_error(format!("Failed to create new session: {}", e))
        })?;

        let pid = std::process::id();
        let log_file = log_path_for_wid(runtime_store.hyper_dir(), &wid);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)?;

        let log_fd = log.as_raw_fd();
        nix::unistd::dup2(log_fd, std::io::stdout().as_raw_fd())
            .map_err(|e| ActrCliError::command_error(format!("dup2 failed: {}", e)))?;
        nix::unistd::dup2(log_fd, std::io::stderr().as_raw_fd())
            .map_err(|e| ActrCliError::command_error(format!("dup2 failed: {}", e)))?;

        info!("🚀 Detached child process initialized, PID: {}", pid);
        info!("📝 Log file: {}", log_file.display());

        Ok(DetachedRuntimeContext {
            runtime_store,
            config_path: config_path.to_path_buf(),
            log_file,
            pid,
            wid,
        })
    }

    #[cfg(not(unix))]
    async fn prepare_detached_child(&self, _config_path: &Path) -> Result<DetachedRuntimeContext> {
        Err(ActrCliError::command_error(
            "Detached mode is only supported on Unix systems".to_string(),
        ))
    }

    async fn write_runtime_record(
        &self,
        detached_runtime: &DetachedRuntimeContext,
        actr_ref: &actr_hyper::ActrRef,
    ) -> Result<()> {
        let actr_id_str = actr_protocol::ActrId::to_string_repr(&actr_ref.actor_id());

        // Upsert: if a record already exists for this wid (start/restart scenario),
        // update pid/started_at and clear stopped_at while preserving wid and actr_id.
        let existing = detached_runtime
            .runtime_store
            .read_record_by_wid(&detached_runtime.wid)
            .await?;

        let record = if let Some(mut r) = existing {
            r.pid = detached_runtime.pid;
            r.started_at = Utc::now();
            r.stopped_at = None;
            r.config_path = detached_runtime.config_path.clone();
            r.log_path = detached_runtime.log_file.clone();
            r
        } else {
            RuntimeRecord::new(
                detached_runtime.wid.clone(),
                actr_id_str,
                detached_runtime.pid,
                detached_runtime.config_path.clone(),
                detached_runtime.log_file.clone(),
                Utc::now(),
            )
        };

        detached_runtime.runtime_store.write_record(&record).await
    }

    async fn spawn_detached_child(&self, config_path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use uuid::Uuid;

            let hyper_dir = resolve_hyper_dir(Some(config_path), self.hyper_dir.as_deref())?;
            let runtime_store = RuntimeStateStore::new(hyper_dir);
            runtime_store.ensure_layout().await?;

            // Generate a new wid in the parent; pass it to the child via --internal-wid.
            // For start/restart, the caller sets self.internal_wid to the existing wid.
            let wid = self
                .internal_wid
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            let wid_short = short_wid(&wid).to_string();
            let log_path = log_path_for_wid(runtime_store.hyper_dir(), &wid);

            let current_exe = std::env::current_exe().map_err(|e| {
                ActrCliError::command_error(format!(
                    "Failed to resolve current executable for detached mode: {}",
                    e
                ))
            })?;

            let mut child = StdCommand::new(current_exe);
            child
                .arg("run")
                .arg("--config")
                .arg(config_path)
                .args(
                    self.hyper_dir
                        .as_ref()
                        .map(|path| vec!["--hyper-dir".into(), path.display().to_string()])
                        .unwrap_or_default(),
                )
                .arg("--internal-detached-child")
                .arg("--internal-wid")
                .arg(&wid)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let mut child = child.spawn().map_err(|e| {
                ActrCliError::command_error(format!(
                    "Failed to launch detached child process: {}",
                    e
                ))
            })?;

            let pid = child.id();
            match wait_for_detached_runtime_ready(
                &runtime_store,
                &wid,
                &log_path,
                &mut child,
                DETACHED_READY_TIMEOUT,
                DETACHED_READY_POLL_INTERVAL,
            )
            .await?
            {
                DetachedRuntimeStartup::Ready => {
                    println!("Detached runtime started");
                    println!("   WID:  {}", wid_short);
                    println!("   PID:  {}", pid);
                    println!();
                    println!("Follow logs: actr logs {} -f", wid_short);
                }
                DetachedRuntimeStartup::Initializing => {
                    println!("Detached runtime launched but is still initializing");
                    println!("   WID:   {}", wid_short);
                    println!("   PID:   {}", pid);
                    println!("   Logs:  {}", log_path.display());
                    println!();
                    println!(
                        "Wait for the runtime record to be written before using `actr logs {} -f`.",
                        wid_short
                    );
                }
            }
            Ok(())
        }

        #[cfg(not(unix))]
        {
            let _ = config_path;
            Err(ActrCliError::command_error(
                "Detached mode is only supported on Unix systems".to_string(),
            ))
        }
    }

    // ═════════════════════════════════════════════════════════════════
    // Web server mode
    // ═════════════════════════════════════════════════════════════════

    // ═════════════════════════════════════════════════════════════════
    // Web server mode
    // ═════════════════════════════════════════════════════════════════

    async fn execute_web_mode(&self) -> Result<()> {
        use axum::Router;
        use axum::routing::get;
        use tower_http::cors::CorsLayer;
        use tower_http::services::ServeDir;

        info!("🌐 Starting web server mode");

        // Resolve config path
        let config_path = self
            .config
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNTIME_CONFIG));

        if !config_path.exists() {
            return Err(ActrCliError::command_error(format!(
                "Configuration file not found: {}\n\n\
                 Please create an actr.toml file with [web] section or specify with -c/--config",
                config_path.display()
            )));
        }

        // Parse config to get runtime settings
        let config_content = tokio::fs::read_to_string(&config_path).await?;
        let raw_config: actr_config::RuntimeRawConfig = toml::from_str(&config_content)
            .map_err(|e| ActrCliError::command_error(format!("Failed to parse config: {}", e)))?;

        let config_dir = config_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        // Extract web config with defaults
        let web_port = self
            .port
            .unwrap_or_else(|| raw_config.web.as_ref().map(|w| w.port).unwrap_or(8080));
        let web_host = raw_config
            .web
            .as_ref()
            .map(|w| w.host.clone())
            .unwrap_or_else(|| "0.0.0.0".to_string());
        let static_dir = raw_config
            .web
            .as_ref()
            .map(|w| config_dir.join(&w.static_dir))
            .unwrap_or_else(|| config_dir.join("public"));

        // Resolve the .actr package from [package].path
        let package_path = raw_config
            .package
            .as_ref()
            .and_then(|p| p.path.as_ref())
            .map(|p| resolve_against(&config_dir, p));

        // Read the package bytes for serving
        let package_bytes = if let Some(ref pkg_path) = package_path {
            if pkg_path.exists() {
                Some(tokio::fs::read(pkg_path).await.map_err(|e| {
                    ActrCliError::command_error(format!(
                        "Failed to read package file {}: {}",
                        pkg_path.display(),
                        e
                    ))
                })?)
            } else {
                info!(
                    "⚠️  Package file not found: {}, /packages/*.actr will not be served",
                    pkg_path.display()
                );
                None
            }
        } else {
            None
        };

        // Derive the package filename for the URL
        let package_filename = package_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "package.actr".to_string());

        // Build runtime config JSON — auto-inject embedded asset URLs
        let runtime_config_json = self
            .build_web_runtime_config(&raw_config, &config_path, &package_filename)
            .await?;

        let shared_state = Arc::new(WebServerState {
            runtime_config_json,
            package_bytes,
            package_filename,
        });

        // Build router:
        // 1. /actr-runtime-config.json — generated runtime config
        // 2. /actor.sw.js — embedded Service Worker (wasm-bindgen guest bridge)
        // 3. /packages/actr_sw_host_bg.wasm — embedded SW host WASM
        // 4. /packages/actr_sw_host.js — embedded SW host JS glue
        // 5. /packages/<name>.actr — the .actr package from [package].path
        // 6. / — embedded host HTML (fallback: static_dir)
        let app = Router::new()
            .route("/actr-runtime-config.json", get(serve_runtime_config))
            .route("/actor.sw.js", get(serve_actor_sw_js))
            .route("/packages/actr_sw_host_bg.wasm", get(serve_runtime_wasm))
            .route("/packages/actr_sw_host.js", get(serve_runtime_js))
            .route("/packages/{filename}", get(serve_actr_package))
            .with_state(shared_state.clone());

        let app = app
            .fallback_service(if static_dir.exists() {
                ServeDir::new(&static_dir)
            } else {
                // Serve embedded host page from a temp dir is not ideal,
                // so we handle "/" in the fallback via the index route
                ServeDir::new(&config_dir)
            })
            .route("/", get(serve_host_html))
            .with_state(shared_state)
            .layer(CorsLayer::permissive())
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::header::HeaderName::from_static("cross-origin-opener-policy"),
                axum::http::header::HeaderValue::from_static("same-origin"),
            ))
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::header::HeaderName::from_static("cross-origin-embedder-policy"),
                axum::http::header::HeaderValue::from_static("require-corp"),
            ));

        let addr: std::net::SocketAddr = format!("{}:{}", web_host, web_port)
            .parse()
            .map_err(|e| ActrCliError::command_error(format!("Invalid bind address: {}", e)))?;

        println!("🌐 Web server started");
        println!("   URL:        http://{}:{}", web_host, web_port);
        if static_dir.exists() {
            println!("   Static dir: {}", static_dir.display());
        }
        println!("   Config:     {}", config_path.display());
        if let Some(ref pkg_path) = package_path {
            println!("   Package:    {}", pkg_path.display());
        }
        println!("   Press Ctrl+C to stop");

        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            ActrCliError::command_error(format!("Failed to bind to {}: {}", addr, e))
        })?;

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|e| ActrCliError::command_error(format!("Web server error: {}", e)))?;

        println!("👋 Web server stopped");
        Ok(())
    }

    async fn build_web_runtime_config(
        &self,
        raw: &actr_config::RuntimeRawConfig,
        config_path: &Path,
        package_filename: &str,
    ) -> Result<String> {
        let signaling_url = raw
            .signaling
            .url
            .clone()
            .unwrap_or_else(|| "ws://localhost:8081/signaling/ws".to_string());
        let issuer_endpoint = raw
            .issuer_endpoint
            .url
            .clone()
            .unwrap_or_else(|| "http://localhost:8081/issuer".to_string());
        let realm = raw.deployment.realm.clone().ok_or_else(|| {
            ActrCliError::command_error("deployment.realm is required".to_string())
        })?;
        let visible = raw.discovery.visible.unwrap_or(true);
        let force_relay = raw.webrtc.force_relay;
        let stun_urls = &raw.webrtc.stun_urls;
        let turn_urls = &raw.webrtc.turn_urls;

        // Read package path to extract package info
        let config_dir = config_path.parent().unwrap_or(Path::new("."));
        let package_path = raw
            .package
            .as_ref()
            .and_then(|p| p.path.as_ref())
            .map(|p| resolve_against(config_dir, p));

        // Try to read package manifest for metadata
        let mut package_name = String::new();
        let mut publisher = String::new();
        let mut actr_name = String::new();
        let mut version = String::new();
        let mut acl_rules: Vec<serde_json::Value> = Vec::new();

        if let Some(ref pkg_path) = package_path {
            if pkg_path.exists() {
                if let Ok(bytes) = std::fs::read(pkg_path) {
                    if let Ok(manifest) = actr_pack::read_manifest(&bytes) {
                        package_name.clone_from(&manifest.name);
                        publisher.clone_from(&manifest.publisher);
                        actr_name.clone_from(&manifest.name);
                        version.clone_from(&manifest.version);
                    }
                }
            }
        }

        // Parse ACL from raw config
        if let Some(ref acl_value) = raw.acl {
            if let Some(rules) = acl_value.get("rules").and_then(|v| v.as_array()) {
                for rule in rules {
                    if let Some(table) = rule.as_table() {
                        let permission = table
                            .get("permission")
                            .and_then(|v| v.as_str())
                            .unwrap_or("allow");
                        let type_str = table.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        acl_rules.push(serde_json::json!({
                            "permission": permission,
                            "type": type_str
                        }));
                    }
                }
            }
        }

        let acl_allow_types: Vec<&str> = acl_rules
            .iter()
            .filter_map(|r| {
                if r.get("permission").and_then(|v| v.as_str()) == Some("allow") {
                    r.get("type").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect();

        let full_type = format!("{}/{}@{}", publisher, actr_name, version);

        // Extract web-specific fields
        let web = raw.web.as_ref();
        // Auto-generate package_url and runtime_wasm_url from embedded assets.
        // Config-level overrides are still respected if present (backward compat).
        let package_url = web
            .and_then(|w| w.package_url.clone())
            .unwrap_or_else(|| format!("/packages/{}", package_filename));
        let runtime_wasm_url = web
            .and_then(|w| w.runtime_wasm_url.clone())
            .unwrap_or_else(|| "/packages/actr_sw_host_bg.wasm".to_string());

        // Browser verification cannot read host files. Resolve every static
        // `pubkey_file` here and send only inline base64 keys; the SW tries all
        // static anchors with the same fallback semantics as native ChainTrust.
        let mut trust_json = Vec::with_capacity(raw.trust.len());
        if raw.trust.is_empty() {
            let package_path = package_path.as_deref().ok_or_else(|| {
                ActrCliError::command_error(
                    "Web package verification requires [[trust]] or [package].path".to_string(),
                )
            })?;
            let key = self.load_public_key(package_path).await?;
            use base64::Engine as _;
            trust_json.push(serde_json::json!({
                "kind": "static",
                "pubkey_b64": base64::engine::general_purpose::STANDARD.encode(key),
            }));
        }
        for anchor in &raw.trust {
            match anchor {
                actr_config::TrustAnchor::Static {
                    pubkey_file,
                    pubkey_b64,
                } => {
                    let resolved_file = pubkey_file
                        .as_ref()
                        .map(|path| resolve_against(config_dir, path));
                    let key = self.load_static_pubkey(&resolved_file, pubkey_b64).await?;
                    use base64::Engine as _;
                    trust_json.push(serde_json::json!({
                        "kind": "static",
                        "pubkey_b64": base64::engine::general_purpose::STANDARD.encode(key),
                    }));
                }
                actr_config::TrustAnchor::Registry { endpoint } => {
                    trust_json.push(serde_json::json!({
                        "kind": "registry",
                        "endpoint": endpoint,
                    }));
                }
            }
        }

        let config_json = serde_json::json!({
            "signaling_url": signaling_url,
            "issuer_endpoint": issuer_endpoint,
            "realm": realm,
            "visible": visible,
            "force_relay": force_relay,
            "stun_urls": stun_urls,
            "turn_urls": turn_urls,
            "package": {
                "name": package_name,
                "publisher": publisher,
                "actr_name": actr_name,
                "version": version,
                "full_type": full_type,
            },
            "acl_allow_types": acl_allow_types,
            "package_url": package_url,
            "runtime_wasm_url": runtime_wasm_url,
            "trust": trust_json,
        });

        serde_json::to_string_pretty(&config_json).map_err(|e| {
            ActrCliError::command_error(format!("Failed to serialize runtime config: {}", e))
        })
    }
}

struct WebServerState {
    runtime_config_json: String,
    package_bytes: Option<Vec<u8>>,
    package_filename: String,
}

async fn serve_runtime_config(
    axum::extract::State(state): axum::extract::State<Arc<WebServerState>>,
) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.runtime_config_json.clone(),
    )
}

/// Serve the embedded host HTML page at `/`.
async fn serve_host_html(
    axum::extract::State(_state): axum::extract::State<Arc<WebServerState>>,
) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        crate::web_assets::HOST_HTML,
    )
}

/// Serve the embedded actor.sw.js Service Worker (wasm-bindgen guest bridge).
///
/// Phase 8 collapsed this to a single body — the previous Component Model
/// path and its `ACTR_WEB_GUEST_MODE` selector were deleted along with
/// `actor.sw.js` (CM variant). See `packages/actr-web/docs/option-u-wit-compile-web.zh.md`
/// §11.
async fn serve_actor_sw_js(
    axum::extract::State(_state): axum::extract::State<Arc<WebServerState>>,
) -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        crate::web_assets::ACTOR_SW_JS,
    )
}

/// Serve the embedded runtime WASM binary.
async fn serve_runtime_wasm(
    axum::extract::State(_state): axum::extract::State<Arc<WebServerState>>,
) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/wasm")],
        crate::web_assets::RUNTIME_WASM,
    )
}

/// Serve the embedded runtime JS glue.
async fn serve_runtime_js(
    axum::extract::State(_state): axum::extract::State<Arc<WebServerState>>,
) -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        crate::web_assets::RUNTIME_JS,
    )
}

/// Serve the .actr package from [package].path.
async fn serve_actr_package(
    axum::extract::State(state): axum::extract::State<Arc<WebServerState>>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    if filename == state.package_filename {
        if let Some(ref bytes) = state.package_bytes {
            return (
                axum::http::StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                bytes.clone(),
            );
        }
    }
    (
        axum::http::StatusCode::NOT_FOUND,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        b"Not found".to_vec(),
    )
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
}

struct DetachedRuntimeContext {
    runtime_store: RuntimeStateStore,
    config_path: PathBuf,
    log_file: PathBuf,
    pid: u32,
    wid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetachedRuntimeStartup {
    Ready,
    Initializing,
}

async fn wait_for_detached_runtime_ready(
    runtime_store: &RuntimeStateStore,
    wid: &str,
    log_path: &Path,
    child: &mut Child,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<DetachedRuntimeStartup> {
    let deadline = Instant::now() + timeout;

    loop {
        if runtime_store.read_record_by_wid(wid).await?.is_some() {
            return Ok(DetachedRuntimeStartup::Ready);
        }

        if let Some(status) = child.try_wait()? {
            return Err(ActrCliError::command_error(format!(
                "Detached child exited before runtime became ready (status: {status}). Check logs at {}",
                log_path.display()
            )));
        }

        if Instant::now() >= deadline {
            return Ok(DetachedRuntimeStartup::Initializing);
        }

        tokio::time::sleep(poll_interval).await;
    }
}

fn short_wid(wid: &str) -> &str {
    const SHORT_WID_CHARS: usize = 12;

    let end = wid
        .char_indices()
        .nth(SHORT_WID_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(wid.len());
    &wid[..end]
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
