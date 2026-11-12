//! Issuer integration tests (self-bootstrapping)
//!
//! Starts a temporary Signer gRPC service within the test process to verify the Issuer issuance and validation chain.

use actr_protocol::realm_membership::{
    CLIENT_PROOF_HEADER, ClientProof, ProofOp, RealmInvite, Role, client_key_id, invite_secret_hash,
};
use actr_protocol::{
    Acl, AclRule, ActrType, ErrorResponse, RegisterAuthMode, RegisterRequest, RegisterResponse,
    ReissueCredentialRequest, RenewCredentialRequest, RenewCredentialResponse, acl_rule,
    register_response, renew_credential_response,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::Engine as _;
use issuer::publisher_profiles::{PublicDocumentFetcher, PublisherProfileRefresher};
use issuer::signer_client_wrapper::create_signer_client;
use issuer::{
    handlers::{IssuerState, create_router},
    issuer::{AIdIssuer, IssuerConfig, REISSUE_POSSESSION_NOT_PROVEN_CODE},
};
use nonce_auth::storage::MemoryStorage;
use platform::aid::credential::validator::AIdCredentialValidator;
use platform::config::signer::SignerClientConfig;
use platform::realm::membership::{self, BY_OPERATOR, Limits, Signer};
use prost::Message;
use serial_test::serial;
use signer::{GrpcClient, GrpcClientConfig, KeyStorage, SignerServiceConfig, create_grpc_service};
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tower::ServiceExt;

const TEST_REALM: &str = "test@example.com";
mod device_membership;
mod v2_package;
const ROUTE_KEY_LABEL: &str = "test-route-realm-client-key";
const PROVIDER_ID: &str = "provider.example";
/// Label of the owner key a Realm is seeded with when a test names none.
const DEFAULT_KEY_LABEL: &str = "seed-client-key";

/// Realm client keys by `key_id`, to sign as the key an Actor is bound to.
static KEYS: LazyLock<Mutex<HashMap<String, ed25519_dalek::SigningKey>>> =
    LazyLock::new(Default::default);

/// The Realm client key a test names by `label`.
fn key_for(label: &str) -> ed25519_dalek::SigningKey {
    use sha2::Digest;
    let key = ed25519_dalek::SigningKey::from_bytes(&sha2::Sha256::digest(label).into());
    KEYS.lock()
        .unwrap()
        .insert(client_key_id(&key.verifying_key().to_bytes()), key.clone());
    key
}

fn limits() -> Limits {
    Limits::from(&platform::config::RealmConfig::default())
}

/// Register the key `label` in `realm` for a member: a new member of `role`,
/// or the owner (recovering it, if it exists). Returns its signer.
async fn add_client(realm: &str, label: &str, role: Role) -> Signer {
    let key = key_for(label).verifying_key().to_bytes();
    let key_id = client_key_id(&key);
    if let Some(existing) = membership::client_key(realm, &key_id).await.unwrap() {
        return membership::authenticate(existing).await.unwrap();
    }
    let invite = match role {
        Role::Owner => membership::issue_owner_invite(realm, BY_OPERATOR)
            .await
            .unwrap(),
        role => {
            let owner = owner(realm).await;
            let expires = chrono::Utc::now().timestamp() + 3600;
            membership::create_invite(&owner, Some(role), None, expires, limits())
                .await
                .unwrap()
        }
    };
    let secret = RealmInvite::decode(&invite.invite).unwrap().secret;
    membership::join(
        realm,
        &invite_secret_hash(&secret).unwrap(),
        &key,
        label,
        limits(),
    )
    .await
    .unwrap();
    let registered = membership::client_key(realm, &key_id)
        .await
        .unwrap()
        .unwrap();
    membership::authenticate(registered).await.unwrap()
}

/// The owner of `realm`, seeded with the default key if it has none.
async fn owner(realm: &str) -> Signer {
    let key_id: Option<String> = sqlx::query_scalar(
        "SELECT k.key_id FROM realm_client_key k
         JOIN realm_member m ON m.realm = k.realm AND m.member_id = k.member_id
         WHERE k.realm = ? AND m.role = 'owner' AND k.revoked_at IS NULL
         ORDER BY k.registered_at LIMIT 1",
    )
    .bind(realm)
    .fetch_optional(platform::storage::db::get_database().get_pool())
    .await
    .unwrap();
    match key_id {
        Some(key_id) => membership::authenticate(
            membership::client_key(realm, &key_id)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap(),
        None => Box::pin(add_client(realm, DEFAULT_KEY_LABEL, Role::Owner)).await,
    }
}

/// An `x-actr-client-proof` for `body`, signed by `key`.
fn proof(op: ProofOp, realm: &str, body: &[u8], key: &ed25519_dalek::SigningKey) -> String {
    ClientProof::new(
        op,
        realm,
        PROVIDER_ID,
        body,
        &uuid::Uuid::new_v4().to_string(),
    )
    .header_value(key)
}

/// The key `actor` is bound to, or the default one.
async fn bound_key(actor: &actr_protocol::ActrId) -> ed25519_dalek::SigningKey {
    let key_id: Option<String> = sqlx::query_scalar(
        "SELECT client_key_id FROM issued_actor_identity WHERE realm = ? AND serial_number = ?",
    )
    .bind(&actor.realm)
    .bind(actor.serial_number as i64)
    .fetch_optional(platform::storage::db::get_database().get_pool())
    .await
    .unwrap();
    key_id
        .and_then(|key_id| KEYS.lock().unwrap().get(&key_id).cloned())
        .unwrap_or_else(|| key_for(DEFAULT_KEY_LABEL))
}

struct TestEnv {
    issuer_temp_dir: TempDir,
    _signer_temp_dir: TempDir,
    signer_handle: Option<JoinHandle<()>>,
    signer_shutdown_tx: Option<oneshot::Sender<()>>,
    signer_config: SignerClientConfig,
    shared_key: String,
}

impl TestEnv {
    async fn shutdown_signer(&mut self) {
        if let Some(tx) = self.signer_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.signer_handle.take() {
            let _ = handle.await;
        }
    }
}

async fn start_embedded_signer_with_config(
    shared_key: &str,
    sqlite_path: &Path,
    service_config: SignerServiceConfig,
) -> (String, JoinHandle<()>, oneshot::Sender<()>) {
    start_embedded_signer_at(shared_key, sqlite_path, service_config, None).await
}

/// Start an embedded Signer, optionally pinned to a fixed address (used to
/// simulate a Signer restart on the same endpoint).
async fn start_embedded_signer_at(
    shared_key: &str,
    sqlite_path: &Path,
    service_config: SignerServiceConfig,
    fixed_addr: Option<std::net::SocketAddr>,
) -> (String, JoinHandle<()>, oneshot::Sender<()>) {
    let storage = KeyStorage::from_config(
        &service_config.storage,
        signer::KeyEncryptor::no_encryption(),
        sqlite_path,
    )
    .await
    .expect("Failed to create Signer storage");

    let addr = match fixed_addr {
        Some(addr) => addr,
        None => {
            let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind ephemeral port");
            let addr = listener.local_addr().expect("Failed to get local addr");
            drop(listener);
            addr
        }
    };

    let service = create_grpc_service(storage, MemoryStorage::new(), shared_key.to_string());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        if let Err(err) = Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, async move {
                let _ = shutdown_rx.await;
            })
            .await
        {
            panic!("Embedded Signer server failed: {err}");
        }
    });

    let endpoint = format!("http://{addr}");

    // Wait until gRPC health check is reachable.
    let mut last_error = String::new();
    for _ in 0..40 {
        let cfg = GrpcClientConfig {
            endpoint: endpoint.clone(),
            loran_shared_key: shared_key.to_string(),
            timeout_seconds: 2,
            enable_tls: false,
            tls_domain: None,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };

        match GrpcClient::new(&cfg).await {
            Ok(mut client) => match client.health_check().await {
                Ok(status) if status == "healthy" => return (endpoint, handle, shutdown_tx),
                Ok(status) => last_error = format!("unexpected Signer health status: {status}"),
                Err(err) => last_error = err.to_string(),
            },
            Err(err) => last_error = err.to_string(),
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("Embedded Signer did not become healthy in time: {last_error}");
}

async fn setup_test_environment() -> TestEnv {
    setup_test_environment_with_signer_config(SignerServiceConfig::default()).await
}

async fn setup_test_environment_with_signer_config(
    signer_service_config: SignerServiceConfig,
) -> TestEnv {
    use std::sync::OnceLock;
    static DB_DIR: OnceLock<TempDir> = OnceLock::new();

    let issuer_temp_dir = TempDir::new().expect("Failed to create issuer temp dir");
    let signer_temp_dir = TempDir::new().expect("Failed to create signer temp dir");
    let shared_key = "test-shared-key".to_string();
    let (endpoint, signer_handle, signer_shutdown_tx) = start_embedded_signer_with_config(
        &shared_key,
        signer_temp_dir.path(),
        signer_service_config,
    )
    .await;

    // Initialize the global database once with a persistent temp dir.
    // The OnceLock ensures the TempDir (and its SQLite file) lives for the
    // entire process, avoiding "unable to open database file" in serial tests.
    let db_dir = DB_DIR.get_or_init(|| TempDir::new().expect("Failed to create DB temp dir"));
    if !platform::storage::db::is_database_initialized() {
        platform::storage::db::set_db_path(db_dir.path())
            .await
            .expect("Failed to initialize test database");
    }
    seed_realm(TEST_REALM, None).await;

    let signer_config = SignerClientConfig {
        endpoint,
        timeout_seconds: 10,
        enable_tls: false,
        tls_domain: None,
        ca_cert: None,
        client_cert: None,
        client_key: None,
    };

    TestEnv {
        issuer_temp_dir,
        _signer_temp_dir: signer_temp_dir,
        signer_handle: Some(signer_handle),
        signer_shutdown_tx: Some(signer_shutdown_tx),
        signer_config,
        shared_key,
    }
}

fn default_issuer_config(temp_dir: &TempDir) -> IssuerConfig {
    IssuerConfig {
        worker_id: 0,
        token_ttl_secs: 3600,
        renewal_token_ttl_secs: 24 * 60 * 60,
        renewal_rotation_window_secs: 60 * 60,
        renewal_token_secret: vec![0; 32],
        signaling_heartbeat_interval_secs: 30,
        turn_secret: "test-turn-secret-at-least-32-bytes!".to_string(),
        provider_id: "provider.example".to_string(),
        sqlite_path: temp_dir.path().to_path_buf(),
    }
}

fn linked_register_request() -> RegisterRequest {
    RegisterRequest {
        actr_type: ActrType {
            publisher: "linked-src".to_string(),
            name: "source-workload".to_string(),
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
    }
}

/// Actor type signed into the credential claims of a registration.
fn registered_type(ok: &register_response::RegisterOk) -> ActrType {
    actr_protocol::IdentityClaims::decode(&ok.credential.claims[..])
        .expect("decode claims")
        .actor_type
}

/// The development indicator signed into the credential claims.
fn development(ok: &register_response::RegisterOk) -> Option<bool> {
    actr_protocol::IdentityClaims::decode(&ok.credential.claims[..])
        .expect("decode claims")
        .development
}

fn control_attempt_id() -> prost::bytes::Bytes {
    prost::bytes::Bytes::copy_from_slice(uuid::Uuid::new_v4().as_bytes())
}

fn package_register_request(
    publisher: &str,
    name: &str,
    version: &str,
    realm: &str,
    target: &str,
    manifest: Vec<u8>,
    manifest_signature: Option<Vec<u8>>,
) -> RegisterRequest {
    RegisterRequest {
        actr_type: ActrType {
            publisher: publisher.to_string(),
            name: name.to_string(),
            version: version.to_string(),
        },
        realm: realm.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest)),
        publisher_signature: manifest_signature.map(prost::bytes::Bytes::from),
        target: Some(target.to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    }
}

async fn create_test_router(env: &TestEnv) -> Router {
    create_router(IssuerState::new(create_test_issuer(env).await))
}

async fn create_test_issuer(env: &TestEnv) -> AIdIssuer {
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer")
}

/// An active Realm whose owner holds the key `key_label` (the default key
/// if `None`).
async fn seed_realm(realm: &str, key_label: Option<&str>) {
    let pool = platform::storage::db::get_database().get_pool();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // A fresh Realm: what an earlier test left (issuer keys of another
    // Signer, members) goes with the old row.
    sqlx::query("DELETE FROM realm WHERE name = ?")
        .bind(realm)
        .execute(pool)
        .await
        .expect("clear realm");
    sqlx::query("INSERT INTO realm (name, status, enabled, created_at) VALUES (?, 'active', 1, ?)")
        .bind(realm)
        .bind(now)
        .execute(pool)
        .await
        .expect("seed realm");
    add_client(realm, key_label.unwrap_or(DEFAULT_KEY_LABEL), Role::Owner).await;
}

fn test_package_manifest(publisher: &str, name: &str, version: &str, target: &str) -> Vec<u8> {
    format!(
        r#"publisher = "{publisher}"
name = "{name}"
version = "{version}"

[binary]
path = "bin/actor.wasm"
target = "{target}"
hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#,
    )
    .into_bytes()
}

async fn signed_package_for_publisher(
    publisher: &str,
    name: &str,
    version: &str,
    target: &str,
) -> Vec<u8> {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let signing_key = SigningKey::generate(&mut OsRng);
    signed_package_for_key(publisher, name, version, target, &signing_key).await
}

async fn signed_package_for_key(
    publisher: &str,
    name: &str,
    version: &str,
    target: &str,
    signing_key: &ed25519_dalek::SigningKey,
) -> Vec<u8> {
    let pool = platform::storage::db::get_database().get_pool();
    seed_publisher_with_key(pool, publisher, signing_key).await;
    let (manifest, _) = build_signed_manifest(signing_key, publisher, name, version, target);

    manifest
}

/// Register a Publisher and return its signed package manifest and
/// signing key.
async fn signed_package_with_generated_key(
    publisher: &str,
    name: &str,
    version: &str,
    target: &str,
) -> (Vec<u8>, ed25519_dalek::SigningKey) {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let signing_key = SigningKey::generate(&mut OsRng);
    let manifest = signed_package_for_key(publisher, name, version, target, &signing_key).await;
    (manifest, signing_key)
}

/// A package registration for the manifest `signing_key` signed: everything a
/// Host holds with only the public package in hand. No Publisher private-key
/// proof is involved.
fn signed_package_request(
    publisher: &str,
    name: &str,
    version: &str,
    realm: &str,
    target: &str,
    signing_key: &ed25519_dalek::SigningKey,
) -> RegisterRequest {
    let (manifest, signature) =
        build_signed_manifest(signing_key, publisher, name, version, target);
    let mut request = package_register_request(
        publisher,
        name,
        version,
        realm,
        target,
        manifest,
        Some(signature),
    );
    request.signing_public_key = Some(prost::bytes::Bytes::from(
        signing_key.verifying_key().to_bytes().to_vec(),
    ));
    request
}

async fn post_register(
    app: Router,
    request: RegisterRequest,
    key_label: Option<&str>,
) -> RegisterResponse {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/register")
        .header("content-type", "application/protobuf")
        .header("x-real-ip", "127.0.0.1");
    if let Some(label) = key_label {
        let body = request.encode_to_vec();
        builder = builder.header(
            CLIENT_PROOF_HEADER,
            proof(ProofOp::Register, &request.realm, &body, &key_for(label)),
        );
    }

    // The rate limiter keys on the peer IP via ConnectInfo, which the production
    // server injects per connection but oneshot does not, so add it here.
    let mut http_request = builder.body(Body::from(request.encode_to_vec())).unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app
        .oneshot(http_request)
        .await
        .expect("register route response");
    let status = response.status();

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("register response body");
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected /register status {status}: {}",
        String::from_utf8_lossy(&body)
    );
    RegisterResponse::decode(body).expect("decode register response")
}

async fn post_reissue(
    app: Router,
    request: ReissueCredentialRequest,
    key_label: Option<&str>,
) -> RegisterResponse {
    let (status, response) = post_reissue_raw(app, request, key_label).await;
    assert_eq!(status, StatusCode::OK);
    response
}

/// Post a reissue request and return the HTTP status with the decoded body.
async fn post_reissue_raw(
    app: Router,
    request: ReissueCredentialRequest,
    key_label: Option<&str>,
) -> (StatusCode, RegisterResponse) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/reissue")
        .header("content-type", "application/protobuf")
        .header("x-real-ip", "127.0.0.1");
    if let Some(label) = key_label {
        let body = request.encode_to_vec();
        builder = builder.header(
            CLIENT_PROOF_HEADER,
            proof(
                ProofOp::Reissue,
                &request.registration.realm,
                &body,
                &key_for(label),
            ),
        );
    }

    let mut http_request = builder.body(Body::from(request.encode_to_vec())).unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app
        .oneshot(http_request)
        .await
        .expect("reissue route response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reissue response body");
    (
        status,
        RegisterResponse::decode(body).expect("decode reissue response"),
    )
}

/// Register a linked Actor in a fresh Realm and return its registration and
/// first credentials.
async fn register_linked_actor(
    app: &Router,
    realm: &str,
    key_label: &str,
) -> (RegisterRequest, register_response::RegisterOk) {
    seed_realm(realm, Some(key_label)).await;
    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    match initial.result.expect("register result") {
        register_response::Result::Success(ok) => (registration, ok),
        register_response::Result::Error(err) => panic!("register failed: {err:?}"),
    }
}

fn error_code(response: &RegisterResponse) -> Option<u32> {
    match &response.result {
        Some(register_response::Result::Error(err)) => Some(err.code),
        _ => None,
    }
}

#[tokio::test]
#[serial]
async fn test_register_route_linked_with_client_proof_succeeds() {
    let env = setup_test_environment().await;
    let key_label = "linked-http-route-secret";
    let realm = "linked-http-route@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut request = linked_register_request();
    request.realm = realm.to_string();
    let response = post_register(app, request, Some(key_label)).await;

    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id.realm, realm);
            assert_eq!(registered_type(&ok).name, "source-workload");
        }
        register_response::Result::Error(err) => {
            panic!("linked /register with a client proof should succeed: {err:?}")
        }
    }
}

async fn register_error(env: &TestEnv, realm: &str, secret: Option<&str>) -> ErrorResponse {
    let mut request = linked_register_request();
    request.realm = realm.to_string();
    let app = create_test_router(env).await;
    match post_register(app, request, secret)
        .await
        .result
        .expect("result")
    {
        register_response::Result::Error(err) => err,
        register_response::Result::Success(ok) => panic!("registered {ok:?}"),
    }
}

#[tokio::test]
#[serial]
async fn test_register_does_not_reveal_realm_state_before_authentication() {
    let env = setup_test_environment().await;
    let secret = "realm-state-secret";
    let suspended = "state-suspended@example.com";
    let retired = "state-retired@example.com";
    seed_realm(suspended, Some(secret)).await;
    seed_realm(retired, Some(secret)).await;
    platform::realm::Realm::set_enabled(suspended, false)
        .await
        .expect("suspend");
    platform::realm::Realm::retire(retired)
        .await
        .expect("retire");

    // Without a registered Realm client key, every Realm looks the same.
    for realm in ["state-unknown@example.com", suspended, retired] {
        let err = register_error(&env, realm, None).await;
        assert_eq!(err.code, 400, "{realm}");
        assert!(
            err.message.starts_with("invalid_request:"),
            "{}",
            err.message
        );
        let err = register_error(&env, realm, Some("unregistered-key")).await;
        assert_eq!(err.code, 403, "{realm}");
        assert!(
            err.message.starts_with("client_unknown:"),
            "{}",
            err.message
        );
    }
    // With one, the caller learns why.
    let err = register_error(&env, suspended, Some(secret)).await;
    assert_eq!(err.code, 403);
    assert!(err.message.contains("suspended"), "{}", err.message);
}

#[tokio::test]
#[serial]
async fn test_register_pending_failure_rolls_back_acl() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-register-atomic-secret";
    let realm = "linked-register-atomic@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;
    let pool = platform::storage::db::get_database().get_pool();

    let mut request = linked_register_request();
    request.realm = realm.to_string();
    request.ws_address = Some("wss://example.invalid/actor".to_string());
    request.acl = Some(Acl {
        rules: vec![AclRule {
            permission: acl_rule::Permission::Allow as i32,
            from_type: ActrType {
                publisher: "new-client".to_string(),
                name: "caller".to_string(),
                version: "1.0.0".to_string(),
            },
            source_realm: Some(acl_rule::SourceRealm::Realm(realm.to_string())),
        }],
    });
    let target_type = request.actr_type.to_string_repr();

    sqlx::query("DELETE FROM actoracl WHERE realm = ?1 AND to_type = ?2")
        .bind(realm)
        .bind(&target_type)
        .execute(pool)
        .await
        .expect("clear target ACL");
    sqlx::query(
        "INSERT INTO actoracl
            (realm, source_realm, from_type, to_type, access)
         VALUES (?1, ?1, 'old-client/caller@1.0.0', ?2, 0)",
    )
    .bind(realm)
    .bind(&target_type)
    .execute(pool)
    .await
    .expect("seed old ACL");

    let pending_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_registration WHERE realm = ?1")
            .bind(realm)
            .fetch_one(pool)
            .await
            .expect("count pending registrations before");

    sqlx::query("DROP TRIGGER IF EXISTS test_fail_pending_registration")
        .execute(pool)
        .await
        .expect("drop stale test trigger");
    sqlx::query(
        "CREATE TRIGGER test_fail_pending_registration
         BEFORE INSERT ON pending_registration
         BEGIN
             SELECT RAISE(FAIL, 'forced pending registration failure');
         END",
    )
    .execute(pool)
    .await
    .expect("create pending failure trigger");

    let response = post_register(app, request, Some(key_label)).await;
    sqlx::query("DROP TRIGGER IF EXISTS test_fail_pending_registration")
        .execute(pool)
        .await
        .expect("drop pending failure trigger");

    match response.result.expect("register result") {
        register_response::Result::Error(error) => assert_eq!(error.code, 500),
        register_response::Result::Success(_) => panic!("forced pending failure must not succeed"),
    }

    let acl_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT from_type, access FROM actoracl
         WHERE realm = ?1 AND to_type = ?2",
    )
    .bind(realm)
    .bind(&target_type)
    .fetch_all(pool)
    .await
    .expect("read target ACL after rollback");
    assert_eq!(
        acl_rows,
        vec![("old-client/caller@1.0.0".to_string(), 0)],
        "previous type-level ACL must remain intact"
    );

    let pending_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_registration WHERE realm = ?1")
            .bind(realm)
            .fetch_one(pool)
            .await
            .expect("count pending registrations after");
    assert_eq!(pending_after, pending_before);

    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn test_register_sets_pending_retention_and_drops_expired_rows() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-register-retention-secret";
    let realm = "linked-register-retention@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;
    let pool = platform::storage::db::get_database().get_pool();

    sqlx::query(
        "INSERT INTO pending_registration
            (realm, serial_number, ws_address, created_at, expires_at)
         VALUES (?1, 1, 'wss://example.invalid/stale', 1, 2)",
    )
    .bind(realm)
    .execute(pool)
    .await
    .expect("seed expired pending registration");

    let mut request = linked_register_request();
    request.realm = realm.to_string();
    request.ws_address = Some("wss://example.invalid/actor".to_string());
    let ok = match post_register(app, request, Some(key_label))
        .await
        .result
        .expect("register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(error) => panic!("register failed: {error:?}"),
    };

    let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT serial_number, expires_at FROM pending_registration WHERE realm = ?1",
    )
    .bind(realm)
    .fetch_all(pool)
    .await
    .expect("read pending registrations");
    let renewal_expires_at = ok
        .renewal_token_expires_at
        .expect("renewal token expiry")
        .seconds;
    assert_eq!(
        rows,
        vec![(
            ok.actr_id.serial_number as i64,
            Some(renewal_expires_at + issuer::renewal::REISSUE_BINDING_GRACE_SECS)
        )],
        "expired pending rows must be dropped and the new row must carry a retention"
    );

    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn test_renew_and_reissue_extend_pending_retention() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-pending-extend-secret";
    let realm = "linked-pending-extend@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;
    let pool = platform::storage::db::get_database().get_pool();

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    registration.ws_address = Some("wss://example.invalid/actor".to_string());
    let registered = match post_register(app.clone(), registration.clone(), Some(key_label))
        .await
        .result
        .expect("register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(error) => panic!("register failed: {error:?}"),
    };
    let actr_id = registered.actr_id.clone();

    let shorten_retention = || async {
        sqlx::query(
            "UPDATE pending_registration SET expires_at = ?1
             WHERE realm = ?2 AND serial_number = ?3",
        )
        .bind(issuer::renewal::now_secs() + 60)
        .bind(realm)
        .bind(actr_id.serial_number as i64)
        .execute(pool)
        .await
        .expect("shorten pending retention");
    };
    let retention = || async {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT expires_at FROM pending_registration
             WHERE realm = ?1 AND serial_number = ?2",
        )
        .bind(realm)
        .bind(actr_id.serial_number as i64)
        .fetch_one(pool)
        .await
        .expect("read pending retention")
    };

    shorten_retention().await;
    let renewed = match post_renew(
        app.clone(),
        RenewCredentialRequest {
            actr_id: actr_id.clone(),
            renewal_token: registered.renewal_token.expect("renewal token"),
            attempt_id: control_attempt_id(),
        },
    )
    .await
    .result
    .expect("renew result")
    {
        renew_credential_response::Result::Success(ok) => ok,
        renew_credential_response::Result::Error(error) => panic!("renew failed: {error:?}"),
    };
    assert_eq!(
        retention().await,
        Some(
            renewed
                .renewal_token_expires_at
                .expect("renewal token expiry")
                .seconds
                + issuer::renewal::REISSUE_BINDING_GRACE_SECS
        )
    );

    shorten_retention().await;
    let reissued = match post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: actr_id.clone(),
            registration,
            renewal_proof: renewed.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await
    .result
    .expect("reissue result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(error) => panic!("reissue failed: {error:?}"),
    };
    assert_eq!(
        retention().await,
        Some(
            reissued
                .renewal_token_expires_at
                .expect("renewal token expiry")
                .seconds
                + issuer::renewal::REISSUE_BINDING_GRACE_SECS
        )
    );

    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn test_reissue_route_preserves_linked_actor_id() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-reissue-secret";
    let realm = "linked-reissue@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    let initial_ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("initial registration failed: {err:?}"),
    };

    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: initial_ok.actr_id.clone(),
            registration,
            renewal_proof: initial_ok.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("reissue result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id, initial_ok.actr_id);
            assert!(ok.renewal_token.is_some());
            assert!(ok.renewal_token_expires_at.is_some());
        }
        register_response::Result::Error(err) => {
            panic!("credential reissue should preserve the actor id: {err:?}")
        }
    }

    env.shutdown_signer().await;
}

/// P0 regression: a caller that authenticates the workload context (realm
/// secret) but supplies another actor's serial must NOT receive credentials.
/// The registration context is realm/type-wide; only the renewal-token
/// possession proof binds the caller to a specific serial.
#[tokio::test]
#[serial]
async fn test_reissue_route_rejects_foreign_serial() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-foreign-serial-secret";
    let realm = "linked-foreign-serial@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();

    // Two same-realm/type actors, each with its own renewal token.
    let victim = match post_register(app.clone(), registration.clone(), Some(key_label))
        .await
        .result
        .expect("victim register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("victim registration failed: {err:?}"),
    };
    let attacker = match post_register(app.clone(), registration.clone(), Some(key_label))
        .await
        .result
        .expect("attacker register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("attacker registration failed: {err:?}"),
    };
    assert_ne!(victim.actr_id, attacker.actr_id);

    // The attacker presents its own (valid) renewal token as proof but names
    // the victim's serial. Must be rejected: the proof is recorded under the
    // attacker's ActrId, not the victim's.
    let response = post_reissue(
        app.clone(),
        ReissueCredentialRequest {
            actr_id: victim.actr_id.clone(),
            registration: registration.clone(),
            renewal_proof: attacker.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("foreign-serial reissue result") {
        register_response::Result::Error(err) => {
            // A possession failure carries its OWN wire code, distinct from the
            // realm-denial 403, so the client never mistakes it for terminal
            // realm denial.
            assert_eq!(
                err.code, REISSUE_POSSESSION_NOT_PROVEN_CODE,
                "foreign serial should fail possession, not realm denial"
            );
            assert_ne!(
                err.code, 403,
                "possession failure must not reuse realm-denial 403"
            );
            assert!(err.message.contains("possession"));
        }
        register_response::Result::Success(_) => {
            panic!("reissue must not mint credentials for a foreign serial")
        }
    }

    // An arbitrary, never-registered serial must be rejected the same way.
    let mut unknown_id = attacker.actr_id.clone();
    unknown_id.serial_number = attacker.actr_id.serial_number.wrapping_add(987_654_321);
    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: unknown_id,
            registration,
            renewal_proof: attacker.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("unknown-serial reissue result") {
        register_response::Result::Error(err) => {
            assert_eq!(
                err.code, REISSUE_POSSESSION_NOT_PROVEN_CODE,
                "unknown serial should fail possession, not realm denial"
            );
        }
        register_response::Result::Success(_) => {
            panic!("reissue must not mint credentials for an unknown serial")
        }
    }

    env.shutdown_signer().await;
}

/// Reissue keeps the Actor type recorded at issuance; a request naming another
/// type is rejected even with a valid possession proof.
#[tokio::test]
#[serial]
async fn test_reissue_route_rejects_changed_actor_type() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-reissue-type-secret";
    let realm = "linked-reissue-type@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let ok = match post_register(app.clone(), registration.clone(), Some(key_label))
        .await
        .result
        .expect("register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("registration failed: {err:?}"),
    };

    registration.actr_type.name = "other-workload".to_string();
    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: ok.actr_id.clone(),
            registration,
            renewal_proof: ok.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    assert!(
        matches!(response.result, Some(register_response::Result::Error(_))),
        "reissue must not change the Actor type"
    );

    env.shutdown_signer().await;
}

/// A reissue request without any possession proof must be rejected even when
/// the workload context authenticates.
#[tokio::test]
#[serial]
async fn test_reissue_route_rejects_missing_possession_proof() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-missing-proof-secret";
    let realm = "linked-missing-proof@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    let initial_ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("initial registration failed: {err:?}"),
    };

    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: initial_ok.actr_id.clone(),
            registration,
            renewal_proof: None,
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("missing-proof reissue result") {
        register_response::Result::Error(err) => {
            assert_eq!(
                err.code, REISSUE_POSSESSION_NOT_PROVEN_CODE,
                "missing proof is a possession failure, not realm denial"
            );
            assert!(err.message.contains("possession"));
        }
        register_response::Result::Success(_) => {
            panic!("reissue without possession proof must be rejected")
        }
    }

    env.shutdown_signer().await;
}

/// A renewal token that already expired (so `/renew` rejects it) must still
/// prove possession for `/reissue` within the binding grace window — this is
/// the self-healing path for long-offline nodes.
#[tokio::test]
#[serial]
async fn test_reissue_route_accepts_expired_recorded_proof() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-expired-proof-secret";
    let realm = "linked-expired-proof@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    let initial_ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("initial registration failed: {err:?}"),
    };

    // Force the recorded token to be expired (but well inside the grace
    // window), as if the node had been offline past the renewal TTL.
    let pool = platform::storage::db::get_database().get_pool();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query("UPDATE issuer_renewal_token SET expires_at = ?1 WHERE actor_id = ?2")
        .bind(now - 3600)
        .bind(initial_ok.actr_id.to_string_repr())
        .execute(pool)
        .await
        .expect("expire recorded renewal token");

    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: initial_ok.actr_id.clone(),
            registration,
            renewal_proof: initial_ok.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("expired-proof reissue result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id, initial_ok.actr_id);
            assert!(ok.renewal_token.is_some());
        }
        register_response::Result::Error(err) => {
            panic!("expired-but-recorded proof should still reissue within grace: {err:?}")
        }
    }

    env.shutdown_signer().await;
}

/// A reissue whose response is lost must replay its exact response without
/// repeating the durable state change.
///
/// The client cannot tell "the server never got it" from "the server committed
/// and the response was dropped", so it reuses the same attempt id and exact
/// body. The durable attempt journal must return the cached first result.
#[tokio::test]
#[serial]
async fn test_reissue_route_is_idempotent_across_lost_responses() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-replay-secret";
    let realm = "linked-replay@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    let initial_ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("initial registration failed: {err:?}"),
    };

    let request = ReissueCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        registration,
        renewal_proof: initial_ok.renewal_token.clone(),
        attempt_id: control_attempt_id(),
    };

    // Three attempts with the identical proof, as if the first two responses
    // never reached the client.
    let mut outcomes = Vec::new();
    for attempt in 1..=3 {
        let response = post_reissue(app.clone(), request.clone(), Some(key_label)).await;
        match response.result.expect("reissue result") {
            register_response::Result::Success(ok) => outcomes.push(ok),
            register_response::Result::Error(err) => {
                panic!("reissue attempt {attempt} rejected after lost responses: {err:?}")
            }
        }
    }

    let first = &outcomes[0];
    for (index, outcome) in outcomes.iter().enumerate().skip(1) {
        assert_eq!(
            outcome.encode_to_vec(),
            first.encode_to_vec(),
            "attempt {} must replay the exact cached response",
            index + 1
        );
        assert_eq!(
            outcome.actr_id,
            first.actr_id,
            "attempt {} returned a different ActrId",
            index + 1
        );
        assert_eq!(
            outcome.renewal_token,
            first.renewal_token,
            "attempt {} returned a different successor token; replays must be idempotent",
            index + 1
        );
        assert_eq!(
            outcome.renewal_token_expires_at,
            first.renewal_token_expires_at,
            "attempt {} returned an expiry that disagrees with the stored record",
            index + 1
        );
    }

    // The proof itself must still resolve — that is what lets a fourth retry
    // succeed — and the replays must not have accumulated extra rows.
    let pool = platform::storage::db::get_database().get_pool();
    let proof_hash = issuer::renewal::hash_token(
        initial_ok
            .renewal_token
            .as_ref()
            .expect("initial renewal token"),
    );
    let proof_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM issuer_renewal_token WHERE actor_id = ?1 AND token_hash = ?2",
    )
    .bind(initial_ok.actr_id.to_string_repr())
    .bind(&proof_hash)
    .fetch_one(pool)
    .await
    .expect("count proof binding");
    assert_eq!(
        proof_rows, 1,
        "the presented proof must remain resolvable after replays"
    );

    let total_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM issuer_renewal_token WHERE actor_id = ?1")
            .bind(initial_ok.actr_id.to_string_repr())
            .fetch_one(pool)
            .await
            .expect("count actor tokens");
    assert_eq!(
        total_rows, 2,
        "three replays of one attempt must leave exactly the proof and its single successor"
    );

    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn test_register_route_package_with_publisher_identity_succeeds() {
    let env = setup_test_environment().await;
    let realm = "package-http-route@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let (_, key) =
        signed_package_with_generated_key("httppkg", "PackagedService", "1.0.0", "wasm32-wasip1")
            .await;
    let app = create_test_router(&env).await;

    let request = signed_package_request(
        "httppkg",
        "PackagedService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id.realm, realm);
            assert_eq!(registered_type(&ok).publisher, "httppkg");
            assert_eq!(development(&ok), None);
        }
        register_response::Result::Error(err) => {
            panic!("package /register with Publisher Registry identity should succeed: {err:?}")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_register_accepts_canonical_digit_and_long_publisher_domains() {
    let env = setup_test_environment().await;
    let realm = "publisher-domain-validation@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let app = create_test_router(&env).await;
    for publisher in [
        "1acme.example",
        "a-very-long-publisher-domain-name.example.com",
    ] {
        let (_, key) =
            signed_package_with_generated_key(publisher, "echo", "1.0.0", "wasm32-wasip1").await;
        let request =
            signed_package_request(publisher, "echo", "1.0.0", realm, "wasm32-wasip1", &key);
        let response = post_register(app.clone(), request, Some(ROUTE_KEY_LABEL)).await;
        match response.result.unwrap() {
            register_response::Result::Success(ok) => {
                assert_eq!(registered_type(&ok).publisher, publisher)
            }
            other => panic!("canonical Publisher was rejected: {other:?}"),
        }
    }
}

#[tokio::test]
#[serial]
async fn test_register_route_unspecified_auth_mode_uses_package_identity() {
    let env = setup_test_environment().await;
    let realm = "unspecified-http-route@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let (_, key) =
        signed_package_with_generated_key("legacyhttp", "LegacyService", "1.0.0", "wasm32-wasip1")
            .await;
    let app = create_test_router(&env).await;

    // Omitted auth_mode must still take the package path.
    let mut request = signed_package_request(
        "legacyhttp",
        "LegacyService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    request.auth_mode = None;
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id.realm, realm);
            assert_eq!(registered_type(&ok).publisher, "legacyhttp");
        }
        register_response::Result::Error(err) => {
            panic!("omitted auth_mode should remain package-compatible: {err:?}")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_register_route_package_without_manifest_raw_is_rejected() {
    let env = setup_test_environment().await;
    let realm = "package-missing-manifest@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    signed_package_for_publisher(
        "missingmanifest",
        "PublishedService",
        "1.0.0",
        "wasm32-wasip1",
    )
    .await;
    let app = create_test_router(&env).await;

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "missingmanifest".to_string(),
            name: "PublishedService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: realm.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 400);
        }
        register_response::Result::Success(_) => {
            panic!("package registration without manifest_raw should be rejected")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_register_route_package_without_target_is_rejected() {
    let env = setup_test_environment().await;
    let realm = "package-missing-target@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let manifest_raw = signed_package_for_publisher(
        "missingtarget",
        "PublishedService",
        "1.0.0",
        "wasm32-wasip1",
    )
    .await;
    let app = create_test_router(&env).await;

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "missingtarget".to_string(),
            name: "PublishedService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: realm.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest_raw)),
        publisher_signature: None,
        target: None,
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 400);
        }
        register_response::Result::Success(_) => {
            panic!("package registration without target should be rejected")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_expired_publisher_key_still_verifies_packages() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let issuer = create_test_issuer(&env).await;
    let realm = "expired-publisher-runtime-verification@example.com";
    seed_realm(realm, None).await;

    let pool = platform::storage::db::get_database().get_pool();
    let key = SigningKey::generate(&mut OsRng);
    signed_package_for_key(
        "expiredruntime",
        "ExistingService",
        "1.0.0",
        "wasm32-wasip1",
        &key,
    )
    .await;
    sqlx::query("UPDATE publisher SET key_expires_at = ? WHERE name = ?")
        .bind(test_unix_now_secs() as i64 - 1)
        .bind("expiredruntime")
        .execute(pool)
        .await
        .expect("expire package signing key");

    // Natural key expiry does not invalidate existing package signatures.
    for name in ["ExistingService", "OtherService"] {
        let request = signed_package_request(
            "expiredruntime",
            name,
            "1.0.0",
            realm,
            "wasm32-wasip1",
            &key,
        );
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .expect("issue credential");
        match response.result.expect("result") {
            register_response::Result::Success(ok) => assert_eq!(development(&ok), None),
            register_response::Result::Error(err) => {
                panic!("natural key expiry must not invalidate {name}: {err:?}")
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn test_suspended_publisher_rejects_its_packages() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let issuer = create_test_issuer(&env).await;
    let realm = "suspended-publisher-runtime-verification@example.com";
    seed_realm(realm, None).await;

    let pool = platform::storage::db::get_database().get_pool();
    let key = SigningKey::generate(&mut OsRng);
    signed_package_for_key(
        "suspendedruntime",
        "ExistingService",
        "1.0.0",
        "wasm32-wasip1",
        &key,
    )
    .await;
    sqlx::query("UPDATE publisher SET status = 'suspended' WHERE name = ?")
        .bind("suspendedruntime")
        .execute(pool)
        .await
        .expect("suspend Publisher Registry");

    // Different package names and a development key claiming the suspended
    // Publisher name are all rejected.
    let development_key = SigningKey::generate(&mut OsRng);
    let cases = [
        ("ExistingService", &key),
        ("OtherService", &key),
        ("OtherService", &development_key),
    ];
    for (name, signing_key) in cases {
        let request = signed_package_request(
            "suspendedruntime",
            name,
            "1.0.0",
            realm,
            "wasm32-wasip1",
            signing_key,
        );
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .expect("issue credential");
        assert!(
            matches!(
                &response.result,
                Some(register_response::Result::Error(error)) if error.code == 403
            ),
            "suspended publisher must reject {name}: {response:?}"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_package_retired_key_survives_until_explicit_revocation() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let issuer = create_test_issuer(&env).await;
    let realm = "revoked-history-path1@example.com";
    seed_realm(realm, None).await;

    let pool = platform::storage::db::get_database().get_pool();
    let old_key = SigningKey::generate(&mut OsRng);
    signed_package_for_key(
        "revokedhistory",
        "ExistingService",
        "1.0.0",
        "wasm32-wasip1",
        &old_key,
    )
    .await;
    let (publisher_id, old_key_id): (i64, String) =
        sqlx::query_as("SELECT id, key_id FROM publisher WHERE name = ?")
            .bind("revokedhistory")
            .fetch_one(pool)
            .await
            .expect("load Publisher Registry current key");
    let old_public_key =
        base64::prelude::BASE64_STANDARD.encode(old_key.verifying_key().to_bytes());
    archive_key_to_history(pool, publisher_id, &old_key_id, &old_public_key, "retired").await;

    let new_key = SigningKey::generate(&mut OsRng);
    let new_key_id =
        loran_publisher_registry::crypto::compute_key_id(&new_key.verifying_key().to_bytes());
    let new_public_key =
        base64::prelude::BASE64_STANDARD.encode(new_key.verifying_key().to_bytes());
    sqlx::query("UPDATE publisher SET public_key = ?, key_id = ? WHERE id = ?")
        .bind(new_public_key)
        .bind(new_key_id)
        .bind(publisher_id)
        .execute(pool)
        .await
        .expect("rotate current Publisher Registry key");

    // The package was signed by the now-retired old_key; retirement keeps it
    // valid.
    let request = signed_package_request(
        "revokedhistory",
        "ExistingService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &old_key,
    );
    let retired_response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue with retired key");
    assert!(
        matches!(
            &retired_response.result,
            Some(register_response::Result::Success(_))
        ),
        "ordinary key retirement must not invalidate an existing package: {retired_response:?}"
    );

    sqlx::query(
        "UPDATE publisher_key_history SET status = 'revoked' WHERE publisher_id = ? AND key_id = ?",
    )
    .bind(publisher_id)
    .bind(&old_key_id)
    .execute(pool)
    .await
    .expect("revoke historical key");

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue with revoked key");
    assert!(
        matches!(
            &response.result,
            Some(register_response::Result::Error(error)) if error.code == 403
        ),
        "key revocation must invalidate packages signed by that key: {response:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_register_route_unsigned_package_is_rejected() {
    let env = setup_test_environment().await;
    let realm = "package-http-route-missing-publisher@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let app = create_test_router(&env).await;

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "missing-http-publisher".to_string(),
            name: "PackagedService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: realm.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(test_package_manifest(
            "missing-http-publisher",
            "PackagedService",
            "1.0.0",
            "wasm32-wasip1",
        ))),
        publisher_signature: None,
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403);
            assert!(err.message.contains("publisher not verified"));
        }
        register_response::Result::Success(_) => {
            panic!("package /register without a manifest signature must be rejected")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_linked_registration_of_a_member_succeeds() {
    let env = setup_test_environment().await;

    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    seed_realm(TEST_REALM, None).await;
    let viewer = add_client(TEST_REALM, "linked-viewer", Role::Viewer).await;
    let response = issuer
        .issue_credential(&linked_register_request(), &viewer)
        .await
        .expect("issue linked credential");

    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(registered_type(&ok).name, "source-workload");
            assert_eq!(ok.actr_id.realm, TEST_REALM);
            // The credential names the member, its client key and role.
            let claims =
                actr_protocol::IdentityClaims::decode(ok.credential.claims.as_ref()).unwrap();
            assert_eq!(claims.member, viewer.member_id);
            assert_eq!(claims.client_key, viewer.key_id);
            assert_eq!(claims.role(), actr_protocol::MemberRole::Viewer);
        }
        register_response::Result::Error(err) => {
            panic!("linked registration of a member should succeed: {err:?}")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_unsigned_package_registration_is_rejected() {
    let env = setup_test_environment().await;

    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "missing-publisher-for-package-auth-test".to_string(),
            name: "package-workload".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(test_package_manifest(
            "missing-publisher-for-package-auth-test",
            "package-workload",
            "1.0.0",
            "wasm32-wasip1",
        ))),
        publisher_signature: None,
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue package credential");

    match response.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403);
            assert!(err.message.contains("publisher not verified"));
        }
        register_response::Result::Success(_) => {
            panic!("package registration without a manifest signature must be rejected")
        }
    }
}

#[tokio::test]
#[serial]
async fn test_end_to_end_credential_flow() {
    let env = setup_test_environment().await;

    AIdCredentialValidator::init(env.issuer_temp_dir.path())
        .await
        .expect("Failed to initialize validator");

    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("Failed to create Signer gRPC client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("Failed to create issuer");

    let (_, key) =
        signed_package_with_generated_key("acme", "test-device", "1.0.0", "wasm32-wasip1").await;
    let request = signed_package_request(
        "acme",
        "test-device",
        "1.0.0",
        TEST_REALM,
        "wasm32-wasip1",
        &key,
    );

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("Failed to issue credential");

    let register_ok = match response.result.expect("Response should contain result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("Expected success but got error: {err:?}"),
    };

    assert!(
        !register_ok.turn_credential.username.is_empty(),
        "TURN credential should be present"
    );
    assert!(
        register_ok.credential_expires_at.is_some(),
        "Credential expiry should be present"
    );
    assert_eq!(register_ok.actr_id.realm, TEST_REALM);
    assert!(register_ok.actr_id.serial_number > 0);

    let (claims, _) = AIdCredentialValidator::check(
        &register_ok.credential,
        TEST_REALM,
        "provider.example",
        "actr.signaling",
    )
    .await
    .expect("Token validation should succeed");
    assert_eq!(claims.realm, TEST_REALM);
    assert!(
        !claims.actor_id.is_empty(),
        "Actor ID should be present in claims"
    );
    assert_eq!(
        claims.actor_id,
        register_ok.actr_id.to_string_repr(),
        "Actor ID must use <serial_hex>.<realm_id>@<domain>"
    );
    assert_eq!(claims.issuer, TEST_REALM);
    assert_eq!(claims.actor_type, request.actr_type);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();
    assert!(claims.expires_at > now);
    assert!(claims.expires_at <= now + 3600);

    let wrong_realm_result = AIdCredentialValidator::check(
        &register_ok.credential,
        "other@example.com",
        "provider.example",
        "actr.signaling",
    )
    .await;
    assert!(
        wrong_realm_result.is_err(),
        "Validation should fail with a mismatched Realm"
    );

    // Issue and validate multiple credentials to verify stability.
    for idx in 0..5 {
        let req = request.clone();

        let rsp = issuer
            .issue_credential(&req, &owner(&req.realm).await)
            .await
            .unwrap_or_else(|e| panic!("Failed to issue credential {idx}: {e}"));
        let ok = match rsp.result.expect("Response should contain result") {
            register_response::Result::Success(ok) => ok,
            register_response::Result::Error(err) => {
                panic!("Expected success for token {idx}, got error: {err:?}")
            }
        };

        let (claims, _) = AIdCredentialValidator::check(
            &ok.credential,
            TEST_REALM,
            "provider.example",
            "actr.signaling",
        )
        .await
        .unwrap_or_else(|e| panic!("Failed to validate credential {idx}: {e}"));
        assert_eq!(claims.realm, TEST_REALM);
    }
}

#[tokio::test]
#[serial]
async fn test_issuer_health_checks() {
    let env = setup_test_environment().await;

    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("Failed to create Signer gRPC client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("Failed to create issuer");

    issuer
        .check_database_health()
        .await
        .expect("Database health check should pass");
    issuer
        .check_signer_health()
        .await
        .expect("Signer health check should pass");
}

#[tokio::test]
#[serial]
async fn test_issuer_creation_fails_with_wrong_shared_key() {
    let env = setup_test_environment().await;

    let signer_client = create_signer_client(&env.signer_config, "wrong-shared-key")
        .await
        .expect("gRPC channel creation should succeed even with wrong secret");

    // With lazy Signer connection, issuer creation succeeds — auth failure happens on first use
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer creation succeeds with lazy Signer connection");

    // Credential issuance should return an error response due to wrong shared key
    let request = actr_protocol::RegisterRequest {
        actr_type: actr_protocol::ActrType {
            publisher: "acme".to_string(),
            name: "bad-key-test".to_string(),
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
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };
    let resp = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue_credential returns Ok wrapping the error");
    assert!(
        matches!(
            resp.result,
            Some(actr_protocol::register_response::Result::Error(_))
        ),
        "expected error result in register response with wrong shared key, got {:?}",
        resp.result
    );
}

#[tokio::test]
#[serial]
async fn test_issuer_check_signer_health_fails_after_signer_shutdown() {
    let mut env = setup_test_environment().await;

    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("Failed to create Signer gRPC client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("Failed to create issuer");

    env.shutdown_signer().await;

    for _ in 0..40 {
        match issuer.check_signer_health().await {
            Ok(()) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("Signer service unhealthy")
                        || msg.contains("Failed")
                        || msg.contains("transport")
                        || msg.contains("unavailable")
                        || msg.contains("connection"),
                    "unexpected Signer health error after shutdown: {msg}"
                );
                return;
            }
        }
    }

    panic!("issuer Signer health should fail after embedded Signer shutdown");
}

// ═══════════════════════════════════════════════════════════════════════════
// Package provenance: manifest_raw + publisher_signature + signing key
// ═══════════════════════════════════════════════════════════════════════════

/// Helper: build a signed manifest TOML string and its Ed25519 signature.
fn build_signed_manifest(
    signing_key: &ed25519_dalek::SigningKey,
    publisher: &str,
    name: &str,
    version: &str,
    target: &str,
) -> (Vec<u8>, Vec<u8>) {
    use ed25519_dalek::Signer;

    let key_id =
        loran_publisher_registry::crypto::compute_key_id(&signing_key.verifying_key().to_bytes());
    let manifest = format!(
        r#"publisher = "{publisher}"
name = "{name}"
version = "{version}"
signing_key_id = "{key_id}"

[binary]
path = "bin/actor.wasm"
target = "{target}"
hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#,
    );
    let manifest_bytes = manifest.into_bytes();
    let signature = signing_key.sign(&manifest_bytes);
    (manifest_bytes, signature.to_bytes().to_vec())
}

fn test_unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Helper: seed an Publisher Registry with a specific keypair and return (publisher_id, key_id, public_key_b64).
async fn seed_publisher_with_key(
    pool: &sqlx::SqlitePool,
    publisher_name: &str,
    signing_key: &ed25519_dalek::SigningKey,
) -> (i64, String) {
    let pub_b64 = base64::prelude::BASE64_STANDARD.encode(signing_key.verifying_key().to_bytes());
    let key_id =
        loran_publisher_registry::crypto::compute_key_id(&signing_key.verifying_key().to_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let expires_at = now + 86400 * 365;

    sqlx::query(
        "INSERT OR REPLACE INTO publisher (name, public_key, key_id, contact, status, created_at, verified_at, key_expires_at) \
         VALUES (?, ?, ?, 'test@example.com', 'active', ?, ?, ?)"
    )
    .bind(publisher_name)
    .bind(&pub_b64)
    .bind(&key_id)
    .bind(now)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await
    .expect("seed publisher");

    let publisher_id: i64 = sqlx::query_scalar("SELECT id FROM publisher WHERE name = ?")
        .bind(publisher_name)
        .fetch_one(pool)
        .await
        .expect("get publisher id");

    (publisher_id, key_id)
}

/// Helper: archive a key into publisher_key_history with given status.
async fn archive_key_to_history(
    pool: &sqlx::SqlitePool,
    publisher_id: i64,
    key_id: &str,
    public_key_b64: &str,
    status: &str,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    sqlx::query(
        "INSERT INTO publisher_key_history (publisher_id, key_id, public_key, status, created_at, retired_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(publisher_id)
    .bind(key_id)
    .bind(public_key_b64)
    .bind(status)
    .bind(now - 1000)
    .bind(now)
    .execute(pool)
    .await
    .expect("archive key to history");
}

/// Publisher A signs a package; the owner of a Realm on another Host holds
/// only the public package and its own Realm client key, and registers the
/// Realm's Service, then renews and reissues it. No Publisher private key takes
/// part after preparation, and the package keeps its Publisher identity although
/// its signature and Publisher identity are sufficient.
#[tokio::test]
#[serial]
async fn test_trusted_package_registers_without_publisher_private_key() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "third-party-service@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let pool = platform::storage::db::get_database().get_pool();
    // Machine A: the Publisher's key, known to this Loran's registry.
    let publisher_key = SigningKey::generate(&mut OsRng);
    seed_publisher_with_key(pool, "publisher-a.example", &publisher_key).await;
    sqlx::query(
        "INSERT INTO realm_profile (realm, sequence, digest, document, accepted_at)
         VALUES (?, 1, 'd', ?, 1)",
    )
    .bind(realm)
    .bind(
        serde_json::json!({ "package": { "type": "publisher-a.example/notes@1.0.0" } }).to_string(),
    )
    .execute(pool)
    .await
    .unwrap();
    // Machine B: the public package bytes only.
    let request = signed_package_request(
        "publisher-a.example",
        "notes",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &publisher_key,
    );
    drop(publisher_key);

    let app = create_test_router(&env).await;
    let response = post_register(app.clone(), request.clone(), Some(ROUTE_KEY_LABEL)).await;
    let registered = match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(registered_type(&ok).publisher, "publisher-a.example");
            assert_eq!(development(&ok), None, "keeps the Publisher identity");
            ok
        }
        register_response::Result::Error(err) => {
            panic!("the Realm owner must register the Service without a Publisher key: {err:?}")
        }
    };
    let renewed = renewed(
        post_renew(
            app.clone(),
            RenewCredentialRequest {
                actr_id: registered.actr_id.clone(),
                renewal_token: registered.renewal_token.unwrap(),
                attempt_id: control_attempt_id(),
            },
        )
        .await,
    );
    assert_eq!(renewed.actr_id, registered.actr_id);
    assert_eq!(development(&renewed), None);
    let reissued = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: registered.actr_id.clone(),
            registration: request,
            renewal_proof: renewed.renewal_token,
            attempt_id: control_attempt_id(),
        },
        Some(ROUTE_KEY_LABEL),
    )
    .await;
    match reissued.result.expect("reissue result") {
        register_response::Result::Success(ok) => {
            assert_eq!(ok.actr_id, registered.actr_id);
            assert_eq!(registered_type(&ok).publisher, "publisher-a.example");
            assert_eq!(development(&ok), None);
        }
        register_response::Result::Error(err) => {
            panic!("reissue must not require a Publisher private key: {err:?}")
        }
    }
    let publication_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
         AND name IN ('publisher_package', 'publisher_publish_nonce')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(publication_tables, 0, "runtime needs no publication ledger");
}

/// A valid package whose publisher this Loran's registry does not know runs as
/// a development identity: signature and key id are still checked, and the
/// credential carries the development indicator.
#[tokio::test]
#[serial]
async fn test_unregistered_publisher_package_runs_as_development_identity() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let issuer = create_test_issuer(&env).await;
    let realm = "development-package@example.com";
    seed_realm(realm, None).await;

    let key = SigningKey::generate(&mut OsRng);
    let request = signed_package_request(
        "dev.local.example",
        "OtherService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let response = issuer
        .issue_credential(&request, &owner(realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(development(&ok), Some(true));
            assert_eq!(registered_type(&ok).publisher, "dev.local.example");
        }
        register_response::Result::Error(err) => {
            panic!("a valid development package must register: {err:?}")
        }
    }

    // Without its signing key, or with a key that is not the one the
    // manifest names, nothing verifies the signature.
    let other = SigningKey::generate(&mut OsRng);
    let mut missing_key = request.clone();
    missing_key.signing_public_key = None;
    let mut wrong_key = request.clone();
    wrong_key.signing_public_key = Some(prost::bytes::Bytes::from(
        other.verifying_key().to_bytes().to_vec(),
    ));
    let mut bad_signature = request.clone();
    let mut signature = bad_signature
        .publisher_signature
        .as_deref()
        .unwrap()
        .to_vec();
    signature[0] ^= 0xFF;
    bad_signature.publisher_signature = Some(prost::bytes::Bytes::from(signature));
    for (label, request) in [
        ("missing_key", missing_key),
        ("wrong_key", wrong_key),
        ("bad_signature", bad_signature),
    ] {
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .expect("issue");
        assert_eq!(error_code(&response), Some(403), "{label}: {response:?}");
    }
}

/// A manifest that does not declare `binary.target` is rejected.
/// The manifest signature must bind target; Issuer must not fall back to the
/// request's target, otherwise one signed manifest could register under
/// arbitrary targets.
#[tokio::test]
#[serial]
async fn test_manifest_without_binary_target_is_rejected() {
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let pool = platform::storage::db::get_database().get_pool();
    seed_realm(TEST_REALM, None).await;

    let key = SigningKey::generate(&mut OsRng);
    let (_publisher_id, _key_id) = seed_publisher_with_key(pool, "notargetpublisher", &key).await;

    // Validly signed manifest that omits [binary].target.
    let key_id = loran_publisher_registry::crypto::compute_key_id(&key.verifying_key().to_bytes());
    let manifest = format!(
        r#"publisher = "notargetpublisher"
name = "NoTargetService"
version = "1.0.0"
signing_key_id = "{key_id}"

[binary]
path = "bin/actor.wasm"
hash = "0000000000000000000000000000000000000000000000000000000000000000"
"#,
    );
    let manifest_bytes = manifest.into_bytes();
    let sig_bytes = key.sign(&manifest_bytes).to_bytes().to_vec();

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "notargetpublisher".to_string(),
            name: "NoTargetService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest_bytes.clone())),
        publisher_signature: Some(prost::bytes::Bytes::from(sig_bytes)),
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(
                err.code, 400,
                "manifest without binary.target should be InvalidFormat (400)"
            );
        }
        register_response::Result::Success(_) => {
            panic!("manifest without binary.target should be rejected")
        }
    }
}

/// A replayed `/register` (same body, same Realm client proof) is rejected:
/// the client proof's nonce is single use.
#[tokio::test]
#[serial]
async fn test_register_replay_is_rejected() {
    let env = setup_test_environment().await;
    let key_label = "register-replay-secret";
    let realm = "register-replay@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut request = linked_register_request();
    request.realm = realm.to_string();
    let body = request.encode_to_vec();
    let header = proof(ProofOp::Register, realm, &body, &key_for(key_label));
    let send = |app: Router| {
        let body = body.clone();
        let header = header.clone();
        async move {
            let mut http_request = Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/protobuf")
                .header(CLIENT_PROOF_HEADER, header)
                .body(Body::from(body))
                .unwrap();
            http_request
                .extensions_mut()
                .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    12345,
                ))));
            let response = app.oneshot(http_request).await.unwrap();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            RegisterResponse::decode(body).unwrap()
        }
    };

    let first = send(app.clone()).await;
    assert_eq!(error_code(&first), None, "first registration: {first:?}");
    let replay = send(app).await;
    match replay.result.expect("result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 400);
            assert!(err.message.contains("nonce"), "{}", err.message);
        }
        register_response::Result::Success(_) => panic!("a replayed /register must be rejected"),
    }
}

/// A historical (retired) Publisher key still verifies its packages.
#[tokio::test]
#[serial]
async fn test_retired_publisher_key_still_verifies() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let pool = platform::storage::db::get_database().get_pool();

    // Key A (old) and Key B (current)
    let key_a = SigningKey::generate(&mut OsRng);
    let key_b = SigningKey::generate(&mut OsRng);

    let key_a_pub_b64 = base64::prelude::BASE64_STANDARD.encode(key_a.verifying_key().to_bytes());
    let key_a_id =
        loran_publisher_registry::crypto::compute_key_id(&key_a.verifying_key().to_bytes());

    // Seed Publisher Registry with Key B as current
    let (publisher_id, _key_b_id) = seed_publisher_with_key(pool, "histpublisher", &key_b).await;

    // Archive Key A as retired
    archive_key_to_history(pool, publisher_id, &key_a_id, &key_a_pub_b64, "retired").await;

    // Seed realm
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query("INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('test@example.com', 'active', 1, ?)")
        .bind(now)
        .execute(pool)
        .await
        .expect("seed realm");

    // Build manifest signed by Key A (the retired key)
    let (manifest_bytes, sig_bytes) = build_signed_manifest(
        &key_a,
        "histpublisher",
        "HistService",
        "1.0.0",
        "wasm32-wasip1",
    );

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "histpublisher".to_string(),
            name: "HistService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest_bytes.clone())),
        publisher_signature: Some(prost::bytes::Bytes::from(sig_bytes)),
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Success(_) => {} // expected
        register_response::Result::Error(err) => {
            panic!("a retired historical key should still verify, got error: {err:?}")
        }
    }
}

/// A revoked Publisher key is rejected, and does not fall back to a
/// development identity.
#[tokio::test]
#[serial]
async fn test_revoked_publisher_key_is_rejected() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let pool = platform::storage::db::get_database().get_pool();

    // Seed realm
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query("INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('test@example.com', 'active', 1, ?)")
        .bind(now)
        .execute(pool)
        .await
        .expect("seed realm");

    let key_revoked = SigningKey::generate(&mut OsRng);
    let key_current = SigningKey::generate(&mut OsRng);

    let revoked_pub_b64 =
        base64::prelude::BASE64_STANDARD.encode(key_revoked.verifying_key().to_bytes());
    let revoked_key_id =
        loran_publisher_registry::crypto::compute_key_id(&key_revoked.verifying_key().to_bytes());

    // Seed Publisher Registry with current key
    let (publisher_id, _) = seed_publisher_with_key(pool, "revpublisher", &key_current).await;

    // Archive revoked key
    archive_key_to_history(
        pool,
        publisher_id,
        &revoked_key_id,
        &revoked_pub_b64,
        "revoked",
    )
    .await;

    // Build manifest signed by the revoked key
    let (manifest_bytes, sig_bytes) = build_signed_manifest(
        &key_revoked,
        "revpublisher",
        "RevService",
        "1.0.0",
        "wasm32-wasip1",
    );

    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "revpublisher".to_string(),
            name: "RevService".to_string(),
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest_bytes.clone())),
        publisher_signature: Some(prost::bytes::Bytes::from(sig_bytes)),
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: Some(prost::bytes::Bytes::from(
            key_revoked.verifying_key().to_bytes().to_vec(),
        )),
    };

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Error(_) => {} // expected: revoked key rejected
        register_response::Result::Success(_) => {
            panic!("a revoked key should be rejected, but got success")
        }
    }
}

/// Manifest identity mismatch (actr_type spoofing) is rejected.
#[tokio::test]
#[serial]
async fn test_manifest_identity_mismatch_rejected() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let pool = platform::storage::db::get_database().get_pool();

    // Seed realm (shared global DB may not have it if run in isolation)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query("INSERT OR IGNORE INTO realm (name, status, enabled, created_at) VALUES ('test@example.com', 'active', 1, ?)")
        .bind(now)
        .execute(pool)
        .await
        .expect("seed realm");

    let key = SigningKey::generate(&mut OsRng);
    let (_publisher_id, _key_id) = seed_publisher_with_key(pool, "spoofpublisher", &key).await;

    // Build manifest for ServiceA
    let (manifest_bytes, sig_bytes) =
        build_signed_manifest(&key, "spoofpublisher", "ServiceA", "1.0.0", "wasm32-wasip1");

    // But register as ServiceB — identity mismatch!
    let request = RegisterRequest {
        actr_type: ActrType {
            publisher: "spoofpublisher".to_string(),
            name: "ServiceB".to_string(), // ← does not match manifest
            version: "1.0.0".to_string(),
        },
        realm: TEST_REALM.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(manifest_bytes.clone())),
        publisher_signature: Some(prost::bytes::Bytes::from(sig_bytes)),
        target: Some("wasm32-wasip1".to_string()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };

    let response = issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Error(_) => {} // expected: identity mismatch rejected
        register_response::Result::Success(_) => {
            panic!("an identity mismatch should be rejected, but got success")
        }
    }
}

// ============================================================================
// Package provenance tests: signature matrix, trusted vs development
// identity, revocation, Linked rejection and renewal.
// ============================================================================

/// Shared package fixture: realm seeded, publisher with an active key in the
/// Publisher Registry, and a validly signed manifest.
///
/// `env` is held for the fixture's lifetime: dropping it would drop the
/// embedded signer's shutdown sender, which resolves `serve_with_shutdown`
/// and tears the signer down before the test can call it.
struct PackageFixture {
    _env: TestEnv,
    issuer: AIdIssuer,
    key: ed25519_dalek::SigningKey,
    wrong_key: ed25519_dalek::SigningKey,
    manifest_bytes: Vec<u8>,
    sig_bytes: Vec<u8>,
    publisher: String,
    name: String,
    target: String,
}

async fn package_fixture(publisher: &str, name: &str, target: &str) -> PackageFixture {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("signer client");
    let issuer = AIdIssuer::new(
        signer_client,
        default_issuer_config(&env.issuer_temp_dir),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("issuer");

    let pool = platform::storage::db::get_database().get_pool();
    seed_realm(TEST_REALM, None).await;

    let key = SigningKey::generate(&mut OsRng);
    let wrong_key = SigningKey::generate(&mut OsRng);
    let (_publisher_id, _key_id) = seed_publisher_with_key(pool, publisher, &key).await;
    let (manifest_bytes, sig_bytes) = build_signed_manifest(&key, publisher, name, "1.0.0", target);

    PackageFixture {
        _env: env,
        issuer,
        key,
        wrong_key,
        manifest_bytes,
        sig_bytes,
        publisher: publisher.to_string(),
        name: name.to_string(),
        target: target.to_string(),
    }
}

/// A `RegisterRequest` for the fixture's signed manifest.
fn base_package_request(fx: &PackageFixture, realm: &str) -> RegisterRequest {
    RegisterRequest {
        actr_type: ActrType {
            publisher: fx.publisher.clone(),
            name: fx.name.clone(),
            version: "1.0.0".to_string(),
        },
        realm: realm.to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(prost::bytes::Bytes::from(fx.manifest_bytes.clone())),
        publisher_signature: Some(prost::bytes::Bytes::from(fx.sig_bytes.clone())),
        target: Some(fx.target.clone()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: Some(prost::bytes::Bytes::from(
            fx.key.verifying_key().to_bytes().to_vec(),
        )),
    }
}

async fn post_renew(app: Router, request: RenewCredentialRequest) -> RenewCredentialResponse {
    let body = request.encode_to_vec();
    let key = bound_key(&request.actr_id).await;
    let builder = Request::builder()
        .method("POST")
        .uri("/renew")
        .header("content-type", "application/protobuf")
        .header("x-real-ip", "127.0.0.1")
        .header(
            CLIENT_PROOF_HEADER,
            proof(ProofOp::Renew, &request.actr_id.realm, &body, &key),
        );
    let mut http_request = builder.body(Body::from(body)).unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app
        .oneshot(http_request)
        .await
        .expect("renew route response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("renew response body");
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected /renew status {status}: {}",
        String::from_utf8_lossy(&body)
    );
    RenewCredentialResponse::decode(body).expect("decode renew response")
}

#[tokio::test]
#[serial]
async fn test_renew_rotation_deletes_expired_token_rows() {
    let mut env = setup_test_environment().await;
    let key_label = "renew-expired-cleanup-secret";
    let realm = "renew-expired-cleanup@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration, Some(key_label)).await;
    let initial_ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(error) => panic!("register failed: {error:?}"),
    };
    let actor_id = initial_ok.actr_id;
    let renewal_token = initial_ok.renewal_token.expect("initial renewal token");
    let actor_id_repr = actor_id.to_string_repr();
    let now = test_unix_now_secs() as i64;
    let pool = platform::storage::db::get_database().get_pool();

    sqlx::query("UPDATE issuer_renewal_token SET expires_at = ?1 WHERE actor_id = ?2")
        .bind(now + 60)
        .bind(&actor_id_repr)
        .execute(pool)
        .await
        .expect("move presented token into rotation window");
    sqlx::query(
        "INSERT INTO issuer_renewal_token (actor_id, token_hash, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&actor_id_repr)
    .bind(vec![0xEE; 32])
    .bind(now - 1)
    .bind(now - 10)
    .execute(pool)
    .await
    .expect("seed expired token row");

    let response = post_renew(
        app,
        RenewCredentialRequest {
            actr_id: actor_id.clone(),
            renewal_token,
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert!(matches!(
        response.result,
        Some(renew_credential_response::Result::Success(_))
    ));

    let expired_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM issuer_renewal_token
         WHERE actor_id = ?1 AND expires_at <= ?2",
    )
    .bind(&actor_id_repr)
    .bind(now)
    .fetch_one(pool)
    .await
    .expect("count expired rows after rotation");
    assert_eq!(expired_rows, 0);

    env.shutdown_signer().await;
}

type TamperMutation<'a> = &'a dyn Fn(&mut RegisterRequest, &PackageFixture);

#[tokio::test]
#[serial]
async fn test_renew_route_replays_exact_response_and_single_state_change() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let (_, initial_ok) =
        register_linked_actor(&app, "renew-idempotency@example.com", "renew-replay-secret").await;
    let actor_id = initial_ok.actr_id.clone();
    let actor_id_repr = actor_id.to_string_repr();
    let pool = platform::storage::db::get_database().get_pool();

    // Put the token in its rotation window so the renewal changes state.
    let now = test_unix_now_secs() as i64;
    sqlx::query("UPDATE issuer_renewal_token SET expires_at = ?1 WHERE actor_id = ?2")
        .bind(now + 60)
        .bind(&actor_id_repr)
        .execute(pool)
        .await
        .expect("move token into rotation window");
    let request = RenewCredentialRequest {
        actr_id: actor_id.clone(),
        renewal_token: initial_ok
            .renewal_token
            .clone()
            .expect("initial renewal token"),
        attempt_id: control_attempt_id(),
    };

    let first = post_renew(app.clone(), request.clone()).await;
    let replay = post_renew(app, request).await;
    assert_eq!(
        first.encode_to_vec(),
        replay.encode_to_vec(),
        "renew replay must return the stored response"
    );
    match first.result.expect("renew result") {
        renew_credential_response::Result::Success(ok) => {
            assert_ne!(
                ok.renewal_token, initial_ok.renewal_token,
                "token must rotate"
            );
        }
        renew_credential_response::Result::Error(err) => panic!("renew failed: {err:?}"),
    }

    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM issuer_control_attempt
         WHERE operation = 'renew' AND actor_id = ?1 AND state = 1",
    )
    .bind(&actor_id_repr)
    .fetch_one(pool)
    .await
    .expect("count completed renew attempts");
    assert_eq!(attempts, 1);
    let tokens: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM issuer_renewal_token WHERE actor_id = ?1")
            .bind(&actor_id_repr)
            .fetch_one(pool)
            .await
            .expect("count tokens");
    assert_eq!(
        tokens, 2,
        "one rotation: the presented token and its successor"
    );

    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn test_renew_attempt_id_body_mismatch_returns_conflict() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let (_, initial_ok) =
        register_linked_actor(&app, "renew-conflict@example.com", "renew-conflict-secret").await;
    let first = RenewCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        renewal_token: initial_ok.renewal_token.expect("initial renewal token"),
        attempt_id: control_attempt_id(),
    };
    let _ = post_renew(app.clone(), first.clone()).await;

    let mut conflicting = first;
    conflicting.renewal_token = prost::bytes::Bytes::from(vec![9; 32]);
    let (status, response) = post_renew_raw(app, conflicting).await;
    assert_eq!(status, StatusCode::CONFLICT);
    match response.result.expect("conflict result") {
        renew_credential_response::Result::Error(error) => assert_eq!(error.code, 409),
        renew_credential_response::Result::Success(_) => panic!("conflict must not succeed"),
    }

    env.shutdown_signer().await;
}

/// Package provenance matrix for a publisher in the registry: the control row
/// verifies with the Publisher's key; every tampered row is rejected (403).
/// A different key never falls back to a development identity, because the
/// registry holds the publisher name.
#[tokio::test]
#[serial]
async fn test_package_signature_matrix() {
    let fx = package_fixture("tamperpublisher", "TamperService", "wasm32-wasip1").await;

    let cases: &[(&str, bool, TamperMutation)] = &[
        ("control", true, &|_r, _f| {}),
        ("other_key_for_trusted_name", false, &|r, f| {
            // A validly self-signed manifest under the trusted publisher name.
            let (manifest, signature) =
                build_signed_manifest(&f.wrong_key, &f.publisher, &f.name, "1.0.0", &f.target);
            r.manifest_raw = Some(prost::bytes::Bytes::from(manifest));
            r.publisher_signature = Some(prost::bytes::Bytes::from(signature));
            r.signing_public_key = Some(prost::bytes::Bytes::from(
                f.wrong_key.verifying_key().to_bytes().to_vec(),
            ));
        }),
        ("signature_by_other_key", false, &|r, f| {
            use ed25519_dalek::Signer;
            let signature = f.wrong_key.sign(&f.manifest_bytes).to_bytes().to_vec();
            r.publisher_signature = Some(prost::bytes::Bytes::from(signature));
            r.signing_public_key = Some(prost::bytes::Bytes::from(
                f.wrong_key.verifying_key().to_bytes().to_vec(),
            ));
        }),
        ("tamper_target", false, &|r, _f| {
            r.target = Some("x86_64-unknown-linux-gnu".to_string());
        }),
        ("tamper_manifest_raw", false, &|r, _f| {
            // flip a byte inside the manifest hash field: still valid TOML, but
            // the signature over the original bytes no longer verifies.
            let mut bytes = r.manifest_raw.as_deref().unwrap().to_vec();
            if let Some(pos) = bytes.iter().rposition(|&b| b == b'0') {
                bytes[pos] = b'1';
            }
            r.manifest_raw = Some(prost::bytes::Bytes::from(bytes));
        }),
        ("tamper_signature", false, &|r, _f| {
            let mut signature = r.publisher_signature.as_deref().unwrap().to_vec();
            signature[0] ^= 0xFF;
            r.publisher_signature = Some(prost::bytes::Bytes::from(signature));
        }),
        ("missing_signature", false, &|r, _f| {
            r.publisher_signature = None;
        }),
    ];

    for (label, expect_success, mutation) in cases {
        let mut request = base_package_request(&fx, TEST_REALM);
        mutation(&mut request, &fx);

        let response = fx
            .issuer
            .issue_credential(&request, &owner(TEST_REALM).await)
            .await
            .expect("issue");
        match (response.result.expect("result"), expect_success) {
            (register_response::Result::Success(ok), true) => {
                assert_eq!(development(&ok), None, "case {label}: Publisher identity");
            }
            (register_response::Result::Error(err), false) => assert_eq!(
                err.code, 403,
                "case {label}: expected PublisherNotVerified (403), got {}",
                err.code
            ),
            (register_response::Result::Success(_), false) => {
                panic!("case {label}: tamper should be rejected, got success")
            }
            (register_response::Result::Error(err), true) => {
                panic!("case {label}: control should succeed, got error {err:?}")
            }
        }
    }
}

/// An official package imported from a local file (never published to this
/// Loran) keeps its trusted Publisher identity: provenance comes from the
/// signing key, not from where the bytes came from.
#[tokio::test]
#[serial]
async fn test_trusted_package_from_a_local_file_keeps_publisher_identity() {
    let fx = package_fixture("official.example", "LocalImport", "wasm32-wasip1").await;
    let request = base_package_request(&fx, TEST_REALM);

    let response = fx
        .issuer
        .issue_credential(&request, &owner(TEST_REALM).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(registered_type(&ok).publisher, "official.example");
            assert_eq!(development(&ok), None);
        }
        register_response::Result::Error(err) => {
            panic!("a trusted package from a local file must register: {err:?}")
        }
    }
}

/// A package registers with its manifest and
/// signature alone; no Publisher private-key proof is asked for.
#[tokio::test]
#[serial]
async fn test_package_registers_without_publisher_proof() {
    let env = setup_test_environment().await;
    let realm = "package-no-auth-route@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let (_, key) =
        signed_package_with_generated_key("noauthpkg", "NoAuthService", "1.0.0", "wasm32-wasip1")
            .await;
    let app = create_test_router(&env).await;

    let request = signed_package_request(
        "noauthpkg",
        "NoAuthService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let response = post_register(app, request, Some(ROUTE_KEY_LABEL)).await;

    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(registered_type(&ok).publisher, "noauthpkg");
            assert_eq!(development(&ok), None);
        }
        register_response::Result::Error(err) => {
            panic!("a signed package must register without a Publisher proof: {err:?}")
        }
    }
}

/// A development key cannot claim a publisher name the registry holds, in
/// any spelling: impersonating a trusted Publisher is rejected.
#[tokio::test]
#[serial]
async fn test_impersonating_a_trusted_publisher_is_rejected() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let fx = package_fixture("trusted.example", "Notes", "wasm32-wasip1").await;
    let impostor = SigningKey::generate(&mut OsRng);

    for publisher in ["trusted.example", "Trusted.Example"] {
        let request = signed_package_request(
            publisher,
            "Notes",
            "1.0.0",
            TEST_REALM,
            "wasm32-wasip1",
            &impostor,
        );
        let response = fx
            .issuer
            .issue_credential(&request, &owner(TEST_REALM).await)
            .await
            .expect("issue");
        assert_eq!(
            error_code(&response),
            Some(403),
            "{publisher}: impersonation must be rejected: {response:?}"
        );
    }
}

/// Linked auth carries no package: package fields are rejected
/// (InvalidFormat).
#[tokio::test]
#[serial]
async fn test_linked_auth_with_package_fields_rejected() {
    let env = setup_test_environment().await;
    let key_label = "linked-triple-secret";
    let realm = "linked-triple@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut request = linked_register_request();
    request.realm = realm.to_string();
    request.signing_public_key = Some(prost::bytes::Bytes::from(vec![0x88; 32]));

    let response = post_register(app, request, Some(key_label)).await;
    match response.result.expect("result") {
        register_response::Result::Error(err) => assert_eq!(
            err.code, 400,
            "Linked with package fields should be InvalidFormat (400), got {}",
            err.code
        ),
        register_response::Result::Success(_) => {
            panic!("Linked auth should reject package fields")
        }
    }
}

/// Register a signed package, then renew via `POST /issuer/renew` with the
/// renewal token alone.
#[tokio::test]
#[serial]
async fn test_renewal_token_renew_of_a_package_identity() {
    let env = setup_test_environment().await;
    let realm = "renew-route@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let (_, key) =
        signed_package_with_generated_key("renewpkg", "RenewService", "1.0.0", "wasm32-wasip1")
            .await;
    let app = create_test_router(&env).await;

    let register = signed_package_request(
        "renewpkg",
        "RenewService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    // `post_register` consumes the Router; clone it so the renew call hits the
    // same issuer (shared via Arc<AIdIssuer> inside IssuerState).
    let reg_response = post_register(app.clone(), register, Some(ROUTE_KEY_LABEL)).await;
    let ok = match reg_response.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => {
            panic!("register should succeed before renew, got error: {err:?}")
        }
    };
    let actr_id = ok.actr_id;
    let renewal_token = ok
        .renewal_token
        .expect("register should issue a renewal_token");

    let renew_request = RenewCredentialRequest {
        actr_id,
        renewal_token,
        attempt_id: control_attempt_id(),
    };
    let renew_response = post_renew(app, renew_request).await;
    match renew_response.result.expect("renew result") {
        renew_credential_response::Result::Success(ok) => {
            let new_token = ok
                .renewal_token
                .expect("renew should issue a rotated renewal_token");
            assert!(!new_token.is_empty(), "renew should rotate the token");
        }
        renew_credential_response::Result::Error(err) => {
            panic!("renewal of a package identity should succeed, got error: {err:?}")
        }
    }
}

/// A development identity keeps its indicator across renewal, and stops
/// renewing once a registry record holds its publisher name.
#[tokio::test]
#[serial]
async fn test_development_identity_renewal() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "development-renew@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let app = create_test_router(&env).await;
    let key = SigningKey::generate(&mut OsRng);
    let request = signed_package_request(
        "later.example",
        "DevService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let ok = match post_register(app.clone(), request, Some(ROUTE_KEY_LABEL))
        .await
        .result
        .expect("register result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("register failed: {err:?}"),
    };
    assert_eq!(development(&ok), Some(true));

    let renewed = match post_renew(
        app.clone(),
        RenewCredentialRequest {
            actr_id: ok.actr_id.clone(),
            renewal_token: ok.renewal_token.clone().unwrap(),
            attempt_id: control_attempt_id(),
        },
    )
    .await
    .result
    .expect("renew result")
    {
        renew_credential_response::Result::Success(ok) => ok,
        renew_credential_response::Result::Error(err) => panic!("renew failed: {err:?}"),
    };
    assert_eq!(development(&renewed), Some(true));

    // The name gets a registry record: the development identity stops.
    let pool = platform::storage::db::get_database().get_pool();
    seed_publisher_with_key(pool, "later.example", &SigningKey::generate(&mut OsRng)).await;
    let (status, _) = post_renew_raw(
        app,
        RenewCredentialRequest {
            actr_id: ok.actr_id,
            renewal_token: renewed.renewal_token.unwrap(),
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Issue a `/renew` request and return the raw HTTP status alongside the decoded
/// response. Unlike [`post_renew`], this does not assert a 200 status, so it can
/// exercise rejection paths (revocation, realm/package/publisher denial) that
/// surface as a non-2xx status.
async fn post_renew_raw(
    app: Router,
    request: RenewCredentialRequest,
) -> (StatusCode, RenewCredentialResponse) {
    let body = request.encode_to_vec();
    let key = bound_key(&request.actr_id).await;
    let builder = Request::builder()
        .method("POST")
        .uri("/renew")
        .header("content-type", "application/protobuf")
        .header("x-real-ip", "127.0.0.1")
        .header(
            CLIENT_PROOF_HEADER,
            proof(ProofOp::Renew, &request.actr_id.realm, &body, &key),
        );
    let mut http_request = builder.body(Body::from(body)).unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app
        .oneshot(http_request)
        .await
        .expect("renew route response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("renew response body");
    (
        status,
        RenewCredentialResponse::decode(body).expect("decode renew response"),
    )
}

/// AIS-04: an actor placed on the revocation list can no longer renew, even
/// while it still holds a valid, unexpired renewal token.
#[tokio::test]
#[serial]
async fn test_renew_rejected_after_actor_revoked() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-revoked-renew-secret";
    let realm = "linked-revoked-renew@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration, Some(key_label)).await;
    let ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("register failed: {err:?}"),
    };

    issuer::renewal::revoke_actor(&ok.actr_id, "security-review-test")
        .await
        .expect("revoke actor");

    let renew_request = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: ok.renewal_token.clone().expect("renewal token"),
        attempt_id: control_attempt_id(),
    };
    let (status, response) = post_renew_raw(app, renew_request).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a revoked actor must be forbidden from renewing"
    );
    match response.result.expect("renew result") {
        renew_credential_response::Result::Error(err) => {
            assert_eq!(err.code, 403, "revoked actor renew must be forbidden");
        }
        renew_credential_response::Result::Success(_) => {
            panic!("a revoked actor must not be able to renew")
        }
    }

    env.shutdown_signer().await;
}

/// A replayed rejection is the stored response as is: it says nothing about
/// the actor to a caller that never proved it holds its renewal token. A
/// replayed credential is rechecked, so it does not bypass a revocation.
#[tokio::test]
#[serial]
async fn test_renew_replay_rechecks_only_issued_credentials() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let (_, ok) = register_linked_actor(
        &app,
        "renew-replay-oracle@example.com",
        "renew-oracle-secret",
    )
    .await;

    let guess = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: prost::bytes::Bytes::from(vec![9; 32]),
        attempt_id: control_attempt_id(),
    };
    let (status, rejected) = post_renew_raw(app.clone(), guess.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let renewed = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: ok.renewal_token.clone().expect("renewal token"),
        attempt_id: control_attempt_id(),
    };
    let issued = post_renew(app.clone(), renewed.clone()).await;

    issuer::renewal::revoke_actor(&ok.actr_id, "replay oracle")
        .await
        .expect("revoke actor");

    let (status, replayed) = post_renew_raw(app.clone(), guess).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(replayed.encode_to_vec(), rejected.encode_to_vec());

    let (status, replayed) = post_renew_raw(app, renewed).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_ne!(replayed.encode_to_vec(), issued.encode_to_vec());

    env.shutdown_signer().await;
}

/// AIS-04: an actor placed on the revocation list can no longer reissue.
#[tokio::test]
#[serial]
async fn test_reissue_rejected_after_actor_revoked() {
    let mut env = setup_test_environment().await;
    let key_label = "linked-revoked-reissue-secret";
    let realm = "linked-revoked-reissue@example.com";
    seed_realm(realm, Some(key_label)).await;
    let app = create_test_router(&env).await;

    let mut registration = linked_register_request();
    registration.realm = realm.to_string();
    let initial = post_register(app.clone(), registration.clone(), Some(key_label)).await;
    let ok = match initial.result.expect("register result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("register failed: {err:?}"),
    };

    issuer::renewal::revoke_actor(&ok.actr_id, "security-review-test")
        .await
        .expect("revoke actor");

    let response = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: ok.actr_id.clone(),
            registration,
            renewal_proof: ok.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    match response.result.expect("reissue result") {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403, "revoked actor reissue must be forbidden");
        }
        register_response::Result::Success(_) => {
            panic!("a revoked actor must not be able to reissue")
        }
    }

    env.shutdown_signer().await;
}

/// AIS-03: renewing a package identity re-checks the publisher. A publisher that
/// was suspended after issuance must block re-issuance for its packages.
#[tokio::test]
#[serial]
async fn test_issue_for_actor_rejected_after_publisher_suspended() {
    let env = setup_test_environment().await;
    let issuer = create_test_issuer(&env).await;
    let realm = "renew-suspended-publisher@example.com";
    seed_realm(realm, None).await;
    let (_, key) = signed_package_with_generated_key(
        "suspendpub",
        "SuspendRenewService",
        "1.0.0",
        "wasm32-wasip1",
    )
    .await;

    let request = signed_package_request(
        "suspendpub",
        "SuspendRenewService",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let ok = match issuer
        .issue_credential(&request, &owner(&request.realm).await)
        .await
        .expect("issue")
        .result
        .expect("result")
    {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("register failed: {err:?}"),
    };

    let pool = platform::storage::db::get_database().get_pool();
    sqlx::query("UPDATE publisher SET status = 'suspended' WHERE name = ?")
        .bind("suspendpub")
        .execute(pool)
        .await
        .expect("suspend publisher");

    let err = issuer
        .issue_credential_for_actor(&ok.actr_id, &owner(&ok.actr_id.realm).await)
        .await
        .expect_err("a suspended publisher must block re-issuance");
    assert!(
        matches!(err, platform::aid::AidError::PublisherNotVerified),
        "expected PublisherNotVerified, got {err:?}"
    );
}

/// AIS-04 proof invalidation: a possession proof is good for one reissue. A
/// lost response is recovered by retrying the same attempt; a new attempt with
/// the consumed proof is rejected, so a leaked proof cannot be chained.
#[tokio::test]
#[serial]
async fn test_reissue_proof_is_consumed_by_one_attempt() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let key_label = "linked-proof-consumed-secret";
    let (registration, initial_ok) =
        register_linked_actor(&app, "linked-proof-consumed@example.com", key_label).await;

    let request = ReissueCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        registration,
        renewal_proof: initial_ok.renewal_token.clone(),
        attempt_id: control_attempt_id(),
    };
    let first = post_reissue(app.clone(), request.clone(), Some(key_label)).await;
    assert_eq!(error_code(&first), None, "first reissue must succeed");

    let mut new_attempt = request;
    new_attempt.attempt_id = control_attempt_id();
    let replay = post_reissue(app, new_attempt, Some(key_label)).await;
    assert_eq!(
        error_code(&replay),
        Some(REISSUE_POSSESSION_NOT_PROVEN_CODE),
        "a consumed proof must not mint another successor"
    );

    env.shutdown_signer().await;
}

/// Only the exact authenticated request replays a stored reissue response:
/// the same attempt with any other body is a conflict, and a replay honors
/// revocations made since.
// Reloading the signing key from storage uses `block_in_place`.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_reissue_replay_requires_the_original_request() {
    let fx = package_fixture("cache.example.com", "CacheBinding", "wasm32-wasip1").await;
    let app = create_test_router(&fx._env).await;
    let registration = base_package_request(&fx, TEST_REALM);
    let initial = post_register(app.clone(), registration.clone(), Some(DEFAULT_KEY_LABEL)).await;
    let initial = match initial.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("initial registration failed: {other:?}"),
    };
    let request = ReissueCredentialRequest {
        actr_id: initial.actr_id.clone(),
        registration,
        renewal_proof: initial.renewal_token,
        attempt_id: control_attempt_id(),
    };
    let first = post_reissue(app.clone(), request.clone(), Some(DEFAULT_KEY_LABEL)).await;
    assert!(matches!(
        &first.result,
        Some(register_response::Result::Success(_))
    ));
    // A new issuer instance must still recognize the durable attempt.
    let restarted = create_test_router(&fx._env).await;
    let replay = post_reissue(restarted, request.clone(), Some(DEFAULT_KEY_LABEL)).await;
    assert_eq!(first.encode_to_vec(), replay.encode_to_vec());

    let mut mutations = Vec::new();
    let mut missing_proof = request.clone();
    missing_proof.renewal_proof = None;
    mutations.push(missing_proof);
    let mut wrong_proof = request.clone();
    wrong_proof.renewal_proof = Some(prost::bytes::Bytes::from(vec![0xff; 32]));
    mutations.push(wrong_proof);
    let mut missing_signature = request.clone();
    missing_signature.registration.publisher_signature = None;
    mutations.push(missing_signature);
    let mut wrong_actor = request.clone();
    wrong_actor.actr_id.serial_number += 1;
    mutations.push(wrong_actor);
    let mut wrong_target = request.clone();
    wrong_target.registration.target = Some("aarch64-apple-darwin".to_string());
    mutations.push(wrong_target);
    let mut linked = request.clone();
    linked.registration.auth_mode = Some(RegisterAuthMode::Linked as i32);
    mutations.push(linked);
    for changed in mutations {
        let (status, response) =
            post_reissue_raw(app.clone(), changed, Some(DEFAULT_KEY_LABEL)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error_code(&response), Some(409));
    }

    // A stored response must not bypass an actor revocation either.
    issuer::renewal::revoke_actor(&request.actr_id, "replay regression")
        .await
        .unwrap();
    let revoked = post_reissue(app, request, Some(DEFAULT_KEY_LABEL)).await;
    assert_eq!(error_code(&revoked), Some(403));
}

#[tokio::test]
#[serial]
async fn test_reissue_attempt_id_body_mismatch_returns_conflict() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let key_label = "reissue-conflict-secret";
    let (registration, initial_ok) =
        register_linked_actor(&app, "reissue-conflict@example.com", key_label).await;
    let request = ReissueCredentialRequest {
        actr_id: initial_ok.actr_id,
        registration,
        renewal_proof: initial_ok.renewal_token,
        attempt_id: control_attempt_id(),
    };
    let first = post_reissue(app.clone(), request.clone(), Some(key_label)).await;
    assert_eq!(error_code(&first), None);

    let mut conflicting = request;
    conflicting.registration.ws_address = Some("wss://conflict.example.invalid".to_string());
    let (status, response) = post_reissue_raw(app, conflicting, Some(key_label)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&response), Some(409));

    env.shutdown_signer().await;
}

/// Once a stored response reaches its retention horizon, Issuer returns 410
/// instead of silently treating the replay as a new operation.
#[tokio::test]
#[serial]
async fn test_reissue_replay_after_response_expiry_returns_gone() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let key_label = "reissue-gone-secret";
    let (registration, initial_ok) =
        register_linked_actor(&app, "reissue-gone@example.com", key_label).await;
    let request = ReissueCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        registration,
        renewal_proof: initial_ok.renewal_token.clone(),
        attempt_id: control_attempt_id(),
    };
    let first = post_reissue(app.clone(), request.clone(), Some(key_label)).await;
    assert_eq!(error_code(&first), None);

    let pool = platform::storage::db::get_database().get_pool();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let expired = sqlx::query(
        "UPDATE issuer_control_attempt SET response_expires_at = ?1
         WHERE operation = 'reissue' AND attempt_id = ?2",
    )
    .bind(now - 1)
    .bind(request.attempt_id.as_ref())
    .execute(pool)
    .await
    .expect("expire stored reissue response");
    assert_eq!(expired.rows_affected(), 1);

    let (status, response) = post_reissue_raw(app, request, Some(key_label)).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(error_code(&response), Some(410));

    env.shutdown_signer().await;
}

/// A second request for an attempt another worker still holds is told to wait.
#[tokio::test]
#[serial]
async fn test_reissue_attempt_in_progress_returns_too_early() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let key_label = "reissue-in-progress-secret";
    let (registration, initial_ok) =
        register_linked_actor(&app, "reissue-in-progress@example.com", key_label).await;
    let request = ReissueCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        registration,
        renewal_proof: initial_ok.renewal_token.clone(),
        attempt_id: control_attempt_id(),
    };

    // Another worker claimed this attempt a moment ago and still holds it.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let body_hash = <sha2::Sha256 as sha2::Digest>::digest(request.encode_to_vec()).to_vec();
    sqlx::query(
        "INSERT INTO issuer_control_attempt (
            operation, attempt_id, actor_id, request_hash, state, lease_id,
            lease_expires_at, tombstone_expires_at, created_at, updated_at
         ) VALUES ('reissue', ?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?7)",
    )
    .bind(request.attempt_id.as_ref())
    .bind(initial_ok.actr_id.to_string_repr())
    .bind(&body_hash)
    .bind(vec![7u8; 16])
    .bind(now + 60)
    .bind(now + 300)
    .bind(now)
    .execute(platform::storage::db::get_database().get_pool())
    .await
    .expect("seed in-progress attempt");

    let body = request.encode_to_vec();
    let mut http_request = Request::builder()
        .method("POST")
        .uri("/reissue")
        .header("content-type", "application/protobuf")
        .header("x-real-ip", "127.0.0.1")
        .header(
            CLIENT_PROOF_HEADER,
            proof(
                ProofOp::Reissue,
                &request.registration.realm,
                &body,
                &key_for(key_label),
            ),
        )
        .body(Body::from(body))
        .unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app.oneshot(http_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_EARLY);
    let retry_after: u64 = response
        .headers()
        .get("retry-after")
        .expect("Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry_after), "retry after {retry_after}");

    env.shutdown_signer().await;
}

/// A signed package identity depends on its trusted Publisher. Exercise
/// the real renewal endpoint, legacy NULL auth modes, and publisher deletion.
// Reloading the signing key from storage uses `block_in_place`.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_package_renewal_enforces_publisher_status() {
    let fx = package_fixture("renew.example.com", "PackageRenew", "wasm32-wasip1").await;
    let app = create_test_router(&fx._env).await;
    let registration = base_package_request(&fx, TEST_REALM);
    let initial = post_register(app.clone(), registration, Some(DEFAULT_KEY_LABEL)).await;
    let initial = match initial.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("initial registration failed: {other:?}"),
    };
    let pool = platform::storage::db::get_database().get_pool();
    let mode: i32 = sqlx::query_scalar(
        "SELECT auth_mode FROM issued_actor_identity WHERE realm = ? AND serial_number = ?",
    )
    .bind(TEST_REALM)
    .bind(initial.actr_id.serial_number as i64)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(mode, RegisterAuthMode::Package as i32);
    let renew = RenewCredentialRequest {
        actr_id: initial.actr_id.clone(),
        renewal_token: initial.renewal_token.unwrap(),
        attempt_id: control_attempt_id(),
    };
    let (status, _) = post_renew_raw(app.clone(), renew.clone()).await;
    assert_eq!(status, StatusCode::OK);

    for legacy in [false, true] {
        if legacy {
            sqlx::query("UPDATE issued_actor_identity SET auth_mode = NULL WHERE realm = ? AND serial_number = ?")
                .bind(TEST_REALM).bind(initial.actr_id.serial_number as i64)
                .execute(pool).await.unwrap();
        }
        for publisher_status in ["suspended", "revoked"] {
            sqlx::query("UPDATE publisher SET status = ? WHERE name = ?")
                .bind(publisher_status)
                .bind(&fx.publisher)
                .execute(pool)
                .await
                .unwrap();
            let (status, response) = post_renew_raw(app.clone(), renew.clone()).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "legacy={legacy}, status={publisher_status}"
            );
            assert!(matches!(
                response.result,
                Some(renew_credential_response::Result::Error(_))
            ));
        }
    }
    sqlx::query("DELETE FROM publisher WHERE name = ?")
        .bind(&fx.publisher)
        .execute(pool)
        .await
        .unwrap();
    let (status, _) = post_renew_raw(app.clone(), renew).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Explicit Linked registration still renews without a publisher row.
    let linked = post_register(
        app.clone(),
        linked_register_request(),
        Some(DEFAULT_KEY_LABEL),
    )
    .await;
    let linked = match linked.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("linked registration failed: {other:?}"),
    };
    let (status, _) = post_renew_raw(
        app,
        RenewCredentialRequest {
            actr_id: linked.actr_id,
            renewal_token: linked.renewal_token.unwrap(),
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

fn renewed(response: RenewCredentialResponse) -> register_response::RegisterOk {
    match response.result.expect("renew result") {
        renew_credential_response::Result::Success(ok) => ok,
        renew_credential_response::Result::Error(err) => panic!("renew failed: {err:?}"),
    }
}

/// A token consumed by `/reissue` renews only to the successor the reissue
/// already handed out, and only for five minutes after the consumption; a
/// leaked copy of it is useless afterwards.
#[tokio::test]
#[serial]
async fn test_renew_with_a_token_consumed_by_reissue() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let key_label = "renew-consumed-secret";
    let (registration, initial_ok) =
        register_linked_actor(&app, "renew-consumed@example.com", key_label).await;
    let old_token = initial_ok.renewal_token.clone().expect("renewal token");

    let reissued = post_reissue(
        app.clone(),
        ReissueCredentialRequest {
            actr_id: initial_ok.actr_id.clone(),
            registration,
            renewal_proof: Some(old_token.clone()),
            attempt_id: control_attempt_id(),
        },
        Some(key_label),
    )
    .await;
    let reissued = match reissued.result.expect("reissue result") {
        register_response::Result::Success(ok) => ok,
        register_response::Result::Error(err) => panic!("reissue failed: {err:?}"),
    };
    assert_ne!(reissued.renewal_token.as_ref(), Some(&old_token));

    let renew = |attempt_id| RenewCredentialRequest {
        actr_id: initial_ok.actr_id.clone(),
        renewal_token: old_token.clone(),
        attempt_id,
    };
    let ok = renewed(post_renew(app.clone(), renew(control_attempt_id())).await);
    assert_eq!(ok.renewal_token, reissued.renewal_token);
    assert_eq!(
        ok.renewal_token_expires_at,
        reissued.renewal_token_expires_at
    );

    let pool = platform::storage::db::get_database().get_pool();
    sqlx::query(
        "UPDATE issuer_renewal_token SET created_at = created_at - 301
         WHERE actor_id = ?1 AND token_hash = ?2",
    )
    .bind(initial_ok.actr_id.to_string_repr())
    .bind(issuer::renewal::hash_token(
        reissued.renewal_token.as_ref().expect("successor"),
    ))
    .execute(pool)
    .await
    .expect("age the consumption");
    let (status, _) = post_renew_raw(app.clone(), renew(control_attempt_id())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The successor itself keeps renewing.
    let ok = renewed(
        post_renew(
            app,
            RenewCredentialRequest {
                actr_id: initial_ok.actr_id.clone(),
                renewal_token: reissued.renewal_token.clone().expect("successor"),
                attempt_id: control_attempt_id(),
            },
        )
        .await,
    );
    assert_eq!(ok.renewal_token, reissued.renewal_token);

    env.shutdown_signer().await;
}

/// `/renew` checks the renewal token before the Realm: a caller without a
/// valid token cannot learn that a Realm is suspended.
#[tokio::test]
#[serial]
async fn test_renew_does_not_reveal_realm_state_before_authentication() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let realm = "renew-hidden-state@example.com";
    let (_, ok) = register_linked_actor(&app, realm, "renew-hidden-secret").await;
    platform::realm::Realm::set_enabled(realm, false)
        .await
        .expect("suspend realm");

    let (status, _) = post_renew_raw(
        app.clone(),
        RenewCredentialRequest {
            actr_id: ok.actr_id.clone(),
            renewal_token: prost::bytes::Bytes::from(vec![9; 32]),
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = post_renew_raw(
        app,
        RenewCredentialRequest {
            actr_id: ok.actr_id.clone(),
            renewal_token: ok.renewal_token.clone().expect("renewal token"),
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    env.shutdown_signer().await;
}

// ── Realm membership (`actr/protocols/realm-membership.md`) ──────────

async fn post_membership(
    app: Router,
    document: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut http_request = Request::builder()
        .method("POST")
        .uri("/membership")
        .header("content-type", "application/json")
        .body(Body::from(document.to_string()))
        .unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app.oneshot(http_request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn membership_request(
    op: actr_protocol::realm_membership::MembershipOp,
    realm: &str,
) -> actr_protocol::realm_membership::MembershipRequest {
    actr_protocol::realm_membership::MembershipRequest::new(
        op,
        realm,
        PROVIDER_ID,
        &uuid::Uuid::new_v4().to_string(),
    )
}

/// Join with the key `label` through `POST /membership`.
async fn join_over_http(
    app: &Router,
    realm: &str,
    invite: &str,
    label: &str,
) -> (StatusCode, serde_json::Value) {
    use actr_protocol::realm_membership::MembershipOp;
    let key = key_for(label);
    let mut request = membership_request(MembershipOp::Join, realm);
    request.invite = Some(RealmInvite::decode(invite).unwrap().secret);
    request.public_key =
        Some(base64::prelude::BASE64_STANDARD.encode(key.verifying_key().to_bytes()));
    post_membership(app.clone(), request.sign(&key)).await
}

#[tokio::test]
#[serial]
async fn test_membership_requests_over_http() {
    use actr_protocol::realm_membership::MembershipOp;
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let realm = "members-http@example.com";
    seed_realm(realm, Some("members-owner")).await;
    let owner_key = key_for("members-owner");

    // The owner invites an editor.
    let mut invite = membership_request(MembershipOp::CreateInvite, realm);
    invite.role = Some(Role::Editor);
    invite.invite_expires_at = Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
    let (status, created) = post_membership(app.clone(), invite.sign(&owner_key)).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let text = created["invite"].as_str().unwrap().to_string();

    // The invite names its Realm; another Realm's name is a mismatch.
    seed_realm("members-other@example.com", None).await;
    let (status, error) =
        join_over_http(&app, "members-other@example.com", &text, "members-editor").await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::CONFLICT, Some("invite_realm_mismatch"))
    );
    let (status, joined) = join_over_http(&app, realm, &text, "members-editor").await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "editor");
    let (status, error) = join_over_http(&app, realm, &text, "members-third").await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::GONE, Some("invite_used"))
    );

    // Roles come before targets: an editor learns nothing about members.
    let editor_key = key_for("members-editor");
    let remove = || {
        let mut remove = membership_request(MembershipOp::RemoveMember, realm);
        remove.member_id = Some("0000000000000000".to_string());
        remove
    };
    let (status, error) = post_membership(app.clone(), remove().sign(&editor_key)).await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    let (status, error) = post_membership(app.clone(), remove().sign(&owner_key)).await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );

    // A replayed nonce, a foreign provider and an unknown key are refused.
    let list = membership_request(MembershipOp::ListMembers, realm);
    let signed = list.sign(&editor_key);
    let (status, own) = post_membership(app.clone(), signed.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own["members"].as_array().unwrap().len(), 1);
    let (status, error) = post_membership(app.clone(), signed).await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request"))
    );
    let mut foreign = membership_request(MembershipOp::ListMembers, realm);
    foreign.provider_id = "other.example".to_string();
    let (status, _) = post_membership(app.clone(), foreign.sign(&owner_key)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, error) = post_membership(
        app.clone(),
        membership_request(MembershipOp::ListMembers, realm).sign(&key_for("stranger")),
    )
    .await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("client_unknown"))
    );

    // update_realm is for Realms whose Realm Profile this Loran signs.
    let mut update = membership_request(MembershipOp::UpdateRealm, realm);
    update.display_name = Some("Renamed".to_string());
    let (status, error) = post_membership(app.clone(), update.sign(&owner_key)).await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );

    env.shutdown_signer().await;
}

/// Only the owner registers the Realm's Service, any version of it.
// Reloading the signing key from storage uses `block_in_place`.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_only_the_owner_registers_the_service() {
    let fx = package_fixture("svc.example.com", "notes", "wasm32-wasip1").await;
    let app = create_test_router(&fx._env).await;
    add_client(TEST_REALM, "service-editor", Role::Editor).await;
    // The Realm Profile names another version of the program.
    sqlx::query(
        "INSERT INTO realm_profile (realm, sequence, digest, document, accepted_at)
         VALUES (?, 1, 'd', ?, 1)",
    )
    .bind(TEST_REALM)
    .bind(serde_json::json!({ "package": { "type": "svc.example.com/notes@0.9.0" } }).to_string())
    .execute(platform::storage::db::get_database().get_pool())
    .await
    .unwrap();

    let request = base_package_request(&fx, TEST_REALM);
    match post_register(app.clone(), request.clone(), Some("service-editor"))
        .await
        .result
        .unwrap()
    {
        register_response::Result::Error(err) => {
            assert_eq!(err.code, 403);
            assert!(
                err.message.starts_with("service_owner_only:"),
                "{}",
                err.message
            );
        }
        other => panic!("an editor registered the Service: {other:?}"),
    }
    let registered = post_register(app, request, Some(DEFAULT_KEY_LABEL)).await;
    assert!(
        matches!(
            registered.result,
            Some(register_response::Result::Success(_))
        ),
        "{registered:?}"
    );
}

/// Revoking a key stops its Actors; another key of the same member reissues
/// them and moves them to itself.
#[tokio::test]
#[serial]
async fn test_a_revoked_key_disables_its_actors_until_the_member_reissues() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let realm = "key-revoked@example.com";
    let (registration, ok) = register_linked_actor(&app, realm, "revoked-first").await;
    let owner = owner(realm).await;
    let second = add_second_client(realm, &owner, "revoked-second").await;
    let token = ok.renewal_token.clone().unwrap();
    use turn_crate::auth::AuthHandler as _;
    let inactive = turn::InactiveRealms::default();
    let _watch = inactive.watch().await;
    let auth = turn::Authenticator::new(
        "test-turn-secret-at-least-32-bytes!".into(),
        PROVIDER_ID.into(),
    )
    .unwrap()
    .with_inactive_realms(inactive.clone());
    let source = "127.0.0.1:3478".parse().unwrap();
    let old_turn = &ok.turn_credential;
    auth.auth_handle(&old_turn.username, "turn.test", source)
        .unwrap();

    // Another key of the member does not renew the Actor.
    let renew = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: token.clone(),
        attempt_id: control_attempt_id(),
    };
    let body = renew.encode_to_vec();
    let (status, response) = post_renew_with(
        app.clone(),
        &body,
        proof(ProofOp::Renew, realm, &body, &key_for("revoked-second")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(renew_message(&response).starts_with("client_unknown:"));

    membership::revoke_client(&second, &owner.key_id)
        .await
        .unwrap();
    assert!(membership::actor_disabled(&ok.actr_id).await.unwrap());
    // The revoked key signs nothing.
    let (status, response) = post_renew_raw(app.clone(), renew).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(renew_message(&response).starts_with("client_revoked:"));

    // The member's other key reissues the Actor and becomes its key.
    let reissue = ReissueCredentialRequest {
        actr_id: ok.actr_id.clone(),
        registration,
        renewal_proof: Some(token),
        attempt_id: control_attempt_id(),
    };
    let response = post_reissue(app.clone(), reissue, Some("revoked-second")).await;
    let reissued = match response.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("reissue failed: {other:?}"),
    };
    assert_eq!(reissued.actr_id, ok.actr_id);
    let claims =
        actr_protocol::IdentityClaims::decode(reissued.credential.claims.as_ref()).unwrap();
    assert_eq!(claims.client_key, second.key_id);
    assert!(!membership::actor_disabled(&ok.actr_id).await.unwrap());
    for _ in 0..100 {
        // The follower publishes the new binding before clearing the disabled
        // flag. Wait for both parts of that transition before authenticating.
        if inactive.matches_binding(&ok.actr_id.to_string_repr(), &second.key_id)
            && !inactive.contains_actor(&ok.actr_id.to_string_repr())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(inactive.matches_binding(&ok.actr_id.to_string_repr(), &second.key_id));
    assert!(!inactive.contains_actor(&ok.actr_id.to_string_repr()));
    assert!(
        auth.auth_handle(&old_turn.username, "turn.test", source)
            .is_err()
    );
    auth.clear_cache();
    assert!(
        auth.auth_handle(&old_turn.username, "turn.test", source)
            .is_err()
    );
    auth.auth_handle(&reissued.turn_credential.username, "turn.test", source)
        .unwrap();

    env.shutdown_signer().await;
}

/// A second client of `owner`'s member, with the key `label`.
async fn add_second_client(realm: &str, owner: &Signer, label: &str) -> Signer {
    let key = key_for(label).verifying_key().to_bytes();
    let expires = chrono::Utc::now().timestamp() + 3600;
    let invite = membership::create_invite(owner, None, Some(&owner.member_id), expires, limits())
        .await
        .unwrap();
    let secret = RealmInvite::decode(&invite.invite).unwrap().secret;
    membership::join(
        realm,
        &invite_secret_hash(&secret).unwrap(),
        &key,
        label,
        limits(),
    )
    .await
    .unwrap();
    membership::authenticate(
        membership::client_key(realm, &client_key_id(&key))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn post_renew_with(
    app: Router,
    body: &[u8],
    client_proof: String,
) -> (StatusCode, RenewCredentialResponse) {
    let mut http_request = Request::builder()
        .method("POST")
        .uri("/renew")
        .header("content-type", "application/protobuf")
        .header(CLIENT_PROOF_HEADER, client_proof)
        .body(Body::from(body.to_vec()))
        .unwrap();
    http_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let response = app.oneshot(http_request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, RenewCredentialResponse::decode(body).unwrap())
}

fn renew_message(response: &RenewCredentialResponse) -> &str {
    match &response.result {
        Some(renew_credential_response::Result::Error(err)) => &err.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Loopback-only fixture for packages/actr-web/scripts/test-issuer-proof.sh.
/// Uses real Issuer handlers, membership transactions, and an embedded Signer.
#[tokio::test]
#[serial]
#[ignore = "started by the Web Issuer integration test script"]
async fn web_issuer_http_fixture() {
    use axum::response::IntoResponse as _;
    use axum::{Json, middleware, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let ready = std::env::var("ACTR_WEB_FIXTURE_READY").expect("fixture ready path");
    let mut env = setup_test_environment().await;
    let realm = "browser@example.com";
    let label = "browser-device";
    seed_realm(realm, Some(label)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let file = actr_protocol::realm_membership::RealmClientKeyFile::new(
        realm,
        PROVIDER_ID,
        &endpoint,
        &key_for(label),
    );
    let first_renew = Arc::new(AtomicBool::new(true));
    let observed = Arc::new(Mutex::new(Vec::<(Vec<u8>, String)>::new()));
    let app = create_test_router(&env).await.layer(middleware::from_fn({
        let first_renew = first_renew.clone();
        let observed = observed.clone();
        move |request: Request<Body>, next: middleware::Next| {
            let first_renew = first_renew.clone();
            let observed = observed.clone();
            async move {
                if request.uri().path() != "/renew" || request.method() != axum::http::Method::POST
                {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let body = to_bytes(body, usize::MAX).await.unwrap();
                let proof =
                    ClientProof::parse_header(parts.headers[CLIENT_PROOF_HEADER].to_str().unwrap())
                        .unwrap();
                observed.lock().unwrap().push((body.to_vec(), proof.nonce));
                let mut response = next.run(Request::from_parts(parts, Body::from(body))).await;
                if response.status().is_success() && first_renew.swap(false, Ordering::SeqCst) {
                    // The operation committed and consumed the proof. Retrying
                    // needs the same body/attempt and a new signed nonce.
                    // Preserve the production router's CORS response headers.
                    *response.status_mut() = StatusCode::TOO_EARLY;
                    response
                        .headers_mut()
                        .insert("retry-after", "0".parse().unwrap());
                }
                response
            }
        }
    }));
    let stop = Arc::new(tokio::sync::Notify::new());
    let app = app
        .route(
            "/fixture",
            get(move || {
                let file = file.clone();
                async move { Json(file) }
            }),
        )
        .route(
            "/stop",
            get({
                let stop = stop.clone();
                move || {
                    let stop = stop.clone();
                    async move {
                        stop.notify_one();
                        "stopped"
                    }
                }
            }),
        );
    // Fixture-only helper endpoints need their own CORS. Issuer requests must
    // use the production router's policy, including its preflight handling.
    let app = app.layer(middleware::from_fn(
        |request: Request<Body>, next: middleware::Next| async move {
            if !matches!(request.uri().path(), "/fixture" | "/stop") {
                return next.run(request).await;
            }
            let mut response = if request.method() == axum::http::Method::OPTIONS {
                StatusCode::NO_CONTENT.into_response()
            } else {
                next.run(request).await
            };
            for (key, value) in [
                ("access-control-allow-origin", "*"),
                ("access-control-allow-methods", "GET, POST, OPTIONS"),
                (
                    "access-control-allow-headers",
                    "content-type, accept, x-actr-client-proof",
                ),
                ("access-control-expose-headers", "retry-after"),
            ] {
                response.headers_mut().insert(key, value.parse().unwrap());
            }
            response
        },
    ));
    std::fs::write(ready, &endpoint).unwrap();
    let shutdown = stop.clone();
    tokio::time::timeout(
        Duration::from_secs(180),
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown.notified().await;
        })
        .into_future(),
    )
    .await
    .unwrap()
    .unwrap();
    {
        let observed = observed.lock().unwrap();
        assert!(
            observed.len() >= 2,
            "the browser must retry a committed renewal"
        );
        assert_eq!(
            observed[0].0, observed[1].0,
            "retry changed the body/attempt"
        );
        assert_ne!(observed[0].1, observed[1].1, "retry reused the proof nonce");
    }
    env.shutdown_signer().await;
}

// ── Publisher Profile freshness at registration ───────────────────────────
//
// `register_publisher` captures the publisher domain's Publisher Profile into
// this Loran's Publisher Registry once. These tests wire a
// `PublisherProfileRefresher` so registration re-checks the domain, and cover
// what the captured projection alone cannot: a key the domain has removed, an
// unreachable domain, and a rolled-back Profile.

/// Serves one canned descriptor for every fetch; counts the calls.
struct StubFetcher {
    response: Mutex<Result<Arc<[u8]>, String>>,
    calls: AtomicUsize,
}

impl StubFetcher {
    fn serving(document: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(Arc::from(document))),
            calls: AtomicUsize::new(0),
        })
    }

    fn failing(message: &str) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Err(message.to_string())),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn serve(&self, document: Vec<u8>) {
        *self.response.lock().unwrap() = Ok(Arc::from(document));
    }
}

#[async_trait::async_trait]
impl PublicDocumentFetcher for StubFetcher {
    async fn fetch(&self, _url: &str) -> Result<Arc<[u8]>, platform::net::FetchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.response
            .lock()
            .unwrap()
            .clone()
            .map_err(platform::net::FetchError::new)
    }
}

/// The signed `publisher` member for `domain`: `root` (id `root-1`) signs the
/// Profile; `package_keys` lists package signing keys with their status.
fn signed_publisher_profile(
    domain: &str,
    sequence: u64,
    root: &ed25519_dalek::SigningKey,
    package_keys: &[(&ed25519_dalek::SigningKey, &str)],
) -> serde_json::Value {
    let member = |id: &str, key: &ed25519_dalek::SigningKey, status: &str| {
        serde_json::json!({
            "id": id,
            "algorithm": "Ed25519",
            "status": status,
            "public_key": base64::prelude::BASE64_STANDARD.encode(key.verifying_key().to_bytes()),
        })
    };
    let now = chrono::Utc::now();
    let mut profile = serde_json::json!({
        "schema": "actr.publisher/v1",
        "sequence": sequence,
        "domain": domain,
        "profile_signing_keys": [member("root-1", root, "active")],
        "keys": package_keys
            .iter()
            .map(|(package, status)| member(
                &actr_protocol::package_key_id(package.verifying_key().as_bytes()),
                package,
                status,
            ))
            .collect::<Vec<_>>(),
        "profile_issued_at": (now - chrono::Duration::hours(1)).to_rfc3339(),
        "profile_expires_at": (now + chrono::Duration::days(30)).to_rfc3339(),
    });
    actr_protocol::realm_profile::sign_document(&mut profile, "root-1", root);
    profile
}

/// `domain`'s authority descriptor carrying its signed Publisher Profile.
fn publisher_descriptor(domain: &str, profile: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "authority": format!("actr://{domain}"),
        "services": [{ "rel": "contracts", "href": format!("https://{domain}/contracts") }],
        "publisher": profile,
    }))
    .unwrap()
}

/// The registration baseline: the Profile `register_publisher` accepts into
/// this Loran's Publisher Registry.
async fn register_publisher_from_profile(domain: &str, profile: &serde_json::Value) {
    let profile: actr_protocol::PublisherProfile =
        serde_json::from_value(profile.clone()).expect("Publisher Profile");
    let keys = profile
        .verify(domain, chrono::Utc::now())
        .expect("verify Publisher Profile");
    let pool = platform::storage::db::get_database().get_pool().clone();
    loran_publisher_registry::PublisherRegistryManager::new(pool)
        .register_publisher(domain, &profile, &keys)
        .await
        .expect("register publisher");
}

/// The registry row's `actr-pub-…` id of `package`.
fn package_key_id_of(package: &ed25519_dalek::SigningKey) -> String {
    loran_publisher_registry::crypto::compute_key_id(&package.verifying_key().to_bytes())
}

/// The registry still lists the key when the registration arrives; the
/// publisher's domain has removed it. Registration must be refused and the
/// registry must agree afterwards — without a publisher re-registration.
#[tokio::test]
#[serial]
async fn test_key_removed_by_the_domain_is_rejected_at_registration() {
    use ed25519_dalek::SigningKey;
    use loran_publisher_registry::manager::PackageSigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "domain-removed-key@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let pool = platform::storage::db::get_database().get_pool().clone();

    let domain = "rotated-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);
    let successor = SigningKey::generate(&mut OsRng);

    let baseline = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &baseline).await;
    let key_id = package_key_id_of(&package);
    let manager = loran_publisher_registry::PublisherRegistryManager::new(pool.clone());
    assert!(
        matches!(
            manager.package_signing_key(domain, &key_id).await.unwrap(),
            PackageSigningKey::Trusted(_)
        ),
        "the registry projection alone still trusts the key"
    );

    // The domain has moved on: the key removed, a successor active.
    let updated = signed_publisher_profile(domain, 2, &root, &[(&successor, "active")]);
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, updated));
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::ZERO,
        Duration::ZERO,
    ));

    let request =
        signed_package_request(domain, "notes", "1.0.0", realm, "wasm32-wasip1", &package);
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);
    let response = issuer
        .issue_credential(&request, &owner(realm).await)
        .await
        .expect("issue");
    assert_eq!(
        error_code(&response),
        Some(403),
        "a key the domain removed must not register: {response:?}"
    );
    assert_eq!(fetcher.calls(), 1, "the registration checked the domain");
    assert!(
        manager.package_signing_key(domain, &key_id).await.is_err(),
        "the refresh revoked the key in the registry"
    );
}

/// A Profile the domain has not changed: registration succeeds, and a second
/// registration reads the fresh check instead of fetching again.
#[tokio::test]
#[serial]
async fn test_unchanged_profile_registers_with_one_fetch() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "fresh-profile@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;

    let domain = "steady-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);

    let baseline = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &baseline).await;
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, baseline));
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::from_secs(3600),
        Duration::from_secs(60),
    ));
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);

    for name in ["notes", "other-notes"] {
        let request =
            signed_package_request(domain, name, "1.0.0", realm, "wasm32-wasip1", &package);
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .expect("issue");
        match response.result.expect("result") {
            register_response::Result::Success(ok) => {
                assert_eq!(
                    development(&ok),
                    None,
                    "{name}: keeps the Publisher identity"
                )
            }
            register_response::Result::Error(err) => {
                panic!("an unchanged Publisher Profile must register: {err:?}")
            }
        }
    }
    assert_eq!(
        fetcher.calls(),
        1,
        "the second registration reads the fresh check"
    );
}

/// While the domain cannot be reached, registration fails closed, and the
/// failed check is not retried within the retry window.
#[tokio::test]
#[serial]
async fn test_unreachable_publisher_domain_fails_closed_and_throttles() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "unreachable-domain@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;

    let domain = "unreachable-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);

    let baseline = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &baseline).await;
    let fetcher = StubFetcher::failing("connection refused");
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::ZERO,
        Duration::from_secs(60),
    ));
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);

    for name in ["notes", "other-notes"] {
        let request =
            signed_package_request(domain, name, "1.0.0", realm, "wasm32-wasip1", &package);
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .expect("issue");
        assert_eq!(
            error_code(&response),
            Some(500),
            "{name}: an unreachable domain must not fall back to the stale row: {response:?}"
        );
    }
    assert_eq!(
        fetcher.calls(),
        1,
        "the failed check is not retried within the retry window"
    );
}

/// A domain serving an older Profile than the accepted baseline is a
/// rollback; registration is refused and the baseline stays.
#[tokio::test]
#[serial]
async fn test_rolled_back_profile_is_rejected() {
    use ed25519_dalek::SigningKey;
    use loran_publisher_registry::manager::PackageSigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "rolled-back-profile@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let pool = platform::storage::db::get_database().get_pool().clone();

    let domain = "rollback-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);

    let baseline = signed_publisher_profile(domain, 2, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &baseline).await;

    let rolled_back = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, rolled_back));
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::ZERO,
        Duration::ZERO,
    ));

    let request =
        signed_package_request(domain, "notes", "1.0.0", realm, "wasm32-wasip1", &package);
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);
    let response = issuer
        .issue_credential(&request, &owner(realm).await)
        .await
        .expect("issue");
    assert_eq!(
        error_code(&response),
        Some(500),
        "a rolled-back Profile must not be accepted: {response:?}"
    );

    let manager = loran_publisher_registry::PublisherRegistryManager::new(pool);
    assert!(
        matches!(
            manager
                .package_signing_key(domain, &package_key_id_of(&package))
                .await
                .unwrap(),
            PackageSigningKey::Trusted(_)
        ),
        "the accepted baseline stays"
    );
}

/// A key the domain retired but still lists keeps verifying, like the
/// documented `retired` package signing keys.
#[tokio::test]
#[serial]
async fn test_key_retired_by_the_domain_still_registers() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "retired-key@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;

    let domain = "retiring-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);
    let successor = SigningKey::generate(&mut OsRng);

    let baseline = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &baseline).await;

    let updated = signed_publisher_profile(
        domain,
        2,
        &root,
        &[(&package, "retired"), (&successor, "active")],
    );
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, updated));
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::ZERO,
        Duration::ZERO,
    ));

    let request =
        signed_package_request(domain, "notes", "1.0.0", realm, "wasm32-wasip1", &package);
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);
    let response = issuer
        .issue_credential(&request, &owner(realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(
                development(&ok),
                None,
                "a retired key keeps its Publisher identity"
            )
        }
        register_response::Result::Error(err) => {
            panic!("a retired-but-listed key must keep verifying: {err:?}")
        }
    }
}

/// A publisher name no registry record holds is a development identity; the
/// freshness check must not fetch a Profile for it.
#[tokio::test]
#[serial]
async fn test_development_identity_registers_without_fetching_a_profile() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = setup_test_environment().await;
    let realm = "dev-identity-no-fetch@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;

    let fetcher = StubFetcher::failing("a development identity must not fetch a Profile");
    let refresher = Arc::new(PublisherProfileRefresher::new(
        fetcher.clone(),
        Duration::ZERO,
        Duration::ZERO,
    ));
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(refresher);

    let key = SigningKey::generate(&mut OsRng);
    let request = signed_package_request(
        "stranger.example",
        "notes",
        "1.0.0",
        realm,
        "wasm32-wasip1",
        &key,
    );
    let response = issuer
        .issue_credential(&request, &owner(realm).await)
        .await
        .expect("issue");
    match response.result.expect("result") {
        register_response::Result::Success(ok) => {
            assert_eq!(
                development(&ok),
                Some(true),
                "an unknown publisher stays development"
            )
        }
        register_response::Result::Error(err) => {
            panic!("a valid development package must register: {err:?}")
        }
    }
    assert_eq!(fetcher.calls(), 0);
}

/// A new domain key must trigger discovery even when the projection has
/// never seen it, including when an earlier key's check is still fresh.
#[tokio::test]
#[serial]
async fn test_new_domain_key_refreshes_before_registration() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    for cached in [false, true] {
        let mut env = setup_test_environment().await;
        let realm = if cached {
            "cached-new-domain-key@example.com"
        } else {
            "new-domain-key@example.com"
        };
        seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
        let domain = if cached {
            "cached-new-key-publisher.example"
        } else {
            "new-key-publisher.example"
        };
        let root = SigningKey::generate(&mut OsRng);
        let old = SigningKey::generate(&mut OsRng);
        let new = SigningKey::generate(&mut OsRng);
        let baseline = signed_publisher_profile(domain, 1, &root, &[(&old, "active")]);
        register_publisher_from_profile(domain, &baseline).await;
        let fetcher = StubFetcher::serving(publisher_descriptor(domain, baseline));
        let issuer = create_test_issuer(&env)
            .await
            .with_publisher_profiles(Arc::new(PublisherProfileRefresher::new(
                fetcher.clone(),
                Duration::from_secs(3600),
                Duration::ZERO,
            )));
        if cached {
            let request =
                signed_package_request(domain, "old-notes", "1.0.0", realm, "wasm32-wasip1", &old);
            let response = issuer
                .issue_credential(&request, &owner(realm).await)
                .await
                .unwrap();
            assert_eq!(error_code(&response), None, "{response:?}");
        }
        let updated =
            signed_publisher_profile(domain, 2, &root, &[(&old, "retired"), (&new, "active")]);
        fetcher.serve(publisher_descriptor(domain, updated));
        let request =
            signed_package_request(domain, "new-notes", "1.0.0", realm, "wasm32-wasip1", &new);
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .unwrap();
        match response.result.unwrap() {
            register_response::Result::Success(ok) => assert_eq!(development(&ok), None),
            other => panic!("new key must register, cached={cached}: {other:?}"),
        }
        assert_eq!(fetcher.calls(), if cached { 2 } else { 1 });
        let manager = loran_publisher_registry::PublisherRegistryManager::new(
            platform::storage::db::get_database().get_pool().clone(),
        );
        assert!(matches!(
            manager
                .package_signing_key(domain, &package_key_id_of(&new))
                .await
                .unwrap(),
            loran_publisher_registry::manager::PackageSigningKey::Trusted(_)
        ));
        env.shutdown_signer().await;
    }
}

/// Repeated unknown-key requests must neither hammer the publisher domain
/// nor downgrade a registered publisher to a development identity.
#[tokio::test]
#[serial]
async fn test_missing_domain_key_is_rejected_with_one_fetch_per_retry_window() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    let env = setup_test_environment().await;
    let realm = "missing-domain-key@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let domain = "missing-key-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);
    let unknown = SigningKey::generate(&mut OsRng);
    let profile = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &profile).await;
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, profile));
    let issuer = create_test_issuer(&env)
        .await
        .with_publisher_profiles(Arc::new(PublisherProfileRefresher::with_defaults(
            fetcher.clone(),
        )));
    for name in ["notes", "other-notes"] {
        let request =
            signed_package_request(domain, name, "1.0.0", realm, "wasm32-wasip1", &unknown);
        let response = issuer
            .issue_credential(&request, &owner(realm).await)
            .await
            .unwrap();
        assert_eq!(error_code(&response), Some(403), "{response:?}");
    }
    assert_eq!(fetcher.calls(), 1);
}

/// Checks arriving while a fetch is in progress share that accepted result.
#[tokio::test]
#[serial]
async fn test_concurrent_publisher_profile_checks_share_one_fetch() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    let _env = setup_test_environment().await;
    let domain = "concurrent-publisher.example";
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);
    let profile = signed_publisher_profile(domain, 1, &root, &[(&package, "active")]);
    register_publisher_from_profile(domain, &profile).await;
    let fetcher = StubFetcher::serving(publisher_descriptor(domain, profile));
    let refresher = PublisherProfileRefresher::with_defaults(fetcher.clone());
    let (a, b, c) = tokio::join!(
        refresher.ensure_current(domain),
        refresher.ensure_current(domain),
        refresher.ensure_current(domain),
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(fetcher.calls(), 1);
}

/// A successful 24-hour cache entry must stop authorizing registrations at
/// the signed expiry, including while a retry throttle prevents another fetch.
#[tokio::test]
#[serial]
async fn test_publisher_profile_cache_stops_at_signed_expiry() {
    use ed25519_dalek::SigningKey;
    for mode in ["expired", "unreachable", "throttled", "refreshed"] {
        let mut env = setup_test_environment().await;
        let realm = format!("expiry-{mode}@example.com");
        seed_realm(&realm, Some(ROUTE_KEY_LABEL)).await;
        let domain = format!("expiry-{mode}.example");
        let root = SigningKey::from_bytes(&[81; 32]);
        let package = SigningKey::from_bytes(&[82; 32]);
        let expires = chrono::Utc::now() + chrono::Duration::seconds(2);
        let mut profile = signed_publisher_profile(&domain, 1, &root, &[(&package, "active")]);
        profile["profile_expires_at"] = serde_json::json!(expires.to_rfc3339());
        profile.as_object_mut().unwrap().remove("signature");
        actr_protocol::realm_profile::sign_document(&mut profile, "root-1", &root);
        register_publisher_from_profile(&domain, &profile).await;
        let fetcher = StubFetcher::serving(publisher_descriptor(&domain, profile));
        let refresher = Arc::new(PublisherProfileRefresher::new(
            fetcher.clone(),
            Duration::from_secs(24 * 3600),
            if mode == "throttled" {
                Duration::from_secs(60)
            } else {
                Duration::ZERO
            },
        ));
        refresher.ensure_current(&domain).await.unwrap();
        refresher.ensure_current(&domain).await.unwrap();
        assert_eq!(
            fetcher.calls(),
            1,
            "a still-valid document should be cached"
        );
        if mode == "unreachable" {
            *fetcher.response.lock().unwrap() = Err("publisher unavailable".into());
        } else if mode != "expired" {
            let fresh = signed_publisher_profile(&domain, 2, &root, &[(&package, "active")]);
            fetcher.serve(publisher_descriptor(&domain, fresh));
        }
        let issuer = create_test_issuer(&env)
            .await
            .with_publisher_profiles(refresher);
        // Wait for the signed deadline, rather than the 24-hour cache TTL.
        tokio::time::sleep(
            (expires - chrono::Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(20),
        )
        .await;
        let request =
            signed_package_request(&domain, "notes", "1.0.0", &realm, "wasm32-wasip1", &package);
        let response = issuer
            .issue_credential(&request, &owner(&realm).await)
            .await
            .unwrap();
        if mode == "refreshed" {
            assert!(
                matches!(response.result, Some(register_response::Result::Success(_))),
                "{response:?}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                Some(500),
                "expired authorization accepted in {mode}: {response:?}"
            );
        }
        assert_eq!(fetcher.calls(), if mode == "throttled" { 1 } else { 2 });
        env.shutdown_signer().await;
    }
}

#[tokio::test]
#[serial]
async fn relay_enrollment_requires_bound_client_and_private_renewal_proof() {
    let mut env = setup_test_environment().await;
    let app = create_test_router(&env).await;
    let registration = post_register(
        app.clone(),
        linked_register_request(),
        Some(DEFAULT_KEY_LABEL),
    )
    .await;
    let register_response::Result::Success(ok) = registration.result.unwrap() else {
        panic!("registration failed");
    };
    let identity_key = actr_relay_security::IdentityKey::generate().unwrap();
    let enrollment = actr_relay_security::IdentityRequest {
        endpoint: ok.actr_id.to_string_repr(),
        renewal_token: ok.renewal_token.clone().unwrap().to_vec(),
        public_key: identity_key.public,
        admission_commitment: identity_key.admission_commitment(),
    };
    async fn request(
        app: Router,
        enrollment: &actr_relay_security::IdentityRequest,
        label: Option<&str>,
    ) -> (StatusCode, Vec<u8>) {
        let body = serde_json::to_vec(enrollment).unwrap();
        let mut builder = Request::builder()
            .method("POST")
            .uri("/relay-identity")
            .header("content-type", "application/json")
            .header("x-real-ip", "127.0.0.1");
        if let Some(label) = label {
            builder = builder.header(
                CLIENT_PROOF_HEADER,
                proof(ProofOp::RelayIdentity, TEST_REALM, &body, &key_for(label)),
            );
        }
        let mut request = builder.body(Body::from(body)).unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                12345,
            ))));
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        (
            status,
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
    }
    assert_eq!(
        request(app.clone(), &enrollment, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(app.clone(), &enrollment, Some("unregistered-key"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let invalid = actr_relay_security::IdentityRequest {
        endpoint: enrollment.endpoint.clone(),
        renewal_token: vec![9; 32],
        public_key: enrollment.public_key,
        admission_commitment: enrollment.admission_commitment,
    };
    assert_eq!(
        request(app.clone(), &invalid, Some(DEFAULT_KEY_LABEL))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let invalid_commitment = actr_relay_security::IdentityRequest {
        endpoint: enrollment.endpoint.clone(),
        renewal_token: enrollment.renewal_token.clone(),
        public_key: enrollment.public_key,
        admission_commitment: [0; 32],
    };
    assert_eq!(
        request(app.clone(), &invalid_commitment, Some(DEFAULT_KEY_LABEL))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, body) = request(app.clone(), &enrollment, Some(DEFAULT_KEY_LABEL)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let issued: actr_relay_security::IssuedIdentity = serde_json::from_slice(&body).unwrap();
    let claims: actr_relay_security::IdentityClaims =
        serde_json::from_slice(&issued.certificate.claims).unwrap();
    assert_eq!(claims.actor_type, registered_type(&ok).to_string_repr());
    assert_eq!(
        claims.client_key,
        client_key_id(&key_for(DEFAULT_KEY_LABEL).verifying_key().to_bytes())
    );
    identity_key
        .certify(
            issued,
            &enrollment.endpoint,
            issuer::renewal::now_secs() as u64,
        )
        .unwrap();
    sqlx::query("UPDATE realm SET enabled = 0 WHERE name = ?")
        .bind(TEST_REALM)
        .execute(platform::storage::db::get_database().get_pool())
        .await
        .unwrap();
    assert_eq!(
        request(app, &enrollment, Some(DEFAULT_KEY_LABEL)).await.0,
        StatusCode::FORBIDDEN
    );
    env.shutdown_signer().await;
}
