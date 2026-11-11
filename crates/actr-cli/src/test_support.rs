use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use tempfile::TempDir;

#[path = "upstream_patch_support.rs"]
mod upstream_patch_support;

pub const LOCAL_E2E_REALM: &str = "e2e-local@e2e.test";
const SIGNER_GRPC_PORT: u16 = 50052;
const E2E_RENEWAL_TOKEN_SECRET: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

pub fn actr_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_actr") {
        return PathBuf::from(path);
    }

    let candidate = workspace_root()
        .join("target/debug")
        .join(format!("actr{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        return candidate;
    }

    panic!(
        "failed to locate actr binary: CARGO_BIN_EXE_actr is unset and fallback missing at {}",
        candidate.display()
    );
}

pub fn rust_e2e_target_dir() -> PathBuf {
    workspace_root().join("target/e2e-cache/rust-target")
}

fn local_swift_package_dir() -> PathBuf {
    workspace_root().join("packages/actr-swift")
}

fn local_swift_e2e_output_dir() -> PathBuf {
    local_swift_package_dir().join(".build/e2e")
}

pub fn run_actr(args: &[&str], cwd: &Path) -> Output {
    let mut cmd = Command::new(actr_bin());
    cmd.args(args).current_dir(cwd);
    cmd.env("CARGO_TARGET_DIR", rust_e2e_target_dir());

    // Make Swift template/e2e use the workspace-local Swift package directly.
    let local_swift = local_swift_package_dir();
    if local_swift.join("Package.swift").is_file() {
        cmd.env("ACTR_SWIFT_LOCAL_PATH", local_swift);
    }

    cmd.output().expect("failed to run actr binary")
}

pub fn assert_success(out: &Output, context: &str) {
    assert!(
        out.status.success(),
        "{context} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn ensure_success(out: &Output, context: &str) -> Result<()> {
    if out.status.success() {
        return Ok(());
    }
    bail!(
        "{context} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn cargo_build(dir: &Path) {
    let out = Command::new("cargo")
        .args(["build"])
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", rust_e2e_target_dir())
        .output()
        .expect("cargo build failed");
    assert_success(&out, &format!("cargo build in {}", dir.display()));
}

pub fn random_publisher() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("test{nanos:08x}")
}

pub fn align_project_with_local_loran(project_dir: &Path) -> Result<()> {
    rewrite_project_realm(project_dir, LOCAL_E2E_REALM)
}

pub fn pin_echo_service_dependency_version(project_dir: &Path, publisher: &str) -> Result<()> {
    let actr_toml_path = project_dir.join("manifest.toml");
    let content = fs::read_to_string(&actr_toml_path)
        .with_context(|| format!("failed to read {}", actr_toml_path.display()))?;

    // Newer Rust echo template renders the dependency with publisher
    // already inlined as `EchoService = { actr_type = "<publisher>/EchoService@1.0.0" }`.
    // Detect that case and skip — no pinning needed.
    let already_rendered = format!("EchoService = {{ actr_type = \"{publisher}/");
    if content.contains(&already_rendered) {
        return Ok(());
    }

    // Two placeholder shapes appear across templates:
    // - Rust legacy: `echo-service = {}`
    // - TS / Swift current: `EchoService = {}`
    let placeholders: &[(&str, String)] = &[
        (
            "EchoService = {}",
            format!("EchoService = {{ actr_type = \"{publisher}/EchoService@1.0.0\" }}"),
        ),
        (
            "echo-service = {}",
            format!("echo-service = {{ actr_type = \"{publisher}/EchoService@1.0.0\" }}"),
        ),
    ];

    for (placeholder, replacement) in placeholders {
        if content.contains(placeholder) {
            let rewritten = content.replacen(placeholder, replacement, 1);
            fs::write(&actr_toml_path, rewritten)
                .with_context(|| format!("failed to write {}", actr_toml_path.display()))?;
            return Ok(());
        }
    }

    bail!(
        "failed to pin echo dependency version in {}: neither 'EchoService = {{}}' nor \
         'echo-service = {{}}' placeholders nor a pre-rendered EchoService entry were found",
        actr_toml_path.display()
    );
}

pub fn align_rust_project_with_workspace(project_dir: &Path) -> Result<()> {
    let workspace = workspace_root();
    let cargo_toml_path = project_dir.join("Cargo.toml");
    upstream_patch_support::align_with_workspace(&workspace, &cargo_toml_path)
}

#[derive(Clone, Debug)]
pub struct LocalSwiftPackageAssets {
    pub package_dir: PathBuf,
    pub bindings_path: String,
    pub xcframework_path: String,
}

pub fn ensure_local_swift_xcframework() -> Result<LocalSwiftPackageAssets> {
    static SWIFT_ASSETS: OnceLock<LocalSwiftPackageAssets> = OnceLock::new();
    if let Some(assets) = SWIFT_ASSETS.get() {
        return Ok(assets.clone());
    }

    let workspace = workspace_root();
    let package_dir = local_swift_package_dir();
    let output_dir = local_swift_e2e_output_dir();
    let bindings_path = ".build/e2e/ActrBindings".to_string();
    let xcframework_path = ".build/e2e/ActrFFI.xcframework".to_string();
    let output_path = package_dir.join(&xcframework_path);
    let bindings_dir = package_dir.join(&bindings_path);

    let cache_root = workspace.join("target/e2e-cache");
    fs::create_dir_all(&cache_root).context("failed to create e2e cache root")?;
    let _lock = DirLock::acquire(
        &cache_root.join("swift-xcframework-build.lock"),
        Duration::from_secs(900),
    )?;

    if output_dir.exists() {
        fs::remove_dir_all(&output_dir)
            .with_context(|| format!("failed to remove {}", output_dir.display()))?;
    }
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    if output_path.exists() {
        fs::remove_dir_all(&output_path)
            .with_context(|| format!("failed to remove {}", output_path.display()))?;
    }
    let headers_dir = bindings_dir.join("include");
    fs::create_dir_all(&headers_dir)
        .with_context(|| format!("failed to create {}", headers_dir.display()))?;
    run_checked(
        {
            let mut cmd = Command::new("./build-xcframework.sh");
            cmd.current_dir(&package_dir)
                .env("ACTR_BINDINGS_PATH", &bindings_path)
                .env("ACTR_BINARY_PATH", &xcframework_path);
            cmd
        },
        "build local swift ffi xcframework",
    )?;

    if !bindings_dir.join("Actr.swift").is_file() {
        bail!(
            "local swift bindings not found at {}",
            bindings_dir.join("Actr.swift").display()
        );
    }
    if !output_path.exists() {
        bail!(
            "local swift ffi xcframework not found at {}",
            output_path.display()
        );
    }

    let assets = LocalSwiftPackageAssets {
        package_dir,
        bindings_path,
        xcframework_path,
    };
    let _ = SWIFT_ASSETS.set(assets.clone());
    Ok(assets)
}

pub struct LoggedProcess {
    child: Child,
    logs: Arc<Mutex<Vec<String>>>,
}

impl LoggedProcess {
    pub fn spawn(mut cmd: Command, name: &str) -> Result<Self> {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn process '{name}'"))?;
        let logs = Arc::new(Mutex::new(Vec::new()));
        drain_stream(child.stdout.take(), Arc::clone(&logs), name, "stdout");
        drain_stream(child.stderr.take(), Arc::clone(&logs), name, "stderr");
        Ok(Self { child, logs })
    }

    pub fn wait_for_log(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .logs
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains(needle))
            {
                return true;
            }

            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return false;
            }
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn logs(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LoggedProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Mock signaling server for e2e tests (replaces LocalLoran).
///
/// Runs `MockSignalingServer` on a background tokio runtime and exposes
/// a synchronous API compatible with the test harness.
pub struct MockSignaling {
    pub signaling_ws_url: String,
    _runtime: tokio::runtime::Runtime,
    _server: std::sync::Arc<tokio::sync::Mutex<actr_mock_loran::MockLoranServer>>,
}

impl MockSignaling {
    pub fn start() -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to build tokio runtime for mock signaling")?;

        let server = rt
            .block_on(actr_mock_loran::MockLoranServer::start())
            .context("failed to start mock signaling server")?;

        let ws_url = server.url();
        let server = std::sync::Arc::new(tokio::sync::Mutex::new(server));

        Ok(Self {
            signaling_ws_url: ws_url,
            _runtime: rt,
            _server: server,
        })
    }
}

pub struct LocalLoran {
    pub state_dir: TempDir,
    process: LoggedProcess,
    loran_bin: PathBuf,
    config_path: PathBuf,
    http_port: u16,
    pub http_base_url: String,
    pub signaling_ws_url: String,
}

impl LocalLoran {
    pub fn start() -> Result<Self> {
        ensure_signer_port_available()?;
        let loran_bin = ensure_loran_binary()?;

        let state_dir = TempDir::new().context("failed to create loran state dir")?;
        let http_port = free_port().context("failed to allocate HTTP port")?;
        let ice_port = free_port().context("failed to allocate ICE port")?;
        let config_path = state_dir.path().join("loran-e2e.toml");
        write_loran_config(&config_path, state_dir.path(), http_port, ice_port)?;

        let mut cmd = Command::new(&loran_bin);
        cmd.arg("--config")
            .arg(&config_path)
            .current_dir(state_dir.path());
        let mut process = LoggedProcess::spawn(cmd, "loran")?;

        let health_path = "/signaling/health";
        if !wait_for_http_ok(
            &mut process,
            http_port,
            health_path,
            Duration::from_secs(120),
        ) {
            let logs = process.logs();
            bail!(
                "loran did not become healthy within timeout (http://127.0.0.1:{http_port}{health_path})\n{logs}"
            );
        }

        ensure_realm_exists(&state_dir.path().join("sqlite"), LOCAL_E2E_REALM)?;

        Ok(Self {
            state_dir,
            process,
            loran_bin,
            config_path,
            http_port,
            http_base_url: format!("http://127.0.0.1:{http_port}"),
            signaling_ws_url: format!("ws://127.0.0.1:{http_port}/signaling/ws"),
        })
    }

    /// Kill the loran process without cleaning up state (SQLite remains).
    pub fn kill(&mut self) {
        self.process.kill();
    }

    /// Restart loran using the same state directory and ports.
    /// The SQLite database is preserved so the Actor Registry can be restored.
    pub fn restart(&mut self) -> Result<()> {
        self.kill();
        // Brief pause to let the OS release the port
        thread::sleep(Duration::from_millis(500));

        let mut cmd = Command::new(&self.loran_bin);
        cmd.arg("--config")
            .arg(&self.config_path)
            .current_dir(self.state_dir.path());
        let mut process = LoggedProcess::spawn(cmd, "loran")?;

        let health_path = "/signaling/health";
        if !wait_for_http_ok(
            &mut process,
            self.http_port,
            health_path,
            Duration::from_secs(120),
        ) {
            let logs = process.logs();
            bail!("loran did not become healthy after restart\n{logs}");
        }

        self.process = process;
        Ok(())
    }

    pub fn logs(&self) -> String {
        self.process.logs()
    }

    pub fn wait_for_log(&mut self, needle: &str, timeout: Duration) -> bool {
        self.process.wait_for_log(needle, timeout)
    }
}

pub struct LocalRustEchoService {
    _workspace: TempDir,
    process: LoggedProcess,
}

impl LocalRustEchoService {
    pub fn start(signaling_ws_url: &str) -> Result<Self> {
        let workspace = TempDir::new().context("failed to create rust echo e2e workspace")?;
        let project_name = "registry-echo-service";

        let init_out = run_actr(
            &[
                "init",
                "-l",
                "rust",
                "--template",
                "echo",
                "--role",
                "service",
                "--publisher",
                "acme",
                "--realm",
                LOCAL_E2E_REALM,
                "--signaling",
                signaling_ws_url,
                project_name,
            ],
            workspace.path(),
        );
        ensure_success(&init_out, "actr init rust service")?;

        let service_dir = workspace.path().join(project_name);
        align_project_with_local_loran(&service_dir)?;
        rewrite_manifest_version(&service_dir, "0.1.0")?;
        align_rust_project_with_workspace(&service_dir)?;
        let install_out = run_actr(&["deps", "install"], &service_dir);
        ensure_success(&install_out, "actr deps install rust service")?;
        let gen_out = run_actr(&["gen", "-l", "rust"], &service_dir);
        ensure_success(&gen_out, "actr gen rust service")?;
        let build_out = run_actr(&["build"], &service_dir);
        ensure_success(&build_out, "actr build rust service")?;

        let runtime_config_path = write_package_runtime_config(&service_dir, signaling_ws_url)?;

        let mut cmd = Command::new(actr_bin());
        cmd.args(["run", "-c", runtime_config_path.to_string_lossy().as_ref()])
            .current_dir(&service_dir)
            .env("CARGO_TARGET_DIR", rust_e2e_target_dir());
        let mut process = LoggedProcess::spawn(cmd, "rust-echo-service")?;
        if !process.wait_for_log("✅ ActrNode started", Duration::from_secs(180)) {
            let logs = process.logs();
            bail!("rust echo service did not register in time\n{logs}");
        }

        Ok(Self {
            _workspace: workspace,
            process,
        })
    }

    pub fn logs(&self) -> String {
        self.process.logs()
    }
}

fn rewrite_project_realm(project_dir: &Path, realm: &str) -> Result<()> {
    // The deployment Realm placement varies by template:
    // - Rust echo (split layout): runtime config goes to `actr.toml`; the
    //   service subproject has no deployment file at all (services are
    //   realm-neutral packages) so we skip silently.
    // - TS / Swift / Kotlin echo (single-file layout): the Realm still lives
    //   inline in `manifest.toml`.
    // Visit every candidate, rewrite where the line exists, no-op otherwise.
    let candidates = ["actr.toml", "manifest.toml"];
    let mut visited_any = false;
    for filename in candidates {
        let path = project_dir.join(filename);
        if !path.exists() {
            continue;
        }
        visited_any = true;
        rewrite_deployment_realm_in_file(&path, realm)?;
    }
    if !visited_any {
        bail!(
            "no actr.toml or manifest.toml under {}",
            project_dir.display()
        );
    }
    Ok(())
}

/// Rewrites `realm = ...` inside `[deployment]` / `[system.deployment]`.
/// Returns `false` when the file has no deployment Realm (e.g. a
/// service-only `manifest.toml` whose deployment lives elsewhere). ACL and
/// dependency `realm` keys live in other sections and are left alone.
fn rewrite_deployment_realm_in_file(path: &Path, realm: &str) -> Result<bool> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let mut replaced = false;
    let mut in_deployment = false;
    let mut rewritten = String::with_capacity(content.len() + 32);
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let section = trimmed.trim_end();
            in_deployment = section == "[deployment]" || section == "[system.deployment]";
        }
        if in_deployment && trimmed.starts_with("realm =") {
            let prefix = &line[..line.len() - trimmed.len()];
            rewritten.push_str(prefix);
            rewritten.push_str(&format!("realm = \"{realm}\""));
            rewritten.push('\n');
            replaced = true;
        } else {
            rewritten.push_str(line);
            rewritten.push('\n');
        }
    }

    if !replaced {
        return Ok(false);
    }

    if !content.ends_with('\n') {
        rewritten.pop();
    }

    fs::write(path, rewritten).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn ensure_realm_exists(sqlite_dir: &Path, realm: &str) -> Result<()> {
    let db_path = sqlite_dir.join("loran.db");
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        match Connection::open(&db_path) {
            Ok(conn) => {
                conn.busy_timeout(Duration::from_secs(3))
                    .context("failed to set sqlite busy timeout")?;
                // Loran created the schema before reporting healthy.
                conn.execute(
                    "INSERT OR IGNORE INTO realm (name, status, enabled, created_at)
                     VALUES (?1, 'active', 1, strftime('%s','now'))",
                    rusqlite::params![realm],
                )
                .context("failed to ensure local e2e realm exists")?;
                return Ok(());
            }
            Err(err) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(200));
                let _ = err;
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to open loran db {}", db_path.display()));
            }
        }
    }
}

fn write_package_runtime_config(project_dir: &Path, signaling_ws_url: &str) -> Result<PathBuf> {
    let cli_config = crate::config::resolver::resolve_effective_cli_config()?;
    let key_path = crate::commands::package_build::resolve_key_path(
        None,
        cli_config.publisher.keychain.as_deref(),
    )?;
    let package_path = find_built_package(project_dir)?;

    let runtime_root = project_dir.join(".actr-e2e-runtime");
    let data_dir = runtime_root.join("hyper");
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;
    let issuer_url = signaling_ws_url
        .trim_end_matches("/signaling/ws")
        .trim_end_matches("/signaling")
        .trim_end_matches("/ws");
    let issuer_url = if let Some(rest) = issuer_url.strip_prefix("ws://") {
        format!("http://{rest}/issuer")
    } else if let Some(rest) = issuer_url.strip_prefix("wss://") {
        format!("https://{rest}/issuer")
    } else {
        format!("{issuer_url}/issuer")
    };

    let runtime_config = format!(
        "edition = 1\n\n[signaling]\nurl = \"{signaling_ws_url}\"\n\n[issuer_endpoint]\nurl = \"{issuer_url}\"\n\n[deployment]\nrealm = \"{LOCAL_E2E_REALM}\"\n\n[package]\npath = \"{}\"\n\n[hyper]\ndata_dir = \"{}\"\n\n[[trust]]\nkind = \"static\"\npubkey_file = \"{}\"\n",
        normalize_path_for_toml(&package_path),
        normalize_path_for_toml(&data_dir),
        normalize_path_for_toml(&key_path),
    );
    let runtime_config_path = project_dir.join("actr.e2e.toml");
    fs::write(&runtime_config_path, runtime_config)
        .with_context(|| format!("failed to write {}", runtime_config_path.display()))?;
    Ok(runtime_config_path)
}

fn rewrite_manifest_version(project_dir: &Path, version: &str) -> Result<()> {
    let manifest_path = project_dir.join("manifest.toml");
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let rewritten = content.replacen(
        "version = \"1.0.0\"",
        &format!("version = \"{version}\""),
        1,
    );
    fs::write(&manifest_path, rewritten)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;
    Ok(())
}

fn find_built_package(project_dir: &Path) -> Result<PathBuf> {
    fs::read_dir(project_dir.join("dist"))
        .with_context(|| format!("failed to read {}", project_dir.join("dist").display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("actr"))
        .context("built rust service package missing .actr artifact under dist/")
}

fn drain_stream(
    stream: Option<impl Read + Send + 'static>,
    logs: Arc<Mutex<Vec<String>>>,
    name: &str,
    stream_name: &str,
) {
    if let Some(stream) = stream {
        let tag = format!("[{name}:{stream_name}]");
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                logs.lock().unwrap().push(format!("{tag} {line}"));
            }
        });
    }
}

fn ensure_signer_port_available() -> Result<()> {
    let probe = TcpListener::bind(("127.0.0.1", SIGNER_GRPC_PORT))
        .with_context(|| format!("port {SIGNER_GRPC_PORT} is already in use"))?;
    drop(probe);
    Ok(())
}

fn wait_for_http_ok(process: &mut LoggedProcess, port: u16, path: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if http_get_ok(port, path) {
            return true;
        }
        if matches!(process.try_wait(), Ok(Some(_))) {
            return false;
        }
        if Instant::now() > deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn http_get_ok(port: u16, path: &str) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let timeout = Duration::from_secs(1);
    let Ok(mut stream) =
        TcpStream::connect_timeout(&addr.parse().expect("valid socket addr"), timeout)
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }

    let Some(status_line) = response.lines().next() else {
        return false;
    };
    status_line.contains(" 200 ")
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to bind ephemeral port")?;
    let port = listener
        .local_addr()
        .context("failed to read local address")?
        .port();
    drop(listener);
    Ok(port)
}

fn ensure_loran_binary() -> Result<PathBuf> {
    static LORAN_BIN: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = LORAN_BIN.get() {
        return Ok(path.clone());
    }

    if let Ok(path) = std::env::var("LORAN_BIN") {
        let binary_path = PathBuf::from(path);
        if binary_path.is_file() {
            let _ = LORAN_BIN.set(binary_path.clone());
            return Ok(binary_path);
        }
        bail!(
            "LORAN_BIN points to a missing file: {}",
            binary_path.display()
        );
    }

    let cache_root = workspace_root().join("target/e2e-cache");
    fs::create_dir_all(&cache_root).context("failed to create e2e cache root")?;
    let _lock = DirLock::acquire(
        &cache_root.join("loran-build.lock"),
        Duration::from_secs(600),
    )?;

    let binary_path = build_in_tree_loran_binary(&cache_root)?;
    let _ = LORAN_BIN.set(binary_path.clone());
    Ok(binary_path)
}

fn build_in_tree_loran_binary(cache_root: &Path) -> Result<PathBuf> {
    let manifest_path = workspace_root().join("crates/loran-server/Cargo.toml");
    if !manifest_path.is_file() {
        bail!(
            "in-tree loran manifest not found at {}",
            manifest_path.display()
        );
    }

    let target_dir = cache_root.join("loran-target");
    let mut build_cmd = Command::new("cargo");
    build_cmd
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("loran-server")
        .arg("--bin")
        .arg("loran")
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", &target_dir);
    run_checked(
        build_cmd,
        "cargo build --release -p loran-server --bin loran",
    )?;

    let binary_path = target_dir
        .join("release")
        .join(format!("loran{}", std::env::consts::EXE_SUFFIX));
    if !binary_path.exists() {
        bail!("loran binary not found at {}", binary_path.display());
    }

    Ok(binary_path)
}

fn run_checked(mut cmd: Command, context_name: &str) -> Result<Output> {
    let output = cmd
        .output()
        .with_context(|| format!("{context_name}: failed to execute"))?;
    if output.status.success() {
        return Ok(output);
    }

    bail!(
        "{} failed:\nstdout: {}\nstderr: {}",
        context_name,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write_loran_config(
    config_path: &Path,
    state_dir: &Path,
    http_port: u16,
    ice_port: u16,
) -> Result<()> {
    let sqlite_dir = state_dir.join("sqlite");
    let log_dir = state_dir.join("logs");
    fs::create_dir_all(&sqlite_dir).context("failed to create sqlite dir")?;
    fs::create_dir_all(&log_dir).context("failed to create log dir")?;

    let sqlite_path = normalize_path_for_toml(&sqlite_dir);

    let config = format!(
        r#"enable = 25
name = "loran-e2e"
env = "dev"
sqlite_path = "{sqlite_path}"
location_tag = "local,e2e,default"
loran_shared_key = "loran-e2e-shared-key-0123456789abcdef"

[control]
head = "admin_ui"

[control.admin_ui]
password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E"

[authority]
uri = "actr://localhost"

[bind.http]
domain_name = "127.0.0.1"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {http_port}

[bind.ice]
ip = "127.0.0.1"
port = {ice_port}
advertised_ip = "127.0.0.1"
advertised_port = {ice_port}

[turn]
relay_port_range = "49152-49200"
realm = "local.loran"
turn_secret = "loran-e2e-turn-secret-0123456789abcdef"

[services.signer]

[services.issuer]

[services.issuer.server]
renewal_token_secret = "{renewal_secret}"

[services.signaling]

[services.signaling.server]
ws_path = "/signaling"
"#,
        renewal_secret = E2E_RENEWAL_TOKEN_SECRET,
    );

    fs::write(config_path, config)
        .with_context(|| format!("failed to write loran config to {}", config_path.display()))?;
    Ok(())
}

fn normalize_path_for_toml(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|candidate| candidate.join("crates/actr-cli").is_dir())
        .expect("actr-cli should live under workspace root")
        .to_path_buf()
}

struct DirLock {
    path: PathBuf,
}

impl DirLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        loop {
            match fs::create_dir(path) {
                Ok(()) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() > deadline {
                        bail!("timed out waiting for lock directory {}", path.display());
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                Err(err) => return Err(err).context("failed to acquire lock directory"),
            }
        }
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
#[path = "test_support_tests.rs"]
mod tests;
