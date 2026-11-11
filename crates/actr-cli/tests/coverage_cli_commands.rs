//! Integration coverage for low-dependency CLI command paths.
//!
//! These tests intentionally exercise the real `actr` binary from isolated
//! temporary directories so clap dispatch, filesystem IO, and rendered command
//! results are all covered without relying on network services.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn actr_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_actr"))
}

fn run_actr(args: &[&str], cwd: &Path, home: &Path) -> Output {
    Command::new(actr_bin())
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "off")
        .output()
        .expect("failed to run actr binary")
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn clean_stdout(output: &Output) -> String {
    strip_ansi(&stdout(output))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn strip_ansi(input: &str) -> String {
    let mut clean = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        clean.push(ch);
    }
    clean
}

fn first_json_object(output: &Output) -> serde_json::Value {
    let text = stdout(output);
    let start = text.find('{').expect("stdout should contain a JSON object");
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return serde_json::from_str(&text[start..end])
                        .expect("stdout JSON object should parse");
                }
            }
            _ => {}
        }
    }

    panic!("stdout JSON object did not terminate: {text}");
}

fn isolated_home(root: &Path) -> PathBuf {
    let home = root.join("home");
    fs::create_dir_all(&home).expect("create isolated home");
    home
}

fn write_manifest_with_proto(root: &Path) {
    fs::create_dir_all(root.join("proto")).expect("create proto dir");
    fs::write(
        root.join("proto/echo.proto"),
        r#"syntax = "proto3";
package echo;

service EchoService {
  rpc Echo (EchoRequest) returns (EchoReply);
}

message EchoRequest {
  string message = 1;
}

message EchoReply {
  string message = 1;
}
"#,
    )
    .expect("write proto");

    fs::write(
        root.join("manifest.toml"),
        r#"edition = 1
exports = ["proto/echo.proto"]

[package]
name = "echo-service"
publisher = "acme"
version = "0.1.0"
description = "Echo service"

[dependencies]
"#,
    )
    .expect("write manifest");
}

#[test]
fn config_local_scope_round_trips_values_and_validates() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    let set = run_actr(
        &[
            "config",
            "--local",
            "set",
            "network.realm",
            "dev@example.com",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&set, "config local set");
    assert!(clean_stdout(&set).contains("Updated local config"));

    let config_path = tmp.path().join(".actr/config.toml");
    let saved = fs::read_to_string(&config_path).expect("read local config");
    assert!(
        saved.contains("realm = \"dev@example.com\""),
        "saved config:\n{saved}"
    );

    let get = run_actr(
        &["config", "--local", "get", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_success(&get, "config local get");
    assert_eq!(clean_stdout(&get).trim(), "\"dev@example.com\"");

    let show = run_actr(
        &["config", "--local", "show", "--format", "json"],
        tmp.path(),
        &home,
    );
    assert_success(&show, "config local show json");
    let show_json: serde_json::Value =
        serde_json::from_str(clean_stdout(&show).trim()).expect("show output should be JSON");
    assert_eq!(show_json["network"]["realm"], "dev@example.com");

    let test = run_actr(&["config", "--local", "test"], tmp.path(), &home);
    assert_success(&test, "config local test");
    assert!(clean_stdout(&test).contains("Local config syntax and schema are valid"));

    let unset = run_actr(
        &["config", "--local", "unset", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_success(&unset, "config local unset");

    let get_missing = run_actr(
        &["config", "--local", "get", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_failure(&get_missing, "config local get after unset");
    assert!(stderr(&get_missing).contains("Configuration key 'network.realm' not found"));
}

#[test]
fn config_global_scope_round_trips_and_list_shows_effective_values() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Set a value in the global scope (writes to ~/.actr/config.toml).
    let set = run_actr(
        &[
            "config",
            "--global",
            "set",
            "network.realm",
            "qa@example.com",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&set, "config global set");
    let global_config = home.join(".actr/config.toml");
    assert!(global_config.exists(), "global config should be written");
    let saved = fs::read_to_string(&global_config).expect("read global config");
    assert!(saved.contains("realm"), "global config:\n{saved}");

    let get = run_actr(
        &["config", "--global", "get", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_success(&get, "config global get");
    assert!(clean_stdout(&get).contains("qa@example.com"));

    // list shows effective (merged) config.
    let list = run_actr(&["config", "list"], tmp.path(), &home);
    assert_success(&list, "config list");
    let list_out = clean_stdout(&list);
    assert!(list_out.contains("network.realm"), "list: {list_out}");
    assert!(list_out.contains("publisher.name"), "list: {list_out}");

    let global_unset = run_actr(
        &["config", "--global", "unset", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_success(&global_unset, "config global unset");

    let get_missing = run_actr(
        &["config", "--global", "get", "network.realm"],
        tmp.path(),
        &home,
    );
    assert_failure(&get_missing, "config global get after unset");
}

#[test]
fn config_show_supports_yaml_and_test_validates_all_scopes() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Show in YAML format.
    let show = run_actr(
        &["config", "--local", "show", "--format", "yaml"],
        tmp.path(),
        &home,
    );
    assert_success(&show, "config show yaml");
    assert!(
        !clean_stdout(&show).trim().is_empty(),
        "show yaml should produce output"
    );

    // Test merged scope (both global and local are validated).
    let test = run_actr(&["config", "test"], tmp.path(), &home);
    assert_success(&test, "config test merged");
    assert!(clean_stdout(&test).contains("valid"));

    // Test global-only scope.
    let test_global = run_actr(&["config", "--global", "test"], tmp.path(), &home);
    assert_success(&test_global, "config test global");
    assert!(clean_stdout(&test_global).contains("valid"));

    // Test local-only scope.
    let test_local = run_actr(&["config", "--local", "test"], tmp.path(), &home);
    assert_success(&test_local, "config test local");
    assert!(clean_stdout(&test_local).contains("valid"));
}

#[test]
fn build_reports_missing_manifest_and_missing_binary_section() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // No manifest → error.
    let no_manifest = run_actr(&["build"], tmp.path(), &home);
    assert_failure(&no_manifest, "build no manifest");
    assert!(
        stderr(&no_manifest).contains("manifest.toml not found"),
        "build no manifest stderr:\n{}",
        stderr(&no_manifest)
    );

    // Manifest without [binary] → error.
    write_manifest_with_proto(tmp.path());
    let no_binary = run_actr(
        &["build", "--manifest-path", "manifest.toml"],
        tmp.path(),
        &home,
    );
    assert_failure(&no_binary, "build no binary");
    assert!(
        stderr(&no_binary).contains("manifest.toml is missing [binary]"),
        "build no binary stderr:\n{}",
        stderr(&no_binary)
    );
}

#[test]
fn config_rejects_unknown_keys_and_bad_values() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    let bad_key = run_actr(
        &["config", "--local", "set", "nope.nope", "x"],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_key, "config unknown key");
    assert!(stderr(&bad_key).contains("Unknown configuration key"));

    let bad_bool = run_actr(
        &["config", "--local", "set", "cache.auto_lock", "maybe"],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_bool, "config bad bool");
    assert!(stderr(&bad_bool).contains("expects a boolean"));

    let bad_int = run_actr(
        &["config", "--local", "set", "network.realm", "4242"],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_int, "config bad realm");
    assert!(stderr(&bad_int).contains("expects a Realm name"));
}

#[test]
fn fingerprint_text_and_proto_yaml_covers_remaining_formats() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // Service-level in YAML format (already tested via existing test, but
    // text format with service fingerprint is not)
    let svc_text = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--format",
            "text",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&svc_text, "fingerprint svc text");
    assert!(
        clean_stdout(&svc_text).contains("Service Semantic Fingerprint"),
        "svc text: {}",
        clean_stdout(&svc_text)
    );

    // Proto YAML format was not covered.
    let proto_yaml = run_actr(
        &[
            "registry",
            "fingerprint",
            "--proto",
            "proto/echo.proto",
            "--format",
            "yaml",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&proto_yaml, "fingerprint proto yaml");
    assert!(
        clean_stdout(&proto_yaml).contains("proto_file:"),
        "proto yaml: {}",
        clean_stdout(&proto_yaml)
    );
}

#[test]
fn deps_install_validates_actr_type_format_and_fingerprint_requirements() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // --actr-type with bad format → rejected before network.
    let bad_type = run_actr(
        &["deps", "install", "my-alias", "--actr-type", "bad-format"],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_type, "deps install bad actr_type");
    assert!(
        stderr(&bad_type).contains("Invalid actr_type format"),
        "bad type stderr:\n{}",
        stderr(&bad_type)
    );

    // --fingerprint without --actr-type → rejected.
    let fp_no_type = run_actr(
        &["deps", "install", "pkg", "--fingerprint", "fp"],
        tmp.path(),
        &home,
    );
    assert_failure(&fp_no_type, "deps install fingerprint no type");
    assert!(
        stderr(&fp_no_type).contains("--fingerprint requires specifying --actr-type"),
        "fp no type stderr:\n{}",
        stderr(&fp_no_type)
    );
}

#[test]
fn deps_install_rejects_actr_type_without_alias() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    let no_alias = run_actr(
        &["deps", "install", "--actr-type", "acme/Echo@1.0.0"],
        tmp.path(),
        &home,
    );
    assert_failure(&no_alias, "deps install actr_type no alias");
    assert!(
        stderr(&no_alias).contains("must provide an alias"),
        "no alias stderr:\n{}",
        stderr(&no_alias)
    );
}

#[test]
fn fingerprint_verify_passes_with_matching_lock_file() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // Get the service-level and proto-level fingerprints.
    let svc_out = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&svc_out, "svc fingerprint");
    let svc_json = first_json_object(&svc_out);
    let svc_fp = svc_json["service_fingerprint"]
        .as_str()
        .expect("service_fingerprint");

    let proto_out = run_actr(
        &[
            "registry",
            "fingerprint",
            "--proto",
            "proto/echo.proto",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&proto_out, "proto fingerprint");
    let proto_json = first_json_object(&proto_out);
    let proto_fp = proto_json["fingerprint"].as_str().expect("fingerprint");

    // Write a lock file with matching fingerprints (no prefix to avoid
    // the service_semantic: prefix comparison).
    let lock = format!(
        r#"[metadata]
version = 2
generated_at = "2026-08-06T00:00:00Z"

[[dependency]]
name = "echo"
alias = "echo"
actr_type = "acme/EchoService@1.0.0"
fingerprint = "{svc_fp}"
cached_at = "2026-08-06T00:00:00Z"

[[dependency.files]]
path = "proto/echo.proto"
fingerprint = "{proto_fp}"
"#
    );
    fs::write(tmp.path().join("manifest.lock.toml"), &lock).expect("write lock");

    let verify = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--verify",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&verify, "fingerprint verify passed");
    let verify_json = first_json_object(&verify);
    assert_eq!(
        verify_json["verification"]["status"], "passed",
        "verification should pass for matching lock file: {verify_json}"
    );
    assert_eq!(
        verify_json["verification"]["matched_fingerprint"], "all_files_verified",
        "matching lock file should verify every file: {verify_json}"
    );
}

#[test]
fn init_rust_echo_service_produces_expected_files() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let project = tmp.path().join("my-echo");

    let output = run_actr(
        &[
            "init",
            "my-echo",
            "--template",
            "echo",
            "--role",
            "service",
            "--signaling",
            "ws://localhost:8080",
            "--realm",
            "dev@example.com",
            "--publisher",
            "test-org",
            "--language",
            "rust",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&output, "init rust echo");
    assert!(
        project.join("manifest.toml").exists(),
        "manifest.toml missing"
    );
    assert!(project.join("Cargo.toml").exists(), "Cargo.toml missing");
    let manifest = fs::read_to_string(project.join("manifest.toml")).unwrap();
    assert!(manifest.contains("test-org"), "manifest:\n{manifest}");
}

#[test]
fn registry_fingerprint_reports_service_json_and_lock_mismatches() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    let output = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&output, "registry fingerprint json");
    let json = first_json_object(&output);
    assert_eq!(json["proto_files"][0], "echo.proto");
    assert_eq!(json["verification"]["status"], "not_requested");

    fs::write(
        tmp.path().join("manifest.lock.toml"),
        r#"[metadata]
version = 2
generated_at = "2026-08-06T00:00:00Z"

[[dependency]]
name = "remote-echo"
alias = "remote-echo"
actr_type = "acme/EchoService@1.0.0"
fingerprint = "service_semantic:stale"
cached_at = "2026-08-06T00:00:00Z"

[[dependency.files]]
path = "proto/echo.proto"
fingerprint = "semantic:stale"
"#,
    )
    .expect("write lock");

    let verify = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--format",
            "json",
            "--verify",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&verify, "registry fingerprint verify json");
    let verify_json = first_json_object(&verify);
    assert_eq!(verify_json["verification"]["status"], "failed");
    let mismatches = verify_json["verification"]["mismatches"]
        .as_array()
        .expect("mismatches should be an array");
    assert!(
        mismatches
            .iter()
            .any(|item| item["file_path"] == "proto/echo.proto")
    );
    assert!(
        mismatches
            .iter()
            .any(|item| item["file_path"] == "SERVICE_FINGERPRINT")
    );
}

#[test]
fn registry_fingerprint_supports_proto_yaml_and_missing_proto_errors() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    let output = run_actr(
        &[
            "registry",
            "fingerprint",
            "--proto",
            "proto/echo.proto",
            "--format",
            "yaml",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&output, "registry fingerprint proto yaml");
    let yaml = clean_stdout(&output);
    assert!(
        yaml.contains("proto_file: proto/echo.proto"),
        "yaml:\n{yaml}"
    );
    assert!(yaml.contains("fingerprint:"), "yaml:\n{yaml}");

    let missing = run_actr(
        &["registry", "fingerprint", "--proto", "proto/missing.proto"],
        tmp.path(),
        &home,
    );
    assert_failure(&missing, "registry fingerprint missing proto");
    assert!(stderr(&missing).contains("Proto file not found: proto/missing.proto"));
}

#[test]
fn doc_generates_static_pages_from_manifest_and_proto_tree() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());
    fs::create_dir_all(tmp.path().join("protos/local")).expect("create doc proto dir");
    fs::copy(
        tmp.path().join("proto/echo.proto"),
        tmp.path().join("protos/local/echo.proto"),
    )
    .expect("copy doc proto");

    let output = run_actr(&["doc", "--output", "docs-out"], tmp.path(), &home);
    assert_success(&output, "doc generation");

    for page in ["index.html", "api.html", "config.html"] {
        let path = tmp.path().join("docs-out").join(page);
        assert!(path.exists(), "expected generated page: {}", path.display());
    }

    let api = fs::read_to_string(tmp.path().join("docs-out/api.html")).expect("read api page");
    assert!(api.contains("EchoService"), "api page:\n{api}");
}

#[test]
fn pkg_keygen_writes_key_config_and_rejects_existing_key_without_force() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let key_path = tmp.path().join("keys/dev-key.json");
    let key_arg = key_path.to_string_lossy().into_owned();

    let output = run_actr(&["pkg", "keygen", "--output", &key_arg], tmp.path(), &home);
    assert_success(&output, "pkg keygen");
    assert!(key_path.exists(), "key was not written");

    let key_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&key_path).expect("read key"))
            .expect("key should be JSON");
    assert!(key_json["private_key"].as_str().is_some());
    assert!(key_json["public_key"].as_str().is_some());

    let global_config =
        fs::read_to_string(home.join(".actr/config.toml")).expect("read global config");
    assert!(
        global_config.contains("keychain"),
        "global config:\n{global_config}"
    );
    assert!(
        global_config.contains(&key_arg),
        "global config:\n{global_config}"
    );

    let duplicate = run_actr(&["pkg", "keygen", "--output", &key_arg], tmp.path(), &home);
    assert_failure(&duplicate, "duplicate pkg keygen");
    assert!(stderr(&duplicate).contains("Key file already exists"));

    let force = run_actr(
        &["pkg", "keygen", "--output", &key_arg, "--force"],
        tmp.path(),
        &home,
    );
    assert_success(&force, "forced pkg keygen");
}

#[test]
fn pkg_keygen_rejects_directory_output_path() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let key_dir = tmp.path().join("key-chain");
    fs::create_dir_all(&key_dir).expect("create key dir");
    let dir_arg = key_dir.to_string_lossy().into_owned();

    let output = run_actr(&["pkg", "keygen", "--output", &dir_arg], tmp.path(), &home);
    assert_failure(&output, "pkg keygen with directory output");
    assert!(
        stderr(&output).contains("Key output path is a directory"),
        "stderr:\n{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("--force"),
        "stderr should not offer --force for a directory:\n{}",
        stderr(&output)
    );

    // --force cannot rescue a directory target; it must fail the same way.
    let forced = run_actr(
        &["pkg", "keygen", "--output", &dir_arg, "--force"],
        tmp.path(),
        &home,
    );
    assert_failure(&forced, "forced pkg keygen with directory output");
    assert!(
        stderr(&forced).contains("Key output path is a directory"),
        "stderr:\n{}",
        stderr(&forced)
    );
}

#[test]
fn pkg_keygen_with_publisher_writes_binding() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let key_path = tmp.path().join("keys/dev-key.json");
    let key_arg = key_path.to_string_lossy().into_owned();

    // --publisher binds the keychain to that publisher (issue #425).
    let with_publisher = run_actr(
        &["pkg", "keygen", "--output", &key_arg, "--publisher", "acme"],
        tmp.path(),
        &home,
    );
    assert_success(&with_publisher, "pkg keygen with publisher");
    let key_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&key_path).expect("read key"))
            .expect("key should be JSON");
    assert_eq!(key_json["publisher"].as_str(), Some("acme"));

    // Without --publisher the field is omitted (backward compatible).
    let unbound_path = tmp.path().join("keys/unbound.json");
    let unbound_arg = unbound_path.to_string_lossy().into_owned();
    let without_publisher = run_actr(
        &["pkg", "keygen", "--output", &unbound_arg],
        tmp.path(),
        &home,
    );
    assert_success(&without_publisher, "pkg keygen without publisher");
    let unbound_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&unbound_path).expect("read key"))
            .expect("unbound key should be JSON");
    assert!(
        unbound_json.get("publisher").is_none(),
        "unbound keychain must not carry a publisher field"
    );

    // Empty publisher is rejected.
    let empty_path = tmp.path().join("keys/empty.json");
    let empty_arg = empty_path.to_string_lossy().into_owned();
    let empty = run_actr(
        &["pkg", "keygen", "--output", &empty_arg, "--publisher", "  "],
        tmp.path(),
        &home,
    );
    assert_failure(&empty, "pkg keygen empty publisher");
    assert!(stderr(&empty).contains("--publisher cannot be empty"));
}

#[test]
fn pkg_sign_rejects_cross_publisher_keychain_and_accepts_matching() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // Keychain bound to "rival" but manifest publisher is "acme".
    let rival_key = tmp.path().join("keys/rival.json");
    let rival_arg = rival_key.to_string_lossy().into_owned();
    let gen_rival = run_actr(
        &[
            "pkg",
            "keygen",
            "--output",
            &rival_arg,
            "--publisher",
            "rival",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&gen_rival, "gen rival key");

    let mismatch = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "manifest.toml",
            "--key",
            &rival_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&mismatch, "pkg sign cross-publisher");
    let err = stderr(&mismatch);
    assert!(
        err.contains("Publisher mismatch"),
        "pkg sign mismatch stderr:\n{err}"
    );
    // The signature must not be written when the check fails.
    assert!(!tmp.path().join("manifest.sig").exists());

    // Matching binding → signs successfully.
    let acme_key = tmp.path().join("keys/acme.json");
    let acme_arg = acme_key.to_string_lossy().into_owned();
    let gen_acme = run_actr(
        &[
            "pkg",
            "keygen",
            "--output",
            &acme_arg,
            "--publisher",
            "acme",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&gen_acme, "gen acme key");

    // Remove a stale signature from any prior run before re-signing.
    let _ = fs::remove_file(tmp.path().join("manifest.sig"));

    let signed = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "manifest.toml",
            "--key",
            &acme_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_success(&signed, "pkg sign matching publisher");
    assert!(clean_stdout(&signed).contains("Manifest signed successfully"));
    assert!(tmp.path().join("manifest.sig").exists());
}

#[test]
fn version_prints_human_and_json_forms() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    let human = run_actr(&["version"], tmp.path(), &home);
    assert_success(&human, "version human");
    let human_out = clean_stdout(&human);
    assert!(
        human_out.starts_with("actr "),
        "human version output:\n{human_out}"
    );

    let json = run_actr(&["version", "--json"], tmp.path(), &home);
    assert_success(&json, "version json");
    let json_value: serde_json::Value =
        serde_json::from_str(clean_stdout(&json).trim()).expect("version json should parse");
    assert!(json_value["version"].as_str().is_some(), "version field");
    assert!(json_value["git_hash"].as_str().is_some(), "git_hash field");
    assert!(json_value["git_date"].as_str().is_some(), "git_date field");
}

#[test]
fn completion_emits_scripts_for_every_shell() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = run_actr(&["completion", shell], tmp.path(), &home);
        assert_success(&output, &format!("completion {shell}"));
        assert!(
            !clean_stdout(&output).trim().is_empty(),
            "completion {shell} produced empty output"
        );
    }
}

#[test]
fn ps_reports_no_runtimes_for_isolated_hyper_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let hyper = tmp.path().join("hyper");
    let hyper_arg = hyper.to_string_lossy().into_owned();

    let output = run_actr(&["ps", "--hyper-dir", &hyper_arg], tmp.path(), &home);
    assert_success(&output, "ps empty");
    assert!(
        clean_stdout(&output).contains("No detached runtimes found."),
        "ps empty output:\n{}",
        clean_stdout(&output)
    );

    // `--all` on an empty store should behave the same.
    let all = run_actr(
        &["ps", "--all", "--hyper-dir", &hyper_arg],
        tmp.path(),
        &home,
    );
    assert_success(&all, "ps all empty");
    assert!(clean_stdout(&all).contains("No detached runtimes found."));
}

#[test]
fn runtime_lifecycle_commands_reject_short_and_unknown_wid() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let hyper = tmp.path().join("hyper");
    let hyper_arg = hyper.to_string_lossy().into_owned();

    // Short WID prefix is rejected before any lookup.
    let short = run_actr(
        &["logs", "abc", "--hyper-dir", &hyper_arg],
        tmp.path(),
        &home,
    );
    assert_failure(&short, "logs short wid");
    assert!(
        stderr(&short).contains("WID prefix must be at least 8 characters"),
        "logs short wid stderr:\n{}",
        stderr(&short)
    );

    // Unknown WID prefix → no record found, for every lifecycle command.
    let unknown = "aaaaaaaa";
    for cmd in ["logs", "stop", "rm", "start", "restart"] {
        let output = run_actr(
            &[cmd, unknown, "--hyper-dir", &hyper_arg],
            tmp.path(),
            &home,
        );
        assert_failure(&output, &format!("{cmd} unknown wid"));
        assert!(
            stderr(&output).contains("No runtime record found for WID prefix"),
            "{cmd} unknown wid stderr:\n{}",
            stderr(&output)
        );
    }
}

/// Generate a dev signing key via the CLI and return its path argument.
fn gen_key(root: &Path, home: &Path) -> String {
    let key_path = root.join("keys/dev-key.json");
    let key_arg = key_path.to_string_lossy().into_owned();
    let output = run_actr(&["pkg", "keygen", "--output", &key_arg], root, home);
    assert_success(&output, "pkg keygen helper");
    key_arg
}

#[test]
fn dlq_list_stats_and_purge_handle_empty_database_and_bad_input() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let db_arg = tmp.path().join("dlq.db").to_string_lossy().into_owned();

    // Empty database → list reports empty.
    let list = run_actr(&["dlq", "list", "--db", &db_arg], tmp.path(), &home);
    assert_success(&list, "dlq list empty");
    assert!(clean_stdout(&list).contains("DLQ is empty (no matching records)."));

    // Invalid --after timestamp → rejected.
    let bad_after = run_actr(
        &["dlq", "list", "--db", &db_arg, "--after", "not-a-timestamp"],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_after, "dlq list bad after");
    assert!(
        stderr(&bad_after).contains("must be a valid RFC 3339 timestamp"),
        "dlq list bad after stderr:\n{}",
        stderr(&bad_after)
    );

    // Stats on empty database.
    let stats = run_actr(&["dlq", "stats", "--db", &db_arg], tmp.path(), &home);
    assert_success(&stats, "dlq stats empty");
    let stats_out = clean_stdout(&stats);
    assert!(
        stats_out.contains("DLQ Statistics"),
        "stats output:\n{stats_out}"
    );
    assert!(stats_out.contains("Total messages:           0"));

    // Purge without id or --all → rejected.
    let purge_none = run_actr(&["dlq", "purge", "--db", &db_arg], tmp.path(), &home);
    assert_failure(&purge_none, "dlq purge none");
    assert!(
        stderr(&purge_none).contains("Specify a record ID, or pass --all"),
        "dlq purge none stderr:\n{}",
        stderr(&purge_none)
    );

    // Purge --all on empty database → purges zero.
    let purge_all = run_actr(
        &["dlq", "purge", "--all", "--db", &db_arg],
        tmp.path(),
        &home,
    );
    assert_success(&purge_all, "dlq purge all empty");
    assert!(clean_stdout(&purge_all).contains("Purged 0 DLQ record(s)."));

    // Purge --all with bad --before → rejected.
    let purge_bad_before = run_actr(
        &["dlq", "purge", "--all", "--before", "nope", "--db", &db_arg],
        tmp.path(),
        &home,
    );
    assert_failure(&purge_bad_before, "dlq purge bad before");
    assert!(
        stderr(&purge_bad_before).contains("must be a valid RFC 3339 timestamp"),
        "dlq purge bad before stderr:\n{}",
        stderr(&purge_bad_before)
    );
}

#[test]
fn dlq_show_and_replay_reject_invalid_uuid_and_missing_records() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let db_arg = tmp.path().join("dlq.db").to_string_lossy().into_owned();

    // Invalid UUID → rejected.
    let bad_uuid = run_actr(
        &["dlq", "show", "not-a-uuid", "--db", &db_arg],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_uuid, "dlq show bad uuid");
    assert!(
        stderr(&bad_uuid).contains("Invalid UUID: 'not-a-uuid'"),
        "dlq show bad uuid stderr:\n{}",
        stderr(&bad_uuid)
    );

    // Valid UUID but no record → not found.
    let valid_uuid = "11111111-1111-1111-1111-111111111111";
    let missing = run_actr(
        &["dlq", "show", valid_uuid, "--db", &db_arg],
        tmp.path(),
        &home,
    );
    assert_failure(&missing, "dlq show missing");
    assert!(
        stderr(&missing).contains("No DLQ record found with ID:"),
        "dlq show missing stderr:\n{}",
        stderr(&missing)
    );

    // Replay with missing record → not found.
    let replay_missing = run_actr(
        &["dlq", "replay", valid_uuid, "--db", &db_arg],
        tmp.path(),
        &home,
    );
    assert_failure(&replay_missing, "dlq replay missing");
    assert!(
        stderr(&replay_missing).contains("No DLQ record found with ID:"),
        "dlq replay missing stderr:\n{}",
        stderr(&replay_missing)
    );
}

#[test]
fn pkg_sign_validates_manifest_and_signs_on_success() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());
    let key_arg = gen_key(tmp.path(), &home);

    // Missing manifest → error.
    let missing = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "missing.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&missing, "pkg sign missing manifest");
    assert!(stderr(&missing).contains("manifest.toml not found"));

    // Invalid TOML → error.
    fs::write(tmp.path().join("bad.toml"), "invalid = {{{").expect("write bad toml");
    let bad = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "bad.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&bad, "pkg sign bad toml");
    assert!(stderr(&bad).contains("Invalid manifest.toml"));

    // Missing [package] → error.
    fs::write(tmp.path().join("no_package.toml"), "edition = 1\n").expect("write no_package");
    let no_pkg = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "no_package.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&no_pkg, "pkg sign no package");
    assert!(
        stderr(&no_pkg).contains("manifest.toml missing [package] section"),
        "pkg sign no package stderr:\n{}",
        stderr(&no_pkg)
    );

    // Missing publisher field → error.
    fs::write(
        tmp.path().join("partial.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .expect("write partial");
    let partial = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "partial.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&partial, "pkg sign partial");
    assert!(
        stderr(&partial).contains("[package].publisher missing"),
        "pkg sign partial stderr:\n{}",
        stderr(&partial)
    );

    // Valid manifest → signs successfully and writes a 64-byte signature.
    let signed = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "manifest.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );
    assert_success(&signed, "pkg sign success");
    assert!(clean_stdout(&signed).contains("Manifest signed successfully"));
    let sig_path = tmp.path().join("manifest.sig");
    assert!(sig_path.exists(), "manifest.sig was not written");
    let sig_len = fs::metadata(&sig_path).expect("read sig metadata").len();
    assert_eq!(sig_len, 64, "signature should be 64 raw Ed25519 bytes");
}

#[test]
fn pkg_sign_authenticates_v2_dependency_lock() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    let key_arg = gen_key(tmp.path(), &home);
    fs::write(
        tmp.path().join("manifest.toml"),
        r#"edition = 1

[package]
name = "client"
publisher = "acme"
version = "0.1.0"

[dependencies]
echo = { actr_type = "acme/EchoService@1.0.0", service = "EchoService:service_semantic:echo" }
"#,
    )
    .expect("write manifest");
    fs::write(
        tmp.path().join("manifest.lock.toml"),
        r#"[metadata]
version = 2
generated_at = "2026-08-06T00:00:00Z"

[[dependency]]
name = "EchoService"
alias = "echo"
actr_type = "acme/EchoService@1.0.0"
fingerprint = "service_semantic:echo"
cached_at = "2026-08-06T00:00:00Z"

[[dependency.files]]
path = "echo/echo.proto"
fingerprint = "semantic:echo"
"#,
    )
    .expect("write lock");

    let signed = run_actr(
        &[
            "pkg",
            "sign",
            "--manifest-path",
            "manifest.toml",
            "--key",
            &key_arg,
        ],
        tmp.path(),
        &home,
    );

    assert_success(&signed, "pkg sign dependency lock");
    let signed_manifest = fs::read_to_string(tmp.path().join("manifest.toml")).unwrap();
    let package_manifest = actr_pack::PackageManifest::from_toml(&signed_manifest)
        .expect("signed package manifest should parse");
    let lock_entry = package_manifest
        .lock_file
        .expect("signed package manifest should authenticate the lock file");
    assert_eq!(lock_entry.path, "manifest.lock.toml");
    let lock_bytes = fs::read(tmp.path().join("manifest.lock.toml")).unwrap();
    assert_eq!(lock_entry.hash, hex::encode(Sha256::digest(lock_bytes)));
}

#[test]
fn pkg_sign_without_key_reports_missing_configuration() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // No --key and no config keychain → error before manifest parsing.
    let no_key = run_actr(
        &["pkg", "sign", "--manifest-path", "manifest.toml"],
        tmp.path(),
        &home,
    );
    assert_failure(&no_key, "pkg sign no key");
    assert!(
        stderr(&no_key).contains("No signing key configured"),
        "pkg sign no key stderr:\n{}",
        stderr(&no_key)
    );
}

#[test]
fn pkg_verify_reports_unreadable_package() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Nonexistent package → error before key resolution.
    let missing = run_actr(
        &["pkg", "verify", "--package", "nonexistent.actr"],
        tmp.path(),
        &home,
    );
    assert_failure(&missing, "pkg verify missing package");
    assert!(
        stderr(&missing).contains("Failed to read package"),
        "pkg verify missing stderr:\n{}",
        stderr(&missing)
    );
}

#[test]
fn gen_validates_input_and_language_compatibility_without_a_lock() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // Codegen reads dependency aliases from manifest.toml; no lock is needed.
    // The default input `protos` does not exist, so validation reaches the
    // input-path error instead of asking for an artificial lock file.
    let bad_input = run_actr(&["gen"], tmp.path(), &home);
    assert_failure(&bad_input, "gen bad input");
    assert!(
        stderr(&bad_input).contains("Input path does not exist"),
        "gen bad input stderr:\n{}",
        stderr(&bad_input)
    );

    // Empty protos directory → no proto files found.
    fs::create_dir_all(tmp.path().join("protos")).expect("create protos dir");
    let empty_protos = run_actr(&["gen"], tmp.path(), &home);
    assert_failure(&empty_protos, "gen empty protos");
    assert!(
        stderr(&empty_protos).contains("No proto files found"),
        "gen empty protos stderr:\n{}",
        stderr(&empty_protos)
    );

    // Cargo.toml present → detected as rust; requesting swift → refused.
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .expect("write cargo toml");
    let lang_mismatch = run_actr(&["gen", "-l", "swift"], tmp.path(), &home);
    assert_failure(&lang_mismatch, "gen lang mismatch");
    assert!(
        stderr(&lang_mismatch).contains("Refusing to generate"),
        "gen lang mismatch stderr:\n{}",
        stderr(&lang_mismatch)
    );
}

#[test]
fn registry_fingerprint_covers_text_no_exports_and_unsupported_format() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());
    write_manifest_with_proto(tmp.path());

    // Service-level text output with exports.
    let text = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&text, "fingerprint text");
    assert!(
        clean_stdout(&text).contains("Service Semantic Fingerprint"),
        "fingerprint text output:\n{}",
        clean_stdout(&text)
    );

    // Proto-level text output.
    let proto_text = run_actr(
        &[
            "registry",
            "fingerprint",
            "--proto",
            "proto/echo.proto",
            "--format",
            "text",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&proto_text, "fingerprint proto text");
    assert!(clean_stdout(&proto_text).contains("Proto Semantic Fingerprint"));

    // Unsupported format → error.
    let bad_fmt = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "manifest.toml",
            "--format",
            "xml",
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&bad_fmt, "fingerprint bad format");
    assert!(stderr(&bad_fmt).contains("Unsupported format: xml"));

    // Manifest with no exports.
    fs::write(
        tmp.path().join("empty.toml"),
        "edition = 1\n\n[package]\nname = \"empty-service\"\npublisher = \"acme\"\nversion = \"0.1.0\"\ndescription = \"Empty\"\n",
    )
    .expect("write empty manifest");

    let no_exports_text = run_actr(
        &["registry", "fingerprint", "--manifest-path", "empty.toml"],
        tmp.path(),
        &home,
    );
    assert_success(&no_exports_text, "fingerprint no exports text");
    assert!(clean_stdout(&no_exports_text).contains("No proto files found in exports"));

    let no_exports_json = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "empty.toml",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&no_exports_json, "fingerprint no exports json");
    assert_eq!(first_json_object(&no_exports_json)["status"], "no_exports");

    // No exports + verify → no_lock_file status.
    let no_lock = run_actr(
        &[
            "registry",
            "fingerprint",
            "--manifest-path",
            "empty.toml",
            "--verify",
            "--format",
            "json",
        ],
        tmp.path(),
        &home,
    );
    assert_success(&no_lock, "fingerprint no lock verify");
    assert_eq!(first_json_object(&no_lock)["status"], "no_lock_file");
}

#[test]
fn actr_without_subcommand_prints_help() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    let output = run_actr(&[], tmp.path(), &home);
    assert_success(&output, "actr no subcommand");
    let out = clean_stdout(&output);
    assert!(
        out.contains("Actor-RTC Command Line Tool"),
        "help output:\n{out}"
    );
    assert!(
        out.contains("development:") || out.contains("init"),
        "help output:\n{out}"
    );
}

#[test]
fn completion_invalid_shell_is_rejected() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Invalid shell name → clap validation rejects it.
    let bad_shell = run_actr(&["completion", "nope"], tmp.path(), &home);
    assert_failure(&bad_shell, "completion bad shell");
}

#[test]
fn doc_generates_default_pages_without_manifest_and_rejects_subdir() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // No manifest.toml in cwd and none above → generate default docs.
    let output = run_actr(&["doc", "--output", "docs-out"], tmp.path(), &home);
    assert_success(&output, "doc no manifest");
    assert!(
        tmp.path().join("docs-out/index.html").exists(),
        "default doc index.html should be generated"
    );

    // manifest.toml in a parent dir → refuse to run from a subdir.
    let parent = tmp.path().join("parent");
    fs::create_dir_all(parent.join("child")).expect("create parent/child");
    fs::write(
        parent.join("manifest.toml"),
        "edition = 1\n\n[package]\nname = \"x\"\npublisher = \"acme\"\nversion = \"0.1.0\"\n",
    )
    .expect("write parent manifest");
    let subdir = run_actr(
        &["doc", "--output", "docs-out"],
        &parent.join("child"),
        &home,
    );
    assert_failure(&subdir, "doc subdir");
    assert!(
        stderr(&subdir).contains("Please run 'actr doc' from the workload root"),
        "doc subdir stderr:\n{}",
        stderr(&subdir)
    );
}

#[test]
fn deps_install_in_empty_dir_fails_without_writing_project_config() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Empty directory → no manifest, so the command must fail without mutating
    // project-local config. The error must identify the missing manifest.toml
    // and the current directory, not the internal DI registration failure.
    let output = run_actr(&["deps", "install"], tmp.path(), &home);
    assert_failure(&output, "deps install non-project");
    let err = strip_ansi(&stderr(&output));
    assert!(
        err.contains("manifest.toml not found"),
        "expected manifest.toml-not-found error, got stderr:\n{err}"
    );
    let cwd = std::fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    let cwd_str = cwd.to_string_lossy();
    assert!(
        err.contains(&*cwd_str),
        "expected current directory {cwd:?} in error, got stderr:\n{err}"
    );
    assert!(
        !err.contains("ConfigManager"),
        "must not leak internal DI error, got stderr:\n{err}"
    );
    assert!(
        !tmp.path().join(".actr/config.toml").exists(),
        "deps install in an empty directory must not write project config"
    );
}

#[test]
fn deps_install_with_only_actr_toml_explains_manifest_required() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Only actr.toml present — insufficient because dependencies live in
    // manifest.toml. The error must explain that manifest.toml is required.
    fs::write(
        tmp.path().join("actr.toml"),
        "[network]\nsignaling_url = \"wss://example/signaling\"\nais_endpoint = \"http://example/issuer\"\n",
    )
    .expect("write actr.toml");

    let output = run_actr(&["deps", "install"], tmp.path(), &home);
    assert_failure(&output, "deps install with only actr.toml");
    let err = strip_ansi(&stderr(&output));
    assert!(
        err.contains("manifest.toml is required"),
        "expected manifest.toml-required error, got stderr:\n{err}"
    );
    assert!(
        !err.contains("ConfigManager"),
        "must not leak internal DI error, got stderr:\n{err}"
    );
    assert!(
        !tmp.path().join("manifest.toml").exists(),
        "must not create manifest.toml"
    );
}

#[test]
fn standalone_discover_skips_project_config_requirement() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // Standalone discover (--endpoint --realm --realm-client-key) must bypass
    // the project-config requirement even in an empty directory. It fails on
    // network unreachability (localhost:1), not on missing configuration.
    let key_path = tmp.path().join("realm-client.json");
    let key_file = actr_protocol::realm_membership::RealmClientKeyFile::new(
        "dev@example.com",
        "localhost:1",
        "http://localhost:1",
        &ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
    );
    std::fs::write(&key_path, serde_json::to_string(&key_file).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let output = run_actr(
        &[
            "registry",
            "discover",
            "--list-only",
            "--endpoint",
            "http://localhost:1",
            "--realm",
            "dev@example.com",
            "--realm-client-key",
            key_path.to_str().unwrap(),
        ],
        tmp.path(),
        &home,
    );
    assert_failure(&output, "standalone discover (unreachable)");
    let err = strip_ansi(&stderr(&output));
    // Must bypass the project-config check and reach the network layer — not
    // leak the internal "X not registered" DI invariant if the standalone
    // container build ever regresses.
    assert!(
        !err.contains("No project configuration")
            && !err.contains("manifest.toml not found")
            && !err.contains("not registered"),
        "standalone discover must skip project-config check and not leak DI errors, got stderr:\n{err}"
    );
    assert!(
        err.contains("HTTP") || err.contains("request") || err.contains("localhost"),
        "standalone discover should fail on network unreachability, got stderr:\n{err}"
    );
}

#[test]
fn check_in_empty_dir_reports_missing_manifest_not_di_error() {
    let tmp = TempDir::new().expect("tempdir");
    let home = isolated_home(tmp.path());

    // `actr check` in a non-project directory must surface the missing
    // manifest.toml, not the internal "ConfigManager not registered" DI
    // error (#388).
    let output = run_actr(&["check"], tmp.path(), &home);
    assert_failure(&output, "check non-project");
    let err = strip_ansi(&stderr(&output));
    assert!(
        err.contains("manifest.toml not found"),
        "expected manifest.toml-not-found error, got stderr:\n{err}"
    );
    assert!(
        !err.contains("ConfigManager") && !err.contains("not registered"),
        "must not leak internal DI error, got stderr:\n{err}"
    );
}
