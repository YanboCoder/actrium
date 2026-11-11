//! Smoke tests for the CLI command shape.
//!
//! These tests verify that the user-facing command surface matches the
//! documented organization principles (see `crates/actr-cli/README.md`). They do NOT
//! exercise behaviour — they only assert that command tree itself is intact,
//! so renames / regressions show up fast.

use clap::CommandFactory;

fn cli() -> clap::Command {
    actr_cli::cli::Cli::command()
}

fn top_level_names() -> Vec<String> {
    cli()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect()
}

fn subcommand_names(parent: &str) -> Vec<String> {
    cli()
        .get_subcommands()
        .find(|c| c.get_name() == parent)
        .unwrap_or_else(|| panic!("top-level subcommand {parent} not found"))
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect()
}

#[test]
fn top_level_has_expected_commands() {
    let names = top_level_names();

    // Development (flat, high-frequency)
    for name in ["init", "gen", "build", "check", "doc"] {
        assert!(
            names.contains(&name.to_string()),
            "missing top-level `{name}`"
        );
    }
    // Runtime (flat, docker-style)
    for name in ["run", "ps", "logs", "start", "stop", "restart", "rm"] {
        assert!(
            names.contains(&name.to_string()),
            "missing top-level `{name}`"
        );
    }
    // Resource groups
    for name in ["deps", "pkg", "registry", "dlq", "realm", "publisher"] {
        assert!(names.contains(&name.to_string()), "missing group `{name}`");
    }
    // Meta
    for name in ["config", "version", "completion"] {
        assert!(names.contains(&name.to_string()), "missing meta `{name}`");
    }
}

#[test]
fn legacy_commands_are_gone() {
    let names = top_level_names();
    // `install` moved under `deps`; `ops`/`fingerprint` no longer top-level.
    for gone in ["install", "ops", "fingerprint", "discover", "discovery"] {
        assert!(
            !names.contains(&gone.to_string()),
            "`{gone}` should not be a top-level command anymore"
        );
    }
}

#[test]
fn deps_group_has_install() {
    let subs = subcommand_names("deps");
    assert!(
        subs.contains(&"install".to_string()),
        "deps.install missing"
    );
}

#[test]
fn pkg_group_exposes_pack_sign_verify_keygen() {
    let mut subs = subcommand_names("pkg");
    subs.sort();
    assert_eq!(
        subs,
        vec![
            "keygen".to_string(),
            "pack".to_string(),
            "sign".to_string(),
            "verify".to_string()
        ],
        "pkg should expose pack/sign/verify/keygen (build stays top-level)"
    );
}

#[test]
fn registry_group_has_discover_fingerprint() {
    let mut subs = subcommand_names("registry");
    subs.sort();
    assert_eq!(
        subs,
        vec!["discover".to_string(), "fingerprint".to_string()]
    );
    let error = cli()
        .try_get_matches_from(["actr", "registry", "publish"])
        .expect_err("package publication was removed");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

#[test]
fn realm_group_exposes_the_signing_and_membership_tools() {
    let mut subs = subcommand_names("realm");
    subs.sort();
    assert_eq!(
        subs,
        [
            "create-request",
            "draft-request",
            "invite",
            "join",
            "keygen",
            "leave",
            "members",
            "register-publisher",
            "reissue-request",
            "remove-member",
            "retire",
            "revoke-client",
            "revoke-invite",
            "set-role",
            "sign-profile",
            "transfer-owner",
        ]
    );
}

#[test]
fn dlq_group_exposes_list_show_stats_replay_purge() {
    let mut subs = subcommand_names("dlq");
    subs.sort();
    assert_eq!(
        subs,
        vec![
            "list".to_string(),
            "purge".to_string(),
            "replay".to_string(),
            "show".to_string(),
            "stats".to_string(),
        ]
    );
}

#[test]
fn build_accepts_manifest_path() {
    let cli = cli();
    let build = cli
        .get_subcommands()
        .find(|c| c.get_name() == "build")
        .expect("build cmd");
    let has_manifest_path = build
        .get_arguments()
        .any(|a| a.get_long() == Some("manifest-path"));
    assert!(has_manifest_path, "actr build must accept --manifest-path");

    // The old `-f/--file` spelling must be gone.
    let has_file = build.get_arguments().any(|a| a.get_long() == Some("file"));
    assert!(!has_file, "actr build must not expose --file anymore");
}

#[test]
fn run_hides_internal_flags() {
    let cli = cli();
    let run = cli
        .get_subcommands()
        .find(|c| c.get_name() == "run")
        .expect("run cmd");
    for arg in run.get_arguments() {
        let Some(long) = arg.get_long() else { continue };
        if long.starts_with("internal-") {
            assert!(arg.is_hide_set(), "--{long} must be hidden");
        }
    }
}
