//! Drive the offline Publisher tooling through the real CLI and verify its
//! output with the same protocol entry points used by Actr Facility Providers.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use actr_protocol::realm_management::{ManagementOp, ManagementRequest};
use actr_protocol::{KeyPinOutcome, KeyStatus, PublisherProfile};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actr"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("USERPROFILE", dir)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "off")
        .output()
        .expect("run actr")
}

fn succeeds(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is only JSON")
}

fn generate(dir: &Path) -> Value {
    succeeds(run(
        dir,
        &[
            "publisher",
            "profile",
            "keygen",
            "--key-id",
            "publisher-root-1",
            "--output",
            "profile-key.json",
        ],
    ))
}

fn package_public(dir: &Path, seed: u8) -> String {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let public_key = BASE64.encode(key.verifying_key().to_bytes());
    fs::write(
        dir.join("package-public.json"),
        json!({ "public_key": public_key }).to_string(),
    )
    .unwrap();
    public_key
}

fn sign(dir: &Path, extra: &[&str]) -> Output {
    sign_for_domain(dir, "Apps.Example.COM", extra)
}

fn sign_for_domain(dir: &Path, domain: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "publisher",
        "profile",
        "sign",
        "--key",
        "profile-key.json",
        "--domain",
        domain,
        "--package-key",
        "package-public.json",
        "--security-contact",
        "mailto:security@apps.example.com",
    ];
    args.extend_from_slice(extra);
    run(dir, &args)
}

#[test]
fn profile_keygen_protects_existing_keys_and_does_not_configure_package_signing() {
    let dir = tempfile::tempdir().unwrap();
    let public = generate(dir.path());
    assert_eq!(public["id"], "publisher-root-1");
    assert_eq!(public["status"], "active");
    assert_eq!(public["algorithm"], "Ed25519");
    assert!(public.get("private_key").is_none());
    assert!(!dir.path().join(".actr").exists());

    let path = dir.path().join("profile-key.json");
    let before = fs::read(&path).unwrap();
    let file: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(file["public_key"], public["public_key"]);
    let refused = run(
        dir.path(),
        &[
            "publisher",
            "profile",
            "keygen",
            "--key-id",
            "replacement",
            "--output",
            "profile-key.json",
        ],
    );
    assert!(!refused.status.success());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));

    let replaced = succeeds(run(
        dir.path(),
        &[
            "publisher",
            "profile",
            "keygen",
            "--key-id",
            "replacement",
            "--output",
            "profile-key.json",
            "--force",
        ],
    ));
    assert_ne!(replaced["public_key"], public["public_key"]);
    assert_ne!(fs::read(&path).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn signs_an_acceptable_profile_and_a_compatible_registration_without_private_output() {
    let dir = tempfile::tempdir().unwrap();
    let public = generate(dir.path());
    let package = package_public(dir.path(), 7);
    let document = succeeds(sign(dir.path(), &[]));
    assert_eq!(document["domain"], "apps.example.com");
    assert_eq!(document["profile_signing_keys"][0], public);
    assert_eq!(document["keys"][0]["public_key"], package);
    let package_bytes: [u8; 32] = BASE64.decode(package).unwrap().try_into().unwrap();
    assert_eq!(
        document["keys"][0]["id"],
        actr_pack::compute_key_id(&package_bytes)
    );
    let profile: PublisherProfile = serde_json::from_value(document.clone()).unwrap();
    let (keys, pins) = profile
        .accept("apps.example.com", Utc::now(), None)
        .unwrap();
    let KeyPinOutcome::Accepted(pins) = pins else {
        panic!("first acceptance")
    };
    assert_eq!(keys.sequence, 1);

    let file: Value =
        serde_json::from_slice(&fs::read(dir.path().join("profile-key.json")).unwrap()).unwrap();
    assert!(
        !document
            .to_string()
            .contains(file["private_key"].as_str().unwrap())
    );
    assert!(!document.to_string().contains("private_key"));
    assert!(!dir.path().join(".actr").exists());
    let request = succeeds(run(
        dir.path(),
        &[
            "realm",
            "register-publisher",
            "--key",
            "profile-key.json",
            "--publisher",
            "apps.example.com",
            "--provider",
            "infra.example.com",
            "--print",
        ],
    ));
    let request = ManagementRequest::parse(request.to_string().as_bytes()).unwrap();
    request.verify(&keys.profile_signing_keys).unwrap();
    assert_eq!(request.op, ManagementOp::RegisterPublisher);

    let successor = succeeds(sign(
        dir.path(),
        &["--sequence", "2", "--valid-for-days", "60"],
    ));
    let successor: PublisherProfile = serde_json::from_value(successor).unwrap();
    successor
        .accept("apps.example.com", Utc::now(), Some(&pins))
        .unwrap();
    let mut tampered = document;
    tampered["security_contact"] = "mailto:attacker@example.com".into();
    let tampered: PublisherProfile = serde_json::from_value(tampered).unwrap();
    assert!(tampered.verify("apps.example.com", Utc::now()).is_err());
    assert!(profile.verify("other.example", Utc::now()).is_err());
}

#[test]
fn accepts_existing_package_keychains_and_preserves_retired_keys() {
    let dir = tempfile::tempdir().unwrap();
    generate(dir.path());
    // The existing pkg keygen keychain format. Avoid its global-config writes
    // here: Windows resolves its home through a known-folder API.
    let package_key = SigningKey::from_bytes(&[9; 32]);
    let keychain = json!({
        "private_key": BASE64.encode(package_key.to_bytes()),
        "public_key": BASE64.encode(package_key.verifying_key().to_bytes()),
        "publisher": "apps.example.com",
    });
    fs::write(dir.path().join("package-public.json"), keychain.to_string()).unwrap();
    let retired = SigningKey::from_bytes(&[8; 32]);
    fs::write(
        dir.path().join("retired.json"),
        json!({
            "public_key": BASE64.encode(retired.verifying_key().to_bytes()),
            "status": "retired",
            "not_after": "2026-01-01T00:00:00Z",
        })
        .to_string(),
    )
    .unwrap();
    let document = succeeds(sign(dir.path(), &["--package-key", "retired.json"]));
    assert_eq!(document["keys"][0]["public_key"], keychain["public_key"]);
    assert!(
        !document
            .to_string()
            .contains(keychain["private_key"].as_str().unwrap())
    );
    let profile: PublisherProfile = serde_json::from_value(document).unwrap();
    let keys = profile.verify("apps.example.com", Utc::now()).unwrap();
    assert_eq!(keys.keys[1].status, KeyStatus::Retired);
    assert_eq!(
        keys.keys[1].not_after.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
}

#[test]
fn rejects_mismatched_keys_and_invalid_profile_inputs() {
    let dir = tempfile::tempdir().unwrap();
    generate(dir.path());
    package_public(dir.path(), 7);
    for domain in [
        "https://apps.example.com",
        "apps.example.com/path",
        "apps.example.com:443",
    ] {
        assert!(
            !sign_for_domain(dir.path(), domain, &[]).status.success(),
            "{domain}"
        );
    }
    for extra in [
        vec!["--sequence", "0"],
        vec!["--sequence", "9007199254740992"],
        vec!["--valid-for-days", "0"],
        vec!["--valid-for-days", "4294967295"],
        vec!["--package-key", "package-public.json"],
    ] {
        assert!(!sign(dir.path(), &extra).status.success(), "{extra:?}");
    }
    let package_path = dir.path().join("package-public.json");
    let original: Value = serde_json::from_slice(&fs::read(&package_path).unwrap()).unwrap();
    for (field, value) in [
        ("publisher", "other.example"),
        ("public_key", "invalid"),
        ("id", "actr-pub-wrong"),
        ("algorithm", "RSA"),
        ("status", "unknown"),
        ("not_before", "invalid"),
    ] {
        let mut bad = original.clone();
        bad[field] = value.into();
        fs::write(&package_path, bad.to_string()).unwrap();
        assert!(!sign(dir.path(), &[]).status.success(), "{field}");
    }
    fs::write(&package_path, original.to_string()).unwrap();
    let key_path = dir.path().join("profile-key.json");
    let mut key: Value = serde_json::from_slice(&fs::read(&key_path).unwrap()).unwrap();
    key["public_key"] = original["public_key"].clone();
    fs::write(&key_path, key.to_string()).unwrap();
    let refused = sign(dir.path(), &[]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("does not match"));
}

#[cfg(unix)]
#[test]
fn refuses_a_profile_key_that_others_can_read() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    generate(dir.path());
    package_public(dir.path(), 7);
    fs::set_permissions(
        dir.path().join("profile-key.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let refused = sign(dir.path(), &[]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("chmod 600"));
}
