use actr_protocol::acl_rule::{Permission, SourceRealm};
use actr_protocol::{
    Acl, AclRule, ActrRelay, ActrType, RegisterAuthMode, RegisterRequest, RegisterResponse,
    RelayDeliveryContext, RelayDeliveryStatus, RoleNegotiation, SIGNALING_ENVELOPE_VERSION,
    acl_rule, actr_relay, peer_to_signaling, register_response, route_candidates_response,
    signaling_envelope, signaling_to_actr,
};
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use platform::aid::credential::validator::AIdCredentialValidator;
use prost::Message;
use serde_json::{Value, json};
use signer::{GrpcClient, GrpcClientConfig};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message as WsMessage, client::IntoClientRequest},
};
use uuid::Uuid;

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsWrite = futures::stream::SplitSink<WsStream, WsMessage>;
type WsRead = futures::stream::SplitStream<WsStream>;

const START_TIMEOUT: Duration = Duration::from_secs(20);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const LORAN_SHARED_KEY: &str = "0123456789abcdef0123456789abcdef";
const TEST_REALM: &str = "test@example.com";
const SECOND_REALM: &str = "second@acme.com";
/// Label of the owner key every seeded Realm has.
const TEST_CLIENT_KEY: &str = "loran-fullstack-client-key";
/// Member id of that owner.
const TEST_OWNER_MEMBER: &str = "00000000000000f1";
/// Actr Facility Provider ID of every fullstack config (`actr://localhost`).
const PROVIDER_ID: &str = "localhost";
const DEFAULT_TOKEN_TTL: u64 = 3600;
const RENEWAL_TOKEN_SECRET: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// Convenience bundle for a running loran instance
struct LoranHarness {
    #[allow(dead_code)]
    tmp: tempfile::TempDir,
    port: u16,
    tls: bool,
    /// Dedicated Signer gRPC listener port (SGN-02).
    signer_port: u16,
    log_path: PathBuf,
    data_dir: PathBuf,
    child: Child,
}

impl LoranHarness {
    /// Start loran with default features (Issuer/Signer/Signaling) and wait for health
    async fn start(token_ttl: u64) -> Self {
        Self::start_with_login_limits(token_ttl, None).await
    }

    async fn start_with_login_limits(token_ttl: u64, limits: Option<&str>) -> Self {
        Self::start_with_options(token_ttl, limits, false, false).await
    }

    async fn start_with_relay(token_ttl: u64) -> Self {
        Self::start_with_options(token_ttl, None, true, false).await
    }

    async fn start_tls(token_ttl: u64) -> Self {
        Self::start_with_options(token_ttl, None, false, true).await
    }

    async fn start_with_options(
        token_ttl: u64,
        limits: Option<&str>,
        relay: bool,
        tls: bool,
    ) -> Self {
        let tmp = tempfile::tempdir().expect("temp dir");
        let port = choose_port();
        let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, token_ttl);
        if relay {
            let config = fs::read_to_string(&config_path).expect("read config");
            assert!(config.contains("enable = 25"));
            fs::write(
                &config_path,
                config.replacen("enable = 25", "enable = 57", 1),
            )
            .expect("enable relay");
        }
        if tls {
            let (cert, key, _) = test_tls();
            let cert_path = tmp.path().join("tls-cert.pem");
            let key_path = tmp.path().join("tls-key.pem");
            fs::write(&cert_path, cert).unwrap();
            fs::write(&key_path, key).unwrap();
            let mut config: toml::Value =
                fs::read_to_string(&config_path).unwrap().parse().unwrap();
            let http = config["bind"]["http"].as_table_mut().unwrap();
            http.insert(
                "cert".into(),
                toml::Value::String(cert_path.display().to_string()),
            );
            http.insert(
                "key".into(),
                toml::Value::String(key_path.display().to_string()),
            );
            fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
        }
        if let Some(limits) = limits {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&config_path)
                .unwrap();
            writeln!(file, "\n[control.admin_ui.login_limits]\n{limits}").unwrap();
        }
        let log_path = tmp.path().join("loran_fullstack.log");
        let data_dir = tmp.path().join("data");
        ensure_realm(&data_dir, TEST_REALM).await;
        if relay {
            ensure_realm(&data_dir, SECOND_REALM).await;
        }
        let mut child = spawn_loran(&config_path, &log_path);

        let scheme = if tls { "https" } else { "http" };
        let base = format!("{scheme}://127.0.0.1:{port}");
        wait_for_signer_ready(
            &format!("http://127.0.0.1:{signer_port}"),
            &mut child,
            &log_path,
        )
        .await;
        wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
        wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
        ensure_realm(&data_dir, TEST_REALM).await;

        Self {
            tmp,
            port,
            tls,
            signer_port,
            log_path,
            data_dir,
            child,
        }
    }

    fn base_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}", self.port)
    }

    fn log_path(&self) -> &Path {
        self.log_path.as_path()
    }

    /// Shutdown child, ignoring errors
    fn shutdown(self) {
        graceful_shutdown(self.child);
    }
}

// Disposable test certificates are trusted only by this integration binary.
fn test_tls() -> &'static (String, String, Vec<u8>) {
    static TLS: std::sync::OnceLock<(String, String, Vec<u8>)> = std::sync::OnceLock::new();
    TLS.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let tls = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
            .unwrap();
        (
            tls.cert.pem(),
            tls.signing_key.serialize_pem(),
            tls.cert.der().to_vec(),
        )
    })
}

fn test_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(&test_tls().2).unwrap())
        .build()
        .unwrap()
}

#[cfg(test)]
use serial_test::serial;

fn choose_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .unwrap()
        .port()
}

fn write_fullstack_config(dir: &Path, port: u16, token_ttl_secs: u64) -> (PathBuf, u16) {
    let data_dir = dir.join("data");
    fs::create_dir_all(&data_dir).expect("create data dir");
    let config_path = dir.join("config.toml");
    // SGN-02: allocate an independent ephemeral port for the dedicated Signer
    // gRPC listener so parallel test binaries cannot collide on the bind.http
    // port + 1 default.
    let signer_port = choose_port();
    let mut f = fs::File::create(&config_path).expect("create config file");
    writeln!(
        f,
        r#"
name = "loran-fullstack-test"
enable = 25  # ENABLE_SIGNALING | ENABLE_ISSUER | ENABLE_SIGNER
env = "dev"
sqlite_path = "{sqlite}"
loran_shared_key = "{shared}"
location_tag = "local,test,fullstack"
pid = "{pid}"

[authority]
enabled = true
uri = "actr://localhost"

[bind]
[bind.http]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {port}

[bind.signer]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {signer_port}

[bind.ice]
advertised_ip = "127.0.0.1"
advertised_port = 3478
ip = "127.0.0.1"
port = 0

[turn]
relay_port_range = "49152-65535"
realm = "loran.local"
turn_secret = "fullstack-turn-secret-0123456789abcdef"

[services.signer]
[services.signer.storage]
backend = "sqlite"
[services.signer.storage.sqlite]
path = "signer.db"

[services.issuer]
[services.issuer.server]
token_ttl_secs = {token_ttl}
renewal_token_secret = "{renewal_secret}"

[services.signaling]
[services.signaling.server]
ws_path = "/signaling"

[control.admin_ui]
password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E"

[recording]
"#,
        sqlite = data_dir.display(),
        shared = LORAN_SHARED_KEY,
        port = port,
        signer_port = signer_port,
        token_ttl = token_ttl_secs,
        renewal_secret = RENEWAL_TOKEN_SECRET,
        pid = dir.join("loran.pid").display()
    )
    .expect("write config");
    (config_path, signer_port)
}

fn write_fullstack_config_with_rate_limits(
    dir: &Path,
    port: u16,
    token_ttl_secs: u64,
    max_concurrent_per_ip: u32,
    message_per_second: u32,
    message_burst_size: u32,
) -> (PathBuf, u16) {
    let data_dir = dir.join("data");
    fs::create_dir_all(&data_dir).expect("create data dir");
    let config_path = dir.join("config.toml");
    // SGN-02: independent ephemeral port for the dedicated Signer listener.
    let signer_port = choose_port();
    let mut f = fs::File::create(&config_path).expect("create config file");
    writeln!(
        f,
        r#"
name = "loran-fullstack-test-rate-limit"
enable = 25  # ENABLE_SIGNALING | ENABLE_ISSUER | ENABLE_SIGNER
env = "dev"
sqlite_path = "{sqlite}"
loran_shared_key = "{shared}"
location_tag = "local,test,fullstack,ratelimit"
pid = "{pid}"

[authority]
uri = "actr://localhost"

[bind]
[bind.http]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {port}

[bind.signer]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {signer_port}

[bind.ice]
advertised_ip = "127.0.0.1"
advertised_port = 3478
ip = "127.0.0.1"
port = 0

[turn]
relay_port_range = "49152-65535"
realm = "loran.local"
turn_secret = "fullstack-turn-secret-0123456789abcdef"

[services.signer]
[services.signer.storage]
backend = "sqlite"
[services.signer.storage.sqlite]
path = "signer.db"

[services.issuer]
[services.issuer.server]
token_ttl_secs = {token_ttl}
renewal_token_secret = "{renewal_secret}"

[services.signaling]
[services.signaling.server]
ws_path = "/signaling"

[services.signaling.server.rate_limit.connection]
enabled = true
per_minute = 60
burst_size = 10
max_concurrent_per_ip = {max_concurrent}

[services.signaling.server.rate_limit.message]
enabled = true
per_second = {message_per_second}
burst_size = {message_burst}

[control.admin_ui]
password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E"

[recording]
"#,
        sqlite = data_dir.display(),
        shared = LORAN_SHARED_KEY,
        port = port,
        signer_port = signer_port,
        token_ttl = token_ttl_secs,
        renewal_secret = RENEWAL_TOKEN_SECRET,
        max_concurrent = max_concurrent_per_ip,
        message_per_second = message_per_second,
        message_burst = message_burst_size,
        pid = dir.join("loran.pid").display()
    )
    .expect("write config");
    (config_path, signer_port)
}

fn write_fullstack_config_with_issuer_signer_endpoint(
    dir: &Path,
    port: u16,
    token_ttl_secs: u64,
    issuer_signer_endpoint: &str,
) -> (PathBuf, u16) {
    let data_dir = dir.join("data");
    fs::create_dir_all(&data_dir).expect("create data dir");
    let config_path = dir.join("config.toml");
    // SGN-02: independent ephemeral port for the dedicated Signer listener.
    let signer_port = choose_port();
    let mut f = fs::File::create(&config_path).expect("create config file");
    writeln!(
        f,
        r#"
name = "loran-fullstack-test-issuer-dependency"
enable = 25  # ENABLE_SIGNALING | ENABLE_ISSUER | ENABLE_SIGNER
env = "dev"
sqlite_path = "{sqlite}"
loran_shared_key = "{shared}"
location_tag = "local,test,fullstack,issuer-dependency"
pid = "{pid}"

[authority]
uri = "actr://localhost"

[bind]
[bind.http]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {port}

[bind.signer]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {signer_port}

[bind.ice]
advertised_ip = "127.0.0.1"
advertised_port = 3478
ip = "127.0.0.1"
port = 0

[turn]
relay_port_range = "49152-65535"
realm = "loran.local"
turn_secret = "fullstack-turn-secret-0123456789abcdef"

[services.signer]
[services.signer.storage]
backend = "sqlite"
[services.signer.storage.sqlite]
path = "signer.db"

[services.issuer]
[services.issuer.server]
token_ttl_secs = {token_ttl}
renewal_token_secret = "{renewal_secret}"
[services.issuer.dependencies.signer]
endpoint = "{issuer_signer_endpoint}"
timeout_seconds = 1
enable_tls = false

[services.signaling]
[services.signaling.server]
ws_path = "/signaling"

[control.admin_ui]
password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E"

[recording]
"#,
        sqlite = data_dir.display(),
        shared = LORAN_SHARED_KEY,
        port = port,
        signer_port = signer_port,
        token_ttl = token_ttl_secs,
        renewal_secret = RENEWAL_TOKEN_SECRET,
        issuer_signer_endpoint = issuer_signer_endpoint,
        pid = dir.join("loran.pid").display()
    )
    .expect("write config");
    (config_path, signer_port)
}

#[allow(dead_code)]
fn write_signaling_without_local_issuer_config(dir: &Path, port: u16) -> PathBuf {
    let data_dir = dir.join("data");
    fs::create_dir_all(&data_dir).expect("create data dir");
    let config_path = dir.join("config.toml");
    let mut f = fs::File::create(&config_path).expect("create config file");
    writeln!(
        f,
        r#"
name = "loran-fullstack-signaling-without-issuer"
enable = 17  # ENABLE_SIGNALING | ENABLE_SIGNER
env = "dev"
sqlite_path = "{sqlite}"
loran_shared_key = "{shared}"
location_tag = "local,test,fullstack,signaling-no-issuer"
pid = "{pid}"

[authority]
uri = "actr://localhost"

[bind]
[bind.http]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = {port}

[bind.ice]
advertised_ip = "127.0.0.1"
advertised_port = 3478
ip = "127.0.0.1"
port = 0

[turn]
relay_port_range = "49152-65535"
realm = "loran.local"

[services.signer]
[services.signer.storage]
backend = "sqlite"
[services.signer.storage.sqlite]
path = "signer.db"

[services.signaling]
[services.signaling.server]
ws_path = "/signaling"
[services.signaling.dependencies.issuer]
endpoint = "http://127.0.0.1:1"
timeout_seconds = 1

[control.admin_ui]
password_hash = "$argon2id$v=19$m=65536,t=2,p=1$YWN0cml4LXRlc3Qtc2FsdA$lfmjnm9CjoHQC9U79KOWXJkyWTUA7g5MhgG2DDPqe1E"

[recording]
"#,
        sqlite = data_dir.display(),
        shared = LORAN_SHARED_KEY,
        port = port,
        pid = dir.join("loran.pid").display()
    )
    .expect("write config");
    config_path
}

fn spawn_loran(config: &Path, log_path: &Path) -> Child {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_loran"));
    let log_file = fs::File::create(log_path).expect("create log file");
    Command::new(bin)
        .arg("--config")
        .arg(config)
        .stdout(Stdio::from(log_file.try_clone().expect("dup log")))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("spawn loran")
}

/// The Realm client key a test names by `label`.
fn client_key(label: &str) -> ed25519_dalek::SigningKey {
    let seed: [u8; 32] = hex::decode(actr_protocol::realm_membership::body_sha256(
        label.as_bytes(),
    ))
    .expect("hex")
    .try_into()
    .expect("32 bytes");
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

/// An `x-actr-client-proof` for `body`, signed by the key `label`.
fn client_proof(
    op: actr_protocol::realm_membership::ProofOp,
    realm: &str,
    body: &[u8],
    label: &str,
) -> String {
    actr_protocol::realm_membership::ClientProof::new(
        op,
        realm,
        PROVIDER_ID,
        body,
        &Uuid::new_v4().to_string(),
    )
    .header_value(&client_key(label))
}

/// An active Realm whose owner holds the key [`TEST_CLIENT_KEY`].
async fn ensure_realm(sqlite_dir: &Path, realm: &str) {
    let db = platform::storage::db::Database::new(sqlite_dir)
        .await
        .expect("init db");
    let public_key = client_key(TEST_CLIENT_KEY).verifying_key().to_bytes();
    let key_id = actr_protocol::realm_membership::client_key_id(&public_key);
    sqlx::query(
        "INSERT OR IGNORE INTO realm (name, status, enabled, created_at)
         VALUES (?, 'active', 1, strftime('%s','now'))",
    )
    .bind(realm)
    .execute(db.get_pool())
    .await
    .expect("seed realm");
    sqlx::query(
        "INSERT OR IGNORE INTO realm_member (realm, member_id, role, status, created_at)
         VALUES (?, ?, 'owner', 'active', 1)",
    )
    .bind(realm)
    .bind(TEST_OWNER_MEMBER)
    .execute(db.get_pool())
    .await
    .expect("seed realm member");
    sqlx::query(
        "INSERT OR IGNORE INTO realm_client_key
            (realm, key_id, public_key, member_id, registered_at)
         VALUES (?, ?, ?, ?, 1)",
    )
    .bind(realm)
    .bind(key_id)
    .bind(public_key.as_slice())
    .bind(TEST_OWNER_MEMBER)
    .execute(db.get_pool())
    .await
    .expect("seed realm client key");
}

/// `POST /issuer/membership` with `request` signed by the key `label`.
async fn membership(
    base: &str,
    request: actr_protocol::realm_membership::MembershipRequest,
    label: &str,
) -> (reqwest::StatusCode, Value) {
    let response = test_http_client()
        .post(format!("{base}/issuer/membership"))
        .json(&request.sign(&client_key(label)))
        .send()
        .await
        .expect("membership request");
    let status = response.status();
    (status, response.json().await.expect("membership json"))
}

fn membership_request(
    op: actr_protocol::realm_membership::MembershipOp,
    realm: &str,
) -> actr_protocol::realm_membership::MembershipRequest {
    actr_protocol::realm_membership::MembershipRequest::new(
        op,
        realm,
        PROVIDER_ID,
        &Uuid::new_v4().to_string(),
    )
}

/// Redeem `invite` with a new key `label`.
async fn join(base: &str, realm: &str, invite: &str, label: &str) -> (reqwest::StatusCode, Value) {
    let mut request =
        membership_request(actr_protocol::realm_membership::MembershipOp::Join, realm);
    request.invite = Some(
        actr_protocol::realm_membership::RealmInvite::decode(invite)
            .expect("invite text")
            .secret,
    );
    request.public_key = Some(
        base64::engine::general_purpose::STANDARD
            .encode(client_key(label).verifying_key().to_bytes()),
    );
    membership(base, request, label).await
}

/// Register via Issuer HTTP, returns RegisterOk
async fn issuer_register_http(
    base_url: &str,
    realm: &str,
    publisher: &str,
    name: &str,
    service_spec: Option<actr_protocol::ServiceSpec>,
    acl: Option<Acl>,
) -> register_response::RegisterOk {
    issuer_register_http_with_key(
        base_url,
        realm,
        publisher,
        name,
        service_spec,
        acl,
        Some(TEST_CLIENT_KEY),
    )
    .await
}

async fn issuer_register_http_with_key(
    base_url: &str,
    realm: &str,
    publisher: &str,
    name: &str,
    service_spec: Option<actr_protocol::ServiceSpec>,
    acl: Option<Acl>,
    key_label: Option<&str>,
) -> register_response::RegisterOk {
    let result = issuer_register_result(
        base_url,
        realm,
        publisher,
        name,
        service_spec,
        acl,
        key_label,
    )
    .await;
    match result {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => {
            panic!(
                "issuer register failed: code={} message={}",
                err.code, err.message
            );
        }
    }
}

async fn issuer_register_result(
    base_url: &str,
    realm: &str,
    publisher: &str,
    name: &str,
    service_spec: Option<actr_protocol::ServiceSpec>,
    acl: Option<Acl>,
    key_label: Option<&str>,
) -> register_response::Result {
    let register_req = RegisterRequest {
        actr_type: ActrType {
            publisher: publisher.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
        },
        realm: realm.to_string(),
        service_spec,
        acl,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: Some(RegisterAuthMode::Linked as i32),
        signing_public_key: None,
    };
    let client = test_http_client();
    let register_url = format!("{base_url}/issuer/register");
    let body = register_req.encode_to_vec();
    let mut req = client
        .post(&register_url)
        .header("content-type", "application/octet-stream");
    if let Some(label) = key_label {
        req = req.header(
            "x-actr-client-proof",
            client_proof(
                actr_protocol::realm_membership::ProofOp::Register,
                realm,
                &body,
                label,
            ),
        );
    }
    let req = req.body(body);
    let rsp_bytes = req
        .send()
        .await
        .expect("issuer register call")
        .bytes()
        .await
        .expect("issuer register bytes")
        .to_vec();
    let register_rsp = RegisterResponse::decode(&*rsp_bytes).expect("decode register response");
    register_rsp.result.expect("result missing")
}

fn authenticated_ws_request(
    port: u16,
    ok: &register_response::RegisterOk,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let actor_id_str = ok.actr_id.to_string_repr();
    let claims_b64 = base64::engine::general_purpose::STANDARD.encode(&ok.credential.claims);
    let signature_b64 = base64::engine::general_purpose::STANDARD.encode(&ok.credential.signature);
    let key_id = ok.credential.key_id;
    let ws_url = format!(
        "ws://127.0.0.1:{}/signaling/ws?actor_id={}&key_id={}&claims={}&signature={}",
        port,
        urlencoding::encode(&actor_id_str),
        key_id,
        urlencoding::encode(&claims_b64),
        urlencoding::encode(&signature_b64),
    );
    ws_url
        .into_client_request()
        .expect("valid WebSocket request")
}

/// Connect WS with authentication header
async fn connect_ws_authenticated(
    port: u16,
    ok: &register_response::RegisterOk,
) -> (WsWrite, WsRead) {
    let request = authenticated_ws_request(port, ok);
    let (ws_stream, _) = connect_async(request).await.expect("ws connect");
    ws_stream.split()
}

async fn connect_wss(
    port: u16,
    ok: &register_response::RegisterOk,
) -> Result<WsStream, tokio_tungstenite::tungstenite::Error> {
    let mut request = authenticated_ws_request(port, ok);
    *request.uri_mut() = request
        .uri()
        .to_string()
        .replacen("ws://", "wss://", 1)
        .parse()
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(test_tls().2.clone().into()).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
            config,
        ))),
    )
    .await
    .map(|(stream, _)| stream)
}

async fn connect_wss_authenticated(
    port: u16,
    ok: &register_response::RegisterOk,
) -> (WsWrite, WsRead) {
    connect_wss(port, ok).await.expect("wss connect").split()
}

/// Unauthenticated raw WS connection (expected to be rejected — negative test only)
async fn connect_ws_raw(
    port: u16,
) -> Result<(WsWrite, WsRead), tokio_tungstenite::tungstenite::Error> {
    let ws_url = format!("ws://127.0.0.1:{}/signaling/ws", port);
    match connect_async(&ws_url).await {
        Ok((ws_stream, _)) => Ok(ws_stream.split()),
        Err(e) => Err(e),
    }
}

fn make_envelope(flow: signaling_envelope::Flow) -> actr_protocol::SignalingEnvelope {
    actr_protocol::SignalingEnvelope {
        envelope_version: SIGNALING_ENVELOPE_VERSION,
        envelope_id: Uuid::new_v4().to_string(),
        reply_for: None,
        traceparent: None,
        tracestate: None,
        flow: Some(flow),
    }
}

async fn send_envelope(write: &mut WsWrite, env: actr_protocol::SignalingEnvelope) {
    let mut buf = Vec::new();
    env.encode(&mut buf).expect("encode envelope");
    write
        .send(WsMessage::Binary(buf.into()))
        .await
        .expect("send envelope");
}

async fn recv_envelope(read: &mut WsRead) -> actr_protocol::SignalingEnvelope {
    let resp = read.next().await.expect("ws response").expect("ws msg");
    match resp {
        WsMessage::Binary(data) => {
            actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode signaling resp")
        }
        other => panic!("expected binary ws message, got {other:?}"),
    }
}

async fn ws_register_with_spec(
    port: u16,
    publisher: &str,
    name: &str,
    acl: Option<Acl>,
    service_spec: Option<actr_protocol::ServiceSpec>,
) -> (WsWrite, WsRead, register_response::RegisterOk) {
    ws_register_in_realm_with_spec(port, publisher, name, TEST_REALM, acl, service_spec).await
}

async fn ws_register_in_realm_with_spec(
    port: u16,
    publisher: &str,
    name: &str,
    realm: &str,
    acl: Option<Acl>,
    service_spec: Option<actr_protocol::ServiceSpec>,
) -> (WsWrite, WsRead, register_response::RegisterOk) {
    let base_url = format!("http://127.0.0.1:{port}");
    let ok = issuer_register_http(&base_url, realm, publisher, name, service_spec, acl).await;
    let (write, read) = connect_ws_authenticated(port, &ok).await;
    (write, read, ok)
}

async fn ws_register(
    port: u16,
    publisher: &str,
    name: &str,
    acl: Option<Acl>,
) -> (WsWrite, WsRead, register_response::RegisterOk) {
    ws_register_with_spec(port, publisher, name, acl, None).await
}

async fn relay_identity(base: &str, name: &str, acl: Option<Acl>) -> (String, String) {
    relay_identity_in_realm(base, TEST_REALM, name, acl).await
}

fn relay_allow_from(name: &str, realm: &str) -> Acl {
    Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: name.into(),
                version: "1.0.0".into(),
            },
            source_realm: Some(SourceRealm::Realm(realm.into())),
        }],
    }
}

async fn relay_identity_in_realm(
    base: &str,
    realm: &str,
    name: &str,
    acl: Option<Acl>,
) -> (String, String) {
    let registration = issuer_register_http(base, realm, "acme", name, None, acl).await;
    let identity = actr_relay_security::IdentityKey::generate().expect("relay key");
    let request = actr_relay_security::IdentityRequest {
        endpoint: registration.actr_id.to_string_repr(),
        renewal_token: registration.renewal_token.expect("renewal token").to_vec(),
        public_key: identity.public,
        admission_commitment: identity.admission_commitment(),
    };
    let body = serde_json::to_vec(&request).expect("relay identity request");
    let response = reqwest::Client::new()
        .post(format!("{base}/issuer/relay-identity"))
        .header("content-type", "application/json")
        .header(
            "x-actr-client-proof",
            client_proof(
                actr_protocol::realm_membership::ProofOp::RelayIdentity,
                realm,
                &body,
                TEST_CLIENT_KEY,
            ),
        )
        .body(body)
        .send()
        .await
        .expect("relay identity request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let issued: actr_relay_security::IssuedIdentity =
        response.json().await.expect("relay identity");
    let identity = identity
        .certify(
            issued,
            &request.endpoint,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .expect("certify relay identity");
    let token = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&identity.relay_credential()).expect("relay credential"));
    (request.endpoint, token)
}

async fn relay_hello(port: u16, endpoint: &str, token: &str) -> (WsStream, WsMessage) {
    let mut request = format!("ws://127.0.0.1:{port}/relay/ws")
        .into_client_request()
        .expect("relay URL");
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        loran_relay_protocol::SUBPROTOCOL.parse().unwrap(),
    );
    let (mut socket, _) = connect_async(request).await.expect("relay WebSocket");
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&loran_relay_protocol::ClientControl::Hello {
                endpoint: endpoint.into(),
                token: token.into(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .expect("relay hello");
    let response = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("relay hello deadline")
        .expect("relay closed without a response")
        .expect("relay response");
    (socket, response)
}

async fn relay_login(port: u16, endpoint: &str, token: &str) -> WsStream {
    // Issuer registration and relay authorization use the same database;
    // allow the relay's Actor-binding watcher to consume the registration.
    for _ in 0..100 {
        let (socket, response) = relay_hello(port, endpoint, token).await;
        if let WsMessage::Text(text) = response
            && matches!(
                serde_json::from_str::<loran_relay_protocol::ServerControl>(&text),
                Ok(loran_relay_protocol::ServerControl::Welcome { .. })
            )
        {
            return socket;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("registered actor was not admitted by the relay");
}

async fn relay_control(socket: &mut WsStream) -> loran_relay_protocol::ServerControl {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("relay control deadline")
        .expect("relay socket closed")
        .expect("relay frame");
    let WsMessage::Text(text) = message else {
        panic!("expected relay control frame, got {message:?}");
    };
    serde_json::from_str(&text).expect("relay control")
}

async fn send_relay_control(socket: &mut WsStream, control: loran_relay_protocol::ClientControl) {
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&control).unwrap().into(),
        ))
        .await
        .expect("send relay control");
}

async fn relay_pair(source: &mut WsStream, target: &mut WsStream, from: &str, to: &str) -> u64 {
    use loran_relay_protocol::{ClientControl, ServerControl};

    send_relay_control(
        source,
        ClientControl::Connect {
            request: 1,
            peer: to.into(),
        },
    )
    .await;
    let ServerControl::Incoming { channel, peer, .. } = relay_control(target).await else {
        panic!("target did not receive a relay request");
    };
    assert_eq!(peer, from);
    send_relay_control(target, ClientControl::Accept { channel }).await;
    for socket in [source, target] {
        let response = relay_control(socket).await;
        assert!(matches!(response, ServerControl::Connected { channel: id, .. } if id == channel));
    }
    channel
}

async fn with_fullstack_relay<F, Fut>(scenario: F)
where
    F: FnOnce(String, u16) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let harness = LoranHarness::start_with_relay(DEFAULT_TOKEN_TTL).await;
    let task = tokio::spawn(scenario(harness.base_url(), harness.port));
    let result = tokio::time::timeout(Duration::from_secs(30), task).await;
    harness.shutdown();
    result
        .expect("relay integration deadline")
        .expect("relay integration");
}

#[tokio::test]
#[serial]
async fn relay_issuer_credentials_authenticate_real_websocket_channels() {
    use loran_relay_protocol::encode_data;

    with_fullstack_relay(|base, port| async move {
        let (alice, alice_token) = relay_identity(&base, "relay-alice", None).await;
        let (bob, bob_token) = relay_identity(
            &base,
            "relay-bob",
            Some(relay_allow_from("relay-alice", TEST_REALM)),
        )
        .await;
        let mut a = relay_login(port, &alice, &alice_token).await;
        let mut b = relay_login(port, &bob, &bob_token).await;
        let (_, wrong_identity) = relay_hello(port, &bob, &alice_token).await;
        assert!(matches!(wrong_identity, WsMessage::Close(_)));
        let channel = relay_pair(&mut a, &mut b, &alice, &bob).await;

        let frame = encode_data(channel, b"issuer-authenticated relay payload").unwrap();
        a.send(WsMessage::Binary(frame.clone())).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(5), b.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(received, WsMessage::Binary(frame));
    })
    .await;
}

async fn query_route_candidates(
    write: &mut WsWrite,
    read: &mut WsRead,
    source: &register_response::RegisterOk,
    target_publisher: &str,
    target_name: &str,
) -> Vec<actr_protocol::ActrId> {
    let route_req = actr_protocol::ActrToSignaling {
        source: source.actr_id.clone(),
        credential: source.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: target_publisher.into(),
                        name: target_name.into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: "".into(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 8,
                            ranking_factors: vec![],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };

    send_envelope(
        write,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;

    let resp = recv_envelope(read).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => ok.candidates,
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }
}

async fn query_discovery_entries(
    write: &mut WsWrite,
    read: &mut WsRead,
    source: &register_response::RegisterOk,
    publisher: Option<&str>,
) -> Vec<actr_protocol::discovery_response::TypeEntry> {
    let request = actr_protocol::ActrToSignaling {
        source: source.actr_id.clone(),
        credential: source.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::DiscoveryRequest(
            actr_protocol::DiscoveryRequest {
                publisher: publisher.map(str::to_string),
                limit: Some(64),
            },
        )),
    };
    send_envelope(
        write,
        make_envelope(signaling_envelope::Flow::ActrToServer(request)),
    )
    .await;

    let response = recv_envelope(read).await;
    match response.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::DiscoveryResponse(response)) => {
                match response.result {
                    Some(actr_protocol::discovery_response::Result::Success(ok)) => ok.entries,
                    other => panic!("unexpected discovery result {other:?}"),
                }
            }
            other => panic!("unexpected discovery payload {other:?}"),
        },
        other => panic!("unexpected discovery flow {other:?}"),
    }
}

async fn query_service_spec_result(
    write: &mut WsWrite,
    read: &mut WsRead,
    source: &register_response::RegisterOk,
    name: &str,
) -> actr_protocol::get_service_spec_response::Result {
    let request = actr_protocol::ActrToSignaling {
        source: source.actr_id.clone(),
        credential: source.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::GetServiceSpecRequest(
                actr_protocol::GetServiceSpecRequest {
                    name: name.to_string(),
                },
            ),
        ),
    };
    send_envelope(
        write,
        make_envelope(signaling_envelope::Flow::ActrToServer(request)),
    )
    .await;

    let response = recv_envelope(read).await;
    match response.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::GetServiceSpecResponse(response)) => {
                response.result.expect("service spec result missing")
            }
            other => panic!("unexpected service spec payload {other:?}"),
        },
        other => panic!("unexpected service spec flow {other:?}"),
    }
}

async fn query_service_spec(
    write: &mut WsWrite,
    read: &mut WsRead,
    source: &register_response::RegisterOk,
    name: &str,
) -> actr_protocol::ServiceSpec {
    match query_service_spec_result(write, read, source, name).await {
        actr_protocol::get_service_spec_response::Result::Success(spec) => spec,
        other => panic!("unexpected service spec result {other:?}"),
    }
}

async fn ping_and_wait_for_pong(
    write: &mut WsWrite,
    read: &mut WsRead,
    source: &register_response::RegisterOk,
) {
    let ping = actr_protocol::ActrToSignaling {
        source: source.actr_id.clone(),
        credential: source.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 0,
                power_reserve: 1.0,
                mailbox_backlog: 0.0,
                sticky_client_ids: vec![],
            },
        )),
    };
    send_envelope(
        write,
        make_envelope(signaling_envelope::Flow::ActrToServer(ping)),
    )
    .await;

    let response = recv_envelope(read).await;
    match response.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg))
            if matches!(
                server_msg.payload,
                Some(signaling_to_actr::Payload::Pong(_))
            ) => {}
        other => panic!("expected Pong, got {other:?}"),
    }
}

fn make_service_spec(
    fingerprint: &str,
    package: &str,
    content: &str,
) -> actr_protocol::ServiceSpec {
    actr_protocol::ServiceSpec {
        name: package.to_string(),
        description: Some("integration spec".into()),
        fingerprint: fingerprint.into(),
        protobufs: vec![actr_protocol::service_spec::Protobuf {
            package: package.into(),
            content: content.into(),
            fingerprint: format!("proto-fp::{fingerprint}"),
        }],
        published_at: None,
        tags: vec!["stable".into()],
    }
}

async fn wait_for_health(url: &str, child: &mut Child, log_path: &Path) {
    let client = test_http_client();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            panic!("loran exited early: status={status:?}\nlogs:\n{log}");
        }

        if let Ok(resp) = client.get(url).send().await
            && resp.status().is_success()
        {
            return;
        }
        if start.elapsed() > START_TIMEOUT {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            panic!("health check not ready at {}\nlogs:\n{}", url, log);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the Signer gRPC service is healthy on its dedicated listener
/// (SGN-02). The Signer signing oracle is served on its own port, not the
/// shared HTTP base, so its readiness is probed with an authenticated gRPC
/// health check against `grpc_endpoint` (e.g. `http://127.0.0.1:{signer_port}`).
async fn wait_for_signer_ready(grpc_endpoint: &str, child: &mut Child, log_path: &Path) {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            panic!("loran exited early: status={status:?}\nlogs:\n{log}");
        }

        let config = GrpcClientConfig {
            endpoint: grpc_endpoint.to_string(),
            loran_shared_key: LORAN_SHARED_KEY.to_string(),
            timeout_seconds: 2,
            enable_tls: false,
            tls_domain: None,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        if let Ok(mut grpc) = GrpcClient::new(&config).await
            && let Ok(status) = grpc.health_check().await
            && status == "healthy"
        {
            return;
        }
        if start.elapsed() > START_TIMEOUT {
            let log = fs::read_to_string(log_path).unwrap_or_default();
            panic!("signer gRPC not ready at {}\nlogs:\n{}", grpc_endpoint, log);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn graceful_shutdown(mut child: Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return,
            Ok(None) => {
                if start.elapsed() > SHUTDOWN_TIMEOUT {
                    let _ = child.kill();
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return,
        }
    }
}

#[tokio::test]
#[serial]
async fn issuer_cors_uses_the_production_router() {
    use reqwest::header::{
        ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
        ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
        ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
    };
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let client = reqwest::Client::new();
    let origin = "http://localhost:5173";
    for route in ["register", "renew", "reissue", "membership"] {
        let url = format!("{}/issuer/{route}", harness.base_url());
        let response = client
            .request(reqwest::Method::OPTIONS, &url)
            .header(ORIGIN, origin)
            .header(ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                ACCESS_CONTROL_REQUEST_HEADERS,
                "content-type,x-actr-client-proof",
            )
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{route}: {}",
            response.status()
        );
        let headers = response.headers();
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            headers[ACCESS_CONTROL_ALLOW_METHODS]
                .to_str()
                .unwrap()
                .contains("POST")
        );
        let allowed = headers[ACCESS_CONTROL_ALLOW_HEADERS].to_str().unwrap();
        assert!(allowed.contains("content-type"), "{route}: {allowed}");
        assert!(
            allowed.contains("x-actr-client-proof"),
            "{route}: {allowed}"
        );
        assert!(!headers.contains_key(ACCESS_CONTROL_ALLOW_CREDENTIALS));

        // Both protobuf and JSON requests retain CORS on handler errors;
        // permission to reach the handler does not bypass authentication.
        let response = client
            .post(&url)
            .header(ORIGIN, origin)
            .header(
                "content-type",
                if route == "membership" {
                    "application/json"
                } else {
                    "application/x-protobuf"
                },
            )
            .header("x-actr-client-proof", "invalid-proof")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            response.headers()[ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .contains("retry-after")
        );
        if route == "membership" {
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        }
    }
    // The public Issuer policy must not expose the private admin API.
    let response = client
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/admin/api/auth/login", harness.base_url()),
        )
        .header(ORIGIN, origin)
        .header(ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    assert!(!response.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn loran_end_to_end_register_and_health() {
    let mut harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;

    let base = harness.base_url();
    let issuer_health = format!("{base}/issuer/health");
    let signaling_health = format!("{base}/signaling/health");

    let log_path = harness.log_path().to_path_buf();
    let signer_endpoint = format!("http://127.0.0.1:{}", harness.signer_port);
    wait_for_signer_ready(&signer_endpoint, &mut harness.child, &log_path).await;
    wait_for_health(&issuer_health, &mut harness.child, &log_path).await;
    wait_for_health(&signaling_health, &mut harness.child, &log_path).await;
    ensure_realm(&harness.data_dir, TEST_REALM).await;

    let client = reqwest::Client::new();

    // Signer health via authenticated gRPC call on the dedicated listener
    let mut signer_client = GrpcClient::new(&GrpcClientConfig {
        endpoint: signer_endpoint.clone(),
        loran_shared_key: LORAN_SHARED_KEY.to_string(),
        timeout_seconds: 5,
        enable_tls: false,
        tls_domain: None,
        ca_cert: None,
        client_cert: None,
        client_key: None,
    })
    .await
    .expect("create signer grpc client");
    let signer_status = signer_client
        .health_check()
        .await
        .expect("signer grpc health");
    assert_eq!(signer_status, "healthy");

    // Issuer health JSON
    let issuer_resp = client
        .get(&issuer_health)
        .send()
        .await
        .expect("issuer health");
    assert!(issuer_resp.status().is_success());
    let issuer_body: Value = issuer_resp.json().await.expect("issuer health json");
    assert_eq!(issuer_body["status"], "healthy");

    // Signaling health plain text
    let sig_resp = client
        .get(&signaling_health)
        .send()
        .await
        .expect("sig health");
    assert!(sig_resp.status().is_success());
    let sig_text = sig_resp.text().await.expect("sig text");
    assert!(
        sig_text.to_lowercase().contains("healthy"),
        "signaling health text: {sig_text}"
    );

    // Register an actor via Issuer HTTP (protobuf body)
    let register_req = RegisterRequest {
        actr_type: ActrType {
            publisher: "acme".to_string(),
            name: "device".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: Some(RegisterAuthMode::Linked as i32),
        signing_public_key: None,
    };
    let body = register_req.encode_to_vec();
    let register_url = format!("{base}/issuer/register");
    let rsp_bytes = client
        .post(&register_url)
        .header(
            "x-actr-client-proof",
            client_proof(
                actr_protocol::realm_membership::ProofOp::Register,
                TEST_REALM,
                &body,
                TEST_CLIENT_KEY,
            ),
        )
        .body(body)
        .send()
        .await
        .expect("register call")
        .bytes()
        .await
        .expect("register bytes")
        .to_vec();
    let register_rsp =
        actr_protocol::RegisterResponse::decode(&*rsp_bytes).expect("decode register response");
    let ok = match register_rsp.result.expect("result missing") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => {
            panic!("register failed: {:?}", err);
        }
    };
    assert_eq!(ok.actr_id.realm, TEST_REALM);

    // Validate credential through AIdCredentialValidator — reads from the same
    // signaling_key_cache.db that loran wrote during Issuer key loading.
    AIdCredentialValidator::init(&harness.data_dir)
        .await
        .expect("validator init");
    let (claims, _) =
        AIdCredentialValidator::check(&ok.credential, TEST_REALM, "localhost", "actr.signaling")
            .await
            .expect("validate credential");
    assert_eq!(claims.provider, "localhost");
    assert_eq!(claims.realm, TEST_REALM);
    assert_eq!(claims.issuer, TEST_REALM);
    assert_eq!(claims.actor_id, ok.actr_id.to_string_repr());
    assert_eq!(claims.actor_type, register_req.actr_type);

    // WebSocket signaling ping/pong with valid credential
    let (mut write, mut read) = connect_ws_authenticated(harness.port, &ok).await;

    let ping_msg = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 100,
                mailbox_backlog: 0.0,
                power_reserve: 80.0,
                ..Default::default()
            },
        )),
    };
    let envelope = actr_protocol::SignalingEnvelope {
        envelope_version: SIGNALING_ENVELOPE_VERSION,
        envelope_id: Uuid::new_v4().to_string(),
        reply_for: None,
        traceparent: None,
        tracestate: None,
        flow: Some(actr_protocol::signaling_envelope::Flow::ActrToServer(
            ping_msg,
        )),
    };
    let mut buf = Vec::new();
    envelope.encode(&mut buf).expect("encode envelope");
    write
        .send(WsMessage::Binary(buf.into()))
        .await
        .expect("send ping");

    let resp = read.next().await.expect("ws response").expect("ws msg");
    let pong_env = match resp {
        WsMessage::Binary(data) => {
            actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode signaling resp")
        }
        other => panic!("expected binary ws message, got {other:?}"),
    };
    match pong_env.flow {
        Some(actr_protocol::signaling_envelope::Flow::ServerToActr(server_msg)) => {
            match server_msg.payload {
                Some(actr_protocol::signaling_to_actr::Payload::Pong(_)) => {}
                other => panic!("expected Pong, got {other:?}"),
            }
        }
        other => panic!("unexpected flow: {other:?}"),
    }

    // Non-binary frame should be ignored without breaking the connection.
    write
        .send(WsMessage::Text("not-protobuf".into()))
        .await
        .expect("send text frame");

    let ping_after_text = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 99,
                mailbox_backlog: 0.0,
                power_reserve: 75.0,
                ..Default::default()
            },
        )),
    };
    let mut text_ping_buf = Vec::new();
    actr_protocol::SignalingEnvelope {
        envelope_version: SIGNALING_ENVELOPE_VERSION,
        envelope_id: Uuid::new_v4().to_string(),
        reply_for: None,
        traceparent: None,
        tracestate: None,
        flow: Some(actr_protocol::signaling_envelope::Flow::ActrToServer(
            ping_after_text,
        )),
    }
    .encode(&mut text_ping_buf)
    .expect("encode ping after text");
    write
        .send(WsMessage::Binary(text_ping_buf.into()))
        .await
        .expect("send ping after text");

    let resp = read.next().await.expect("ws response").expect("ws msg");
    let pong_after_text = match resp {
        WsMessage::Binary(data) => {
            actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode signaling resp")
        }
        other => panic!("expected binary ws message, got {other:?}"),
    };
    match pong_after_text.flow {
        Some(actr_protocol::signaling_envelope::Flow::ServerToActr(server_msg)) => {
            match server_msg.payload {
                Some(actr_protocol::signaling_to_actr::Payload::Pong(_)) => {}
                other => panic!("expected Pong after text frame, got {other:?}"),
            }
        }
        other => panic!("unexpected flow after text frame: {other:?}"),
    }

    // Tamper credential to ensure 401 is returned
    let mut bad_cred = ok.credential.clone();
    if !bad_cred.signature.is_empty() {
        let mut tampered = bad_cred.signature.to_vec();
        tampered[0] ^= 0xFF;
        bad_cred.signature = tampered.into();
    }
    let bad_msg = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: bad_cred,
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 50,
                mailbox_backlog: 1.0,
                power_reserve: 50.0,
                ..Default::default()
            },
        )),
    };
    let mut buf = Vec::new();
    actr_protocol::SignalingEnvelope {
        envelope_version: SIGNALING_ENVELOPE_VERSION,
        envelope_id: Uuid::new_v4().to_string(),
        reply_for: None,
        traceparent: None,
        tracestate: None,
        flow: Some(actr_protocol::signaling_envelope::Flow::ActrToServer(
            bad_msg,
        )),
    }
    .encode(&mut buf)
    .expect("encode bad envelope");
    write
        .send(WsMessage::Binary(buf.into()))
        .await
        .expect("send bad ping");

    let resp = read.next().await.expect("ws response").expect("ws msg");
    let err_env = match resp {
        WsMessage::Binary(data) => {
            actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode signaling resp")
        }
        other => panic!("expected binary ws message, got {other:?}"),
    };
    match err_env.flow {
        Some(actr_protocol::signaling_envelope::Flow::ServerToActr(server_msg)) => {
            match server_msg.payload {
                Some(actr_protocol::signaling_to_actr::Payload::Error(err)) => {
                    assert_eq!(err.code, 401, "expected 401 for bad credential");
                }
                other => panic!("expected Error, got {other:?}"),
            }
        }
        other => panic!("unexpected flow: {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn issuer_register_returns_error_response_for_malformed_protobuf() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let client = reqwest::Client::new();
    let register_url = format!("{}/issuer/register", harness.base_url());

    let response = client
        .post(&register_url)
        .header("content-type", "application/octet-stream")
        .body(vec![0xff, 0x00, 0x10, 0x80])
        .send()
        .await
        .expect("send malformed register request");
    assert!(
        response.status().is_success(),
        "issuer register should respond with protobuf error body"
    );

    let rsp_bytes = response.bytes().await.expect("read response body").to_vec();
    let register_rsp = RegisterResponse::decode(&*rsp_bytes).expect("decode register response");
    match register_rsp.result.expect("result missing") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 400, "expected bad-request protobuf error");
            assert!(
                err.message.contains("Invalid protobuf"),
                "unexpected error message: {}",
                err.message
            );
        }
        other => panic!("expected register error for malformed protobuf, got {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn issuer_register_rejects_non_preprovisioned_realm() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let client = reqwest::Client::new();
    let register_url = format!("{}/issuer/register", harness.base_url());

    let register_req = RegisterRequest {
        actr_type: ActrType {
            publisher: "acme".to_string(),
            name: "realm-device".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: "other@example.com".to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: Some(RegisterAuthMode::Linked as i32),
        signing_public_key: None,
    };

    let body = register_req.encode_to_vec();
    let rsp_bytes = client
        .post(&register_url)
        .header(
            "x-actr-client-proof",
            client_proof(
                actr_protocol::realm_membership::ProofOp::Register,
                "other@example.com",
                &body,
                TEST_CLIENT_KEY,
            ),
        )
        .body(body)
        .send()
        .await
        .expect("send register with non-preprovisioned realm")
        .bytes()
        .await
        .expect("read register response")
        .to_vec();
    let register_rsp = RegisterResponse::decode(&*rsp_bytes).expect("decode register response");
    match register_rsp.result.expect("result missing") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403, "non-preprovisioned realm should be denied");
            // Unauthenticated callers learn nothing about the Realm.
            assert!(
                err.message.starts_with("client_unknown:"),
                "{}",
                err.message
            );
        }
        other => panic!("expected register error for non-preprovisioned realm, got {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn authority_descriptor_publishes_signed_realm_signing_keys() {
    use ed25519_dalek::Verifier as _;

    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let url = format!("{}/.well-known/actr", harness.base_url());
    let client = reqwest::Client::new();

    // The Realm signing key is generated at startup, possibly after health.
    let mut response = client.get(&url).send().await.expect("descriptor request");
    for _ in 0..50 {
        if response.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        response = client.get(&url).send().await.expect("descriptor request");
    }
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let body = response.bytes().await.expect("descriptor body");
    let descriptor: actr_protocol::AuthorityDescriptor =
        serde_json::from_slice(&body).expect("parse descriptor");
    let authority = "actr://localhost".parse().expect("authority");
    descriptor
        .validate_for(&authority)
        .expect("valid descriptor");

    let realms = descriptor.require_service("realms").expect("realms");
    assert!(
        realms.href.ends_with("/actr/realms/localhost"),
        "{}",
        realms.href
    );
    let management = descriptor
        .require_service("realm-management")
        .expect("realm-management");
    assert!(management.href.ends_with("/actr/realm-management"));

    // The member is self-signed over its JCS form without `signature`.
    let keys = descriptor.realm_signing_keys.expect("realm_signing_keys");
    assert_eq!(keys.keys.len(), 1);
    assert_eq!(keys.keys[0].id, "realm-signing:localhost");
    assert_eq!(keys.signature.key_id, "realm-signing:localhost");
    let mut payload: Value = serde_json::from_slice(&body).expect("json");
    let payload = payload["realm_signing_keys"]
        .as_object_mut()
        .map(|member| {
            member.remove("signature");
            actr_protocol::realm_profile::jcs(&Value::Object(member.clone()))
        })
        .expect("member object");
    let b64 = base64::engine::general_purpose::STANDARD;
    let public_key: [u8; 32] = b64
        .decode(&keys.keys[0].public_key)
        .expect("public key")
        .try_into()
        .expect("32 bytes");
    let signature: [u8; 64] = b64
        .decode(&keys.signature.value)
        .expect("signature")
        .try_into()
        .expect("64 bytes");
    ed25519_dalek::VerifyingKey::from_bytes(&public_key)
        .expect("verifying key")
        .verify(
            payload.as_bytes(),
            &ed25519_dalek::Signature::from_bytes(&signature),
        )
        .expect("member signature verifies");

    // HEAD returns the same headers and no body.
    let head = client.head(&url).send().await.expect("HEAD descriptor");
    assert_eq!(head.status(), reqwest::StatusCode::OK);
    assert_eq!(head.headers()["access-control-allow-origin"], "*");
    assert!(head.bytes().await.expect("HEAD body").is_empty());

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn provider_domain_realm_profiles_are_signed_served_and_retired() {
    use actr_protocol::realm_profile::{RealmProfile, RealmStatus};

    let harness = LoranHarness::start_tls(DEFAULT_TOKEN_TTL).await;
    let client = test_http_client();
    let base = harness.base_url();
    let name = "k7m2q9xh@localhost";
    let public_url = format!("{base}/actr/realms/localhost/k7m2q9xh");

    // The Realm signing key is generated at startup, possibly after health.
    let mut descriptor = client.get(format!("{base}/.well-known/actr")).send().await;
    for _ in 0..50 {
        if descriptor
            .as_ref()
            .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        descriptor = client.get(format!("{base}/.well-known/actr")).send().await;
    }
    let descriptor: actr_protocol::AuthorityDescriptor = descriptor
        .expect("descriptor request")
        .json()
        .await
        .expect("descriptor json");
    let keys = descriptor.realm_signing_keys.expect("realm_signing_keys");

    let login: Value = client
        .post(format!("{base}/admin/api/auth/login"))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("admin login request")
        .json()
        .await
        .expect("parse admin login response");
    let token = login["token"].as_str().expect("admin login token");
    let create = |body: Value| {
        client
            .post(format!("{base}/admin/api/realms"))
            .bearer_auth(token)
            .json(&body)
            .send()
    };
    let package = json!({
        "href": "https://cdn.example/notes-1.0.actr",
        "sha256": "ab".repeat(32),
        "type": "acme.com/notes@1.0.0",
    });

    // Under the Provider domain the first Profile needs a package; other
    // domains get no Loran-signed Profile.
    let missing = create(json!({ "realm": name })).await.expect("create");
    assert_eq!(missing.status(), reqwest::StatusCode::BAD_REQUEST);
    let foreign = create(json!({ "realm": SECOND_REALM, "package": package }))
        .await
        .expect("create");
    assert_eq!(foreign.status(), reqwest::StatusCode::BAD_REQUEST);

    let created = create(json!({ "realm": name, "package": package, "display_name": "Notes" }))
        .await
        .expect("create");
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let created: Value = created.json().await.expect("create json");
    assert_eq!(created["realm"]["status"], "active");

    let response = client.get(&public_url).send().await.expect("profile");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert!(response.headers().contains_key("cache-control"));
    let body = response.bytes().await.expect("profile body");
    let first = RealmProfile::verify(&body, name, &keys).expect("verifies");
    assert_eq!(first.sequence, 1);
    assert_eq!(first.realm.display_name, "Notes");
    assert_eq!(first.provider.id, "localhost");
    assert!(first.issuer.url.starts_with("https://"));
    assert!(
        first
            .provider
            .signaling
            .iter()
            .all(|endpoint| endpoint.url.starts_with("wss://"))
    );
    assert_eq!(
        first.package.as_ref().expect("package").sha256,
        "ab".repeat(32)
    );

    let admin: Value = client
        .get(format!(
            "{base}/admin/api/realms/k7m2q9xh%40localhost/profile"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("admin profile")
        .json()
        .await
        .expect("admin profile json");
    assert_eq!(admin["profile"]["sequence"], 1);

    // Unknown Realms and Realms without a Profile are not found. Loran as
    // Actr Facility Provider creates no Realm outside its domain.
    let created = create(json!({ "realm": SECOND_REALM }))
        .await
        .expect("create");
    assert_eq!(created.status(), reqwest::StatusCode::BAD_REQUEST);
    ensure_realm(&harness.data_dir, SECOND_REALM).await;
    for path in ["localhost/missing", "acme.com/second"] {
        let response = client
            .get(format!("{base}/actr/realms/{path}"))
            .send()
            .await
            .expect("profile");
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
    }

    // Retiring signs a retired Profile, which stays served.
    let retired = client
        .post(format!(
            "{base}/admin/api/realms/k7m2q9xh%40localhost/retire"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("retire");
    assert_eq!(retired.status(), reqwest::StatusCode::OK);
    let body = client
        .get(&public_url)
        .send()
        .await
        .expect("profile")
        .bytes()
        .await
        .expect("profile body");
    let last = RealmProfile::verify(&body, name, &keys).expect("verifies");
    first.check_successor(&last).expect("successor");
    assert_eq!(last.realm.status, RealmStatus::Retired);

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn realm_management_requests_are_checked_before_any_state_changes() {
    use actr_protocol::realm_management::{ManagementOp, ManagementRequest};

    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let client = reqwest::Client::new();
    let url = format!("{}/actr/realm-management", harness.base_url());
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let request = |realm: &str, provider: &str, nonce: &str| {
        let mut request = ManagementRequest::new(ManagementOp::CreateRealm, realm, provider, nonce);
        request.package = Some(actr_protocol::realm_profile::RealmPackage {
            href: "https://cdn.example/notes-1.0.actr".to_string(),
            sha256: "ab".repeat(32),
            r#type: "acme.com/notes@1.0.0".to_string(),
        });
        request.sign("rs-1", &key)
    };
    let register = |publisher: &str, nonce: &str| {
        let mut request =
            ManagementRequest::new(ManagementOp::RegisterPublisher, "", "localhost", nonce);
        request.publisher = publisher.to_string();
        request.sign("root-1", &key)
    };

    for (body, status) in [
        (json!({ "op": "create_realm" }), 400),
        (request("a1@acme.test", "other.example", "n-1"), 400),
        // Realms under the Provider domain are the operator's.
        (request("a1@localhost", "localhost", "n-2"), 403),
        // The domain's descriptor cannot be fetched.
        (request("a1@acme.test", "localhost", "n-3"), 503),
        // register_publisher names a normalized publisher domain.
        (register("Acme.Test", "n-4"), 400),
        (register("acme.test", "n-5"), 503),
    ] {
        let response = client.post(&url).json(&body).send().await.expect("post");
        assert_eq!(response.status().as_u16(), status, "{body}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let login: Value = client
        .post(format!("{}/admin/api/auth/login", harness.base_url()))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("login json");
    let token = login["token"].as_str().expect("token");
    let pool_realms: Value = client
        .get(format!("{}/admin/api/realms", harness.base_url()))
        .bearer_auth(token)
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    assert_eq!(pool_realms["total_count"], 1);

    // Publishers register only through register_publisher.
    let applications = client
        .post(format!(
            "{}/admin/api/publishers/applications",
            harness.base_url()
        ))
        .bearer_auth(token)
        .json(&json!({ "publisher": "acme.test" }))
        .send()
        .await
        .expect("applications");
    assert!(
        matches!(applications.status().as_u16(), 404 | 405),
        "{}",
        applications.status()
    );

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn issuer_has_no_global_signing_key() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let client = reqwest::Client::new();
    let base = harness.base_url();

    // Credentials are signed with each Realm's issuer key; the old global
    // rotating key and its endpoints are gone.
    for path in ["/issuer/current-key", "/issuer/signing-pubkey"] {
        let response = client
            .get(format!("{base}{path}"))
            .send()
            .await
            .expect("issuer key request");
        assert_eq!(response.status(), 404, "{path} should no longer exist");
    }
    let public_rotate = client
        .post(format!("{base}/issuer/rotate-key"))
        .send()
        .await
        .expect("public rotate-key request");
    assert_eq!(public_rotate.status(), 404);

    let login: Value = client
        .post(format!("{base}/admin/api/auth/login"))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("admin login request")
        .json()
        .await
        .expect("parse admin login response");
    let token = login["token"]
        .as_str()
        .expect("admin login should return token");
    let admin_rotate = client
        .post(format!("{base}/admin/api/services/issuer/keys/rotate"))
        .bearer_auth(token)
        .send()
        .await
        .expect("admin rotate request");
    assert!(
        matches!(admin_rotate.status().as_u16(), 404 | 405),
        "admin rotate should no longer exist: {}",
        admin_rotate.status()
    );

    let health: Value = client
        .get(format!("{base}/issuer/health"))
        .send()
        .await
        .expect("issuer health")
        .json()
        .await
        .expect("issuer health json");
    assert!(health.get("key_cache").is_none(), "{health}");
    assert!(health["realm_keys"].is_u64(), "{health}");

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn issuer_register_requires_a_registered_client_key() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let base = harness.base_url();
    let register = |label: Option<&'static str>| {
        let base = base.clone();
        async move {
            issuer_register_result(&base, TEST_REALM, "acme", "key-check", None, None, label).await
        }
    };

    // Without a proof, or with a key the Realm does not know, nothing is
    // said about the Realm.
    match register(None).await {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 400);
            assert!(
                err.message.starts_with("invalid_request:"),
                "{}",
                err.message
            );
        }
        other => panic!("registered without a client proof: {other:?}"),
    }
    match register(Some("unregistered-key")).await {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403);
            assert!(
                err.message.starts_with("client_unknown:"),
                "{}",
                err.message
            );
        }
        other => panic!("registered with an unknown key: {other:?}"),
    }
    match register(Some(TEST_CLIENT_KEY)).await {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id.realm, TEST_REALM);
            let claims =
                actr_protocol::IdentityClaims::decode(ok.credential.claims.as_ref()).unwrap();
            assert_eq!(claims.member, TEST_OWNER_MEMBER);
            assert_eq!(claims.role(), actr_protocol::MemberRole::Owner);
        }
        other => panic!("expected register success, got {other:?}"),
    }

    harness.shutdown();
}

/// Removing a member stops its Actors at once: the connection closes with
/// 1008, it does not reconnect and gets no new credential.
#[tokio::test]
#[serial]
async fn removing_a_member_closes_its_connections_and_refuses_it() {
    use actr_protocol::realm_membership::{MembershipOp, Role};
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let base = harness.base_url();

    let mut invite = membership_request(MembershipOp::CreateInvite, TEST_REALM);
    invite.role = Some(Role::Editor);
    invite.invite_expires_at = Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
    let (status, created) = membership(&base, invite, TEST_CLIENT_KEY).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{created}");
    let (status, joined) = join(
        &base,
        TEST_REALM,
        created["invite"].as_str().unwrap(),
        "fullstack-editor",
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{joined}");
    let member = joined["member_id"].as_str().unwrap().to_string();

    let editor = issuer_register_http_with_key(
        &base,
        TEST_REALM,
        "acme",
        "editor-device",
        None,
        None,
        Some("fullstack-editor"),
    )
    .await;
    let (_write, mut read) = connect_ws_authenticated(harness.port, &editor).await;

    let mut remove = membership_request(MembershipOp::RemoveMember, TEST_REALM);
    remove.member_id = Some(member);
    let (status, removed) = membership(&base, remove, TEST_CLIENT_KEY).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{removed}");

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match read.next().await {
                Some(Ok(WsMessage::Close(frame))) => return frame,
                Some(Ok(_)) => continue,
                other => panic!("connection ended without a Close frame: {other:?}"),
            }
        }
    })
    .await
    .expect("the connection closes");
    let frame = closed.expect("close frame");
    assert_eq!(u16::from(frame.code), 1008);
    assert_eq!(frame.reason.as_str(), "client revoked or replaced");

    assert!(
        connect_async(authenticated_ws_request(harness.port, &editor))
            .await
            .is_err(),
        "a removed member's Actor does not reconnect"
    );
    match issuer_register_result(
        &base,
        TEST_REALM,
        "acme",
        "editor-again",
        None,
        None,
        Some("fullstack-editor"),
    )
    .await
    {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403);
            assert!(
                err.message.starts_with("client_revoked:"),
                "{}",
                err.message
            );
        }
        other => panic!("a removed member registered: {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn admin_login_limits_reject_before_hashing_and_leave_services_responsive() {
    let harness = LoranHarness::start_with_login_limits(
        DEFAULT_TOKEN_TTL,
        Some("per_ip_per_minute = 1\nper_ip_burst = 1"),
    )
    .await;
    let client = reqwest::Client::new();
    let url = format!("{}/admin/api/auth/login", harness.base_url());
    let first = client
        .post(&url)
        .json(&json!({"password": "wrong-test-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), reqwest::StatusCode::UNAUTHORIZED);
    let denied = client
        .post(&url)
        .json(&json!({"password": "testpassword123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert!(denied.headers().contains_key("retry-after"));
    let health = client
        .get(format!("{}/issuer/health", harness.base_url()))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());
    harness.shutdown();
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn signer_authentication_reload_requires_restart_and_preserves_live_policy() {
    let mut harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let config_path = harness.tmp.path().join("config.toml");
    let mut config: toml::Value =
        toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let replacement_key = "abcdef0123456789abcdef0123456789";
    config["loran_shared_key"] = toml::Value::String(replacement_key.into());
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(
        unsafe { libc::kill(harness.child.id() as libc::pid_t, libc::SIGHUP) },
        0
    );
    tokio::time::timeout(START_TIMEOUT, async {
        loop {
            if fs::read_to_string(harness.log_path())
                .unwrap_or_default()
                .contains("Signer listener/security change requires a full restart")
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("reload must explicitly require restart");
    let signer_config = |key: &str| GrpcClientConfig {
        endpoint: format!("http://127.0.0.1:{}", harness.signer_port),
        loran_shared_key: key.into(),
        timeout_seconds: 5,
        enable_tls: false,
        tls_domain: None,
        ca_cert: None,
        client_cert: None,
        client_key: None,
    };
    let old_config = signer_config(LORAN_SHARED_KEY);
    let new_config = signer_config(replacement_key);
    let mut old_client = GrpcClient::new(&old_config).await.unwrap();
    assert_eq!(old_client.health_check().await.unwrap(), "healthy");
    let mut new_client = GrpcClient::new(&new_config).await.unwrap();
    assert!(new_client.health_check().await.is_err());

    // A full restart, rather than router reload, adopts the new policy.
    let log_path = harness.log_path().to_path_buf();
    graceful_shutdown(harness.child);
    harness.child = spawn_loran(&config_path, &log_path);
    tokio::time::timeout(START_TIMEOUT, async {
        loop {
            if let Ok(mut client) = GrpcClient::new(&new_config).await
                && client.health_check().await.is_ok()
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("restart must adopt the new Signer authentication policy");
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn issuer_health_and_endpoints_degrade_when_signer_dependency_is_unreachable() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config_with_issuer_signer_endpoint(
        tmp.path(),
        port,
        DEFAULT_TOKEN_TTL,
        "http://127.0.0.1:1",
    );
    let log_path = tmp.path().join("loran_fullstack_issuer_degraded.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;

    // With lazy Signer connection, Issuer routes are mounted but degrade at runtime
    let client = reqwest::Client::new();
    let issuer_health = client
        .get(format!("{base}/issuer/health"))
        .send()
        .await
        .expect("issuer health request");
    assert_eq!(issuer_health.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = issuer_health.json().await.expect("health json");
    assert_eq!(
        body["status"], "degraded",
        "Issuer should report degraded when Signer is unreachable"
    );
    assert_eq!(body["signer_service"], "failed");

    let register_req = RegisterRequest {
        actr_type: ActrType {
            publisher: "acme".to_string(),
            name: "svc".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: Some(RegisterAuthMode::Linked as i32),
        signing_public_key: None,
    };
    let body = register_req.encode_to_vec();
    let register_resp = client
        .post(format!("{base}/issuer/register"))
        .header(
            "x-actr-client-proof",
            client_proof(
                actr_protocol::realm_membership::ProofOp::Register,
                TEST_REALM,
                &body,
                TEST_CLIENT_KEY,
            ),
        )
        .body(body)
        .send()
        .await
        .expect("issuer register request");
    // HTTP 200 but protobuf body should contain an error
    let resp_bytes = register_resp.bytes().await.expect("read register body");
    let resp = RegisterResponse::decode(resp_bytes).expect("decode register response");
    assert!(
        matches!(resp.result, Some(register_response::Result::Error(_))),
        "Issuer register should return protobuf error when Signer is unreachable, got {:?}",
        resp.result
    );

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_rejects_unauthenticated_ws_connection() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;

    // Unauthenticated raw connection should be rejected (401 Unauthorized → WS upgrade failure)
    let result = connect_ws_raw(harness.port).await;
    assert!(
        result.is_err(),
        "unauthenticated WS connection should be rejected"
    );

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_handshake_identity_reconnect_replaces_stale_connection() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;

    let (mut old_write, mut old_read, register_ok) =
        ws_register(harness.port, "acme", "url-id", None).await;

    let request = authenticated_ws_request(harness.port, &register_ok);
    let (reconnect_stream, _) = connect_async(request)
        .await
        .expect("reconnect with handshake identity");
    let (mut new_write, mut new_read) = reconnect_stream.split();

    let new_ping = actr_protocol::ActrToSignaling {
        source: register_ok.actr_id.clone(),
        credential: register_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 90,
                mailbox_backlog: 0.0,
                power_reserve: 90.0,
                ..Default::default()
            },
        )),
    };
    send_envelope(
        &mut new_write,
        make_envelope(signaling_envelope::Flow::ActrToServer(new_ping)),
    )
    .await;
    let new_resp = recv_envelope(&mut new_read).await;
    match new_resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(msg)) => match msg.payload {
            Some(signaling_to_actr::Payload::Pong(_)) => {}
            other => panic!("expected pong on reconnected session, got {other:?}"),
        },
        other => panic!("unexpected reconnected response flow: {other:?}"),
    }

    let stale_ping = actr_protocol::ActrToSignaling {
        source: register_ok.actr_id.clone(),
        credential: register_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 85,
                mailbox_backlog: 0.0,
                power_reserve: 85.0,
                ..Default::default()
            },
        )),
    };
    let mut stale_buf = Vec::new();
    make_envelope(signaling_envelope::Flow::ActrToServer(stale_ping))
        .encode(&mut stale_buf)
        .expect("encode stale ping");

    if old_write
        .send(WsMessage::Binary(stale_buf.into()))
        .await
        .is_ok()
    {
        let stale_result = tokio::time::timeout(Duration::from_millis(600), old_read.next()).await;
        match stale_result {
            Ok(Some(Ok(WsMessage::Binary(data)))) => {
                let stale_resp =
                    actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode stale resp");
                if let Some(signaling_envelope::Flow::ServerToActr(msg)) = stale_resp.flow
                    && matches!(msg.payload, Some(signaling_to_actr::Payload::Pong(_)))
                {
                    panic!("stale connection unexpectedly still receives pong");
                }
            }
            Ok(Some(Ok(_))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                // stale connection should be unusable for normal request/response flow.
            }
        }
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_peer_payload_none_is_ignored_and_connection_remains_usable() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let (mut write, mut read, ok) = ws_register(harness.port, "acme", "peer-none", None).await;

    // Send empty peer payload
    send_envelope(
        &mut write,
        make_envelope(signaling_envelope::Flow::PeerToServer(
            actr_protocol::PeerToSignaling { payload: None },
        )),
    )
    .await;

    let no_response = tokio::time::timeout(Duration::from_millis(500), read.next()).await;
    assert!(
        no_response.is_err(),
        "empty peer payload should be ignored without server response"
    );

    // Connection still usable: send Ping should receive Pong
    let ping_msg = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 100,
                mailbox_backlog: 0.0,
                power_reserve: 80.0,
                ..Default::default()
            },
        )),
    };
    send_envelope(
        &mut write,
        make_envelope(signaling_envelope::Flow::ActrToServer(ping_msg)),
    )
    .await;

    let resp = recv_envelope(&mut read).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Pong(_)) => {}
            other => panic!("expected pong after ignored payload, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let _ = write.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_actr_payload_none_and_invalid_realm_are_rejected_safely() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let (mut write, mut read, ok) = ws_register(harness.port, "acme", "actr-none", None).await;

    let no_payload_msg = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: None,
    };
    send_envelope(
        &mut write,
        make_envelope(signaling_envelope::Flow::ActrToServer(no_payload_msg)),
    )
    .await;

    let no_response = tokio::time::timeout(Duration::from_millis(500), read.next()).await;
    assert!(
        no_response.is_err(),
        "empty actr payload should be ignored without server response"
    );

    let mut invalid_realm_source = ok.actr_id.clone();
    invalid_realm_source.realm = "other@example.com".to_string();
    let invalid_realm_ping = actr_protocol::ActrToSignaling {
        source: invalid_realm_source,
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 70,
                mailbox_backlog: 0.0,
                power_reserve: 60.0,
                ..Default::default()
            },
        )),
    };
    send_envelope(
        &mut write,
        make_envelope(signaling_envelope::Flow::ActrToServer(invalid_realm_ping)),
    )
    .await;

    let invalid_resp = recv_envelope(&mut read).await;
    match invalid_resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 403, "invalid realm should be rejected");
            }
            other => panic!("expected realm validation error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let good_ping = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 95,
                mailbox_backlog: 0.0,
                power_reserve: 90.0,
                ..Default::default()
            },
        )),
    };
    send_envelope(
        &mut write,
        make_envelope(signaling_envelope::Flow::ActrToServer(good_ping)),
    )
    .await;
    let good_resp = recv_envelope(&mut read).await;
    match good_resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Pong(_)) => {}
            other => panic!("expected pong after recovery ping, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let _ = write.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_connection_rate_limit_rejects_second_concurrent_connection() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) =
        write_fullstack_config_with_rate_limits(tmp.path(), port, DEFAULT_TOKEN_TTL, 1, 50, 50);
    let log_path = tmp.path().join("loran_fullstack_ratelimit_conn.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;

    // Register two different actors for testing connection rate limiting
    let base_url = format!("http://127.0.0.1:{port}");
    let ok1 = issuer_register_http(&base_url, TEST_REALM, "acme", "rate1", None, None).await;
    let ok2 = issuer_register_http(&base_url, TEST_REALM, "acme", "rate2", None, None).await;

    let (_first_write, _first_read) = connect_ws_authenticated(port, &ok1).await;

    // Second connection should be rejected by concurrent limit
    let second = connect_async(authenticated_ws_request(port, &ok2)).await;
    match second {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "second connection should be rejected by concurrent limit"
            );
        }
        Ok(_) => panic!("expected second concurrent connection to be rate-limited"),
        Err(other) => panic!("expected HTTP 429 on second connection, got {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_message_rate_limit_returns_envelope_error() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) =
        write_fullstack_config_with_rate_limits(tmp.path(), port, DEFAULT_TOKEN_TTL, 20, 1, 1);
    let log_path = tmp.path().join("loran_fullstack_ratelimit_msg.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;

    let (mut write, mut read, register_ok) = ws_register(port, "acme", "rate-limited", None).await;
    let ping_payload = actr_protocol::ActrToSignaling {
        source: register_ok.actr_id.clone(),
        credential: register_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 80,
                mailbox_backlog: 0.0,
                power_reserve: 80.0,
                ..Default::default()
            },
        )),
    };

    for _ in 0..8 {
        send_envelope(
            &mut write,
            make_envelope(signaling_envelope::Flow::ActrToServer(ping_payload.clone())),
        )
        .await;
    }

    let mut saw_rate_limit = false;
    for _ in 0..12 {
        let next = tokio::time::timeout(Duration::from_secs(2), read.next()).await;
        let Some(Ok(WsMessage::Binary(data))) = next.ok().and_then(|m| m) else {
            continue;
        };
        let envelope =
            actr_protocol::SignalingEnvelope::decode(&data[..]).expect("decode signaling response");
        if let Some(signaling_envelope::Flow::EnvelopeError(err)) = envelope.flow
            && err.code == 429
        {
            saw_rate_limit = true;
            break;
        }
    }
    assert!(
        saw_rate_limit,
        "expected message rate limiter to emit envelope error 429"
    );

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_register_and_discovery_acl_allow() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Service registers with ACL allowing client:* to discover
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };
    let (_ws_service_write, _ws_service_read, _service_ok) =
        ws_register(port, "acme", "svc", Some(acl)).await;

    // Client registers (no ACL needed)
    let (mut client_write, mut client_read, client_ok) =
        ws_register(port, "acme", "client", None).await;

    // Discovery should return the service type because ACL allows it
    let discover = actr_protocol::ActrToSignaling {
        source: client_ok.actr_id.clone(),
        credential: client_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::DiscoveryRequest(
            actr_protocol::DiscoveryRequest {
                publisher: Some("acme".into()),
                limit: Some(10),
            },
        )),
    };
    let env = make_envelope(signaling_envelope::Flow::ActrToServer(discover));
    send_envelope(&mut client_write, env).await;
    let resp = recv_envelope(&mut client_read).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::DiscoveryResponse(rsp)) => match rsp.result {
                Some(actr_protocol::discovery_response::Result::Success(ok)) => {
                    assert!(
                        !ok.entries.is_empty(),
                        "expected at least one service entry"
                    );
                    assert_eq!(ok.entries[0].actr_type.name, "svc");
                    assert_eq!(ok.entries[0].actr_type.publisher, "acme");
                }
                other => panic!("unexpected discovery result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_discovery_acl_denied() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Service registers without ACL (default deny)
    let (_ws_service_write, _ws_service_read, _service_ok) =
        ws_register(port, "acme", "svc-deny", None).await;

    // Client registers
    let (mut client_write, mut client_read, client_ok) =
        ws_register(port, "acme", "client", None).await;

    let discover = actr_protocol::ActrToSignaling {
        source: client_ok.actr_id.clone(),
        credential: client_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::DiscoveryRequest(
            actr_protocol::DiscoveryRequest {
                publisher: Some("acme".into()),
                limit: Some(10),
            },
        )),
    };
    let env = make_envelope(signaling_envelope::Flow::ActrToServer(discover));
    send_envelope(&mut client_write, env).await;
    let resp = recv_envelope(&mut client_read).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::DiscoveryResponse(rsp)) => match rsp.result {
                Some(actr_protocol::discovery_response::Result::Success(ok)) => {
                    assert!(
                        ok.entries.is_empty(),
                        "ACL default deny should yield empty discovery list"
                    );
                }
                other => panic!("unexpected discovery result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_rejects_expired_credential() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, 1);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Register actor
    let (mut write, mut read, ok) = ws_register(port, "acme", "shortlived", None).await;

    // Wait for credential to expire
    sleep(Duration::from_secs(2)).await;

    // Send ping with expired credential -> expect 401 error
    let ping_msg = actr_protocol::ActrToSignaling {
        source: ok.actr_id.clone(),
        credential: ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 50,
                mailbox_backlog: 1.0,
                power_reserve: 50.0,
                ..Default::default()
            },
        )),
    };
    let env = make_envelope(signaling_envelope::Flow::ActrToServer(ping_msg));
    send_envelope(&mut write, env).await;
    let resp = recv_envelope(&mut read).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 401, "expired credential should be rejected");
            }
            other => panic!("expected error for expired credential, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_with_acl() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Service registers with ACL allowing client:sdp to discover/route
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-sdp".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };
    let (_svc_w, _svc_r, svc_ok) = ws_register(port, "acme", "svc-rtp", Some(acl)).await;

    // Client registers
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-sdp", None).await;

    // RouteCandidates should return the service because ACL allows
    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-rtp".into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: "".into(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 5,
                            ranking_factors: vec![],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;
    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert!(
                        ok.candidates
                            .iter()
                            .any(|id| id.serial_number == svc_ok.actr_id.serial_number),
                        "expected routed candidate"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_acl_denied() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Service registers without ACL (default deny)
    let (_svc_w, _svc_r, _svc_ok) = ws_register(port, "acme", "svc-deny-route", None).await;

    // Client registers
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-sdp", None).await;

    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-deny-route".into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: "".into(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 5,
                            ranking_factors: vec![],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;
    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert!(
                        ok.candidates.is_empty(),
                        "ACL deny should yield empty route candidates"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_respects_limit_and_sorting() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    // ACL: allow client-route to reach the services
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-route".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    // Register two service instances with different load indicators
    let (mut svc1_w, svc1_r, svc1_ok) =
        ws_register(port, "acme", "svc-route", Some(acl.clone())).await;
    let (mut svc2_w, svc2_r, svc2_ok) =
        ws_register(port, "acme", "svc-route", Some(acl.clone())).await;

    // Update runtime metrics via ping (own data per call to avoid lifetimes)
    let send_ping =
        |mut w: WsWrite, ok: register_response::RegisterOk, power: f32, backlog: f32| async move {
            let ping = actr_protocol::ActrToSignaling {
                source: ok.actr_id.clone(),
                credential: ok.credential.clone(),
                payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
                    actr_protocol::Ping {
                        availability: 50,
                        mailbox_backlog: backlog,
                        power_reserve: power,
                        ..Default::default()
                    },
                )),
            };
            send_envelope(
                &mut w,
                make_envelope(signaling_envelope::Flow::ActrToServer(ping)),
            )
            .await;
            w
        };

    svc1_w = send_ping(svc1_w, svc1_ok.clone(), 10.0, 5.0).await; // lower power, higher backlog
    svc2_w = send_ping(svc2_w, svc2_ok.clone(), 80.0, 1.0).await; // higher power, lower backlog

    // Wait for ping metrics to be ingested
    sleep(Duration::from_millis(200)).await;

    // Client registers
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-route", None).await;

    // Request route candidates with sorting and limit=1
    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-route".into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: "".into(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 1,
                            ranking_factors: vec![
                                actr_protocol::route_candidates_request::node_selection_criteria::NodeRankingFactor::MaximumPowerReserve as i32,
                                actr_protocol::route_candidates_request::node_selection_criteria::NodeRankingFactor::MinimumMailboxBacklog as i32,
                            ],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;

    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert_eq!(
                        ok.candidates.len(),
                        1,
                        "limit=1 should return exactly one candidate"
                    );
                    assert_eq!(ok.candidates.len(), 1);
                    let winner = ok.candidates[0].serial_number;
                    assert_eq!(
                        winner, svc2_ok.actr_id.serial_number,
                        "higher power reserve should win"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    // Close sockets and shutdown
    let _ = cli_w.send(WsMessage::Close(None)).await;
    let _ = svc1_w.send(WsMessage::Close(None)).await;
    let _ = svc2_w.send(WsMessage::Close(None)).await;
    std::mem::drop(svc1_r.into_future());
    std::mem::drop(svc2_r.into_future());

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_prefers_exact_fingerprint() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    // ACL allow client-fp
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-fp".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    // Helper to build a simple ServiceSpec with unique fingerprint
    let make_spec = |fingerprint: &str| actr_protocol::ServiceSpec {
        name: "svc-fp".into(),
        description: Some("test svc spec".into()),
        fingerprint: fingerprint.into(),
        protobufs: vec![actr_protocol::service_spec::Protobuf {
            package: "echo.v1".into(),
            content: "service Echo { rpc Ping (Ping) returns (Pong); }".into(),
            fingerprint: format!("fp::{fingerprint}"),
        }],
        published_at: None,
        tags: vec!["stable".into()],
    };

    let spec_exact = make_spec("fp-exact");
    let spec_backward = make_spec("fp-backward");

    // Register two service instances with different fingerprints
    let (_svc_exact_w, _svc_exact_r, svc_exact_ok) = ws_register_with_spec(
        port,
        "acme",
        "svc-fp-exact",
        Some(acl.clone()),
        Some(spec_exact.clone()),
    )
    .await;

    let (_svc_bw_w, _svc_bw_r, _svc_bw_ok) = ws_register_with_spec(
        port,
        "acme",
        "svc-fp-bw",
        Some(acl.clone()),
        Some(spec_backward.clone()),
    )
    .await;

    // Client registers
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-fp", None).await;

    // Request route candidates with client_fingerprint matching spec_exact
    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-fp-exact".into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: spec_exact.fingerprint.clone(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 2,
                            ranking_factors: vec![
                                actr_protocol::route_candidates_request::node_selection_criteria::NodeRankingFactor::ExactMatchFirst as i32,
                            ],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };

    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;

    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert_eq!(
                        ok.candidates.first().map(|c| c.serial_number),
                        Some(svc_exact_ok.actr_id.serial_number),
                        "exact fingerprint service should rank first"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let _ = cli_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_get_service_spec_returns_spec() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    let spec = make_service_spec(
        "fp-spec",
        "echo.v1",
        r#"syntax = "proto3";
            package echo.v1;
            message Ping { string msg = 1; }
            message Pong { string msg = 1; }
            service Echo { rpc Say(Ping) returns (Pong); }"#,
    );

    let (_svc_w, _svc_r, _svc_ok) =
        ws_register_with_spec(port, "acme", "svc-spec", None, Some(spec.clone())).await;
    sleep(Duration::from_millis(200)).await;

    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-spec", None).await;

    let get_spec = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::GetServiceSpecRequest(
                actr_protocol::GetServiceSpecRequest {
                    name: spec.name.clone(),
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(get_spec)),
    )
    .await;
    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::GetServiceSpecResponse(rsp)) => match rsp.result {
                Some(actr_protocol::get_service_spec_response::Result::Success(returned)) => {
                    assert_eq!(returned.fingerprint, spec.fingerprint);
                }
                other => panic!("unexpected get spec result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_get_service_spec_not_found() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    // Client without any services, just to issue request
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-nospec", None).await;

    let get_spec = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::GetServiceSpecRequest(
                actr_protocol::GetServiceSpecRequest {
                    name: "non-existent-svc".into(),
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(get_spec)),
    )
    .await;
    let resp = recv_envelope(&mut cli_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::GetServiceSpecResponse(rsp)) => match rsp.result {
                Some(actr_protocol::get_service_spec_response::Result::Error(err)) => {
                    assert_eq!(err.code, 404, "missing spec should return 404");
                }
                other => panic!("unexpected get spec result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_subscribe_and_unsubscribe_actr_up() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    // Subscriber
    let (mut sub_w, mut sub_r, sub_ok) = ws_register(port, "acme", "subscriber", None).await;

    // Subscribe to target type
    let subscribe = actr_protocol::ActrToSignaling {
        source: sub_ok.actr_id.clone(),
        credential: sub_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::SubscribeActrUpRequest(
                actr_protocol::SubscribeActrUpRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-subject".into(),
                        version: "1.0.0".to_string(),
                    },
                },
            ),
        ),
    };
    send_envelope(
        &mut sub_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(subscribe)),
    )
    .await;
    let resp = recv_envelope(&mut sub_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::SubscribeActrUpResponse(rsp)) => match rsp.result {
                Some(actr_protocol::subscribe_actr_up_response::Result::Success(_)) => {}
                other => panic!("unexpected subscribe result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    // Unsubscribe should also succeed
    let unsubscribe = actr_protocol::ActrToSignaling {
        source: sub_ok.actr_id.clone(),
        credential: sub_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::UnsubscribeActrUpRequest(
                actr_protocol::UnsubscribeActrUpRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-subject".into(),
                        version: "1.0.0".to_string(),
                    },
                },
            ),
        ),
    };
    send_envelope(
        &mut sub_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(unsubscribe)),
    )
    .await;
    let resp2 = recv_envelope(&mut sub_r).await;
    match resp2.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::UnsubscribeActrUpResponse(rsp)) => match rsp.result {
                Some(actr_protocol::unsubscribe_actr_up_response::Result::Success(_)) => {}
                other => panic!("unexpected unsubscribe result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_subscribe_receives_actr_up_and_unsubscribe_stops() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    // Subscriber registers
    let (mut sub_w, mut sub_r, sub_ok) = ws_register(port, "acme", "subscriber", None).await;

    // Subscribe to service type
    let target_type = ActrType {
        publisher: "acme".into(),
        name: "svc-presence".into(),
        version: "1.0.0".to_string(),
    };
    let subscribe = actr_protocol::ActrToSignaling {
        source: sub_ok.actr_id.clone(),
        credential: sub_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::SubscribeActrUpRequest(
                actr_protocol::SubscribeActrUpRequest {
                    target_type: target_type.clone(),
                },
            ),
        ),
    };
    send_envelope(
        &mut sub_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(subscribe)),
    )
    .await;
    // ack
    let _ = recv_envelope(&mut sub_r).await;

    // allow subscription to settle
    sleep(Duration::from_millis(100)).await;

    // New service registers -> should trigger ActrUp notification
    let presence_acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "subscriber".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };
    let (_svc_w, _svc_r, svc_ok) =
        ws_register(port, "acme", "svc-presence", Some(presence_acl.clone())).await;
    sleep(Duration::from_millis(200)).await;

    // Expect ActrUp notification (poll with timeout)
    let mut got_up = false;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        if let Ok(env) = timeout(Duration::from_millis(500), recv_envelope(&mut sub_r)).await {
            if let Some(signaling_envelope::Flow::ServerToActr(server_msg)) = env.flow
                && let Some(signaling_to_actr::Payload::ActrUpEvent(evt)) = server_msg.payload
                && evt.actor_id == svc_ok.actr_id
            {
                got_up = true;
                break;
            }
        } else {
            continue;
        }
    }
    if !got_up {
        let logs = fs::read_to_string(harness.log_path()).unwrap_or_default();
        panic!("subscriber should receive ActrUp notice. Logs:\n{logs}");
    }

    // Unsubscribe
    let unsubscribe = actr_protocol::ActrToSignaling {
        source: sub_ok.actr_id.clone(),
        credential: sub_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::UnsubscribeActrUpRequest(
                actr_protocol::UnsubscribeActrUpRequest {
                    target_type: target_type.clone(),
                },
            ),
        ),
    };
    send_envelope(
        &mut sub_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(unsubscribe)),
    )
    .await;
    let _ = recv_envelope(&mut sub_r).await; // unsubscribe ack

    // Register another service; notification should not arrive after unsubscribe
    let (_svc2_w, _svc2_r, _svc2_ok) =
        ws_register(port, "acme", "svc-presence-2", Some(presence_acl)).await;

    // Drain with timeout; expect None
    use tokio::time::timeout;
    let no_msg = timeout(Duration::from_millis(300), sub_r.next()).await;
    assert!(
        no_msg.is_err(),
        "should not receive ActrUp after unsubscribe"
    );

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_compatibility_cache_hit() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            from_type: ActrType {
                publisher: "mfg".into(),
                name: "client-fp-cache".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(acl_rule::SourceRealm::Realm(TEST_REALM.to_string())),
            permission: Permission::Allow as i32,
        }],
    };

    let spec_base = make_service_spec(
        "fp-base",
        "compat.v1",
        r#"syntax = "proto3";
            package compat.v1;
            message Req { string data = 1; }
            message Resp { string data = 1; }
            service Compat { rpc Call(Req) returns (Resp); }"#,
    );
    // content identical but fingerprint changed to exercise compatibility cache path without breaking changes
    let spec_new = make_service_spec(
        "fp-new",
        "compat.v1",
        r#"syntax = "proto3";
            package compat.v1;
            message Req { string data = 1; }
            message Resp { string data = 1; }
            service Compat { rpc Call(Req) returns (Resp); }"#,
    );

    // Register base spec instance (provides client fingerprint spec in storage)
    let (_svc_base_w, _svc_base_r, _svc_base_ok) = ws_register_with_spec(
        port,
        "mfg",
        "svc-compat",
        Some(acl.clone()),
        Some(spec_base.clone()),
    )
    .await;
    // Register upgraded instance with different fingerprint
    let (_svc_w, _svc_r, _svc_ok) = ws_register_with_spec(
        port,
        "mfg",
        "svc-compat",
        Some(acl.clone()),
        Some(spec_new.clone()),
    )
    .await;
    sleep(Duration::from_millis(200)).await;

    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "mfg", "client-fp-cache", None).await;

    // First request triggers compatibility analysis and populates cache
    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
            actr_protocol::RouteCandidatesRequest {
                target_type: ActrType {
                    publisher: "mfg".into(),
                    name: "svc-compat".into(),
                    version: "1.0.0".to_string(),
                },
                client_fingerprint: spec_base.fingerprint.clone(),
                criteria: Some(
                    actr_protocol::route_candidates_request::NodeSelectionCriteria {
                        candidate_count: 3,
                        ranking_factors: vec![actr_protocol::route_candidates_request::node_selection_criteria::NodeRankingFactor::ExactMatchFirst as i32],
                        minimal_dependency_requirement: None,
                        minimal_health_requirement: None,
                    },
                ),
                client_location: None,
            },
        )),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;
    let resp1 = recv_envelope(&mut cli_r).await;
    let candidates1 = match resp1.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => ok.candidates,
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    };
    assert!(
        !candidates1.is_empty(),
        "should return at least one candidate"
    );

    // Second request should reuse cache and still succeed
    let route_req2 = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
            actr_protocol::RouteCandidatesRequest {
                target_type: ActrType {
                    publisher: "mfg".into(),
                    name: "svc-compat".into(),
                    version: "1.0.0".to_string(),
                },
                client_fingerprint: spec_base.fingerprint.clone(),
                criteria: Some(
                    actr_protocol::route_candidates_request::NodeSelectionCriteria {
                        candidate_count: 3,
                        ranking_factors: vec![actr_protocol::route_candidates_request::node_selection_criteria::NodeRankingFactor::ExactMatchFirst as i32],
                        minimal_dependency_requirement: None,
                        minimal_health_requirement: None,
                    },
                ),
                client_location: None,
            },
        )),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req2)),
    )
    .await;
    let resp2 = recv_envelope(&mut cli_r).await;
    let candidates2 = match resp2.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => ok.candidates,
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    };
    assert!(
        !candidates2.is_empty(),
        "cache hit should keep returning candidates"
    );

    let _ = cli_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_concurrent_registration_keeps_unique_route_candidates() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-concurrent".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let service_count = 8usize;
    let mut service_tasks = Vec::with_capacity(service_count);
    for _ in 0..service_count {
        let acl_clone = acl.clone();
        service_tasks.push(tokio::spawn(async move {
            ws_register(port, "acme", "svc-concurrent", Some(acl_clone)).await
        }));
    }

    let mut service_sockets = Vec::with_capacity(service_count);
    let mut expected_serials = HashSet::new();
    for task in service_tasks {
        let (w, _r, ok) = task.await.expect("registration task should complete");
        expected_serials.insert(ok.actr_id.serial_number);
        service_sockets.push(w);
    }
    assert_eq!(
        expected_serials.len(),
        service_count,
        "each concurrent service registration should get a unique serial"
    );

    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-concurrent", None).await;
    let route_req = actr_protocol::ActrToSignaling {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                actr_protocol::RouteCandidatesRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-concurrent".into(),
                        version: "1.0.0".to_string(),
                    },
                    client_fingerprint: "".into(),
                    criteria: Some(
                        actr_protocol::route_candidates_request::NodeSelectionCriteria {
                            candidate_count: 32,
                            ranking_factors: vec![],
                            minimal_dependency_requirement: None,
                            minimal_health_requirement: None,
                        },
                    ),
                    client_location: None,
                },
            ),
        ),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(route_req)),
    )
    .await;

    let resp = recv_envelope(&mut cli_r).await;
    let candidates = match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => ok.candidates,
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    };

    let unique_candidates: HashSet<u64> = candidates.iter().map(|c| c.serial_number).collect();
    assert_eq!(
        candidates.len(),
        unique_candidates.len(),
        "route candidates must not contain duplicates"
    );
    assert_eq!(
        unique_candidates.len(),
        service_count,
        "all concurrently registered services should be routable"
    );

    let _ = cli_w.send(WsMessage::Close(None)).await;
    for mut w in service_sockets {
        let _ = w.send(WsMessage::Close(None)).await;
    }
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_actr_relay_role_assignment() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_fullstack.log");
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;
    let mut child = spawn_loran(&config_path, &log_path);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&tmp.path().join("data"), TEST_REALM).await;

    // Service registers with ACL allowing client-offer
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-offer".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };
    let (mut svc_w, mut svc_r, svc_ok) = ws_register(port, "acme", "svc-relay", Some(acl)).await;

    // Client registers
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-offer", None).await;

    // Send role negotiation relay
    let relay = ActrRelay {
        source: cli_ok.actr_id.clone(),
        credential: cli_ok.credential.clone(),
        target: svc_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: cli_ok.actr_id.clone(),
            to: svc_ok.actr_id.clone(),
        })),
    };
    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    // Client should receive RoleAssignment
    let client_resp = recv_envelope(&mut cli_r).await;
    let mut got_client_assignment = false;
    if let Some(signaling_envelope::Flow::ActrRelay(relay_msg)) = client_resp.flow
        && let Some(actr_relay::Payload::RoleAssignment(assign)) = relay_msg.payload
    {
        got_client_assignment = true;
        let _ = assign.is_offerer;
    }
    assert!(got_client_assignment, "client should get role assignment");

    // Service should receive RoleAssignment
    let service_resp = recv_envelope(&mut svc_r).await;
    let mut got_service_assignment = false;
    if let Some(signaling_envelope::Flow::ActrRelay(relay_msg)) = service_resp.flow
        && let Some(actr_relay::Payload::RoleAssignment(assign)) = relay_msg.payload
    {
        got_service_assignment = true;
        let _ = assign.is_offerer;
    }
    assert!(got_service_assignment, "service should get role assignment");

    // Cleanup
    let _ = cli_w.send(WsMessage::Close(None)).await;
    let _ = svc_w.send(WsMessage::Close(None)).await;
    graceful_shutdown(child);
}

#[tokio::test]
#[serial]
async fn signaling_rejects_register_request_via_ws() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let (mut write, mut read, _ok) = ws_register(port, "acme", "dup-client", None).await;

    // Sending RegisterRequest via WS should return 410 (migrated to Issuer HTTP)
    let register_req = RegisterRequest {
        actr_type: ActrType {
            publisher: "acme".into(),
            name: "dup-client".into(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: None,
        signing_public_key: None,
    };
    let env = make_envelope(signaling_envelope::Flow::PeerToServer(
        actr_protocol::PeerToSignaling {
            payload: Some(peer_to_signaling::Payload::RegisterRequest(register_req)),
        },
    ));
    send_envelope(&mut write, env).await;
    let resp = recv_envelope(&mut read).await;

    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RegisterResponse(response)) => match response.result {
                Some(register_response::Result::Error(err)) => {
                    assert_eq!(err.code, 410, "WS register should return 410 Gone");
                    assert!(
                        err.message.contains("/issuer/register"),
                        "error should direct to Issuer HTTP, got: {}",
                        err.message,
                    );
                }
                other => panic!("expected register error, got {other:?}"),
            },
            other => panic!("expected register error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_unregister_removes_actor_from_route_candidates() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-unreg".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (mut svc_w, mut svc_r, svc_ok) = ws_register(port, "acme", "svc-unreg", Some(acl)).await;
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-unreg", None).await;

    let make_route_request =
        |source: &register_response::RegisterOk| actr_protocol::ActrToSignaling {
            source: source.actr_id.clone(),
            credential: source.credential.clone(),
            payload: Some(
                actr_protocol::actr_to_signaling::Payload::RouteCandidatesRequest(
                    actr_protocol::RouteCandidatesRequest {
                        target_type: ActrType {
                            publisher: "acme".into(),
                            name: "svc-unreg".into(),
                            version: "1.0.0".to_string(),
                        },
                        client_fingerprint: "".into(),
                        criteria: Some(
                            actr_protocol::route_candidates_request::NodeSelectionCriteria {
                                candidate_count: 8,
                                ranking_factors: vec![],
                                minimal_dependency_requirement: None,
                                minimal_health_requirement: None,
                            },
                        ),
                        client_location: None,
                    },
                ),
            ),
        };

    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(make_route_request(
            &cli_ok,
        ))),
    )
    .await;
    let before = recv_envelope(&mut cli_r).await;
    match before.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert!(
                        ok.candidates
                            .iter()
                            .any(|id| id.serial_number == svc_ok.actr_id.serial_number),
                        "service should be routable before unregister"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let unregister = actr_protocol::ActrToSignaling {
        source: svc_ok.actr_id.clone(),
        credential: svc_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::UnregisterRequest(
                actr_protocol::UnregisterRequest {
                    actr_id: svc_ok.actr_id.clone(),
                    reason: Some("test-unregister".into()),
                },
            ),
        ),
    };
    send_envelope(
        &mut svc_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(unregister)),
    )
    .await;
    let unregister_resp = recv_envelope(&mut svc_r).await;
    match unregister_resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::UnregisterResponse(resp)) => match resp.result {
                Some(actr_protocol::unregister_response::Result::Success(_)) => {}
                other => panic!("unexpected unregister result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    sleep(Duration::from_millis(200)).await;

    send_envelope(
        &mut cli_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(make_route_request(
            &cli_ok,
        ))),
    )
    .await;
    let after = recv_envelope(&mut cli_r).await;
    match after.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::RouteCandidatesResponse(rsp)) => match rsp.result {
                Some(route_candidates_response::Result::Success(ok)) => {
                    assert!(
                        !ok.candidates
                            .iter()
                            .any(|id| id.serial_number == svc_ok.actr_id.serial_number),
                        "service should be removed from routing after unregister"
                    );
                }
                other => panic!("unexpected route result {other:?}"),
            },
            other => panic!("unexpected payload {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    let _ = cli_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_rejects_invalid_credential() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "relay-src-auth".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (_dst_w, _dst_r, dst_ok) = ws_register(port, "acme", "relay-dst-auth", Some(acl)).await;
    let (mut src_w, mut src_r, src_ok) = ws_register(port, "acme", "relay-src-auth", None).await;

    let mut bad_cred = src_ok.credential.clone();
    if !bad_cred.signature.is_empty() {
        let mut tampered = bad_cred.signature.to_vec();
        tampered[0] ^= 0xAA;
        bad_cred.signature = tampered.into();
    }

    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: bad_cred,
        target: dst_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: src_ok.actr_id.clone(),
            to: dst_ok.actr_id.clone(),
        })),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let resp = recv_envelope(&mut src_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 401, "invalid relay credential should be rejected");
            }
            other => panic!("expected relay auth error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_acl_denied_in_same_realm() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let deny_acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Deny as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "relay-src-deny".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (_dst_w, _dst_r, dst_ok) =
        ws_register(port, "acme", "relay-dst-deny", Some(deny_acl)).await;
    let (mut src_w, mut src_r, src_ok) = ws_register(port, "acme", "relay-src-deny", None).await;

    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        target: dst_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: src_ok.actr_id.clone(),
            to: dst_ok.actr_id.clone(),
        })),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let resp = recv_envelope(&mut src_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 403, "same-realm relay should be ACL-denied");
                assert!(err.message.contains("ACL policy denies relay"));
            }
            other => panic!("expected relay ACL error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_rejects_role_negotiation_to_another_actor() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let allow_acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "relay-src-pair".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (_dst_w, _dst_r, dst_ok) =
        ws_register(port, "acme", "relay-dst-pair", Some(allow_acl)).await;
    let (_other_w, _other_r, other_ok) = ws_register(port, "acme", "relay-other-pair", None).await;
    let (mut src_w, mut src_r, src_ok) = ws_register(port, "acme", "relay-src-pair", None).await;

    // The ACL allows source -> target, but the negotiation names another Actor.
    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        target: dst_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: src_ok.actr_id.clone(),
            to: other_ok.actr_id.clone(),
        })),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let resp = recv_envelope(&mut src_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 403);
                assert!(err.message.contains("RoleNegotiation must be from"));
            }
            other => panic!("expected RoleNegotiation error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_forwards_ice_candidate_payload() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let allow_acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "relay-src-forward".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (mut dst_w, mut dst_r, dst_ok) =
        ws_register(port, "acme", "relay-dst-forward", Some(allow_acl)).await;
    let (mut src_w, _src_r, src_ok) = ws_register(port, "acme", "relay-src-forward", None).await;

    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        target: dst_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::IceCandidate(
            actr_protocol::IceCandidate {
                candidate: "candidate:1 1 udp 2122252543 127.0.0.1 54400 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
                username_fragment: Some("ufrag-forward".into()),
            },
        )),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let forwarded = recv_envelope(&mut dst_r).await;
    match forwarded.flow {
        Some(signaling_envelope::Flow::ActrRelay(relay_msg)) => {
            assert_eq!(relay_msg.source.serial_number, src_ok.actr_id.serial_number);
            assert_eq!(relay_msg.target.serial_number, dst_ok.actr_id.serial_number);
            match relay_msg.payload {
                Some(actr_relay::Payload::IceCandidate(candidate)) => {
                    assert_eq!(candidate.sdp_mid.as_deref(), Some("0"));
                    assert_eq!(candidate.sdp_mline_index, Some(0));
                    assert!(
                        candidate.candidate.contains("127.0.0.1"),
                        "forwarded ICE candidate payload should be preserved"
                    );
                }
                other => panic!("expected forwarded IceCandidate payload, got {other:?}"),
            }
        }
        other => panic!("expected forwarded ActrRelay flow, got {other:?}"),
    }

    let _ = src_w.send(WsMessage::Close(None)).await;
    let _ = dst_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_to_missing_target_reports_failure_and_source_stays_usable() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let allow_acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "relay-src-missing-target".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (mut dst_w, _dst_r, dst_ok) =
        ws_register(port, "acme", "relay-dst-missing-target", Some(allow_acl)).await;
    let (mut src_w, mut src_r, src_ok) =
        ws_register(port, "acme", "relay-src-missing-target", None).await;

    let mut missing_target = dst_ok.actr_id.clone();
    missing_target.serial_number = missing_target.serial_number.saturating_add(1_000_000);

    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        target: missing_target.clone(),
        source_type: None,
        delivery_context: Some(RelayDeliveryContext {
            controller_lifecycle_id: 7,
            transport_epoch: 11,
            pc_generation: 13,
            negotiation_epoch: Some(29),
        }),
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: src_ok.actr_id.clone(),
            to: missing_target.clone(),
        })),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let result = recv_envelope(&mut src_r).await;
    match result.flow {
        Some(signaling_envelope::Flow::ServerToActr(message)) => match message.payload {
            Some(signaling_to_actr::Payload::RelayDeliveryResult(result)) => {
                assert_eq!(
                    result.status,
                    RelayDeliveryStatus::RelayDeliveryTargetUnavailable as i32
                );
                assert_eq!(result.context.controller_lifecycle_id, 7);
                assert_eq!(result.context.transport_epoch, 11);
                assert_eq!(result.context.pc_generation, 13);
                assert_eq!(result.context.negotiation_epoch, Some(29));
            }
            other => panic!("expected relay delivery result, got {other:?}"),
        },
        other => panic!("unexpected relay failure flow {other:?}"),
    }

    let ping = actr_protocol::ActrToSignaling {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::Ping(
            actr_protocol::Ping {
                availability: 95,
                mailbox_backlog: 0.0,
                power_reserve: 88.0,
                ..Default::default()
            },
        )),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(ping)),
    )
    .await;
    let pong = recv_envelope(&mut src_r).await;
    match pong.flow {
        Some(signaling_envelope::Flow::ServerToActr(msg)) => match msg.payload {
            Some(signaling_to_actr::Payload::Pong(_)) => {}
            other => panic!("expected Pong after missing-target relay, got {other:?}"),
        },
        other => panic!("unexpected flow after ping {other:?}"),
    }

    let _ = src_w.send(WsMessage::Close(None)).await;
    let _ = dst_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_disconnect_removes_actor_from_route_candidates() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-disconnect".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (mut svc_w, _svc_r, svc_ok) = ws_register(port, "acme", "svc-disconnect", Some(acl)).await;
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-disconnect", None).await;

    let before =
        query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc-disconnect").await;
    assert!(
        before
            .iter()
            .any(|id| id.serial_number == svc_ok.actr_id.serial_number),
        "service should be routable before disconnect"
    );

    let _ = svc_w.send(WsMessage::Close(None)).await;

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let after =
            query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc-disconnect").await;
        let still_present = after
            .iter()
            .any(|id| id.serial_number == svc_ok.actr_id.serial_number);
        if !still_present {
            break;
        }

        if Instant::now() > deadline {
            panic!("service should be removed from routing after websocket disconnect");
        }
        sleep(Duration::from_millis(100)).await;
    }

    let _ = cli_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn retiring_a_realm_closes_its_connections_and_forgets_its_actors() {
    const GONE: &str = "gone@acme.com";
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;
    let base = harness.base_url();
    ensure_realm(&harness.data_dir, GONE).await;

    let (_gone_w, mut gone_r, gone_ok) =
        ws_register_in_realm_with_spec(port, "acme", "svc-gone", GONE, None, None).await;
    let (mut live_w, mut live_r, live_ok) = ws_register(port, "acme", "svc-live", None).await;
    ping_and_wait_for_pong(&mut live_w, &mut live_r, &live_ok).await;

    let cache = harness.data_dir.join("signaling_cache.db");
    let cached_rows = || async {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", cache.display()))
            .await
            .expect("open signaling cache");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM actor_registry WHERE actor_realm = ?")
                .bind(GONE)
                .fetch_one(&pool)
                .await
                .expect("count cached services");
        pool.close().await;
        count
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while cached_rows().await == 0 {
        assert!(Instant::now() < deadline, "service was never cached");
        sleep(Duration::from_millis(50)).await;
    }

    let client = reqwest::Client::new();
    let login: Value = client
        .post(format!("{base}/admin/api/auth/login"))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("admin login request")
        .json()
        .await
        .expect("parse admin login response");
    let token = login["token"].as_str().expect("admin login token");
    let retired = client
        .post(format!("{base}/admin/api/realms/gone%40acme.com/retire"))
        .bearer_auth(token)
        .send()
        .await
        .expect("retire realm");
    assert!(retired.status().is_success(), "{}", retired.status());

    // The live connection of the retired Realm is closed.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match gone_r.next().await {
                None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the retired Realm's connection stayed open");

    // Its cached services are gone and it cannot come back.
    let deadline = Instant::now() + Duration::from_secs(5);
    while cached_rows().await != 0 {
        assert!(
            Instant::now() < deadline,
            "cached services survived retirement"
        );
        sleep(Duration::from_millis(50)).await;
    }
    assert!(
        connect_async(authenticated_ws_request(port, &gone_ok))
            .await
            .is_err(),
        "a retired Realm's credential must not connect"
    );

    // Other Realms are untouched.
    ping_and_wait_for_pong(&mut live_w, &mut live_r, &live_ok).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_malformed_binary_removes_actor_from_route_candidates() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client-malformed".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };

    let (mut svc_w, _svc_r, svc_ok) = ws_register(port, "acme", "svc-malformed", Some(acl)).await;
    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client-malformed", None).await;

    let before =
        query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc-malformed").await;
    assert!(
        before
            .iter()
            .any(|id| id.serial_number == svc_ok.actr_id.serial_number),
        "service should be routable before malformed binary"
    );

    svc_w
        .send(WsMessage::Binary(vec![0xFF, 0x00, 0xAA].into()))
        .await
        .expect("send malformed binary frame");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let after =
            query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc-malformed").await;
        let still_present = after
            .iter()
            .any(|id| id.serial_number == svc_ok.actr_id.serial_number);
        if !still_present {
            break;
        }

        if Instant::now() > deadline {
            panic!("service should be removed from routing after malformed binary disconnect");
        }
        sleep(Duration::from_millis(100)).await;
    }

    let _ = cli_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
#[ignore = "Flaky test after loran restart, maybe due to ACL rules persistence"]
async fn actor_registry_persists_across_restart() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = tmp.path().join("data");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_persist.log");
    ensure_realm(&data_dir, TEST_REALM).await;

    // first run: register a service
    let mut child = spawn_loran(&config_path, &log_path);
    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&data_dir, TEST_REALM).await;

    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::Realm(TEST_REALM.to_string())),
        }],
    };
    let spec = make_service_spec(
        "fp-persisted",
        "persisted.v1",
        r#"syntax = "proto3";
            package persisted.v1;
            message Ping { string msg = 1; }"#,
    );
    let (_svc_w, _svc_r, svc_ok) =
        ws_register_with_spec(port, "acme", "svc", Some(acl), Some(spec.clone())).await;
    let svc_type = ActrType {
        publisher: "acme".into(),
        name: "svc".into(),
        version: "1.0.0".to_string(),
    };

    // Verify discovery before restart
    let (mut cli_w1, mut cli_r1, cli_ok1) = ws_register(port, "acme", "client", None).await;
    let entries_before =
        query_discovery_entries(&mut cli_w1, &mut cli_r1, &cli_ok1, Some("acme")).await;
    assert!(
        entries_before
            .iter()
            .any(|entry| entry.actr_type == svc_type),
        "service should be discoverable before restart"
    );
    graceful_shutdown(child);

    // Second run: the cache restores identity and ServiceSpec metadata, but a
    // disconnected Actor must stay out of network discovery and routing.
    let log_path2 = tmp.path().join("loran_persist2.log");
    let mut child2 = spawn_loran(&config_path, &log_path2);
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child2,
        &log_path2,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child2, &log_path2).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child2, &log_path2).await;
    ensure_realm(&data_dir, TEST_REALM).await;

    let (mut cli_w, mut cli_r, cli_ok) = ws_register(port, "acme", "client", None).await;
    let entries_while_offline =
        query_discovery_entries(&mut cli_w, &mut cli_r, &cli_ok, Some("acme")).await;
    assert!(
        !entries_while_offline
            .iter()
            .any(|entry| entry.actr_type == svc_type),
        "persisted metadata must not advertise a disconnected service"
    );
    let routes_while_offline =
        query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc").await;
    assert!(
        routes_while_offline.is_empty(),
        "persisted metadata must not create an offline route candidate"
    );

    let restored_spec = query_service_spec(&mut cli_w, &mut cli_r, &cli_ok, &spec.name).await;
    assert_eq!(restored_spec.fingerprint, spec.fingerprint);

    // Reconnect with the original Actor identity and credential. A Pong proves
    // the authenticated connection completed registry activation.
    let (mut svc_w2, mut svc_r2) = connect_ws_authenticated(port, &svc_ok).await;
    ping_and_wait_for_pong(&mut svc_w2, &mut svc_r2, &svc_ok).await;

    let entries_after_reconnect =
        query_discovery_entries(&mut cli_w, &mut cli_r, &cli_ok, Some("acme")).await;
    assert!(
        entries_after_reconnect
            .iter()
            .any(|entry| entry.actr_type == svc_type),
        "the original service should become discoverable after reconnect"
    );
    let routes_after_reconnect =
        query_route_candidates(&mut cli_w, &mut cli_r, &cli_ok, "acme", "svc").await;
    assert!(
        routes_after_reconnect
            .iter()
            .any(|candidate| candidate == &svc_ok.actr_id),
        "the reconnected service should retain and route to its original Actor ID"
    );

    let _ = svc_w2.send(WsMessage::Close(None)).await;
    let _ = cli_w.send(WsMessage::Close(None)).await;
    graceful_shutdown(child2);
}

#[tokio::test]
#[serial]
async fn service_unregister_success_is_durable_across_restart() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = tmp.path().join("data");
    let port = choose_port();
    let (config_path, signer_port) = write_fullstack_config(tmp.path(), port, DEFAULT_TOKEN_TTL);
    let log_path = tmp.path().join("loran_unregister_persist.log");
    ensure_realm(&data_dir, TEST_REALM).await;

    let mut child = spawn_loran(&config_path, &log_path);
    let base = format!("http://127.0.0.1:{port}");
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child,
        &log_path,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child, &log_path).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child, &log_path).await;
    ensure_realm(&data_dir, TEST_REALM).await;

    let spec = make_service_spec(
        "fp-unregistered",
        "unregistered.v1",
        r#"syntax = "proto3";
            package unregistered.v1;
            message Ping { string msg = 1; }"#,
    );
    let (mut svc_w, mut svc_r, svc_ok) = ws_register_with_spec(
        port,
        "acme",
        "svc-unregister-persist",
        None,
        Some(spec.clone()),
    )
    .await;

    let unregister = actr_protocol::ActrToSignaling {
        source: svc_ok.actr_id.clone(),
        credential: svc_ok.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::UnregisterRequest(
                actr_protocol::UnregisterRequest {
                    actr_id: svc_ok.actr_id.clone(),
                    reason: Some("durability-test".into()),
                },
            ),
        ),
    };
    send_envelope(
        &mut svc_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(unregister)),
    )
    .await;
    let response = recv_envelope(&mut svc_r).await;
    match response.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::UnregisterResponse(response)) => {
                assert!(matches!(
                    response.result,
                    Some(actr_protocol::unregister_response::Result::Success(_))
                ));
            }
            other => panic!("unexpected unregister payload {other:?}"),
        },
        other => panic!("unexpected unregister flow {other:?}"),
    }

    // No sleep or checkpoint: success must already mean the DELETE is durable.
    graceful_shutdown(child);

    let log_path2 = tmp.path().join("loran_unregister_persist2.log");
    let mut child2 = spawn_loran(&config_path, &log_path2);
    wait_for_signer_ready(
        &format!("http://127.0.0.1:{signer_port}"),
        &mut child2,
        &log_path2,
    )
    .await;
    wait_for_health(&format!("{base}/issuer/health"), &mut child2, &log_path2).await;
    wait_for_health(&format!("{base}/signaling/health"), &mut child2, &log_path2).await;
    ensure_realm(&data_dir, TEST_REALM).await;

    let (mut client_w, mut client_r, client_ok) =
        ws_register(port, "acme", "unregister-verifier", None).await;
    match query_service_spec_result(&mut client_w, &mut client_r, &client_ok, &spec.name).await {
        actr_protocol::get_service_spec_response::Result::Error(error) => {
            assert_eq!(error.code, 404);
        }
        other => panic!("unregistered ServiceSpec resurrected after restart: {other:?}"),
    }

    let _ = client_w.send(WsMessage::Close(None)).await;
    graceful_shutdown(child2);
}

/// ACL that would let `acme/<from>` from any Realm reach the registering type.
fn any_realm_allow_acl(from: &str) -> Acl {
    Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: from.into(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(SourceRealm::AnyRealm(true)),
        }],
    }
}

#[tokio::test]
#[serial]
async fn admin_api_manages_many_realms_by_name() {
    let harness = LoranHarness::start_tls(DEFAULT_TOKEN_TTL).await;
    let client = test_http_client();
    let base = harness.base_url();
    const MANAGED: &str = "second@localhost";
    let realm_url = format!("{base}/admin/api/realms/second%40localhost");

    let login: Value = client
        .post(format!("{base}/admin/api/auth/login"))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("admin login request")
        .json()
        .await
        .expect("parse admin login response");
    let token = login["token"].as_str().expect("admin login token");
    let package = json!({
        "href": "https://cdn.example/notes-1.0.actr",
        "sha256": "ab".repeat(32),
        "type": "acme.com/notes@1.0.0",
    });

    let created = create_provider_realm(&client, &base, token, MANAGED).await;
    assert_eq!(created["realm"]["status"], "active");
    let first_invite = created["owner_invite"]
        .as_str()
        .expect("create returns the owner invite")
        .to_string();
    assert!(created.get("realm_secret").is_none());

    let listed: Value = client
        .get(format!("{base}/admin/api/realms"))
        .bearer_auth(token)
        .send()
        .await
        .expect("list realms")
        .json()
        .await
        .expect("list realms json");
    let names: HashSet<&str> = listed["realms"]
        .as_array()
        .expect("realms array")
        .iter()
        .filter_map(|realm| realm["realm"].as_str())
        .collect();
    assert_eq!(names, HashSet::from([TEST_REALM, MANAGED]));

    // Only an admin session authenticates; there is no Realm admin token.
    let unknown = client
        .get(&realm_url)
        .bearer_auth("rat_unknown")
        .send()
        .await
        .expect("unknown token");
    assert_eq!(unknown.status(), reqwest::StatusCode::UNAUTHORIZED);

    // A new owner invite replaces the unused one.
    let reissued: Value = client
        .post(format!("{realm_url}/owner-invite"))
        .bearer_auth(token)
        .send()
        .await
        .expect("owner invite")
        .json()
        .await
        .expect("owner invite json");
    let owner_invite = reissued["owner_invite"].as_str().expect("owner invite");
    let (status, error) = join(&base, MANAGED, &first_invite, "second-owner").await;
    assert_eq!(status, reqwest::StatusCode::GONE);
    assert_eq!(error["error"], "invite_revoked");
    let (status, joined) = join(&base, MANAGED, owner_invite, "second-owner").await;
    assert_eq!(status, reqwest::StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "owner");

    // The owner updates the Realm Loran signs the Realm Profile of; the
    // package stays the same program.
    {
        use actr_protocol::realm_membership::MembershipOp;
        let mut update = membership_request(MembershipOp::UpdateRealm, MANAGED);
        update.display_name = Some("Renamed".to_string());
        let (status, updated) = membership(&base, update, "second-owner").await;
        assert_eq!(status, reqwest::StatusCode::OK, "{updated}");
        assert_eq!(updated["sequence"], 2);
        let mut other_program = membership_request(MembershipOp::UpdateRealm, MANAGED);
        other_program.package = Some(actr_protocol::realm_profile::RealmPackage {
            href: "https://cdn.example/other.actr".to_string(),
            sha256: "cd".repeat(32),
            r#type: "acme.com/other@1.0.0".to_string(),
        });
        let (status, error) = membership(&base, other_program, "second-owner").await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{error}");
    }

    // The operator sees the members, read-only.
    let members: Value = client
        .get(format!("{realm_url}/members"))
        .bearer_auth(token)
        .send()
        .await
        .expect("members")
        .json()
        .await
        .expect("members json");
    assert_eq!(members["members"][0]["role"], "owner");
    assert_eq!(
        members["members"][0]["clients"][0]["key_id"],
        joined["key_id"]
    );
    // A Realm with a Realm Profile its domain's key holder signs is not the
    // operator's to recover.
    // (TEST_REALM has none, so the operator issues its owner invites.)
    let test_realm_invite = client
        .post(format!(
            "{base}/admin/api/realms/test%40example.com/owner-invite"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("owner invite without a Realm Profile");
    assert_eq!(test_realm_invite.status(), reqwest::StatusCode::OK);

    let register = |name: &'static str| {
        let base = base.clone();
        async move {
            issuer_register_result(
                &base,
                MANAGED,
                "acme",
                name,
                None,
                None,
                Some("second-owner"),
            )
            .await
        }
    };
    assert!(matches!(
        register("second-ok").await,
        register_response::Result::Success(_)
    ));

    // The name is immutable: the update body has no name to change.
    let rename = client
        .put(&realm_url)
        .bearer_auth(token)
        .json(&json!({ "name": "renamed@acme.com" }))
        .send()
        .await
        .expect("rename attempt");
    assert!(rename.status().is_client_error());

    let suspended: Value = client
        .put(&realm_url)
        .bearer_auth(token)
        .json(&json!({ "enabled": false }))
        .send()
        .await
        .expect("suspend realm")
        .json()
        .await
        .expect("suspend json");
    assert_eq!(suspended["realm"]["enabled"], false);
    match register("second-suspended").await {
        register_response::Result::Error(err) => assert_eq!(err.code, 403),
        other => panic!("suspended Realm must not register Actors: {other:?}"),
    }

    let resumed = client
        .put(&realm_url)
        .bearer_auth(token)
        .json(&json!({ "enabled": true }))
        .send()
        .await
        .expect("resume realm");
    assert!(resumed.status().is_success());
    assert!(matches!(
        register("second-resumed").await,
        register_response::Result::Success(_)
    ));

    let retired: Value = client
        .post(format!("{realm_url}/retire"))
        .bearer_auth(token)
        .send()
        .await
        .expect("retire realm")
        .json()
        .await
        .expect("retire json");
    assert_eq!(retired["realm"]["status"], "retired");
    match register("second-retired").await {
        register_response::Result::Error(err) => assert_eq!(err.code, 403),
        other => panic!("retired Realm must not register Actors: {other:?}"),
    }

    // A retired Realm is final and its name is never reused.
    for response in [
        client
            .put(&realm_url)
            .bearer_auth(token)
            .json(&json!({ "enabled": true }))
            .send()
            .await
            .expect("resume retired realm"),
        client
            .post(format!("{realm_url}/owner-invite"))
            .bearer_auth(token)
            .send()
            .await
            .expect("owner invite of a retired realm"),
        client
            .post(format!("{base}/admin/api/realms"))
            .bearer_auth(token)
            .json(&json!({ "realm": MANAGED, "package": package }))
            .send()
            .await
            .expect("recreate retired realm"),
    ] {
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    }

    // The other Realm is untouched.
    assert!(matches!(
        issuer_register_result(
            &base,
            TEST_REALM,
            "acme",
            "first-still-ok",
            None,
            None,
            Some(TEST_CLIENT_KEY)
        )
        .await,
        register_response::Result::Success(_)
    ));

    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_discovery_cross_realm_isolated() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    ensure_realm(&harness.data_dir, SECOND_REALM).await;

    // The same service type in both Realms, each open to clients from any Realm.
    let (_other_w, _other_r, _other_ok) = ws_register_in_realm_with_spec(
        harness.port,
        "acme",
        "svc-cross-realm",
        SECOND_REALM,
        Some(any_realm_allow_acl("client")),
        None,
    )
    .await;
    let (_own_w, _own_r, _own_ok) = ws_register(
        harness.port,
        "acme",
        "svc-cross-realm",
        Some(any_realm_allow_acl("client")),
    )
    .await;
    let (mut client_write, mut client_read, client_ok) =
        ws_register(harness.port, "acme", "client", None).await;
    let (mut second_write, mut second_read, second_ok) =
        ws_register_in_realm_with_spec(harness.port, "acme", "client", SECOND_REALM, None, None)
            .await;

    for (write, read, ok) in [
        (&mut client_write, &mut client_read, &client_ok),
        (&mut second_write, &mut second_read, &second_ok),
    ] {
        let entries = query_discovery_entries(write, read, ok, Some("acme")).await;
        let services: Vec<_> = entries
            .iter()
            .filter(|entry| entry.actr_type.name == "svc-cross-realm")
            .collect();
        assert_eq!(
            services.len(),
            1,
            "each Realm sees only its own service: {entries:?}"
        );
    }

    let _ = client_write.send(WsMessage::Close(None)).await;
    let _ = second_write.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_route_candidates_cross_realm_isolated() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    ensure_realm(&harness.data_dir, SECOND_REALM).await;

    let (_other_w, _other_r, _other_ok) = ws_register_in_realm_with_spec(
        harness.port,
        "acme",
        "svc-cross-route",
        SECOND_REALM,
        Some(any_realm_allow_acl("client")),
        None,
    )
    .await;
    let (mut client_write, mut client_read, client_ok) =
        ws_register(harness.port, "acme", "client", None).await;

    let candidates = query_route_candidates(
        &mut client_write,
        &mut client_read,
        &client_ok,
        "acme",
        "svc-cross-route",
    )
    .await;
    assert!(
        candidates.is_empty(),
        "route candidates must not include another Realm's Actors: {candidates:?}"
    );

    let (_own_w, _own_r, own_ok) = ws_register(
        harness.port,
        "acme",
        "svc-cross-route",
        Some(any_realm_allow_acl("client")),
    )
    .await;
    let candidates = query_route_candidates(
        &mut client_write,
        &mut client_read,
        &client_ok,
        "acme",
        "svc-cross-route",
    )
    .await;
    assert_eq!(candidates, vec![own_ok.actr_id.clone()]);

    let _ = client_write.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_get_service_spec_cross_realm_isolated() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    ensure_realm(&harness.data_dir, SECOND_REALM).await;

    let spec = make_service_spec(
        "fp-cross-realm-spec",
        "crossrealm.v1",
        r#"syntax = "proto3";
            package crossrealm.v1;
            message Ping { string msg = 1; }"#,
    );
    let (_other_w, _other_r, _other_ok) = ws_register_in_realm_with_spec(
        harness.port,
        "acme",
        "svc-cross-spec",
        SECOND_REALM,
        Some(any_realm_allow_acl("client")),
        Some(spec.clone()),
    )
    .await;
    let (mut client_write, mut client_read, client_ok) =
        ws_register(harness.port, "acme", "client", None).await;

    let result =
        query_service_spec_result(&mut client_write, &mut client_read, &client_ok, &spec.name)
            .await;
    assert!(
        matches!(
            result,
            actr_protocol::get_service_spec_response::Result::Error(ref e) if e.code == 404
        ),
        "another Realm's ServiceSpec must not be visible: {result:?}"
    );

    let _ = client_write.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn signaling_relay_cross_realm_is_denied() {
    let harness = LoranHarness::start(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;
    ensure_realm(&harness.data_dir, SECOND_REALM).await;

    let (_dst_w, _dst_r, dst_ok) = ws_register_in_realm_with_spec(
        port,
        "acme",
        "relay-dst",
        SECOND_REALM,
        Some(any_realm_allow_acl("relay-src")),
        None,
    )
    .await;
    let (mut src_w, mut src_r, src_ok) = ws_register(port, "acme", "relay-src", None).await;

    let relay = ActrRelay {
        source: src_ok.actr_id.clone(),
        credential: src_ok.credential.clone(),
        target: dst_ok.actr_id.clone(),
        source_type: None,
        delivery_context: None,
        payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
            from: src_ok.actr_id.clone(),
            to: dst_ok.actr_id.clone(),
        })),
    };
    send_envelope(
        &mut src_w,
        make_envelope(signaling_envelope::Flow::ActrRelay(relay)),
    )
    .await;

    let resp = recv_envelope(&mut src_r).await;
    match resp.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => {
                assert_eq!(err.code, 403, "cross-Realm relay should be denied");
                assert!(
                    err.message.contains("outside this connection's Realm"),
                    "unexpected relay deny message: {}",
                    err.message
                );
            }
            other => panic!("expected relay error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    harness.shutdown();
}

/// Create a Realm under the Provider domain through the admin API. The Realm
/// signing key is generated at startup, possibly after health.
async fn create_provider_realm(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    realm: &str,
) -> Value {
    let package = json!({
        "href": "https://cdn.example/notes-1.0.actr",
        "sha256": "ab".repeat(32),
        "type": "acme.com/notes@1.0.0",
    });
    for _ in 0..50 {
        let response = client
            .post(format!("{base}/admin/api/realms"))
            .bearer_auth(token)
            .json(&json!({ "realm": realm, "package": package }))
            .send()
            .await
            .expect("create realm");
        if response.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(response.status(), reqwest::StatusCode::CREATED);
            return response.json().await.expect("create realm json");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("Realm signing key never became ready");
}

/// `base64(HMAC-SHA1(key, username))`, the TURN password format.
fn turn_password(key: &[u8], username: &str) -> String {
    use hmac::Mac as _;
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(key).expect("HMAC key");
    mac.update(username.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// REALM_V1_ARCHITECTURE §6: two Realms with the same Actor type and the
/// same serial number share nothing: Actor credentials, discovery, presence,
/// TURN credentials, connection ACLs and admin APIs stay within each Realm.
#[tokio::test]
#[serial]
async fn realms_with_the_same_actor_type_and_serial_are_isolated() {
    const ALPHA: &str = "alpha@localhost";
    const BETA: &str = "beta@localhost";
    let harness = LoranHarness::start_tls(DEFAULT_TOKEN_TTL).await;
    let port = harness.port;
    let base = harness.base_url();
    let client = test_http_client();

    let login: Value = client
        .post(format!("{base}/admin/api/auth/login"))
        .json(&json!({ "password": "testpassword123" }))
        .send()
        .await
        .expect("admin login request")
        .json()
        .await
        .expect("parse admin login response");
    let token = login["token"].as_str().expect("admin login token");
    let alpha = create_provider_realm(&client, &base, token, ALPHA).await;
    let beta = create_provider_realm(&client, &base, token, BETA).await;
    for (created, realm, label) in [(&alpha, ALPHA, "alpha-owner"), (&beta, BETA, "beta-owner")] {
        let (status, joined) = join(
            &base,
            realm,
            created["owner_invite"].as_str().unwrap(),
            label,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{joined}");
    }
    let (alpha_secret, beta_secret) = ("alpha-owner".to_string(), "beta-owner".to_string());

    // The same service type in both Realms, open to acme/client of its own Realm.
    let acl = Acl {
        rules: vec![AclRule {
            permission: Permission::Allow as i32,
            from_type: ActrType {
                publisher: "acme".into(),
                name: "client".into(),
                version: "1.0.0".to_string(),
            },
            source_realm: None,
        }],
    };
    let register = |realm: &'static str, secret: String, name: &'static str, acl: Option<Acl>| {
        let base = base.clone();
        async move {
            issuer_register_http_with_key(&base, realm, "acme", name, None, acl, Some(&secret))
                .await
        }
    };
    let alpha_client = register(ALPHA, alpha_secret.clone(), "client", None).await;
    let beta_client = register(BETA, beta_secret.clone(), "client", None).await;
    let alpha_svc = register(ALPHA, alpha_secret.clone(), "svc-same", Some(acl.clone())).await;
    let beta_svc = register(BETA, beta_secret.clone(), "svc-same", Some(acl.clone())).await;

    // Each Realm signs with its own issuer key.
    assert_ne!(
        alpha_client.credential.key_id,
        beta_client.credential.key_id
    );
    assert_ne!(alpha_client.signing_pubkey, beta_client.signing_pubkey);
    assert_eq!(alpha_client.credential.key_id, alpha_svc.credential.key_id);
    assert_eq!(alpha_client.signing_key_id, alpha_client.credential.key_id);

    // A credential replayed as the same serial in the other Realm is refused.
    for (ok, other_realm) in [(&alpha_client, BETA), (&beta_client, ALPHA)] {
        let mut replayed = ok.clone();
        replayed.actr_id.realm = other_realm.to_string();
        let result = connect_wss(port, &replayed).await;
        assert!(
            result.is_err(),
            "a credential of one Realm must not open a connection in another"
        );
    }

    let (mut alpha_w, mut alpha_r) = connect_wss_authenticated(port, &alpha_client).await;
    let (mut beta_w, mut beta_r) = connect_wss_authenticated(port, &beta_client).await;

    // An envelope for the other Realm is refused on this connection, even
    // with that Realm's valid credential.
    let foreign = actr_protocol::ActrToSignaling {
        source: beta_client.actr_id.clone(),
        credential: beta_client.credential.clone(),
        payload: Some(actr_protocol::actr_to_signaling::Payload::DiscoveryRequest(
            actr_protocol::DiscoveryRequest {
                publisher: None,
                limit: Some(64),
            },
        )),
    };
    send_envelope(
        &mut alpha_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(foreign)),
    )
    .await;
    match recv_envelope(&mut alpha_r).await.flow {
        Some(signaling_envelope::Flow::ServerToActr(server_msg)) => match server_msg.payload {
            Some(signaling_to_actr::Payload::Error(err)) => assert_eq!(err.code, 403),
            other => panic!("expected an error, got {other:?}"),
        },
        other => panic!("unexpected flow {other:?}"),
    }

    // Presence: Alpha's client hears only about Alpha's service.
    let subscribe = actr_protocol::ActrToSignaling {
        source: alpha_client.actr_id.clone(),
        credential: alpha_client.credential.clone(),
        payload: Some(
            actr_protocol::actr_to_signaling::Payload::SubscribeActrUpRequest(
                actr_protocol::SubscribeActrUpRequest {
                    target_type: ActrType {
                        publisher: "acme".into(),
                        name: "svc-same".into(),
                        version: "1.0.0".to_string(),
                    },
                },
            ),
        ),
    };
    send_envelope(
        &mut alpha_w,
        make_envelope(signaling_envelope::Flow::ActrToServer(subscribe)),
    )
    .await;
    let _ = recv_envelope(&mut alpha_r).await; // subscribe ack
    let (_beta_svc_w, _beta_svc_r) = connect_wss_authenticated(port, &beta_svc).await;
    let (_alpha_svc_w, _alpha_svc_r) = connect_wss_authenticated(port, &alpha_svc).await;
    let up = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(signaling_envelope::Flow::ServerToActr(server_msg)) =
                recv_envelope(&mut alpha_r).await.flow
                && let Some(signaling_to_actr::Payload::ActrUpEvent(event)) = server_msg.payload
            {
                return event.actor_id;
            }
        }
    })
    .await
    .expect("ActrUp for Alpha's service");
    assert_eq!(
        up, alpha_svc.actr_id,
        "the first ActrUp is Alpha's own service"
    );

    // Discovery and route candidates stay within the Realm.
    for (write, read, ok, own_svc) in [
        (&mut alpha_w, &mut alpha_r, &alpha_client, &alpha_svc),
        (&mut beta_w, &mut beta_r, &beta_client, &beta_svc),
    ] {
        let candidates = query_route_candidates(write, read, ok, "acme", "svc-same").await;
        assert_eq!(candidates, vec![own_svc.actr_id.clone()]);
        let entries = query_discovery_entries(write, read, ok, Some("acme")).await;
        let services = entries
            .iter()
            .filter(|entry| entry.actr_type.name == "svc-same")
            .count();
        assert_eq!(
            services, 1,
            "each Realm sees only its own service: {entries:?}"
        );
    }

    // TURN credentials are keyed by each Realm's own derived secret.
    let master = b"fullstack-turn-secret-0123456789abcdef";
    for (ok, realm, other) in [(&alpha_svc, ALPHA, BETA), (&beta_svc, BETA, ALPHA)] {
        let turn = &ok.turn_credential;
        assert_eq!(turn.realm, realm);
        let own = platform::realm::realm_turn_secret(master, realm, "localhost");
        let foreign = platform::realm::realm_turn_secret(master, other, "localhost");
        assert_eq!(turn.password, turn_password(&own, &turn.username));
        assert_ne!(turn.password, turn_password(&foreign, &turn.username));
        assert_ne!(turn.password, turn_password(master, &turn.username));
    }

    // Connection ACLs cannot name the other Realm.
    let cross_acl = Acl {
        rules: vec![AclRule {
            source_realm: Some(SourceRealm::Realm(BETA.to_string())),
            ..acl.rules[0].clone()
        }],
    };
    match issuer_register_result(
        &base,
        ALPHA,
        "acme",
        "svc-cross-acl",
        None,
        Some(cross_acl),
        Some(&alpha_secret),
    )
    .await
    {
        register_response::Result::Error(err) => assert_eq!(err.code, 400),
        other => panic!("a cross-Realm ACL rule must be rejected: {other:?}"),
    }

    // A Realm client key belongs to one Realm only.
    match issuer_register_result(
        &base,
        BETA,
        "acme",
        "client-2",
        None,
        None,
        Some(&alpha_secret),
    )
    .await
    {
        register_response::Result::Error(err) => {
            assert!(
                err.message.starts_with("client_unknown:"),
                "{}",
                err.message
            )
        }
        other => panic!("a key of one Realm registered in another: {other:?}"),
    }

    let _ = alpha_w.send(WsMessage::Close(None)).await;
    let _ = beta_w.send(WsMessage::Close(None)).await;
    harness.shutdown();
}

#[tokio::test]
#[serial]
async fn relay_rejects_a_same_realm_peer_denied_by_actor_acl() {
    use loran_relay_protocol::{ClientControl, ServerControl};

    with_fullstack_relay(|base, port| async move {
        let denied = Acl {
            rules: vec![AclRule {
                permission: Permission::Deny as i32,
                from_type: ActrType {
                    publisher: "acme".into(),
                    name: "relay-acl-source".into(),
                    version: "1.0.0".into(),
                },
                source_realm: Some(SourceRealm::Realm(TEST_REALM.into())),
            }],
        };
        let (source, source_token) = relay_identity(&base, "relay-acl-source", None).await;
        let (target, target_token) = relay_identity(&base, "relay-acl-target", Some(denied)).await;
        let mut source_socket = relay_login(port, &source, &source_token).await;
        let mut target_socket = relay_login(port, &target, &target_token).await;
        send_relay_control(
            &mut source_socket,
            ClientControl::Connect {
                request: 1,
                peer: target,
            },
        )
        .await;

        tokio::select! {
            response = relay_control(&mut source_socket) => {
                assert!(
                    matches!(response, ServerControl::Error { request: Some(1), .. }),
                    "relay should reject the denied actor: {response:?}"
                );
            }
            response = relay_control(&mut target_socket) => {
                panic!("ACL-denied actor reached the target relay socket: {response:?}");
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(250), target_socket.next())
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn relay_rechecks_actor_acl_before_accepting_a_pending_channel() {
    use loran_relay_protocol::{ClientControl, CloseReason, ServerControl};

    with_fullstack_relay(|base, port| async move {
        let (source, source_token) = relay_identity(&base, "relay-acl-change-source", None).await;
        let (target, target_token) = relay_identity(
            &base,
            "relay-acl-change-target",
            Some(relay_allow_from("relay-acl-change-source", TEST_REALM)),
        )
        .await;
        let mut a = relay_login(port, &source, &source_token).await;
        let mut b = relay_login(port, &target, &target_token).await;
        send_relay_control(&mut a, ClientControl::Connect { request: 1, peer: target }).await;
        let ServerControl::Incoming { channel, .. } = relay_control(&mut b).await else {
            panic!("target did not receive a pending request");
        };
        let mut denied = relay_allow_from("relay-acl-change-source", TEST_REALM);
        denied.rules[0].permission = Permission::Deny as i32;
        issuer_register_http(&base, TEST_REALM, "acme", "relay-acl-change-target", None, Some(denied)).await;
        send_relay_control(&mut b, ClientControl::Accept { channel }).await;
        for socket in [&mut a, &mut b] {
            assert!(matches!(
                relay_control(socket).await,
                ServerControl::Closed { channel: id, reason: CloseReason::Rejected, .. } if id == channel
            ));
        }
    }).await;
}

#[tokio::test]
#[serial]
async fn relay_rejects_a_copied_certificate_without_key_possession() {
    use loran_relay_protocol::ServerControl;

    with_fullstack_relay(|base, port| async move {
        let (endpoint, token) = relay_identity(&base, "relay-replay-victim", None).await;
        let mut victim = relay_login(port, &endpoint, &token).await;
        // A peer learns the public certificate during the Noise handshake.
        // This socket has only that certificate, not the endpoint's private key.
        let credential: actr_relay_security::RelayCredential = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD.decode(&token).unwrap(),
        )
        .unwrap();
        let public_certificate = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&credential.certificate).unwrap());
        let (_, certificate_only) = relay_hello(port, &endpoint, &public_certificate).await;
        assert!(matches!(certificate_only, WsMessage::Close(_)));
        let copied = actr_relay_security::RelayCredential {
            certificate: credential.certificate.clone(),
            admission_secret: [42; 32],
        };
        let copied = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&copied).unwrap());
        let (_replay, response) = relay_hello(port, &endpoint, &copied).await;
        let victim_result = tokio::time::timeout(Duration::from_millis(500), victim.next()).await;
        let admitted = matches!(
            &response,
            WsMessage::Text(text)
                if matches!(serde_json::from_str::<ServerControl>(text), Ok(ServerControl::Welcome { .. }))
        );
        assert!(
            !admitted,
            "copied certificate was admitted; original socket result: {victim_result:?}"
        );
        assert!(
            victim_result.is_err(),
            "rejected replay displaced the live socket"
        );
        let (_replacement, legitimate) = relay_hello(port, &endpoint, &token).await;
        assert!(matches!(
            legitimate,
            WsMessage::Text(text)
                if matches!(serde_json::from_str::<ServerControl>(&text), Ok(ServerControl::Welcome { .. }))
        ));
        let displaced = tokio::time::timeout(Duration::from_secs(3), victim.next())
            .await
            .expect("replacement did not close the old socket");
        assert!(matches!(displaced, None | Some(Ok(WsMessage::Close(_)))));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn relay_revokes_one_realm_without_disconnecting_another() {
    use loran_relay_protocol::{ServerControl, encode_data};

    with_fullstack_relay(|base, port| async move {
        let (revoked, revoked_token) = relay_identity(&base, "relay-revoked", None).await;
        let (revoked_peer, revoked_peer_token) = relay_identity(
            &base,
            "relay-revoked-peer",
            Some(relay_allow_from("relay-revoked", TEST_REALM)),
        )
        .await;
        let (first, first_token) =
            relay_identity_in_realm(&base, SECOND_REALM, "relay-first", None).await;
        let (second, second_token) = relay_identity_in_realm(
            &base,
            SECOND_REALM,
            "relay-second",
            Some(relay_allow_from("relay-first", SECOND_REALM)),
        )
        .await;
        let mut revoked_socket = relay_login(port, &revoked, &revoked_token).await;
        let mut revoked_peer_socket = relay_login(port, &revoked_peer, &revoked_peer_token).await;
        let mut first_socket = relay_login(port, &first, &first_token).await;
        let mut second_socket = relay_login(port, &second, &second_token).await;
        relay_pair(
            &mut revoked_socket,
            &mut revoked_peer_socket,
            &revoked,
            &revoked_peer,
        )
        .await;
        let channel = relay_pair(&mut first_socket, &mut second_socket, &first, &second).await;
        let before = encode_data(channel, b"before suspension").unwrap();
        first_socket
            .send(WsMessage::Binary(before.clone()))
            .await
            .unwrap();
        assert_eq!(
            second_socket.next().await.unwrap().unwrap(),
            WsMessage::Binary(before)
        );

        let client = reqwest::Client::new();
        let login: Value = client
            .post(format!("{base}/admin/api/auth/login"))
            .json(&json!({ "password": "testpassword123" }))
            .send()
            .await
            .expect("admin login")
            .json()
            .await
            .expect("admin login response");
        let token = login["token"].as_str().expect("admin token");
        let response = client
            .put(format!("{base}/admin/api/realms/test%40example.com"))
            .bearer_auth(token)
            .json(&json!({ "enabled": false }))
            .send()
            .await
            .expect("suspend realm");
        assert!(response.status().is_success());
        for socket in [&mut revoked_socket, &mut revoked_peer_socket] {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match socket.next().await {
                        None | Some(Ok(WsMessage::Close(_))) => break,
                        Some(Ok(WsMessage::Text(text))) => assert!(matches!(
                            serde_json::from_str::<ServerControl>(&text),
                            Ok(ServerControl::Closed { .. })
                        )),
                        other => panic!("unexpected revoked relay frame: {other:?}"),
                    }
                }
            })
            .await
            .expect("revoked socket deadline");
        }
        let (_, rejected) = relay_hello(port, &revoked, &revoked_token).await;
        assert!(matches!(rejected, WsMessage::Close(_)));

        let frame = encode_data(channel, b"unaffected realm").unwrap();
        first_socket
            .send(WsMessage::Binary(frame.clone()))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(5), second_socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(forwarded, WsMessage::Binary(frame));
    })
    .await;
}
