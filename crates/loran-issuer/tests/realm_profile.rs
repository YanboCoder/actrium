//! Loran-signed Realm Profiles and the refresh of other domains' Realms,
//! checked with the Actr Host rules (`RealmProfile::verify` and
//! `check_successor`).

use actr_protocol::realm_membership::{RealmInvite, Role, client_key_id, invite_secret_hash};
use actr_protocol::realm_profile::{
    IceEndpoint, RealmPackage, RealmProfile, RealmStatus as ProfileStatus, SignalingEndpoint, jcs,
};
use actr_protocol::{KeyStatus, RealmSigningKeys};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer as _, SigningKey};
use issuer::realm_profile::{ProfileChange, ProfileError, ProviderEndpoints, RealmProfiles};
use issuer::signer_client_wrapper::create_signer_client;
use nonce_auth::storage::MemoryStorage;
use platform::config::signer::SignerClientConfig;
use platform::net::FetchError;
use platform::realm::membership::{self, Code, Signer};
use platform::realm::{Realm, RealmError, RealmProfileRecord, RealmStatus};
use serde_json::{Value, json};
use serial_test::serial;
use signer::{KeyStorage, SignerServiceConfig, create_grpc_service};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tonic::transport::Server;

const PROVIDER: &str = "provider.example";

fn package() -> RealmPackage {
    RealmPackage {
        href: "https://cdn.example/notes-1.0.actr".to_string(),
        sha256: "ab".repeat(32),
        r#type: "acme.com/notes@1.0.0".to_string(),
    }
}

/// An embedded Signer, the global database, and `RealmProfiles` for
/// `provider.example` with its signing key ready.
async fn setup() -> (Arc<RealmProfiles>, TempDir) {
    // The database is process-global; keep its directory for the process.
    static DB_DIR: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
    let db_dir = DB_DIR.get_or_init(|| TempDir::new().unwrap());
    if !platform::storage::db::is_database_initialized() {
        platform::storage::db::set_db_path(db_dir.path())
            .await
            .unwrap();
    }
    let dir = TempDir::new().unwrap();
    let pool = platform::storage::db::get_database().get_pool();
    for statement in [
        "DELETE FROM actoracl",
        "DELETE FROM realm_issuer_key",
        "DELETE FROM realm_profile",
        "DELETE FROM realm_domain",
        "DELETE FROM realm_mgmt_nonce",
        "DELETE FROM realm",
        "DELETE FROM publisher_key_history",
        "DELETE FROM publisher_package",
        "DELETE FROM publisher_publish_nonce",
        "DELETE FROM publisher",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }

    let shared_key = "test-shared-key";
    let service_config = SignerServiceConfig::default();
    let storage = KeyStorage::from_config(
        &service_config.storage,
        signer::KeyEncryptor::no_encryption(),
        dir.path(),
    )
    .await
    .unwrap();
    let service = create_grpc_service(storage, MemoryStorage::new(), shared_key.to_string());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    tokio::spawn(Server::builder().add_service(service).serve(addr));

    let signer = create_signer_client(
        &SignerClientConfig {
            endpoint: format!("http://{addr}"),
            timeout_seconds: 10,
            enable_tls: false,
            tls_domain: None,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        },
        shared_key,
    )
    .await
    .unwrap();
    let profiles = Arc::new(RealmProfiles::new(
        ProviderEndpoints {
            provider_id: PROVIDER.to_string(),
            issuer_url: format!("https://{PROVIDER}/issuer"),
            signaling: vec![SignalingEndpoint {
                url: format!("wss://{PROVIDER}/signaling/ws"),
                priority: 0,
                control_plane_id: format!("{PROVIDER}/v1"),
                ticket_audience: "actr.signaling".to_string(),
            }],
            ice: vec![IceEndpoint {
                urls: vec!["turn:198.51.100.1:3478".to_string()],
                credential_audience: Some("actr.turn".to_string()),
            }],
            realms_base: format!("https://{PROVIDER}/actr/realms/"),
        },
        signer,
        3600,
        dir.path().to_path_buf(),
    ));
    for _ in 0..50 {
        if profiles.ensure_signing_keys().await.is_ok() {
            return (profiles, dir);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Signer did not come up");
}

async fn insert(name: &str, status: RealmStatus) {
    Realm {
        name: name.to_string(),
        status,
        ..Default::default()
    }
    .insert(None)
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn signs_verifiable_profiles_for_the_provider_domain() {
    let (profiles, _dir) = setup().await;
    let keys = profiles.signing_keys().unwrap().clone();
    let name = format!("k7m2q9xh@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;

    // An active Profile needs a package.
    assert!(matches!(
        profiles.issue(&name, ProfileChange::default()).await,
        Err(ProfileError::Invalid(_))
    ));
    let realm = profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(realm.status, RealmStatus::Active);

    let record = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let first = RealmProfile::verify(record.document.as_bytes(), &name, &keys).unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(first.realm.display_name, "k7m2q9xh");
    assert_eq!(first.package, Some(package()));
    assert_eq!(first.provider.id, PROVIDER);
    assert_eq!(
        first.provider.signaling[0].control_plane_id,
        "provider.example/v1"
    );
    assert_eq!(first.policy.membership, "invite-only");
    let current = platform::realm::RealmIssuerKey::current(&name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.issuer.keys[0].id, current.key_id.to_string());
    assert_eq!(
        first.issuer_key(current.key_id).unwrap().public_key,
        current.public_key
    );
    assert_eq!(first.signature.key_id, "realm-signing:provider.example");

    profiles
        .issue(
            &name,
            ProfileChange {
                display_name: Some("Notes".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let record = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let second = RealmProfile::verify(record.document.as_bytes(), &name, &keys).unwrap();
    first.check_successor(&second).unwrap();
    assert_eq!(
        (second.sequence, second.realm.display_name.as_str()),
        (2, "Notes")
    );
    assert_eq!(second.package, Some(package()));
    assert_eq!(second.issuer.keys, first.issuer.keys);

    let realm = profiles
        .issue(
            &name,
            ProfileChange {
                retire: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(realm.status, RealmStatus::Retired);
    let record = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let retired = RealmProfile::verify(record.document.as_bytes(), &name, &keys).unwrap();
    second.check_successor(&retired).unwrap();
    assert_eq!(retired.realm.status, ProfileStatus::Retired);
    assert!(matches!(
        profiles.issue(&name, ProfileChange::default()).await,
        Err(ProfileError::Realm(RealmError::Retired))
    ));

    // Other domains' Realms are signed by their own key holders.
    insert("other@acme.com", RealmStatus::Active).await;
    assert!(matches!(
        profiles
            .issue(
                "other@acme.com",
                ProfileChange {
                    package: Some(package()),
                    ..Default::default()
                }
            )
            .await,
        Err(ProfileError::Invalid(_))
    ));
}

/// Join `realm` with the key of `seed` through `invite`.
async fn redeem(realm: &str, invite: &membership::IssuedInvite, seed: u8) -> Signer {
    let secret = RealmInvite::decode(&invite.invite).unwrap().secret;
    let public_key = SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes();
    let limits = membership::Limits::from(&platform::config::RealmConfig::default());
    membership::join(
        realm,
        &invite_secret_hash(&secret).unwrap(),
        &public_key,
        "",
        limits,
    )
    .await
    .unwrap();
    let key = membership::client_key(realm, &client_key_id(&public_key))
        .await
        .unwrap()
        .unwrap();
    membership::authenticate(key).await.unwrap()
}

/// An invite from `owner`: a new member of `role`, or with `role` unset a
/// new client of `member`.
async fn invite(
    owner: &Signer,
    role: Option<Role>,
    member: Option<&str>,
) -> membership::IssuedInvite {
    let limits = membership::Limits::from(&platform::config::RealmConfig::default());
    let expires_at = chrono::Utc::now().timestamp() + 3600;
    membership::create_invite(owner, role, member, expires_at, limits)
        .await
        .unwrap()
}

/// The owner's update is stored only if, when it is stored, the signer is
/// still the owner: a revocation or an ownership transfer that committed
/// while the Profile was signed stops it.
#[tokio::test]
#[serial]
async fn stores_an_owner_update_only_for_the_current_owner() {
    let (profiles, _dir) = setup().await;
    let name = format!("o5n3r8wq@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;
    profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let owner = redeem(
        &name,
        &membership::issue_owner_invite(&name, membership::BY_OPERATOR)
            .await
            .unwrap(),
        11,
    )
    .await;
    let second_key = redeem(
        &name,
        &invite(&owner, None, Some(&owner.member_id)).await,
        12,
    )
    .await;
    let editor = redeem(&name, &invite(&owner, Some(Role::Editor), None).await, 13).await;
    let digest = || async {
        RealmProfileRecord::get(&name)
            .await
            .unwrap()
            .unwrap()
            .digest
    };
    let before = digest().await;
    let refused = |result: Result<Realm, ProfileError>| match result {
        Err(ProfileError::Realm(RealmError::Membership(error))) => error.code,
        other => panic!("expected a membership error, got {other:?}"),
    };

    // Authenticated as the owner; the key is revoked before the store.
    membership::revoke_client(&owner, &second_key.key_id)
        .await
        .unwrap();
    let update = ProfileChange {
        display_name: Some("Renamed".to_string()),
        owner: Some(second_key.clone()),
        ..Default::default()
    };
    assert_eq!(
        refused(profiles.issue(&name, update).await),
        Code::ClientRevoked
    );
    assert_eq!(digest().await, before);

    // Authenticated as the owner; ownership moves before the store.
    membership::transfer_owner(&owner, &editor.member_id, Role::Editor)
        .await
        .unwrap();
    let retire = ProfileChange {
        retire: true,
        owner: Some(owner.clone()),
        ..Default::default()
    };
    assert_eq!(
        refused(profiles.issue(&name, retire).await),
        Code::Forbidden
    );
    assert_eq!(digest().await, before);
    assert_eq!(
        Realm::get(&name).await.unwrap().unwrap().status,
        RealmStatus::Active
    );

    // The new owner's update is stored.
    let update = ProfileChange {
        display_name: Some("Renamed".to_string()),
        owner: Some(editor),
        ..Default::default()
    };
    profiles.issue(&name, update).await.unwrap();
    assert_ne!(digest().await, before);
}

/// `acme.com` as its key holder: a signed `realm_signing_keys` member and
/// Realm Profiles for `provider.example`.
struct Domain {
    keys: Vec<(&'static str, SigningKey)>,
}

impl Domain {
    fn member(&self, sequence: u64, listed: &[&str]) -> Value {
        let keys: Vec<Value> = listed
            .iter()
            .map(|id| {
                let (_, key) = self.keys.iter().find(|(k, _)| k == id).unwrap();
                json!({
                    "id": id, "algorithm": "Ed25519", "status": "active",
                    "public_key": BASE64.encode(key.verifying_key().to_bytes()),
                })
            })
            .collect();
        let member = json!({ "sequence": sequence, "keys": keys });
        self.sign(member, listed[0])
    }

    fn descriptor(&self, member: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": 1,
            "authority": "actr://acme.com",
            "services": [{
                "rel": "realms",
                "href": "https://acme.com/realms/",
                "protocol": "actr-realm-profile/1"
            }],
            "realm_signing_keys": member,
        }))
        .unwrap()
    }

    /// A Realm Profile naming `issuer` (see [`issuer_of`]).
    fn profile(
        &self,
        name: &str,
        sequence: u64,
        status: &str,
        key_id: &str,
        issuer: &Value,
    ) -> Vec<u8> {
        let mut profile = json!({
            "schema": "actr.realm/v1",
            "sequence": sequence,
            "realm": { "name": name, "display_name": "Acme", "status": status },
            "issuer": issuer,
            "provider": { "id": PROVIDER, "signaling": [], "ice": [] },
            "policy": { "membership": "invite-only" },
            "issued_at": "2026-09-25T00:00:00Z",
        });
        if status == "active" {
            profile["package"] = json!(package());
        }
        serde_json::to_vec(&self.sign(profile, key_id)).unwrap()
    }

    fn sign(&self, mut value: Value, key_id: &str) -> Value {
        let (_, key) = self.keys.iter().find(|(k, _)| *k == key_id).unwrap();
        let signature = key.sign(jcs(&value).as_bytes());
        value["signature"] = json!({
            "key_id": key_id, "algorithm": "Ed25519",
            "value": BASE64.encode(signature.to_bytes()),
        });
        value
    }
}

/// Accept `document` as the Realm's Profile, as `publish_profile` will.
async fn accept(name: &str, document: &[u8], member: &Value) {
    let keys: RealmSigningKeys = serde_json::from_value(member.clone()).unwrap();
    platform::realm::RealmDomain::accept("acme.com", member)
        .await
        .unwrap();
    let profile = RealmProfile::verify(document, name, &keys).unwrap();
    let previous = RealmProfileRecord::get(name).await.unwrap();
    RealmProfileRecord::store(
        &profile,
        std::str::from_utf8(document).unwrap(),
        previous.as_ref().map(|record| record.digest.as_str()),
        None,
        None,
    )
    .await
    .unwrap();
}

/// The `issuer` member Loran expects in the Realm's Profile: its Issuer and
/// the Realm's issuer key.
async fn issuer_of(profiles: &RealmProfiles, name: &str) -> Value {
    profiles.draft(name, 0, "", None, false).await.unwrap()["issuer"].clone()
}

type Documents = HashMap<String, Vec<u8>>;

async fn refresh(profiles: &RealmProfiles, documents: &Documents) {
    let fetch = |url: String| {
        let found = documents.get(&url).cloned();
        async move {
            found
                .map(Arc::from)
                .ok_or_else(|| FetchError::new(format!("404 {url}")))
        }
    };
    profiles.refresh(fetch).await.unwrap();
}

#[tokio::test]
#[serial]
async fn rotates_the_issuer_keys_of_provider_domain_realms() {
    use platform::realm::RealmIssuerKey;
    let (profiles, _dir) = setup().await;
    let keys = profiles.signing_keys().unwrap().clone();
    let name = format!("r0t4t10n@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;
    profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let latest = || async {
        let record = RealmProfileRecord::get(&name).await.unwrap().unwrap();
        RealmProfile::verify(record.document.as_bytes(), &name, &keys).unwrap()
    };
    let old = RealmIssuerKey::current(&name).await.unwrap().unwrap();

    // Pre-publish: listed as active, the old key still signs.
    let next = profiles.rotate_issuer_key(&name).await.unwrap();
    let profile = latest().await;
    assert_eq!(profile.sequence, 2);
    assert_eq!(profile.issuer_key(next).unwrap().status, KeyStatus::Active);
    assert!(matches!(
        profiles.rotate_issuer_key(&name).await,
        Err(ProfileError::Realm(RealmError::Conflict(_)))
    ));
    let now = chrono::Utc::now().timestamp();
    profiles.maintain_issuer_keys(now + 60).await.unwrap();
    let current = RealmIssuerKey::current(&name).await.unwrap().unwrap();
    assert_eq!(current.key_id, old.key_id);

    // Switch after 24 hours; the old key is retired but still listed.
    let switch = now + issuer::realm_profile::PREPUBLISH_SECS + 1;
    profiles.maintain_issuer_keys(switch).await.unwrap();
    let current = RealmIssuerKey::current(&name).await.unwrap().unwrap();
    assert_eq!(current.key_id, next);
    let profile = latest().await;
    assert_eq!(profile.sequence, 3);
    assert_eq!(
        profile.issuer_key(old.key_id).unwrap().status,
        KeyStatus::Retired
    );

    // An issuance that read the old key before the switch may cache it
    // again without expiry; its removal still ends it.
    let old_key = ed25519_dalek::VerifyingKey::from_bytes(
        &BASE64.decode(&old.public_key).unwrap().try_into().unwrap(),
    )
    .unwrap();
    platform::aid::credential::validator::AIdCredentialValidator::persist_key(
        _dir.path(),
        &name,
        old.key_id,
        &old_key,
        0,
    )
    .await
    .unwrap();

    // Removed once its last credential (1 hour) and the skew have passed.
    profiles.maintain_issuer_keys(switch + 3600).await.unwrap();
    assert_eq!(latest().await.issuer.keys.len(), 2);
    profiles
        .maintain_issuer_keys(switch + 3600 + 300)
        .await
        .unwrap();
    let profile = latest().await;
    assert_eq!(profile.sequence, 4);
    assert!(profile.issuer_key(old.key_id).is_none());
    assert_eq!(RealmIssuerKey::list(&name).await.unwrap().len(), 1);
    let cache = platform::aid::KeyCache::new(_dir.path().join("signaling_key_cache.db"))
        .await
        .unwrap();
    let (_, expires_at) = cache
        .get_cached_key(&name, old.key_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expires_at as i64, switch + 3600 + 300);

    // The next rotation starts on its own after 90 days.
    let due = switch + issuer::realm_profile::ROTATION_INTERVAL_SECS;
    profiles.maintain_issuer_keys(due - 1).await.unwrap();
    assert_eq!(latest().await.sequence, 4);
    profiles.maintain_issuer_keys(due).await.unwrap();
    let profile = latest().await;
    assert_eq!(profile.issuer.keys.len(), 2);
    assert_eq!(profile.sequence, 5);
}

#[tokio::test]
#[serial]
async fn lists_a_next_issuer_key_whose_profile_was_not_signed() {
    use platform::realm::{IssuerKeyState, RealmIssuerKey};
    let (profiles, _dir) = setup().await;
    let name = format!("unl1st3d@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;
    profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // A rotation whose Realm Profile could not be signed (the Signer was
    // down): the next key exists but no Profile lists it.
    let now = chrono::Utc::now().timestamp();
    RealmIssuerKey {
        realm: name.clone(),
        key_id: u32::MAX,
        signer_name: format!("issuer:{name}/unlisted"),
        public_key: BASE64.encode([5u8; 32]),
        state: IssuerKeyState::Next,
        created_at: now,
        published_at: None,
        current_since: None,
        remove_after: None,
    }
    .insert()
    .await
    .unwrap();

    profiles.maintain_issuer_keys(now).await.unwrap();
    let record = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    assert_eq!(record.sequence, 2);
    let next = RealmIssuerKey::list(&name)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.state == IssuerKeyState::Next)
        .unwrap();
    assert!(next.published_at.is_some());
}

#[tokio::test]
#[serial]
async fn switches_only_to_an_issuer_key_the_accepted_profile_lists() {
    use platform::realm::RealmIssuerKey;
    let (profiles, _dir) = setup().await;
    let acme = Domain {
        keys: vec![("rs-1", SigningKey::from_bytes(&[1; 32]))],
    };
    let member = acme.member(1, &["rs-1"]);
    let name = "keys@acme.com";
    insert(name, RealmStatus::Active).await;
    let before = issuer_of(&profiles, name).await;
    accept(
        name,
        &acme.profile(name, 1, "active", "rs-1", &before),
        &member,
    )
    .await;
    let old = RealmIssuerKey::current(name).await.unwrap().unwrap();

    // The key holder publishes the next key, then a Profile without it
    // (as a refresh from its realms location may bring).
    profiles.rotate_issuer_key(name).await.unwrap();
    let rotating = issuer_of(&profiles, name).await;
    accept(
        name,
        &acme.profile(name, 2, "active", "rs-1", &rotating),
        &member,
    )
    .await;
    accept(
        name,
        &acme.profile(name, 3, "active", "rs-1", &before),
        &member,
    )
    .await;

    let later = chrono::Utc::now().timestamp() + issuer::realm_profile::PREPUBLISH_SECS + 60;
    profiles.maintain_issuer_keys(later).await.unwrap();
    let current = RealmIssuerKey::current(name).await.unwrap().unwrap();
    assert_eq!(
        current.key_id, old.key_id,
        "the accepted Profile does not list the next key"
    );

    // Listed again, it switches once the pre-publish period has passed.
    accept(
        name,
        &acme.profile(name, 4, "active", "rs-1", &rotating),
        &member,
    )
    .await;
    let listed = RealmIssuerKey::list(name).await.unwrap();
    assert!(listed.iter().all(|key| key.published_at.is_some()));
    profiles.maintain_issuer_keys(later).await.unwrap();
    let current = RealmIssuerKey::current(name).await.unwrap().unwrap();
    assert_ne!(current.key_id, old.key_id);
}

#[tokio::test]
#[serial]
async fn refreshes_other_domains_profiles_under_the_key_pin() {
    let (profiles, _dir) = setup().await;
    let acme = Domain {
        keys: vec![
            ("rs-1", SigningKey::from_bytes(&[1; 32])),
            ("rs-2", SigningKey::from_bytes(&[2; 32])),
        ],
    };
    let member = acme.member(1, &["rs-1", "rs-2"]);
    let (one, two, three) = ("one@acme.com", "two@acme.com", "three@acme.com");
    insert(one, RealmStatus::Active).await;
    insert(two, RealmStatus::Active).await;
    insert(three, RealmStatus::Active).await;
    let mut issuer = HashMap::new();
    for name in [one, two, three] {
        issuer.insert(name, issuer_of(&profiles, name).await);
    }
    accept(
        one,
        &acme.profile(one, 1, "active", "rs-1", &issuer[one]),
        &member,
    )
    .await;
    accept(
        two,
        &acme.profile(two, 5, "active", "rs-1", &issuer[two]),
        &member,
    )
    .await;

    // A newer Profile is taken from the domain's realms location.
    let mut documents = Documents::new();
    documents.insert(
        "https://acme.com/.well-known/actr".into(),
        acme.descriptor(&member),
    );
    documents.insert(
        "https://acme.com/realms/one".into(),
        acme.profile(one, 2, "active", "rs-1", &issuer[one]),
    );
    documents.insert(
        "https://acme.com/realms/two".into(),
        acme.profile(two, 4, "active", "rs-1", &issuer[two]),
    );
    refresh(&profiles, &documents).await;
    assert_eq!(
        RealmProfileRecord::get(one)
            .await
            .unwrap()
            .unwrap()
            .sequence,
        2
    );
    // Rollback is refused.
    assert_eq!(
        RealmProfileRecord::get(two)
            .await
            .unwrap()
            .unwrap()
            .sequence,
        5
    );

    // A retired Profile retires the Realm.
    documents.insert(
        "https://acme.com/realms/one".into(),
        acme.profile(one, 3, "retired", "rs-1", &issuer[one]),
    );
    refresh(&profiles, &documents).await;
    assert_eq!(
        Realm::get(one).await.unwrap().unwrap().status,
        RealmStatus::Retired
    );

    // rs-1 is revoked: its Profiles lose their baseline. A Profile signed by
    // a still-listed key replaces it, even at a lower sequence.
    documents.insert(
        "https://acme.com/.well-known/actr".into(),
        acme.descriptor(&acme.member(2, &["rs-2"])),
    );
    documents.insert(
        "https://acme.com/realms/two".into(),
        acme.profile(two, 1, "active", "rs-2", &issuer[two]),
    );
    refresh(&profiles, &documents).await;
    let record = RealmProfileRecord::get(two).await.unwrap().unwrap();
    assert_eq!(record.sequence, 1);
    assert!(Realm::get(two).await.unwrap().unwrap().enabled);

    // Without any valid Profile the Realm is suspended.
    let stranger = Domain {
        keys: vec![("rs-2", SigningKey::from_bytes(&[9; 32]))],
    };
    documents.insert(
        "https://acme.com/realms/two".into(),
        stranger.profile(two, 9, "active", "rs-2", &issuer[two]),
    );
    documents.insert(
        "https://acme.com/.well-known/actr".into(),
        acme.descriptor(&acme.member(2, &["rs-2"])),
    );
    // Make the accepted Profile unverifiable: sign it with a key the domain
    // no longer lists.
    let old: RealmSigningKeys = serde_json::from_value(member.clone()).unwrap();
    let profile = RealmProfile::verify(
        &acme.profile(three, 1, "active", "rs-1", &issuer[three]),
        three,
        &old,
    )
    .unwrap();
    RealmProfileRecord::store(
        &profile,
        std::str::from_utf8(&acme.profile(three, 1, "active", "rs-1", &issuer[three])).unwrap(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    refresh(&profiles, &documents).await;
    let suspended = Realm::get(three).await.unwrap().unwrap();
    assert_eq!(suspended.status, RealmStatus::Active);
    assert!(!suspended.enabled);
    // `two` still verifies under rs-2 and keeps serving.
    assert!(Realm::get(two).await.unwrap().unwrap().enabled);

    // A newer Profile that no longer lists the Realm's issuer key stops
    // issuance: the Realm is suspended.
    let mut other_issuer = issuer[two].clone();
    other_issuer["keys"][0]["public_key"] = BASE64.encode([7u8; 32]).into();
    documents.insert(
        "https://acme.com/realms/two".into(),
        acme.profile(two, 2, "active", "rs-2", &other_issuer),
    );
    refresh(&profiles, &documents).await;
    assert_eq!(
        RealmProfileRecord::get(two)
            .await
            .unwrap()
            .unwrap()
            .sequence,
        2
    );
    assert!(!Realm::get(two).await.unwrap().unwrap().enabled);
}

mod management {
    use super::*;
    use actr_protocol::realm_management::{ManagementOp, ManagementRequest};
    use actr_protocol::realm_profile::sign_document;
    use issuer::realm_management::{ManagementError, handle};
    use platform::config::RealmConfig;

    /// `acme.com` lists rs-1 (active) and rs-2 (retired).
    fn acme() -> (Domain, Vec<u8>) {
        let acme = Domain {
            keys: vec![
                ("rs-1", SigningKey::from_bytes(&[1; 32])),
                ("rs-2", SigningKey::from_bytes(&[2; 32])),
            ],
        };
        let mut member = acme.member(1, &["rs-1", "rs-2"]);
        member["keys"][1]["status"] = "retired".into();
        member.as_object_mut().unwrap().remove("signature");
        let member = acme.sign(member, "rs-1");
        let descriptor = acme.descriptor(&member);
        (acme, descriptor)
    }

    fn request(op: ManagementOp, realm: &str, nonce: &str) -> ManagementRequest {
        ManagementRequest::new(op, realm, PROVIDER, nonce)
    }

    fn signed(request: &ManagementRequest, domain: &Domain, key_id: &str) -> Vec<u8> {
        let (_, key) = domain.keys.iter().find(|(id, _)| *id == key_id).unwrap();
        serde_json::to_vec(&request.sign(key_id, key)).unwrap()
    }

    async fn send(
        profiles: &RealmProfiles,
        config: &RealmConfig,
        descriptor: &[u8],
        document: &[u8],
    ) -> Result<Value, ManagementError> {
        let descriptor: Arc<[u8]> = Arc::from(descriptor);
        handle(profiles, config, document, |url: String| async move {
            assert_eq!(url, "https://acme.com/.well-known/actr");
            Ok(descriptor)
        })
        .await
    }

    fn create(realm: &str, nonce: &str, requester: Option<&str>) -> ManagementRequest {
        let mut request = request(ManagementOp::CreateRealm, realm, nonce);
        request.package = Some(package());
        request.requester = requester.map(str::to_string);
        request
    }

    /// The key holder signs `profile` and wraps it in `publish_profile`.
    fn publish(realm: &str, nonce: &str, mut profile: Value, domain: &Domain) -> Vec<u8> {
        let (_, key) = &domain.keys[0];
        sign_document(&mut profile, "rs-1", key);
        let mut request = request(ManagementOp::PublishProfile, realm, nonce);
        request.profile = Some(profile);
        signed(&request, domain, "rs-1")
    }

    /// `acme.com`'s descriptor with a Publisher Profile signed by `root`
    /// (`root-1`) whose package signing key is `package`.
    fn publisher_descriptor(root: &SigningKey, package: &SigningKey) -> Vec<u8> {
        let key = |id: &str, key: &SigningKey| {
            json!({
                "id": id, "algorithm": "Ed25519", "status": "active",
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
            })
        };
        let now = chrono::Utc::now();
        let mut profile = json!({
            "schema": "actr.publisher/v1",
            "sequence": 1,
            "domain": "acme.com",
            "profile_signing_keys": [key("root-1", root)],
            "keys": [key(
                &actr_protocol::package_key_id(package.verifying_key().as_bytes()),
                package,
            )],
            "profile_issued_at": (now - chrono::Duration::hours(1)).to_rfc3339(),
            "profile_expires_at": (now + chrono::Duration::days(30)).to_rfc3339(),
        });
        sign_document(&mut profile, "root-1", root);
        serde_json::to_vec(&json!({
            "version": 1,
            "authority": "actr://acme.com",
            "services": [{ "rel": "contracts", "href": "https://acme.com/contracts" }],
            "publisher": profile,
        }))
        .unwrap()
    }

    #[tokio::test]
    #[serial]
    async fn registers_publishers_from_their_publisher_profile() {
        let (profiles, _dir) = setup().await;
        let config = RealmConfig::default();
        let root = SigningKey::from_bytes(&[5; 32]);
        let package = SigningKey::from_bytes(&[6; 32]);
        let descriptor = publisher_descriptor(&root, &package);
        let register = |nonce: &str, key: &SigningKey| {
            let mut request = request(ManagementOp::RegisterPublisher, "", nonce);
            request.publisher = "acme.com".into();
            serde_json::to_vec(&request.sign("root-1", key)).unwrap()
        };

        // Signed by a key the Publisher Profile does not list.
        let stranger = SigningKey::from_bytes(&[7; 32]);
        let refused = send(&profiles, &config, &descriptor, &register("p-1", &stranger)).await;
        assert!(matches!(refused, Err(ManagementError::Forbidden(_))));

        let reply = send(&profiles, &config, &descriptor, &register("p-2", &root))
            .await
            .unwrap();
        assert_eq!(
            reply,
            json!({ "publisher": "acme.com", "status": "active", "sequence": 1 })
        );
        let pool = platform::storage::db::get_database().get_pool().clone();
        let registered = loran_publisher_registry::PublisherRegistryManager::new(pool)
            .resolve_by_name("acme.com")
            .await
            .unwrap();
        assert_eq!(
            registered.public_key,
            BASE64.encode(package.verifying_key().to_bytes())
        );

        // The nonce refuses a replay.
        let replay = send(&profiles, &config, &descriptor, &register("p-2", &root)).await;
        assert!(matches!(replay, Err(ManagementError::Invalid(_))));
        // A descriptor without a publisher member registers nothing.
        let (_, realm_only) = acme();
        let missing = send(&profiles, &config, &realm_only, &register("p-3", &root)).await;
        assert!(matches!(missing, Err(ManagementError::Forbidden(_))));
    }

    #[tokio::test]
    #[serial]
    async fn register_publisher_accepts_only_publisher_profile_signing_keys() {
        let (profiles, _dir) = setup().await;
        let config = RealmConfig::default();
        let root = SigningKey::from_bytes(&[5; 32]);
        // One descriptor carrying both the Realm signing keys (rs-1) and the
        // Publisher Profile (root-1).
        let (acme, realm_only) = acme();
        let mut descriptor: Value = serde_json::from_slice(&realm_only).unwrap();
        let publisher: Value = serde_json::from_slice(&publisher_descriptor(
            &root,
            &SigningKey::from_bytes(&[6; 32]),
        ))
        .unwrap();
        descriptor["publisher"] = publisher["publisher"].clone();
        let descriptor = serde_json::to_vec(&descriptor).unwrap();
        let register = |nonce: &str| {
            let mut request = request(ManagementOp::RegisterPublisher, "", nonce);
            request.publisher = "acme.com".into();
            request
        };

        // A Realm signing key of the same domain does not register it.
        let by_realm_key = send(
            &profiles,
            &config,
            &descriptor,
            &signed(&register("q-1"), &acme, "rs-1"),
        )
        .await;
        assert!(
            matches!(by_realm_key, Err(ManagementError::Forbidden(_))),
            "{by_realm_key:?}"
        );
        // Naming a realm too is refused.
        let mut with_realm = register("q-2");
        with_realm.realm = "a1@acme.com".into();
        let with_realm = serde_json::to_vec(&with_realm.sign("root-1", &root)).unwrap();
        assert!(matches!(
            send(&profiles, &config, &descriptor, &with_realm).await,
            Err(ManagementError::Invalid(_))
        ));
        // Refused requests leave their nonce unused.
        let document = serde_json::to_vec(&register("q-1").sign("root-1", &root)).unwrap();
        send(&profiles, &config, &descriptor, &document)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn creates_activates_and_retires_other_domains_realms() {
        let (profiles, _dir) = setup().await;
        let (acme, descriptor) = acme();
        let config = RealmConfig::default();
        let name = "k7m2q9xh@acme.com";
        let send = |document: Vec<u8>| {
            let (profiles, config, descriptor) = (&profiles, &config, &descriptor);
            async move { send(profiles, config, descriptor, &document).await }
        };

        let created = send(signed(&create(name, "n-1", Some("m-1")), &acme, "rs-1"))
            .await
            .unwrap();
        assert_eq!(created["status"], "pending");
        let draft = created["profile"].clone();
        let current = platform::realm::RealmIssuerKey::current(name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(draft["issuer"]["keys"][0]["id"], current.key_id.to_string());
        assert_eq!(draft["provider"]["id"], PROVIDER);
        assert_eq!(draft["package"], json!(package()));
        assert_eq!(
            Realm::get(name).await.unwrap().unwrap().status,
            RealmStatus::Pending
        );

        // Byte-level replay is refused by the nonce; a retry by the same
        // requester returns the state, another requester conflicts.
        let replay = send(signed(&create(name, "n-1", Some("m-1")), &acme, "rs-1")).await;
        assert!(matches!(replay, Err(ManagementError::Invalid(_))));
        let retry = send(signed(&create(name, "n-2", Some("m-1")), &acme, "rs-1"))
            .await
            .unwrap();
        assert_eq!(retry["status"], "pending");
        assert_eq!(retry["profile"]["issuer"], draft["issuer"]);
        let other = send(signed(&create(name, "n-3", Some("m-2")), &acme, "rs-1")).await;
        assert!(matches!(other, Err(ManagementError::Conflict(_))));

        // The first Profile activates the Realm and returns the owner invite once.
        let mut profile = draft.clone();
        profile["realm"]["display_name"] = "Team notes".into();
        let activated = send(publish(name, "n-4", profile.clone(), &acme))
            .await
            .unwrap();
        assert_eq!(activated["status"], "active");
        let token = activated["owner_invite"].as_str().unwrap().to_string();
        assert_eq!(invite_state(&token).await, Some((name.to_string(), false)));
        // Publishing it again conflicts and returns no invite.
        let again = send(publish(name, "n-5", profile.clone(), &acme)).await;
        assert!(matches!(again, Err(ManagementError::Conflict(_))));
        let active = send(signed(&create(name, "n-6", Some("m-1")), &acme, "rs-1"))
            .await
            .unwrap();
        assert_eq!(active, json!({ "realm": name, "status": "active" }));

        // Updates need a higher sequence and never return an invite.
        let mut second = profile.clone();
        second["sequence"] = 2.into();
        let updated = send(publish(name, "n-7", second, &acme)).await.unwrap();
        assert_eq!(updated["sequence"], 2);
        assert!(updated.get("owner_invite").is_none());
        let rollback = send(publish(name, "n-8", profile.clone(), &acme)).await;
        assert!(matches!(rollback, Err(ManagementError::Conflict(_))));

        // Rotating the issuer key: draft_profile lists the next key, and the
        // Actr Facility Provider switches to it only after the key holder published
        // a Profile listing it for the pre-publish period.
        let old_key = platform::realm::RealmIssuerKey::current(name)
            .await
            .unwrap()
            .unwrap();
        let next_key = profiles.rotate_issuer_key(name).await.unwrap();
        let drafted = send(signed(
            &request(ManagementOp::DraftProfile, name, "r-1"),
            &acme,
            "rs-1",
        ))
        .await
        .unwrap();
        let rotated = drafted["profile"].clone();
        assert_eq!(rotated["sequence"], 3);
        assert_eq!(rotated["realm"]["display_name"], "Team notes");
        assert_eq!(rotated["issuer"]["keys"].as_array().unwrap().len(), 2);
        let now = chrono::Utc::now().timestamp();
        let later = now + issuer::realm_profile::PREPUBLISH_SECS + 1;
        profiles.maintain_issuer_keys(later).await.unwrap();
        let current = platform::realm::RealmIssuerKey::current(name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.key_id, old_key.key_id, "not published yet");
        send(publish(name, "r-2", rotated, &acme)).await.unwrap();
        profiles.maintain_issuer_keys(now + 60).await.unwrap();
        profiles.maintain_issuer_keys(later + 60).await.unwrap();
        let current = platform::realm::RealmIssuerKey::current(name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.key_id, next_key);
        let drafted = send(signed(
            &request(ManagementOp::DraftProfile, name, "r-3"),
            &acme,
            "rs-1",
        ))
        .await
        .unwrap();
        assert_eq!(
            drafted["profile"]["issuer"]["keys"],
            json!([
                { "id": old_key.key_id.to_string(), "algorithm": "Ed25519",
                  "public_key": old_key.public_key, "status": "retired" },
                { "id": next_key.to_string(), "algorithm": "Ed25519",
                  "public_key": current.public_key, "status": "active" },
            ])
        );
        let mut second = drafted["profile"].clone();
        second["sequence"] = 2.into();
        let rollback = send(publish(name, "r-4", second, &acme)).await;
        assert!(matches!(rollback, Err(ManagementError::Conflict(_))));

        // Reissuing replaces the owner invite.
        let reissued = send(signed(
            &request(ManagementOp::ReissueOwnerInvite, name, "n-9"),
            &acme,
            "rs-1",
        ))
        .await
        .unwrap();
        let new_token = reissued["owner_invite"].as_str().unwrap();
        assert_eq!(invite_state(&token).await, Some((name.to_string(), true)));
        assert_eq!(
            invite_state(new_token).await,
            Some((name.to_string(), false))
        );

        // A retired Profile retires the Realm; the name is never reused.
        let mut retired = profile.clone();
        retired["sequence"] = 4.into();
        retired["realm"]["status"] = "retired".into();
        retired["issuer"] = drafted["profile"]["issuer"].clone();
        let done = send(publish(name, "n-10", retired, &acme)).await.unwrap();
        assert_eq!(done["status"], "retired");
        // Retirement drops the invites.
        assert_eq!(invite_state(new_token).await, None);
        let recreate = send(signed(&create(name, "n-11", Some("m-1")), &acme, "rs-1")).await;
        assert!(matches!(recreate, Err(ManagementError::Conflict(_))));
        let record = RealmProfileRecord::get(name).await.unwrap().unwrap();
        assert_eq!(record.sequence, 4);
    }

    #[tokio::test]
    #[serial]
    async fn rejects_unauthorized_or_malformed_requests() {
        let (profiles, _dir) = setup().await;
        let (acme, descriptor) = acme();
        let config = RealmConfig {
            max_pending_per_domain: Some(1),
            ..Default::default()
        };
        let send = |document: Vec<u8>| {
            let (profiles, config, descriptor) = (&profiles, &config, &descriptor);
            async move { send(profiles, config, descriptor, &document).await }
        };
        let name = "a1@acme.com";

        let mut elsewhere = create(name, "x-1", None);
        elsewhere.provider_id = "other.example".into();
        let mut expired = create(name, "x-2", None);
        expired.issued_at = "2020-01-01T00:00:00Z".into();
        expired.expires_at = "2020-01-01T00:05:00Z".into();
        let stranger = Domain {
            keys: vec![("rs-1", SigningKey::from_bytes(&[9; 32]))],
        };
        for (document, forbidden) in [
            (signed(&elsewhere, &acme, "rs-1"), false),
            (signed(&expired, &acme, "rs-1"), false),
            (
                serde_json::to_vec(&create(name, "x-3", None)).unwrap(),
                false,
            ),
            (signed(&create(name, "x-4", None), &stranger, "rs-1"), true),
            // A retired key no longer signs requests.
            (signed(&create(name, "x-5", None), &acme, "rs-2"), true),
            // Realms under the Provider domain are the operator's.
            (
                signed(
                    &create(&format!("a1@{PROVIDER}"), "x-6", None),
                    &acme,
                    "rs-1",
                ),
                true,
            ),
        ] {
            match send(document).await {
                Err(ManagementError::Forbidden(_)) if forbidden => {}
                Err(ManagementError::Invalid(_)) if !forbidden => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(Realm::get(name).await.unwrap().is_none());

        // The Profile must name this Actr Facility Provider's issuer key.
        let created = send(signed(&create(name, "x-7", None), &acme, "rs-1"))
            .await
            .unwrap();
        let mut foreign = created["profile"].clone();
        foreign["issuer"]["keys"][0]["public_key"] = BASE64
            .encode(SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes())
            .into();
        let refused = send(publish(name, "x-8", foreign, &acme)).await;
        assert!(matches!(refused, Err(ManagementError::Invalid(_))));
        assert_eq!(
            Realm::get(name).await.unwrap().unwrap().status,
            RealmStatus::Pending
        );

        // One pending Realm per domain here.
        let over = send(signed(&create("a2@acme.com", "x-9", None), &acme, "rs-1")).await;
        assert!(matches!(over, Err(ManagementError::Quota(_))));
        // A pending Realm cannot have an admin token yet.
        let early = send(signed(
            &request(ManagementOp::ReissueOwnerInvite, name, "x-10"),
            &acme,
            "rs-1",
        ))
        .await;
        assert!(matches!(early, Err(ManagementError::Conflict(_))));
    }

    /// Serve `descriptor` as `acme.com`'s authority descriptor.
    fn serve(
        descriptor: &[u8],
    ) -> impl FnOnce(String) -> std::future::Ready<Result<Arc<[u8]>, FetchError>> {
        let descriptor: Arc<[u8]> = Arc::from(descriptor);
        move |url| {
            assert_eq!(url, "https://acme.com/.well-known/actr");
            std::future::ready(Ok(descriptor))
        }
    }

    #[tokio::test]
    #[serial]
    async fn the_operator_confirms_a_realm_domain_identity_change() {
        use issuer::identity::{confirm_realm_domain_identity, realm_domain_identity};
        let (_profiles, _dir) = setup().await;
        let (_, pinned) = acme();
        let member = platform::realm::domain::member_from_descriptor("acme.com", &pinned).unwrap();
        platform::realm::RealmDomain::accept("acme.com", &member)
            .await
            .unwrap();

        // The domain starts over with a key that does not chain to rs-1.
        let changed = Domain {
            keys: vec![("rs-9", SigningKey::from_bytes(&[9; 32]))],
        };
        let descriptor = changed.descriptor(&changed.member(1, &["rs-9"]));
        let view = realm_domain_identity("acme.com", serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(view["outcome"], "identity_change");
        assert_eq!(view["pinned"]["keys"][0]["id"], "rs-1");
        assert_eq!(view["fetched"]["keys"][0]["id"], "rs-9");
        let fingerprint = |key: &SigningKey| {
            use sha2::Digest as _;
            hex::encode(sha2::Sha256::digest(key.verifying_key().to_bytes()))
        };
        assert_eq!(
            view["fetched"]["keys"][0]["fingerprint"],
            fingerprint(&changed.keys[0].1)
        );

        // Only the reviewed member is taken.
        let stale =
            confirm_realm_domain_identity("acme.com", "0".repeat(64).as_str(), serve(&descriptor))
                .await;
        assert!(
            matches!(stale, Err(ManagementError::Conflict(_))),
            "{stale:?}"
        );
        let digest = view["fetched"]["digest"].as_str().unwrap();
        let confirmed = confirm_realm_domain_identity("acme.com", digest, serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(confirmed["pinned"]["keys"][0]["id"], "rs-9");
        let view = realm_domain_identity("acme.com", serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(view["outcome"], "accepted");
        // Nothing to confirm any more.
        assert!(matches!(
            confirm_realm_domain_identity("acme.com", digest, serve(&descriptor)).await,
            Err(ManagementError::Conflict(_))
        ));
    }

    #[tokio::test]
    #[serial]
    async fn the_operator_confirms_a_publisher_identity_change() {
        use issuer::identity::{confirm_publisher_identity, publisher_identity};
        let (_profiles, _dir) = setup().await;
        let package = SigningKey::from_bytes(&[6; 32]);
        let first = publisher_descriptor(&SigningKey::from_bytes(&[5; 32]), &package);
        let profile = serde_json::from_slice::<actr_protocol::AuthorityDescriptor>(&first)
            .unwrap()
            .publisher
            .unwrap();
        let keys = profile.verify("acme.com", chrono::Utc::now()).unwrap();
        let pool = platform::storage::db::get_database().get_pool().clone();
        loran_publisher_registry::PublisherRegistryManager::new(pool)
            .register_publisher("acme.com", &profile, &keys)
            .await
            .unwrap();

        // Same id, a new key: refused under the pin.
        let descriptor = publisher_descriptor(&SigningKey::from_bytes(&[8; 32]), &package);
        let view = publisher_identity("acme.com", serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(view["outcome"], "rejected");
        assert_ne!(
            view["pinned"]["keys"][0]["fingerprint"],
            view["fetched"]["keys"][0]["fingerprint"]
        );
        let digest = view["fetched"]["digest"].as_str().unwrap();
        let confirmed = confirm_publisher_identity("acme.com", digest, serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(confirmed["pinned"], view["fetched"]);
        let view = publisher_identity("acme.com", serve(&descriptor))
            .await
            .unwrap();
        assert_eq!(view["outcome"], "accepted");
        assert!(matches!(
            publisher_identity("other.com", serve(&descriptor)).await,
            Err(ManagementError::NotFound(_))
        ));
    }
}

/// The Realm of an invite and whether it is revoked.
async fn invite_state(text: &str) -> Option<(String, bool)> {
    let secret = actr_protocol::realm_membership::RealmInvite::decode(text)
        .unwrap()
        .secret;
    let hash = actr_protocol::realm_membership::invite_secret_hash(&secret).unwrap();
    sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT realm, revoked_at FROM realm_invite WHERE secret_hash = ?",
    )
    .bind(hash)
    .fetch_optional(platform::storage::db::get_database().get_pool())
    .await
    .unwrap()
    .map(|(realm, revoked)| (realm, revoked.is_some()))
}

/// Simulate an interruption after switch commits but before republish completes.
#[tokio::test]
#[serial]
async fn rotation_republishes_after_interrupted_switch() {
    use platform::realm::RealmIssuerKey;
    let (profiles, _dir) = setup().await;
    let keys = profiles.signing_keys().unwrap().clone();
    let name = format!("interrupted@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;
    profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let old = RealmIssuerKey::current(&name).await.unwrap().unwrap();
    let next = profiles.rotate_issuer_key(&name).await.unwrap();
    let before = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    assert_eq!(before.sequence, 2);
    let now = chrono::Utc::now().timestamp();
    // Fast-forward the prepublication timer through persisted state.
    sqlx::query("UPDATE realm_issuer_key SET published_at = ? WHERE realm = ? AND state = 'next'")
        .bind(now - issuer::realm_profile::PREPUBLISH_SECS - 1)
        .bind(&name)
        .execute(platform::storage::db::get_database().get_pool())
        .await
        .unwrap();
    // This is the durable state left by a crash/RPC failure in the gap
    // between RealmIssuerKey::switch and RealmProfiles::republish.
    RealmIssuerKey::list(&name)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.key_id == next)
        .unwrap()
        .switch(now, now + 3600 + 300)
        .await
        .unwrap();
    assert_eq!(
        RealmIssuerKey::current(&name)
            .await
            .unwrap()
            .unwrap()
            .key_id,
        next
    );
    // On restart/background retry the healthy signer should reconcile the Profile.
    fail_profile_publication_twice(&profiles, &name, now + 60).await;
    profiles.maintain_issuer_keys(now + 60).await.unwrap();
    let after = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let profile = RealmProfile::verify(after.document.as_bytes(), &name, &keys).unwrap();
    assert_eq!(
        profile.issuer_key(old.key_id).unwrap().status,
        KeyStatus::Retired,
        "committed switch was not republished after maintenance resumed"
    );
    assert!(after.sequence > before.sequence);
}

/// Simulate interruption after retired key removal, before publishing the successor.
#[tokio::test]
#[serial]
async fn rotation_republishes_after_interrupted_removal() {
    use platform::realm::RealmIssuerKey;
    let (profiles, _dir) = setup().await;
    let keys = profiles.signing_keys().unwrap().clone();
    let name = format!("removeinterrupted@{PROVIDER}");
    insert(&name, RealmStatus::Pending).await;
    profiles
        .issue(
            &name,
            ProfileChange {
                package: Some(package()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let old = RealmIssuerKey::current(&name).await.unwrap().unwrap();
    let next = profiles.rotate_issuer_key(&name).await.unwrap();
    let now = chrono::Utc::now().timestamp();
    let switch_time = now - 3600 - 300 - 60;
    // Persist a due transition; create the signed switched Profile normally.
    sqlx::query("UPDATE realm_issuer_key SET published_at = ? WHERE realm = ? AND state = 'next'")
        .bind(switch_time - issuer::realm_profile::PREPUBLISH_SECS - 1)
        .bind(&name)
        .execute(platform::storage::db::get_database().get_pool())
        .await
        .unwrap();
    profiles.maintain_issuer_keys(switch_time).await.unwrap();
    assert_eq!(
        RealmIssuerKey::current(&name)
            .await
            .unwrap()
            .unwrap()
            .key_id,
        next
    );
    let before = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let old_profile = RealmProfile::verify(before.document.as_bytes(), &name, &keys).unwrap();
    assert_eq!(
        old_profile.issuer_key(old.key_id).unwrap().status,
        KeyStatus::Retired
    );
    // The persistent DB state after the old key was removed and republish was lost.
    RealmIssuerKey::remove(&name, old.key_id).await.unwrap();
    // A signing RPC that started before removal cannot restore its stale snapshot.
    assert!(matches!(
        RealmProfileRecord::store_issued(
            &old_profile,
            &before.document,
            Some(&before.digest),
            None,
        )
        .await,
        Err(platform::realm::RealmError::Conflict(_))
    ));
    fail_profile_publication_twice(&profiles, &name, now).await;
    profiles.maintain_issuer_keys(now).await.unwrap();
    let after = RealmProfileRecord::get(&name).await.unwrap().unwrap();
    let profile = RealmProfile::verify(after.document.as_bytes(), &name, &keys).unwrap();
    assert!(
        profile.issuer_key(old.key_id).is_none(),
        "removed key remained in published Profile after maintenance resumed"
    );
    assert!(after.sequence > before.sequence);
}

async fn fail_profile_publication_twice(profiles: &RealmProfiles, realm: &str, now: i64) {
    let before = RealmProfileRecord::get(realm).await.unwrap().unwrap();
    let pool = platform::storage::db::get_database().get_pool();
    // Use a persistent trigger: maintenance may acquire another pooled connection.
    sqlx::query("CREATE TRIGGER fail_profile_publish_all BEFORE UPDATE ON realm_profile BEGIN SELECT RAISE(FAIL, 'injected Profile store failure'); END")
        .execute(pool).await.unwrap();
    for _ in 0..2 {
        profiles.maintain_issuer_keys(now).await.unwrap();
        assert_eq!(
            RealmProfileRecord::get(realm)
                .await
                .unwrap()
                .unwrap()
                .digest,
            before.digest
        );
    }
    sqlx::query("DROP TRIGGER fail_profile_publish_all")
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn removed_profile_key_cannot_verify_or_be_redistributed_from_cache() {
    use actr_protocol::prost::Message;
    use platform::aid::credential::validator::AIdCredentialValidator as Validator;
    static CACHE_DIR: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
    let cache_dir = CACHE_DIR.get_or_init(|| TempDir::new().unwrap());
    let (_profiles, _dir) = setup().await;
    Validator::init(cache_dir.path()).await.unwrap();
    let name = "cache-revocation@acme.com";
    insert(name, RealmStatus::Active).await;
    let domain = Domain {
        keys: vec![("rs-1", SigningKey::from_bytes(&[31; 32]))],
    };
    let member = domain.member(1, &["rs-1"]);
    let old = SigningKey::from_bytes(&[32; 32]);
    let current = SigningKey::from_bytes(&[33; 32]);
    let key = |id: &str, status: &str, key: &SigningKey| {
        json!({
            "id": id, "algorithm": "Ed25519", "status": status,
            "public_key": BASE64.encode(key.verifying_key().to_bytes())
        })
    };
    let issuer = json!({"url": "https://provider.example/issuer", "keys": [
        key("901", "retired", &old), key("902", "active", &current)
    ]});
    accept(
        name,
        &domain.profile(name, 1, "active", "rs-1", &issuer),
        &member,
    )
    .await;
    let expires_at = chrono::Utc::now().timestamp() as u64 + 3600;
    Validator::populate_key(name, 901, &old.verifying_key(), expires_at)
        .await
        .unwrap();
    let claims = actr_protocol::IdentityClaims {
        realm: name.into(),
        actor_id: format!("1.{name}"),
        expires_at,
        protocol_version: 1,
        issuer: name.into(),
        provider: PROVIDER.into(),
        audience: "actr.signaling".into(),
        actor_type: actr_protocol::ActrType::from_string_repr("acme.com/notes@1.0.0").unwrap(),
        member: "member".into(),
        client_key: "aaaaaaaaaaaaaaaa".into(),
        role: actr_protocol::MemberRole::Owner as i32,
        development: None,
    }
    .encode_to_vec();
    let credential = actr_protocol::AIdCredential {
        key_id: 901,
        signature: old.sign(&claims).to_bytes().to_vec().into(),
        claims: claims.into(),
    };
    // Listed retired keys retain their normal verification semantics.
    Validator::check(&credential, name, PROVIDER, "actr.signaling")
        .await
        .unwrap();
    assert!(Validator::get_key_bytes(name, 901).await.unwrap().is_some());
    let issuer =
        json!({"url": "https://provider.example/issuer", "keys": [key("902", "active", &current)]});
    accept(
        name,
        &domain.profile(name, 2, "active", "rs-1", &issuer),
        &member,
    )
    .await;
    assert!(Realm::get(name).await.unwrap().unwrap().is_active());
    let cache = platform::aid::KeyCache::new(cache_dir.path().join("signaling_key_cache.db"))
        .await
        .unwrap();
    assert!(cache.get_cached_key(name, 901).await.unwrap().is_none());
    // A delayed issuer write or cache reopened after a restart cannot undo revocation.
    cache
        .cache_key(name, 901, &old.verifying_key(), expires_at)
        .await
        .unwrap();
    assert!(
        Validator::check(&credential, name, PROVIDER, "actr.signaling")
            .await
            .is_err()
    );
    assert!(Validator::get_key_bytes(name, 901).await.unwrap().is_none());
    // A matching id with another public key is also insufficient.
    Validator::populate_key(name, 902, &old.verifying_key(), expires_at)
        .await
        .unwrap();
    assert!(Validator::get_key_bytes(name, 902).await.unwrap().is_none());
    Validator::populate_key(name, 902, &current.verifying_key(), expires_at)
        .await
        .unwrap();
    assert!(Validator::get_key_bytes(name, 902).await.unwrap().is_some());
}

/// Interleave maintenance's eligible-key read with an accepted withdrawal,
/// then republication. Neither stale snapshot may activate the next key.
#[tokio::test]
#[serial]
async fn issuer_key_switch_rechecks_withdrawal_and_continuous_publication() {
    use platform::realm::{IssuerKeyState, RealmIssuerKey};
    let (profiles, _dir) = setup().await;
    let acme = Domain {
        keys: vec![("rs-1", SigningKey::from_bytes(&[1; 32]))],
    };
    let member = acme.member(1, &["rs-1"]);
    let name = "switch-race@acme.com";
    insert(name, RealmStatus::Active).await;
    let before = issuer_of(&profiles, name).await;
    accept(
        name,
        &acme.profile(name, 1, "active", "rs-1", &before),
        &member,
    )
    .await;
    let old = RealmIssuerKey::current(name).await.unwrap().unwrap();
    let next_id = profiles.rotate_issuer_key(name).await.unwrap();
    let rotating = issuer_of(&profiles, name).await;
    accept(
        name,
        &acme.profile(name, 2, "active", "rs-1", &rotating),
        &member,
    )
    .await;
    let now = chrono::Utc::now().timestamp();
    let pool = platform::storage::db::get_database().get_pool();
    sqlx::query("UPDATE realm_issuer_key SET published_at = ? WHERE realm = ? AND state = 'next'")
        .bind(now - issuer::realm_profile::PREPUBLISH_SECS - 1)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let eligible = RealmIssuerKey::list(name)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.state == IssuerKeyState::Next)
        .unwrap();
    assert!(eligible.published_at.unwrap() + issuer::realm_profile::PREPUBLISH_SECS <= now);

    accept(
        name,
        &acme.profile(name, 3, "active", "rs-1", &before),
        &member,
    )
    .await;
    assert!(
        eligible.switch(now, now + 3900).await.is_err(),
        "a withdrawn key was activated"
    );
    assert_eq!(
        RealmIssuerKey::current(name).await.unwrap().unwrap().key_id,
        old.key_id
    );
    // Checking the Profile is necessary even if stale publication metadata remains.
    sqlx::query("UPDATE realm_issuer_key SET published_at = ? WHERE realm = ? AND state = 'next'")
        .bind(eligible.published_at)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    assert!(
        eligible.switch(now, now + 3900).await.is_err(),
        "an unlisted key was activated"
    );
    sqlx::query(
        "UPDATE realm_issuer_key SET published_at = NULL WHERE realm = ? AND state = 'next'",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();

    accept(
        name,
        &acme.profile(name, 4, "active", "rs-1", &rotating),
        &member,
    )
    .await;
    assert!(
        eligible.switch(now, now + 3900).await.is_err(),
        "republication reused the old prepublication period"
    );
    let republished = RealmIssuerKey::list(name)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.state == IssuerKeyState::Next)
        .unwrap();
    let ready_at = republished.published_at.unwrap() + issuer::realm_profile::PREPUBLISH_SECS;
    assert!(
        republished
            .switch(ready_at - 1, ready_at + 3900)
            .await
            .is_err()
    );
    assert_eq!(
        RealmIssuerKey::current(name).await.unwrap().unwrap().key_id,
        old.key_id
    );
    assert!(
        eligible.switch(ready_at, ready_at + 3900).await.is_err(),
        "a stale snapshot must not activate a republished key even after its new interval elapses"
    );
    republished.switch(ready_at, ready_at + 3900).await.unwrap();
    assert_eq!(
        RealmIssuerKey::current(name).await.unwrap().unwrap().key_id,
        next_id
    );
}
