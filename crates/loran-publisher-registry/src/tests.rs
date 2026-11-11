/// Publisher Registry module integration tests
///
/// All tests requiring a database use an in-process SQLite in-memory pool, each test has its own independent pool.
use sqlx::SqlitePool;

use crate::{
    PublisherRegistryError, crypto,
    manager::{PackageSigningKey, PublisherCertificate, PublisherRegistryManager},
    model::{Publisher, PublisherStatus},
};

#[test]
fn public_json_contract_uses_publisher_terminology() {
    let certificate = serde_json::to_value(PublisherCertificate {
        key_id: "publisher-key".to_string(),
        publisher: "apps.example.com".to_string(),
        public_key: "base64-key".to_string(),
        issued_at: 1,
        expires_at: 2,
    })
    .unwrap();
    assert_eq!(certificate["publisher"], "apps.example.com");
    assert_eq!(certificate["public_key"], "base64-key");
    assert!(certificate.get("mfr_name").is_none());
    assert!(certificate.get("mfr_pubkey").is_none());
}

// ─── Test helpers ────────────────────────────────────────────────────────────

async fn setup_test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .expect("failed to create in-memory sqlite pool");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS publisher (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            public_key TEXT NOT NULL DEFAULT '',
            key_id TEXT NOT NULL DEFAULT '',
            profile_json TEXT NOT NULL DEFAULT '{}',
            contact TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at INTEGER NOT NULL,
            updated_at INTEGER,
            verified_at INTEGER,
            suspended_at INTEGER,
            revoked_at INTEGER,
            key_expires_at INTEGER,
            next_public_key TEXT,
            next_key_id TEXT,
            next_key_expires_at INTEGER,
            profile_pins TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("failed to create publisher table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS publisher_key_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            publisher_id INTEGER NOT NULL REFERENCES publisher(id),
            key_id TEXT NOT NULL,
            public_key TEXT NOT NULL,
            fingerprint TEXT,
            status TEXT NOT NULL DEFAULT 'Active',
            created_at INTEGER NOT NULL,
            retired_at INTEGER NOT NULL DEFAULT 0,
            revoked_at INTEGER
        )",
    )
    .execute(&pool)
    .await
    .expect("failed to create publisher_key_history table");

    pool
}

fn test_manifest(publisher: &str, name: &str, version: &str, target: &str) -> String {
    format!(
        "publisher = \"{publisher}\"\nname = \"{name}\"\nversion = \"{version}\"\n\n[binary]\npath = \"bin/actor.wasm\"\ntarget = \"{target}\"\nhash = \"sha256:abc123\"\n"
    )
}

fn valid_public_key() -> String {
    let (_, public_key) = crypto::generate_keypair();
    public_key
}

/// A pending publisher record, created without registration.
async fn pending(
    manager: &PublisherRegistryManager,
    name: &str,
) -> Result<(Publisher, ()), PublisherRegistryError> {
    let name = crate::reserved::canonical_publisher_domain(name)?;
    Ok((Publisher::create(manager.pool(), &name, None).await?, ()))
}

fn b64_key(key: &ed25519_dalek::SigningKey) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
}

/// A Publisher Profile of `domain` listing `roots` (signed by the first)
/// and `packages` as package signing keys.
fn signed_profile(
    domain: &str,
    sequence: u64,
    roots: &[(&str, &ed25519_dalek::SigningKey, &str)],
    packages: &[(&ed25519_dalek::SigningKey, &str)],
) -> actr_protocol::PublisherProfile {
    let key = |id: String, key: &ed25519_dalek::SigningKey, status: &str| {
        serde_json::json!({
            "id": id,
            "algorithm": "Ed25519",
            "public_key": b64_key(key),
            "status": status,
        })
    };
    let now = chrono::Utc::now();
    let mut document = serde_json::json!({
        "schema": "actr.publisher/v1",
        "sequence": sequence,
        "domain": domain,
        "profile_signing_keys": roots
            .iter()
            .map(|(id, root, status)| key(id.to_string(), root, status))
            .collect::<Vec<_>>(),
        "keys": packages
            .iter()
            .map(|(package, status)| key(crypto::compute_key_id(package.verifying_key().as_bytes()), package, status))
            .collect::<Vec<_>>(),
        "security_contact": format!("mailto:security@{domain}"),
        "profile_issued_at": (now - chrono::Duration::hours(1)).to_rfc3339(),
        "profile_expires_at": (now + chrono::Duration::days(30)).to_rfc3339(),
    });
    actr_protocol::realm_profile::sign_document(&mut document, roots[0].0, roots[0].1);
    serde_json::from_value(document).unwrap()
}

/// Verify `profile` as the `register_publisher` path does and register it.
async fn register(
    manager: &PublisherRegistryManager,
    profile: &actr_protocol::PublisherProfile,
) -> Result<Publisher, PublisherRegistryError> {
    let keys = profile
        .verify(&profile.domain, chrono::Utc::now())
        .expect("valid Publisher Profile");
    manager
        .register_publisher(&profile.domain, profile, &keys)
        .await
}

// ─── crypto.rs pure unit tests ───────────────────────────────────────────────

#[test]
fn test_generate_keypair_roundtrip() {
    use base64::Engine as _;

    let (private_b64, public_b64) = crypto::generate_keypair();
    assert!(!private_b64.is_empty());
    assert!(!public_b64.is_empty());

    let priv_bytes = base64::engine::general_purpose::STANDARD
        .decode(&private_b64)
        .expect("private key should be valid base64");
    assert_eq!(
        priv_bytes.len(),
        32,
        "Ed25519 private key should be 32 bytes"
    );

    let pub_bytes = base64::engine::general_purpose::STANDARD
        .decode(&public_b64)
        .expect("public key should be valid base64");
    assert_eq!(pub_bytes.len(), 32, "Ed25519 public key should be 32 bytes");
}

#[test]
fn test_generate_keypair_unique() {
    let (priv1, pub1) = crypto::generate_keypair();
    let (priv2, pub2) = crypto::generate_keypair();
    assert_ne!(priv1, priv2);
    assert_ne!(pub1, pub2);
}

#[test]
fn test_verify_signature_valid() {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let message = b"hello publisher";
    let sig = signing_key.sign(message);

    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let pub_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes());

    let result = crypto::verify_signature(message, &sig_b64, &pub_b64)
        .expect("verify_signature should not error on valid inputs");
    assert!(result, "valid signature should verify as true");
}

#[test]
fn test_verify_signature_invalid_zeros() {
    use base64::Engine as _;
    let (_, pub_b64) = crypto::generate_keypair();
    let bad_sig = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
    let result = crypto::verify_signature(b"message", &bad_sig, &pub_b64);
    assert!(
        matches!(result, Ok(false) | Err(PublisherRegistryError::Crypto(_))),
        "all-zero signature should fail: {result:?}"
    );
}

#[test]
fn test_verify_signature_wrong_key() {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    let key1 = SigningKey::generate(&mut OsRng);
    let key2 = SigningKey::generate(&mut OsRng);
    let message = b"test message";
    let sig = key1.sign(message);

    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let wrong_pub_b64 =
        base64::engine::general_purpose::STANDARD.encode(key2.verifying_key().to_bytes());

    let result = crypto::verify_signature(message, &sig_b64, &wrong_pub_b64)
        .expect("should not return error for valid encoding");
    assert!(!result, "signature with wrong key should verify as false");
}

#[test]
fn test_verify_signature_bad_pubkey_encoding() {
    let bad_pub = "not-valid-base64!!!";
    let result = crypto::verify_signature(b"msg", "anysig", bad_pub);
    assert!(
        matches!(result, Err(PublisherRegistryError::Crypto(_))),
        "bad base64 pubkey should return Crypto error"
    );
}

// ─── model/publisher.rs tests (require DB) ────────────────────────────────

#[tokio::test]
async fn test_publisher_create_and_get() {
    let pool = setup_test_pool().await;
    let record = Publisher::create(&pool, "testco", Some("admin@testco.com"))
        .await
        .expect("create should succeed");

    assert_eq!(record.name, "testco");
    assert_eq!(record.status, PublisherStatus::Pending);
    assert!(record.verified_at.is_none());
    assert_eq!(record.contact.as_deref(), Some("admin@testco.com"));

    let found = Publisher::get(&pool, record.id)
        .await
        .expect("get should succeed")
        .expect("should find created publisher");
    assert_eq!(found.name, "testco");
    assert_eq!(found.id, record.id);
}

#[tokio::test]
async fn test_publisher_get_nonexistent() {
    let pool = setup_test_pool().await;
    let found = Publisher::get(&pool, 9999).await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_publisher_get_by_name() {
    let pool = setup_test_pool().await;
    let record = Publisher::create(&pool, "namedco", None).await.unwrap();

    let found = Publisher::get_by_name(&pool, "namedco")
        .await
        .unwrap()
        .expect("should find by name");
    assert_eq!(found.id, record.id);

    let missing = Publisher::get_by_name(&pool, "nobody").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_publisher_duplicate_name() {
    let pool = setup_test_pool().await;
    Publisher::create(&pool, "dupco", None).await.unwrap();
    let result = Publisher::create(&pool, "dupco", None).await;
    assert!(
        matches!(result, Err(PublisherRegistryError::AlreadyExists(_))),
        "duplicate name should return AlreadyExists"
    );
}

#[tokio::test]
async fn test_publisher_activate() {
    let pool = setup_test_pool().await;
    let mut record = Publisher::create(&pool, "activeco", None).await.unwrap();
    let public_key = valid_public_key();

    record
        .activate(&pool, public_key.clone())
        .await
        .expect("activate from pending should succeed");

    assert_eq!(record.status, PublisherStatus::Active);
    assert_eq!(record.public_key, public_key);
    assert!(record.verified_at.is_some());
    assert!(record.updated_at.is_some());

    let from_db = Publisher::get(&pool, record.id).await.unwrap().unwrap();
    assert_eq!(from_db.status, PublisherStatus::Active);
    assert_eq!(from_db.public_key, public_key);
}

#[tokio::test]
async fn test_publisher_lifecycle_full() {
    let pool = setup_test_pool().await;
    let mut record = Publisher::create(&pool, "lifecycle", None).await.unwrap();

    record.activate(&pool, valid_public_key()).await.unwrap();
    assert_eq!(record.status, PublisherStatus::Active);

    record.suspend(&pool).await.unwrap();
    assert_eq!(record.status, PublisherStatus::Suspended);
    assert!(record.suspended_at.is_some());

    record.reinstate(&pool).await.unwrap();
    assert_eq!(record.status, PublisherStatus::Active);

    record.revoke(&pool).await.unwrap();
    assert_eq!(record.status, PublisherStatus::Revoked);
    assert!(record.revoked_at.is_some());
}

#[tokio::test]
async fn test_publisher_invalid_transitions_from_pending() {
    let pool = setup_test_pool().await;
    let mut record = Publisher::create(&pool, "transco", None).await.unwrap();

    let err = record.suspend(&pool).await.unwrap_err();
    assert!(matches!(err, PublisherRegistryError::InvalidStatus(_)));

    let err = record.reinstate(&pool).await.unwrap_err();
    assert!(matches!(err, PublisherRegistryError::InvalidStatus(_)));
}

#[tokio::test]
async fn test_publisher_cannot_activate_twice() {
    let pool = setup_test_pool().await;
    let mut record = Publisher::create(&pool, "twiceco", None).await.unwrap();

    record.activate(&pool, "key1".to_string()).await.unwrap();
    let err = record
        .activate(&pool, "key2".to_string())
        .await
        .unwrap_err();
    assert!(matches!(err, PublisherRegistryError::InvalidStatus(_)));
}

#[tokio::test]
async fn test_publisher_list_all() {
    let pool = setup_test_pool().await;
    Publisher::create(&pool, "list1", None).await.unwrap();
    Publisher::create(&pool, "list2", None).await.unwrap();

    let all = Publisher::list(&pool, None).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_publisher_list_by_status() {
    let pool = setup_test_pool().await;
    Publisher::create(&pool, "statuslist1", None).await.unwrap();
    let mut publisher2 = Publisher::create(&pool, "statuslist2", None).await.unwrap();
    publisher2
        .activate(&pool, valid_public_key())
        .await
        .unwrap();

    let active = Publisher::list(&pool, Some(PublisherStatus::Active))
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "statuslist2");

    let pending = Publisher::list(&pool, Some(PublisherStatus::Pending))
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "statuslist1");
}

#[tokio::test]
async fn test_publisher_delete() {
    let pool = setup_test_pool().await;
    let record = Publisher::create(&pool, "delco", None).await.unwrap();
    let id = record.id;

    Publisher::delete(&pool, id).await.unwrap();

    let found = Publisher::get(&pool, id).await.unwrap();
    assert!(found.is_none());
}

// ─── model/package.rs tests (require DB) ─────────────────────────────────────

// ─── manager.rs tests (require DB) ────────────────────────────────────────────

#[tokio::test]
async fn test_formerly_reserved_publisher_names_accepted() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    // Previously reserved names are now accepted
    let result = pending(&manager, "acme.example").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_publisher_name_must_be_a_domain() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    // Underscore is not allowed in a DNS name
    let result = pending(&manager, "my_company").await;
    assert!(matches!(
        result,
        Err(PublisherRegistryError::InvalidName(_))
    ));
}

#[tokio::test]
async fn test_manager_get_status() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "statusco.example").await.unwrap();

    let status = manager.get_status(record.id).await.unwrap();
    assert_eq!(status.name, "statusco.example");
    assert_eq!(status.status, PublisherStatus::Pending);
}

#[tokio::test]
async fn test_manager_get_status_not_found() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let result = manager.get_status(9999).await;
    assert!(matches!(result, Err(PublisherRegistryError::NotFound)));
}

#[tokio::test]
async fn test_manager_admin_approve() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "approveco.example").await.unwrap();

    let response = manager.admin_approve(record.id, None).await.unwrap();
    assert_eq!(response.certificate.publisher, "approveco.example");
    assert!(!response.certificate.public_key.is_empty());
    assert!(response.certificate.expires_at > response.certificate.issued_at);
}

#[tokio::test]
async fn test_manager_admin_suspend_reinstate() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "suspco.example").await.unwrap();
    manager.admin_approve(record.id, None).await.unwrap();

    manager.admin_suspend(record.id).await.unwrap();
    let status = manager.get_status(record.id).await.unwrap();
    assert_eq!(status.status, PublisherStatus::Suspended);

    manager.admin_reinstate(record.id).await.unwrap();
    let status = manager.get_status(record.id).await.unwrap();
    assert_eq!(status.status, PublisherStatus::Active);
}

#[tokio::test]
async fn test_manager_admin_delete() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "deleteco.example").await.unwrap();
    let id = record.id;

    manager.admin_delete(id).await.unwrap();
    let result = manager.get_status(id).await;
    assert!(matches!(result, Err(PublisherRegistryError::NotFound)));
}

#[tokio::test]
async fn test_manager_resolve_by_name() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "resolveco.example").await.unwrap();
    manager.admin_approve(record.id, None).await.unwrap();

    let info = manager.resolve_by_name("resolveco.example").await.unwrap();
    assert_eq!(info.name, "resolveco.example");
    assert!(!info.public_key.is_empty());
}

#[tokio::test]
async fn test_manager_resolve_by_name_not_active() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    pending(&manager, "pendingres.example").await.unwrap();

    let result = manager.resolve_by_name("pendingres.example").await;
    assert!(matches!(
        result,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));
}

#[tokio::test]
async fn test_manager_admin_list() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);

    pending(&manager, "adminlist1.example").await.unwrap();
    let (publisher2, _) = pending(&manager, "adminlist2.example").await.unwrap();
    manager.admin_approve(publisher2.id, None).await.unwrap();

    let all = manager.admin_list(None).await.unwrap();
    assert_eq!(all.len(), 2);

    let active = manager
        .admin_list(Some(PublisherStatus::Active))
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "adminlist2.example");
}

// ─── dual key mode tests ──────────────────────────────────────────────────────

#[test]
fn test_validate_public_key_valid() {
    let (_, pub_b64) = crypto::generate_keypair();
    assert!(crypto::validate_public_key(&pub_b64).is_ok());
}

#[test]
fn test_validate_public_key_bad_base64() {
    let result = crypto::validate_public_key("not-valid-base64!!!");
    assert!(
        matches!(result, Err(PublisherRegistryError::Crypto(_))),
        "bad base64 should return Crypto error"
    );
}

#[test]
fn test_validate_public_key_wrong_length() {
    use base64::Engine as _;
    let short = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
    let result = crypto::validate_public_key(&short);
    assert!(
        matches!(result, Err(PublisherRegistryError::Crypto(_))),
        "16-byte key should be rejected"
    );
}

#[test]
fn test_validate_public_key_all_zeros() {
    use base64::Engine as _;
    let zeros = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
    // All-zero bytes may or may not be a valid curve point depending on the library.
    // We just verify it does not panic — either Ok or Err(Crypto) is acceptable.
    let result = crypto::validate_public_key(&zeros);
    assert!(
        result.is_ok() || matches!(result, Err(PublisherRegistryError::Crypto(_))),
        "all-zero key should return Ok or Crypto error, not panic"
    );
}

#[tokio::test]
async fn test_manager_admin_approve_with_uploaded_key() {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "uploadco.example").await.unwrap();

    // User generates their own keypair
    let user_key = SigningKey::generate(&mut OsRng);
    let user_pub_b64 =
        base64::engine::general_purpose::STANDARD.encode(user_key.verifying_key().to_bytes());

    let response = manager
        .admin_approve(record.id, Some(&user_pub_b64))
        .await
        .unwrap();

    assert_eq!(response.certificate.public_key, user_pub_b64);
    assert_eq!(response.certificate.publisher, "uploadco.example");
}

#[tokio::test]
async fn test_manager_admin_approve_with_invalid_key_rejected() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "badkeyco.example").await.unwrap();

    let result = manager
        .admin_approve(record.id, Some("not-a-valid-key"))
        .await;
    assert!(
        matches!(result, Err(PublisherRegistryError::Crypto(_))),
        "invalid public key should be rejected with Crypto error: {result:?}"
    );

    // Verify Publisher Registry is still pending (not corrupted)
    let status = manager.get_status(record.id).await.unwrap();
    assert_eq!(
        status.status,
        PublisherStatus::Pending,
        "Publisher Registry should remain pending after failed activation"
    );
}

#[tokio::test]
async fn test_manager_admin_approve_test_helper_generates_public_key() {
    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool);
    let (record, _) = pending(&manager, "gendefault.example").await.unwrap();

    // The private key exists only inside this test helper and is never returned.
    let response = manager.admin_approve(record.id, None).await.unwrap();

    assert!(!response.certificate.public_key.is_empty());
}

// ─── register_publisher tests ─────────────────────────────────────────────────

#[tokio::test]
async fn register_publisher_pins_the_first_profile() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let package = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let first = signed_profile("apps.example.com", 1, &roots, &[(&package, "active")]);
    let record = register(&manager, &first).await.unwrap();
    assert_eq!(record.status, PublisherStatus::Active);
    assert_eq!(record.public_key, b64_key(&package));
    assert_eq!(
        record.key_id,
        crypto::compute_key_id(package.verifying_key().as_bytes())
    );
    assert_eq!(
        record.contact.as_deref(),
        Some("mailto:security@apps.example.com")
    );
    let info = manager.resolve_by_name("apps.example.com").await.unwrap();
    assert_eq!(info.profile.unwrap()["sequence"], 1);

    // Registering the same profile again changes nothing.
    let again = register(&manager, &first).await.unwrap();
    assert_eq!(again.profile_json, record.profile_json);

    // A later profile needs a higher sequence.
    let replay = signed_profile(
        "apps.example.com",
        1,
        &roots,
        &[(&SigningKey::generate(&mut OsRng), "active")],
    );
    assert!(matches!(
        register(&manager, &replay).await,
        Err(PublisherRegistryError::VerificationFailed(_))
    ));

    // It must be signed by a pinned active key.
    let stranger = SigningKey::generate(&mut OsRng);
    let forged = signed_profile(
        "apps.example.com",
        2,
        &[("root-1", &stranger, "active")],
        &[(&package, "active")],
    );
    assert!(matches!(
        register(&manager, &forged).await,
        Err(PublisherRegistryError::VerificationFailed(_))
    ));
    // A new signing key listed without a rotation chain is not pinned, so it
    // cannot sign the next profile.
    let unchained = signed_profile(
        "apps.example.com",
        2,
        &[("root-1", &root, "active"), ("root-2", &stranger, "active")],
        &[(&package, "active")],
    );
    register(&manager, &unchained).await.unwrap();
    let by_unchained = signed_profile(
        "apps.example.com",
        3,
        &[("root-2", &stranger, "active"), ("root-1", &root, "active")],
        &[(&package, "active")],
    );
    assert!(matches!(
        register(&manager, &by_unchained).await,
        Err(PublisherRegistryError::VerificationFailed(_))
    ));

    // At most two active package signing keys.
    let third = SigningKey::generate(&mut OsRng);
    let crowded = signed_profile(
        "apps.example.com",
        3,
        &roots,
        &[
            (&package, "active"),
            (&stranger, "active"),
            (&third, "active"),
        ],
    );
    assert!(
        crowded
            .verify("apps.example.com", chrono::Utc::now())
            .is_err()
    );
}

#[tokio::test]
async fn register_rotated_profile_imports_retired_package_keys() {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let old = SigningKey::generate(&mut OsRng);
    let current = SigningKey::generate(&mut OsRng);
    let domain = "first-rotated.example";
    let profile = signed_profile(
        domain,
        4,
        &[("root-1", &root, "active")],
        &[(&old, "retired"), (&current, "active")],
    );
    let manifest = test_manifest(domain, "svc", "1.0.0", "wasm32-wasip1");
    let signature =
        base64::engine::general_purpose::STANDARD.encode(old.sign(manifest.as_bytes()).to_bytes());
    let old_id = crypto::compute_key_id(old.verifying_key().as_bytes());
    drop(old);

    let record = register(&manager, &profile).await.unwrap();
    let PackageSigningKey::Trusted(info) = manager
        .package_signing_key(domain, &old_id)
        .await
        .expect("a listed retired key must verify packages on first import")
    else {
        panic!("a listed retired key must retain Publisher trust");
    };
    assert!(crypto::verify_signature(manifest.as_bytes(), &signature, &info.public_key).unwrap());
    assert!(
        !crypto::verify_signature(manifest.as_bytes(), &signature, &record.public_key).unwrap()
    );
    register(&manager, &profile).await.unwrap();
    assert_eq!(manager.admin_list_keys(record.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn refresh_imports_unseen_retired_keys_without_reviving_revocations() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let current = SigningKey::generate(&mut OsRng);
    let old = SigningKey::generate(&mut OsRng);
    let older = SigningKey::generate(&mut OsRng);
    let old_id = crypto::compute_key_id(old.verifying_key().as_bytes());
    let older_id = crypto::compute_key_id(older.verifying_key().as_bytes());
    let domain = "refresh-retired.example";
    register(
        &manager,
        &signed_profile(domain, 1, &roots, &[(&current, "active")]),
    )
    .await
    .unwrap();
    let record = register(
        &manager,
        &signed_profile(
            domain,
            2,
            &roots,
            &[(&current, "active"), (&old, "retired")],
        ),
    )
    .await
    .unwrap();
    let history = manager.admin_list_keys(record.id).await.unwrap();
    assert_eq!(
        history.len(),
        1,
        "refresh must import previously unseen keys"
    );
    crate::model::PublisherKeyHistory::revoke(manager.pool(), history[0].id)
        .await
        .unwrap();

    let profile = signed_profile(
        domain,
        3,
        &roots,
        &[(&current, "active"), (&old, "retired"), (&older, "retired")],
    );
    register(&manager, &profile).await.unwrap();
    register(&manager, &profile).await.unwrap();
    assert!(matches!(
        manager.package_signing_key(domain, &old_id).await,
        Err(PublisherRegistryError::KeyRevoked(_))
    ));
    assert!(matches!(
        manager.package_signing_key(domain, &older_id).await,
        Ok(PackageSigningKey::Trusted(_))
    ));
    assert_eq!(manager.admin_list_keys(record.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn reregister_same_profile_backfills_retired_keys_without_reviving_revocations() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let current = SigningKey::generate(&mut OsRng);
    let old = SigningKey::generate(&mut OsRng);
    let revoked = SigningKey::generate(&mut OsRng);
    let old_id = crypto::compute_key_id(old.verifying_key().as_bytes());
    let revoked_id = crypto::compute_key_id(revoked.verifying_key().as_bytes());
    let domain = "backfill-retired.example";
    let profile = signed_profile(
        domain,
        4,
        &[("root-1", &root, "active")],
        &[
            (&current, "active"),
            (&old, "retired"),
            (&revoked, "retired"),
        ],
    );
    let record = register(&manager, &profile).await.unwrap();
    // Reproduce a Registry imported before retired keys were persisted. Keep
    // an explicit local revocation: backfill must never replace that decision.
    sqlx::query("DELETE FROM publisher_key_history WHERE publisher_id = ?")
        .bind(record.id)
        .execute(manager.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO publisher_key_history
         (publisher_id, key_id, public_key, status, created_at, retired_at)
         VALUES (?, ?, ?, 'revoked', 1, 2)",
    )
    .bind(record.id)
    .bind(&revoked_id)
    .bind(b64_key(&revoked))
    .execute(manager.pool())
    .await
    .unwrap();

    register(&manager, &profile).await.unwrap();
    assert!(matches!(
        manager.package_signing_key(domain, &old_id).await,
        Ok(PackageSigningKey::Trusted(_))
    ));
    assert!(matches!(
        manager.package_signing_key(domain, &revoked_id).await,
        Err(PublisherRegistryError::KeyRevoked(_))
    ));
    register(&manager, &profile).await.unwrap();
    assert_eq!(manager.admin_list_keys(record.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn refresh_preserves_current_key_revocation_when_it_retires() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let old = SigningKey::generate(&mut OsRng);
    let current = SigningKey::generate(&mut OsRng);
    let domain = "revoked-current.example";
    let record = register(
        &manager,
        &signed_profile(domain, 1, &roots, &[(&old, "active")]),
    )
    .await
    .unwrap();
    crate::model::PublisherKeyHistory::archive(
        manager.pool(),
        record.id,
        &record.key_id,
        &record.public_key,
        record.created_at,
    )
    .await
    .unwrap();
    let history = manager.admin_list_keys(record.id).await.unwrap();
    crate::model::PublisherKeyHistory::revoke(manager.pool(), history[0].id)
        .await
        .unwrap();
    register(
        &manager,
        &signed_profile(
            domain,
            2,
            &roots,
            &[(&current, "active"), (&old, "retired")],
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        manager.package_signing_key(domain, &record.key_id).await,
        Err(PublisherRegistryError::KeyRevoked(_))
    ));
    assert_eq!(manager.admin_list_keys(record.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn two_active_package_keys_overlap_during_rotation() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let old = SigningKey::generate(&mut OsRng);
    let new = SigningKey::generate(&mut OsRng);
    let old_id = crypto::compute_key_id(old.verifying_key().as_bytes());
    let new_id = crypto::compute_key_id(new.verifying_key().as_bytes());
    register(
        &manager,
        &signed_profile("overlap.example", 1, &roots, &[(&old, "active")]),
    )
    .await
    .unwrap();

    let record = register(
        &manager,
        &signed_profile(
            "overlap.example",
            2,
            &roots,
            &[(&new, "active"), (&old, "active")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(record.key_id, old_id);
    assert_eq!(record.next_key_id.as_deref(), Some(new_id.as_str()));
    for key_id in [&old_id, &new_id] {
        assert!(matches!(
            manager.package_signing_key("overlap.example", key_id).await,
            Ok(PackageSigningKey::Trusted(_))
        ));
    }

    // Retirement preserves existing signatures; the new key becomes current.
    let record = register(
        &manager,
        &signed_profile(
            "overlap.example",
            3,
            &roots,
            &[(&new, "active"), (&old, "retired")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(record.key_id, new_id);
    assert_eq!(record.next_key_id, None);
    for key_id in [&old_id, &new_id] {
        assert!(matches!(
            manager.package_signing_key("overlap.example", key_id).await,
            Ok(PackageSigningKey::Trusted(_))
        ));
    }

    // Removing a retired key revokes it and rejects its package signatures.
    let record = register(
        &manager,
        &signed_profile("overlap.example", 4, &roots, &[(&new, "active")]),
    )
    .await
    .unwrap();
    assert_eq!(record.key_id, new_id);
    assert!(matches!(
        manager
            .package_signing_key("overlap.example", &old_id)
            .await,
        Err(PublisherRegistryError::KeyRevoked(_))
    ));
}

#[tokio::test]
async fn package_signing_key_trust_decisions() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let key = SigningKey::generate(&mut OsRng);
    let key_id = crypto::compute_key_id(key.verifying_key().as_bytes());
    register(
        &manager,
        &signed_profile(
            "decide.example",
            1,
            &[("root-1", &root, "active")],
            &[(&key, "active")],
        ),
    )
    .await
    .unwrap();

    // A key of the active publisher is trusted.
    assert!(matches!(
        manager.package_signing_key("decide.example", &key_id).await,
        Ok(PackageSigningKey::Trusted(info)) if info.key_id == key_id
    ));
    // A name the registry does not hold is untrusted: a development identity.
    assert!(matches!(
        manager.package_signing_key("other.example", &key_id).await,
        Ok(PackageSigningKey::Untrusted)
    ));
    // A held name never is: another key, another spelling, or a suspended
    // publisher is an error.
    assert!(matches!(
        manager
            .package_signing_key("decide.example", "actr-pub-0000000000000000")
            .await,
        Err(PublisherRegistryError::NotFound)
    ));
    assert!(matches!(
        manager.package_signing_key("Decide.Example", &key_id).await,
        Err(PublisherRegistryError::InvalidRequest(_))
    ));
    sqlx::query("UPDATE publisher SET status = 'suspended' WHERE name = 'decide.example'")
        .execute(manager.pool())
        .await
        .unwrap();
    assert!(matches!(
        manager.package_signing_key("decide.example", &key_id).await,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));
}

#[tokio::test]
async fn register_publisher_never_reinstates_a_used_key() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let key_a = SigningKey::generate(&mut OsRng);
    let key_b = SigningKey::generate(&mut OsRng);
    let domain = "rollback.example";
    register(
        &manager,
        &signed_profile(domain, 1, &roots, &[(&key_a, "active")]),
    )
    .await
    .unwrap();
    let record = register(
        &manager,
        &signed_profile(
            domain,
            2,
            &roots,
            &[(&key_a, "retired"), (&key_b, "active")],
        ),
    )
    .await
    .unwrap();
    let history = manager.admin_list_keys(record.id).await.unwrap();
    assert_eq!(history.len(), 1);

    let back = signed_profile(domain, 3, &roots, &[(&key_a, "active")]);
    assert!(matches!(
        register(&manager, &back).await,
        Err(PublisherRegistryError::InvalidRequest(_))
    ));

    // A suspended publisher accepts no profile.
    manager.admin_suspend(record.id).await.unwrap();
    let later = signed_profile(domain, 4, &roots, &[(&key_b, "active")]);
    assert!(matches!(
        register(&manager, &later).await,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));
}

// ─── Key rotation + publish/verify tests ─────────────────────────────────────

#[tokio::test]
async fn test_key_rotation_historical_key_resolved() {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let pool = setup_test_pool().await;
    let manager = PublisherRegistryManager::new(pool.clone());

    // Register key_A, then rotate to key_B
    let root = SigningKey::generate(&mut OsRng);
    let key_a = SigningKey::generate(&mut OsRng);
    let pub_a = base64::engine::general_purpose::STANDARD.encode(key_a.verifying_key().to_bytes());
    let roots = [("root-1", &root, "active")];
    let record = register(
        &manager,
        &signed_profile("histkey.example", 1, &roots, &[(&key_a, "active")]),
    )
    .await
    .unwrap();
    let key_id_a = record.key_id.clone();

    let key_b = SigningKey::generate(&mut OsRng);
    let pub_b = base64::engine::general_purpose::STANDARD.encode(key_b.verifying_key().to_bytes());
    let key_id_b = register(
        &manager,
        &signed_profile(
            "histkey.example",
            2,
            &roots,
            &[(&key_a, "retired"), (&key_b, "active")],
        ),
    )
    .await
    .unwrap()
    .key_id;

    // Resolve current key (key_B)
    let current = manager.resolve_by_name("histkey.example").await.unwrap();
    assert_eq!(current.key_id, key_id_b);
    assert_eq!(current.public_key, pub_b);

    // Resolve old key (key_A) by key_id — should still work
    let historical = manager
        .resolve_key_by_id("histkey.example", &key_id_a)
        .await
        .unwrap();
    assert_eq!(historical.key_id, key_id_a);
    assert_eq!(historical.public_key, pub_a);

    // Resolve non-existent key_id — should fail
    let result = manager
        .resolve_key_by_id("histkey.example", "actr-pub-nonexistent")
        .await;
    assert!(matches!(result, Err(PublisherRegistryError::NotFound)));

    sqlx::query("UPDATE publisher SET status = 'suspended' WHERE id = ?")
        .bind(record.id)
        .execute(&pool)
        .await
        .unwrap();
    let result = manager
        .resolve_key_by_id("histkey.example", &key_id_a)
        .await;
    assert!(
        matches!(result, Err(PublisherRegistryError::InvalidStatus(_))),
        "historical keys must not bypass publisher suspension: {result:?}"
    );
}

// ─── Error path tests ────────────────────────────────────────────────────────

// ─── Multi-publisher / multi-package tests ────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_register_publisher_requests_do_not_fail_internally() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    // A file database in WAL mode with several connections, as in production.
    let dir = tempfile::tempdir().unwrap();
    let database = platform::storage::db::Database::new(dir.path())
        .await
        .unwrap();
    let manager = std::sync::Arc::new(PublisherRegistryManager::new(database.get_pool().clone()));
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let domain = "race.example";
    let first = signed_profile(
        domain,
        1,
        &roots,
        &[(&SigningKey::generate(&mut OsRng), "active")],
    );
    let later: Vec<_> = (2..10)
        .map(|sequence| {
            signed_profile(
                domain,
                sequence,
                &roots,
                &[(&SigningKey::generate(&mut OsRng), "active")],
            )
        })
        .collect();

    // The first registration, sent many times at once.
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let (manager, profile) = (manager.clone(), first.clone());
            tokio::spawn(async move { register(&manager, &profile).await })
        })
        .collect();
    for task in tasks {
        task.await
            .unwrap()
            .expect("the same first profile registers");
    }

    // Later profiles at once: each either wins or is refused as not
    // following the accepted one, never an internal error.
    let tasks: Vec<_> = later
        .into_iter()
        .map(|profile| {
            let manager = manager.clone();
            tokio::spawn(async move { register(&manager, &profile).await })
        })
        .collect();
    for task in tasks {
        match task.await.unwrap() {
            Ok(_)
            | Err(PublisherRegistryError::VerificationFailed(_))
            | Err(PublisherRegistryError::InvalidStatus(_)) => {}
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }
}

#[tokio::test]
async fn register_publisher_refuses_a_package_key_outside_its_validity() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let now = chrono::Utc::now();
    let with_window = |not_before: Option<String>, not_after: Option<String>| {
        let profile = signed_profile(
            "window.example",
            1,
            &[("root-1", &root, "active")],
            &[(&SigningKey::generate(&mut OsRng), "active")],
        );
        let mut document = serde_json::to_value(&profile).unwrap();
        document["keys"][0]["not_before"] = not_before.into();
        document["keys"][0]["not_after"] = not_after.into();
        actr_protocol::realm_profile::sign_document(&mut document, "root-1", &root);
        serde_json::from_value::<actr_protocol::PublisherProfile>(document).unwrap()
    };
    let at = |offset: chrono::Duration| Some((now + offset).to_rfc3339());

    for (not_before, not_after) in [
        // Not valid yet.
        (at(chrono::Duration::days(1)), None),
        // Already expired.
        (None, at(-chrono::Duration::days(1))),
        // Not RFC 3339.
        (Some("tomorrow".to_string()), None),
    ] {
        let profile = with_window(not_before.clone(), not_after.clone());
        assert!(
            matches!(
                register(&manager, &profile).await,
                Err(PublisherRegistryError::InvalidRequest(_))
            ),
            "{not_before:?} {not_after:?}"
        );
    }
    let valid = with_window(
        at(-chrono::Duration::days(1)),
        at(chrono::Duration::days(30)),
    );
    let record = register(&manager, &valid).await.unwrap();
    assert!(record.key_expires_at.unwrap() > now.timestamp());
}

#[tokio::test]
async fn register_publisher_refuses_a_package_key_that_is_not_a_curve_point() {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    // 32 bytes that do not decode to an Ed25519 point.
    let not_a_point = (0u8..=255)
        .map(|byte| {
            let mut bytes = [0u8; 32];
            bytes[0] = byte;
            bytes[31] = 0x7f;
            bytes
        })
        .find(|bytes| ed25519_dalek::VerifyingKey::from_bytes(bytes).is_err())
        .expect("some bytes are not a point");

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let root = SigningKey::generate(&mut OsRng);
    let profile = signed_profile(
        "point.example",
        1,
        &[("root-1", &root, "active")],
        &[(&SigningKey::generate(&mut OsRng), "active")],
    );
    let mut document = serde_json::to_value(&profile).unwrap();
    document["keys"][0]["public_key"] = base64::engine::general_purpose::STANDARD
        .encode(not_a_point)
        .into();
    document["keys"][0]["id"] = actr_protocol::package_key_id(&not_a_point).into();
    actr_protocol::realm_profile::sign_document(&mut document, "root-1", &root);
    let profile: actr_protocol::PublisherProfile = serde_json::from_value(document).unwrap();
    let result = register(&manager, &profile).await;
    assert!(
        matches!(result, Err(PublisherRegistryError::InvalidRequest(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn admin_delete_removes_a_publisher_with_key_history_atomically() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    // The production schema, with foreign keys enforced.
    let dir = tempfile::tempdir().unwrap();
    let database = platform::storage::db::Database::new(dir.path())
        .await
        .unwrap();
    let manager = PublisherRegistryManager::new(database.get_pool().clone());
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let domain = "delete.example";
    let (key_a, key_b) = (
        SigningKey::generate(&mut OsRng),
        SigningKey::generate(&mut OsRng),
    );
    register(
        &manager,
        &signed_profile(domain, 1, &roots, &[(&key_a, "active")]),
    )
    .await
    .unwrap();
    let record = register(
        &manager,
        &signed_profile(domain, 2, &roots, &[(&key_b, "active")]),
    )
    .await
    .unwrap();
    assert!(!manager.admin_list_keys(record.id).await.unwrap().is_empty());

    manager.admin_delete(record.id).await.unwrap();
    assert!(
        Publisher::get_by_name(manager.pool(), domain)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn register_publisher_stops_at_max_publishers() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await).with_max_publishers(1);
    let root = SigningKey::generate(&mut OsRng);
    let roots = [("root-1", &root, "active")];
    let key = SigningKey::generate(&mut OsRng);
    let first = signed_profile("first.example", 1, &roots, &[(&key, "active")]);
    register(&manager, &first).await.unwrap();

    let second = signed_profile("second.example", 1, &roots, &[(&key, "active")]);
    assert!(matches!(
        register(&manager, &second).await,
        Err(PublisherRegistryError::QuotaExceeded(_))
    ));
    // A registered publisher still updates its profile.
    let update = signed_profile("first.example", 2, &roots, &[(&key, "active")]);
    register(&manager, &update).await.unwrap();
}

#[tokio::test]
async fn the_operator_confirms_a_publisher_identity_change() {
    use actr_protocol::{KeyPinOutcome, PublisherProfile};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let manager = PublisherRegistryManager::new(setup_test_pool().await);
    let domain = "apps.example.com";
    let (root, package) = (
        SigningKey::generate(&mut OsRng),
        SigningKey::generate(&mut OsRng),
    );
    let first = signed_profile(
        domain,
        3,
        &[("root-1", &root, "active")],
        &[(&package, "active")],
    );
    register(&manager, &first).await.unwrap();

    // The domain lost its keys and starts over, reusing the id.
    let (new_root, new_package) = (
        SigningKey::generate(&mut OsRng),
        SigningKey::generate(&mut OsRng),
    );
    let changed = signed_profile(
        domain,
        1,
        &[("root-1", &new_root, "active")],
        &[(&new_package, "active")],
    );
    assert!(matches!(
        register(&manager, &changed).await,
        Err(PublisherRegistryError::VerificationFailed(_))
    ));

    let digest = |profile: &PublisherProfile| match profile
        .accept(domain, chrono::Utc::now(), None)
        .unwrap()
        .1
    {
        KeyPinOutcome::Accepted(pins) => pins.digest,
        KeyPinOutcome::IdentityChange(_) => unreachable!(),
    };
    let confirm = |profile: &PublisherProfile, digest: String| {
        let keys = profile.verify(domain, chrono::Utc::now()).unwrap();
        let (manager, profile) = (&manager, profile.clone());
        async move {
            manager
                .confirm_publisher_identity(domain, &profile, &keys, &digest)
                .await
        }
    };
    // Only the profile the operator reviewed is taken.
    assert!(matches!(
        confirm(&changed, digest(&first)).await,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));
    // A profile accepted under the pins needs no confirmation.
    let later = signed_profile(
        domain,
        4,
        &[("root-1", &root, "active")],
        &[(&package, "active")],
    );
    assert!(matches!(
        confirm(&later, digest(&later)).await,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));
    // Neither does the stored profile itself, whatever the digest.
    assert!(matches!(
        confirm(&first, "not-reviewed".to_string()).await,
        Err(PublisherRegistryError::InvalidStatus(_))
    ));

    let record = confirm(&changed, digest(&changed)).await.unwrap();
    assert_eq!(record.public_key, b64_key(&new_package));
    // The new keys are the baseline from now on.
    let next = signed_profile(
        domain,
        2,
        &[("root-1", &new_root, "active")],
        &[(&new_package, "active")],
    );
    register(&manager, &next).await.unwrap();
    assert!(matches!(
        register(&manager, &first).await,
        Err(PublisherRegistryError::VerificationFailed(_))
    ));

    // Only a registered publisher is confirmed.
    let unknown = signed_profile(
        "other.example.com",
        1,
        &[("root-1", &root, "active")],
        &[(&package, "active")],
    );
    let keys = unknown
        .verify("other.example.com", chrono::Utc::now())
        .unwrap();
    assert!(matches!(
        manager
            .confirm_publisher_identity(
                "other.example.com",
                &unknown,
                &keys,
                &digest_of(&unknown, "other.example.com")
            )
            .await,
        Err(PublisherRegistryError::InvalidStatus(_) | PublisherRegistryError::NotFound)
    ));
}

fn digest_of(profile: &actr_protocol::PublisherProfile, domain: &str) -> String {
    match profile.accept(domain, chrono::Utc::now(), None).unwrap().1 {
        actr_protocol::KeyPinOutcome::Accepted(pins) => pins.digest,
        actr_protocol::KeyPinOutcome::IdentityChange(_) => unreachable!(),
    }
}

#[tokio::test]
async fn removed_package_routes_are_unavailable() {
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    let state = crate::handlers::PublisherRegistryState {
        manager: Arc::new(PublisherRegistryManager::new(setup_test_pool().await)),
    };
    let app = Router::new()
        .nest(
            "/registry",
            crate::handlers::create_public_router(state.clone()),
        )
        .nest(
            "/admin/api/publishers",
            crate::handlers::create_admin_router(state),
        );
    for (method, path) in [
        (Method::POST, "/registry/packages/nonce"),
        (Method::POST, "/registry/packages/publish"),
        (Method::GET, "/admin/api/publishers/packages"),
        (Method::POST, "/admin/api/publishers/packages/1/revoke"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
