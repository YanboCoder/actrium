//! Isolated regressions for Realm issuer keys.
//!
//! This is a separate integration-test binary so its temporary Signer and key
//! cache do not share the process-global validator/database state used by the
//! broader Issuer HTTP integration suite.

use actr_protocol::{ActrId, RegisterAuthMode};
use issuer::{
    issuer::{AIdIssuer, IssuerConfig},
    signer_client_wrapper::create_signer_client,
};
use nonce_auth::storage::MemoryStorage;
use platform::{
    aid::{KeyCache, credential::AIdCredentialVerifier},
    config::signer::SignerClientConfig,
};
use signer::{GrpcClient, GrpcClientConfig, KeyStorage, SignerServiceConfig, create_grpc_service};
use std::{net::TcpListener, path::Path, time::Duration};
use tempfile::TempDir;
use tokio::{sync::oneshot, task::JoinHandle};
use tonic::transport::Server;

const TEST_STEP_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_REALM: &str = "realm-keys@example.com";

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
            let mut handle = handle;
            if tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
    }
}

async fn start_embedded_signer(
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
    .expect("create Signer storage");

    let addr = match fixed_addr {
        Some(addr) => addr,
        None => {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
            let addr = listener.local_addr().expect("read ephemeral port");
            drop(listener);
            addr
        }
    };
    let service = create_grpc_service(storage, MemoryStorage::new(), shared_key.to_string());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("embedded Signer server");
    });

    let endpoint = format!("http://{addr}");
    for _ in 0..40 {
        let config = GrpcClientConfig {
            endpoint: endpoint.clone(),
            loran_shared_key: shared_key.to_string(),
            timeout_seconds: 2,
            enable_tls: false,
            tls_domain: None,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        if let Ok(mut client) = GrpcClient::new(&config).await {
            if matches!(client.health_check().await, Ok(status) if status == "healthy") {
                return (endpoint, handle, shutdown_tx);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("embedded Signer did not become healthy in time");
}

async fn setup_test_environment(signer_config: SignerServiceConfig) -> TestEnv {
    use std::sync::OnceLock;
    static DB_DIR: OnceLock<TempDir> = OnceLock::new();
    static DB_INIT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let issuer_temp_dir = TempDir::new().expect("create issuer temp dir");
    let signer_temp_dir = TempDir::new().expect("create Signer temp dir");
    let shared_key = "test-shared-key".to_string();
    let (endpoint, signer_handle, signer_shutdown_tx) =
        start_embedded_signer(&shared_key, signer_temp_dir.path(), signer_config, None).await;

    // Issue-time rechecks (AIS-03) hit the global platform database, so it must
    // be initialized and hold the realm the test actors belong to. The OnceLock
    // keeps the SQLite file alive for the whole process; the mutex serializes
    // the concurrent tests' first-time initialization.
    {
        let _guard = DB_INIT_LOCK.lock().await;
        let db_dir = DB_DIR.get_or_init(|| TempDir::new().expect("create DB temp dir"));
        if !platform::storage::db::is_database_initialized() {
            platform::storage::db::set_db_path(db_dir.path())
                .await
                .expect("Failed to initialize test database");
        }
        let pool = platform::storage::db::get_database().get_pool();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query(
            "INSERT OR IGNORE INTO realm (name, status, enabled, created_at)
             VALUES (?, 'active', 1, ?)",
        )
        .bind(TEST_REALM)
        .bind(now)
        .execute(pool)
        .await
        .expect("seed realm");
        // The owner and its Realm client key, which the Actor is bound to.
        sqlx::query(
            "INSERT OR IGNORE INTO realm_member (realm, member_id, role, status, created_at)
             VALUES (?, ?, 'owner', 'active', ?)",
        )
        .bind(TEST_REALM)
        .bind(&owner().member_id)
        .bind(now)
        .execute(pool)
        .await
        .expect("seed member");
        sqlx::query(
            "INSERT OR IGNORE INTO realm_client_key
                (realm, key_id, public_key, member_id, registered_at)
             VALUES (?, ?, zeroblob(32), ?, ?)",
        )
        .bind(TEST_REALM)
        .bind(&owner().key_id)
        .bind(&owner().member_id)
        .bind(now)
        .execute(pool)
        .await
        .expect("seed client key");
        // The identity ledger records the Actor type that renewals sign into claims.
        sqlx::query(
            "INSERT OR IGNORE INTO issued_actor_identity
             (realm, serial_number, actor_type, target, issued_at, auth_mode,
              member_id, client_key_id)
             VALUES (?, 42, 'linked-src/source-workload@1.0.0', NULL, ?, ?, ?, ?)",
        )
        .bind(TEST_REALM)
        .bind(now)
        .bind(RegisterAuthMode::Linked as i32)
        .bind(&owner().member_id)
        .bind(&owner().key_id)
        .execute(pool)
        .await
        .expect("seed actor identity");
    }

    TestEnv {
        issuer_temp_dir,
        _signer_temp_dir: signer_temp_dir,
        signer_handle: Some(signer_handle),
        signer_shutdown_tx: Some(signer_shutdown_tx),
        signer_config: SignerClientConfig {
            endpoint,
            timeout_seconds: 10,
            enable_tls: false,
            tls_domain: None,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        },
        shared_key,
    }
}

fn issuer_config(temp_dir: &TempDir) -> IssuerConfig {
    IssuerConfig {
        worker_id: 0,
        token_ttl_secs: 3600,
        renewal_token_ttl_secs: 24 * 60 * 60,
        renewal_rotation_window_secs: 60 * 60,
        renewal_token_secret: vec![0; 32],
        signaling_heartbeat_interval_secs: 30,
        turn_secret: "test-turn-secret-0123456789abcdef".to_string(),
        provider_id: "provider.example".to_string(),
        sqlite_path: temp_dir.path().to_path_buf(),
    }
}

async fn create_issuer(env: &TestEnv, cancel: tokio_util::sync::CancellationToken) -> AIdIssuer {
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("create Signer client");
    AIdIssuer::new(signer_client, issuer_config(&env.issuer_temp_dir), cancel)
        .await
        .expect("create issuer")
}

#[tokio::test]
async fn credentials_are_signed_with_the_realm_issuer_key() {
    let mut env = setup_test_environment(SignerServiceConfig::default()).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let issuer = tokio::time::timeout(TEST_STEP_TIMEOUT, create_issuer(&env, cancel.clone()))
        .await
        .expect("issuer creation timed out");

    let actor_id = ActrId {
        realm: TEST_REALM.to_string(),
        serial_number: 42,
    };
    let issued = tokio::time::timeout(
        TEST_STEP_TIMEOUT,
        issuer.issue_credential_for_actor(&actor_id, &owner()),
    )
    .await
    .expect("credential issuance timed out")
    .expect("issue credential");
    let (realm_key_id, realm_key) = issuer.realm_key(TEST_REALM).await.expect("realm key");
    assert_eq!(issued.credential.key_id, realm_key_id);
    assert_eq!(issued.signing_key_id, realm_key_id);
    assert_eq!(issued.signing_pubkey.as_ref(), realm_key.as_bytes());

    // The key is cached for its Realm only.
    let cache = KeyCache::new(env.issuer_temp_dir.path().join("signaling_key_cache.db"))
        .await
        .expect("open signaling cache");
    let (verifying_key, expires_at) = cache
        .get_cached_key(TEST_REALM, realm_key_id)
        .await
        .expect("read signaling cache")
        .expect("Realm key persisted to signaling cache");
    assert_eq!(expires_at, 0, "a Realm issuer key does not expire");
    assert!(
        cache
            .get_cached_key("other@example.com", realm_key_id)
            .await
            .expect("read signaling cache")
            .is_none()
    );
    let claims = AIdCredentialVerifier::verify(&issued.credential, &verifying_key)
        .expect("fresh credential verifies");
    assert_eq!(claims.realm, TEST_REALM);
    assert_eq!(
        claims.actor_type.to_string_repr(),
        "linked-src/source-workload@1.0.0"
    );

    // After a rotation switches keys, credentials carry the new key id and
    // both keys verify under the Realm.
    let signer = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("create Signer client");
    let name = format!("issuer:{TEST_REALM}/next");
    let (next_id, next_public) = signer.ensure_named_key(&name).await.expect("next key");
    let now = chrono::Utc::now().timestamp();
    platform::realm::RealmIssuerKey {
        realm: TEST_REALM.to_string(),
        key_id: next_id,
        signer_name: name,
        public_key: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, next_public),
        state: platform::realm::IssuerKeyState::Next,
        created_at: now,
        published_at: Some(now),
        current_since: None,
        remove_after: None,
    }
    .insert()
    .await
    .expect("insert next key");
    // Activation requires an actually accepted Profile listing both keys.
    let root = ed25519_dalek::SigningKey::from_bytes(&[83; 32]);
    let root_public = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        root.verifying_key().as_bytes(),
    );
    let mut roots = serde_json::json!({ "sequence": 1, "keys": [{
        "id": "root", "algorithm": "Ed25519", "status": "active", "public_key": root_public,
    }] });
    actr_protocol::realm_profile::sign_document(&mut roots, "root", &root);
    let roots: actr_protocol::RealmSigningKeys = serde_json::from_value(roots).unwrap();
    let public =
        |bytes: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    let mut document = serde_json::json!({
        "schema": "actr.realm/v1", "sequence": 1,
        "realm": { "name": TEST_REALM, "display_name": "Keys", "status": "active" },
        "package": { "href": "https://example.com/keys.actr", "sha256": "ab".repeat(32), "type": "example.com/source-workload@1.0.0" },
        "issuer": { "url": "https://provider.example/issuer", "keys": [
            { "id": realm_key_id.to_string(), "algorithm": "Ed25519", "status": "active", "public_key": public(realm_key.as_bytes()) },
            { "id": next_id.to_string(), "algorithm": "Ed25519", "status": "active", "public_key": public(&next_public) },
        ] },
        "provider": { "id": "provider.example", "signaling": [], "ice": [] },
        "policy": { "membership": "invite-only" }, "issued_at": chrono::Utc::now().to_rfc3339(),
    });
    actr_protocol::realm_profile::sign_document(&mut document, "root", &root);
    let document = serde_json::to_string(&document).unwrap();
    let profile =
        actr_protocol::realm_profile::RealmProfile::verify(document.as_bytes(), TEST_REALM, &roots)
            .unwrap();
    platform::realm::RealmProfileRecord::store(&profile, &document, None, None, None)
        .await
        .unwrap();
    let next = platform::realm::RealmIssuerKey::list(TEST_REALM)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.key_id == next_id)
        .unwrap();
    let switch_at = next.published_at.unwrap() + issuer::realm_profile::PREPUBLISH_SECS;
    next.switch(switch_at, switch_at + 3900)
        .await
        .expect("switch keys");
    let rotated = issuer
        .issue_credential_for_actor(&actor_id, &owner())
        .await
        .expect("issue credential after rotation");
    assert_eq!(rotated.credential.key_id, next_id);
    assert_ne!(next_id, realm_key_id);
    let (next_key, _) = cache
        .get_cached_key(TEST_REALM, next_id)
        .await
        .expect("read signaling cache")
        .expect("new key cached");
    AIdCredentialVerifier::verify(&rotated.credential, &next_key).expect("new credential verifies");

    drop(issuer);
    env.shutdown_signer().await;
}

#[tokio::test]
async fn retiring_a_realm_retires_its_issuer_key() {
    const REALM: &str = "retiring@example.com";
    let mut env = setup_test_environment(SignerServiceConfig::default()).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO realm (name, status, enabled, created_at)
         VALUES (?, 'active', 1, ?)",
    )
    .bind(REALM)
    .bind(now)
    .execute(platform::storage::db::get_database().get_pool())
    .await
    .expect("seed realm");

    let cancel = tokio_util::sync::CancellationToken::new();
    let issuer = create_issuer(&env, cancel.clone()).await;
    let (key_id, _) = issuer.realm_key(REALM).await.expect("realm key");
    let signer = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("create Signer client");
    signer
        .sign(key_id, b"before")
        .await
        .expect("signs while active");
    // A key being rotated in is retired with the Realm too.
    let name = format!("issuer:{REALM}/next");
    let (next_id, next_public) = signer.ensure_named_key(&name).await.expect("next key");
    platform::realm::RealmIssuerKey {
        realm: REALM.to_string(),
        key_id: next_id,
        signer_name: name,
        public_key: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, next_public),
        state: platform::realm::IssuerKeyState::Next,
        created_at: now,
        published_at: None,
        current_since: None,
        remove_after: None,
    }
    .insert()
    .await
    .expect("insert next key");

    platform::realm::Realm::retire(REALM)
        .await
        .expect("retire realm");
    let mut refused = false;
    for _ in 0..100 {
        if signer.sign(key_id, b"after").await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(refused, "the retired Realm's issuer key still signs");
    let mut refused = false;
    for _ in 0..100 {
        if signer.sign(next_id, b"after").await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(refused, "the retired Realm's next issuer key still signs");
    // The Realm's key stays retired even if asked for again.
    let (again, _) = issuer.realm_key(REALM).await.expect("realm key");
    assert_eq!(again, key_id);
    assert!(signer.sign(again, b"again").await.is_err());

    cancel.cancel();
    drop(issuer);
    env.shutdown_signer().await;
}

/// A rotation that loses the race for the Realm's single next key, or that
/// races the Realm's retirement, retires the key it created in the Signer.
#[tokio::test]
async fn an_issuer_key_that_loses_a_race_is_retired() {
    const REALM: &str = "rotation-race@example.com";
    let mut env = setup_test_environment(SignerServiceConfig::default()).await;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO realm (name, status, enabled, created_at)
         VALUES (?, 'active', 1, ?)",
    )
    .bind(REALM)
    .bind(now)
    .execute(platform::storage::db::get_database().get_pool())
    .await
    .expect("seed realm");
    let signer = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .expect("create Signer client");
    let next_key = |name: String, key_id: u32, public: [u8; 32]| platform::realm::RealmIssuerKey {
        realm: REALM.to_string(),
        key_id,
        signer_name: name,
        public_key: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, public),
        state: platform::realm::IssuerKeyState::Next,
        created_at: now,
        published_at: None,
        current_since: None,
        remove_after: None,
    };

    let mut keys = Vec::new();
    for n in 0..3 {
        let name = format!("issuer:{REALM}/{n}");
        let (key_id, public) = signer.ensure_named_key(&name).await.expect("new key");
        keys.push(next_key(name, key_id, public));
    }
    issuer::realm_profile::list_issuer_key(&signer, &keys[0])
        .await
        .expect("the first next key is listed");
    signer
        .sign(keys[0].key_id, b"m")
        .await
        .expect("listed key signs");

    assert!(
        issuer::realm_profile::list_issuer_key(&signer, &keys[1])
            .await
            .is_err()
    );
    assert!(signer.sign(keys[1].key_id, b"m").await.is_err());

    // Two rotations in the same millisecond get the same Signer key; the
    // loser must not retire the key the winner listed.
    assert!(
        issuer::realm_profile::list_issuer_key(&signer, &keys[0])
            .await
            .is_err()
    );
    signer
        .sign(keys[0].key_id, b"m")
        .await
        .expect("the listed key still signs");

    platform::realm::Realm::retire(REALM)
        .await
        .expect("retire realm");
    sqlx::query("DELETE FROM realm_issuer_key WHERE realm = ?")
        .bind(REALM)
        .execute(platform::storage::db::get_database().get_pool())
        .await
        .expect("drop the listed next key");
    assert!(
        issuer::realm_profile::list_issuer_key(&signer, &keys[2])
            .await
            .is_err()
    );
    assert!(signer.sign(keys[2].key_id, b"m").await.is_err());

    env.shutdown_signer().await;
}

/// The owner of `TEST_REALM` the seeded Actor is bound to.
fn owner() -> platform::realm::membership::Signer {
    platform::realm::membership::Signer {
        realm: TEST_REALM.to_string(),
        key_id: "0123456789abcdef".to_string(),
        member_id: "fedcba9876543210".to_string(),
        role: actr_protocol::realm_membership::Role::Owner,
    }
}
