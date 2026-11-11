//! `actr pkg` — local package operations (pack, sign, verify, keygen).
//!
//! ## Subcommands
//!
//! ```text
//! actr pkg pack     [--manifest-path FILE] [--payload-root DIR] --output FILE
//! actr pkg sign     [--manifest-path FILE] [--key FILE] [--binary FILE]
//! actr pkg verify   --package FILE [--pubkey FILE]
//! actr pkg keygen   [--output FILE] [--force]
//! ```
//!
//! End-to-end build + package is a single top-level command: `actr build`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use clap::{Args, Subcommand};
use ed25519_dalek::SigningKey;

use crate::commands::package_build::{
    binary_kind_for_target, ensure_publisher_matches, load_signing_key_with_publisher,
    load_verifying_key, load_verifying_key_from_dev_key, resolve_key_path, validated_packaged_lock,
};
use crate::core::{Command, CommandContext, CommandResult, ComponentType};

#[derive(Args, Debug)]
pub struct PkgArgs {
    #[command(subcommand)]
    pub command: PkgCommand,
}

#[derive(Subcommand, Debug)]
pub enum PkgCommand {
    /// Build and sign a versioned actr-package/2 archive from a manifest and payload tree.
    Pack(PkgPackArgs),
    /// Sign a manifest.toml manifest with an Publisher Registry private key (offline signing).
    Sign(PkgSignArgs),
    /// Verify a signed .actr package.
    Verify(PkgVerifyArgs),
    /// Generate an Ed25519 Publisher signing key pair.
    Keygen(PkgKeygenArgs),
}

#[derive(Args, Debug)]
pub struct PkgPackArgs {
    /// Path to an actr-package/2 manifest.toml template.
    #[arg(
        long = "manifest-path",
        short = 'm',
        default_value = "manifest.toml",
        value_name = "FILE"
    )]
    pub manifest_path: PathBuf,

    /// Directory containing every path declared by [[files]].
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub payload_root: PathBuf,

    /// Path to Publisher Registry signing key file (overrides config publisher.keychain).
    #[arg(long, short = 'k', value_name = "FILE")]
    pub key: Option<PathBuf>,

    /// Output .actr package path.
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: PathBuf,
}

#[derive(Args, Debug)]
pub struct PkgSignArgs {
    /// Path to manifest.toml
    #[arg(
        long = "manifest-path",
        short = 'm',
        default_value = "manifest.toml",
        value_name = "FILE"
    )]
    pub manifest_path: PathBuf,

    /// Path to Publisher signing key file
    #[arg(long, short = 'k', value_name = "FILE")]
    pub key: Option<PathBuf>,

    /// Path to actor binary (for hash computation)
    #[arg(long, short = 'b', value_name = "FILE")]
    pub binary: Option<PathBuf>,

    /// Target platform (e.g. wasm32-wasip1, x86_64-unknown-linux-gnu)
    #[arg(long, short = 't', default_value = "wasm32-wasip1")]
    pub target: String,

    /// Output signature file (default: manifest.sig)
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PkgVerifyArgs {
    /// .actr package file to verify
    #[arg(long, short = 'p', value_name = "FILE")]
    pub package: PathBuf,

    /// Public key file (default: derive from the configured Publisher keychain)
    #[arg(long, value_name = "FILE")]
    pub pubkey: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PkgKeygenArgs {
    /// Key output file path (default: ~/.actr/dev-key.json)
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Force overwrite existing key
    #[arg(long)]
    pub force: bool,
    /// Bind this keychain to a publisher; signing packages for
    /// any other publisher will then be rejected (issue #425).
    #[arg(long, value_name = "NAME")]
    pub publisher: Option<String>,
}

#[async_trait]
impl Command for PkgArgs {
    async fn execute(&self, _ctx: &CommandContext) -> Result<CommandResult> {
        let cli_config = crate::config::resolver::resolve_effective_cli_config()?;
        let keychain_ref = cli_config.publisher.keychain.as_deref();

        match &self.command {
            PkgCommand::Pack(a) => execute_pack(a, keychain_ref)?,
            PkgCommand::Sign(a) => execute_sign(a, keychain_ref).await?,
            PkgCommand::Verify(a) => execute_verify(a, keychain_ref).await?,
            PkgCommand::Keygen(a) => execute_keygen(a)?,
        }
        Ok(CommandResult::Success(String::new()))
    }

    fn required_components(&self) -> Vec<ComponentType> {
        vec![]
    }

    fn name(&self) -> &str {
        "pkg"
    }

    fn description(&self) -> &str {
        "Local package operations (sign, verify, keygen)"
    }
}

fn execute_pack(args: &PkgPackArgs, config_keychain: Option<&str>) -> Result<()> {
    let key_path = resolve_key_path(args.key.as_deref(), config_keychain)?;
    let (signing_key, keychain_publisher) = load_signing_key_with_publisher(&key_path)?;
    let manifest_source = std::fs::read_to_string(&args.manifest_path)
        .with_context(|| format!("Failed to read manifest: {}", args.manifest_path.display()))?;
    let manifest = actr_pack::ActorPackageManifest::from_toml(&manifest_source)?;
    ensure_publisher_matches(keychain_publisher.as_deref(), &manifest.actor.publisher)?;

    let canonical_root = args.payload_root.canonicalize().with_context(|| {
        format!(
            "Failed to resolve payload root: {}",
            args.payload_root.display()
        )
    })?;
    let mut payloads = std::collections::BTreeMap::new();
    for file in &manifest.files {
        let path = canonical_root.join(&file.path);
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("Failed to resolve payload `{}`", path.display()))?;
        if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
            anyhow::bail!(
                "Payload `{}` must be a regular file below {}",
                file.path,
                canonical_root.display()
            );
        }
        payloads.insert(
            file.path.clone(),
            std::fs::read(&canonical_path)
                .with_context(|| format!("Failed to read payload: {}", canonical_path.display()))?,
        );
    }

    let package = actr_pack::pack_actor_package(&actr_pack::ActorPackOptions {
        manifest,
        payloads,
        signing_key,
    })?;
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    std::fs::write(&args.output, package)
        .with_context(|| format!("Failed to write package: {}", args.output.display()))?;
    println!("Actor package created: {}", args.output.display());
    Ok(())
}

// ── keygen ───────────────────────────────────────────────────────────────────

fn execute_keygen(args: &PkgKeygenArgs) -> Result<()> {
    let key_path = match args.output {
        Some(ref path) => path.clone(),
        None => {
            let home = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Unable to determine home directory"))?;
            home.join(".actr").join("dev-key.json")
        }
    };

    if key_path.is_dir() {
        anyhow::bail!(
            "Key output path is a directory: {}\nSpecify a file name inside it.",
            key_path.display()
        );
    }

    if key_path.exists() && !args.force {
        anyhow::bail!(
            "Key file already exists: {}\nUse --force to overwrite, or --output to specify a different path.",
            key_path.display()
        );
    }

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
    let verifying_key = signing_key.verifying_key();

    let private_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.to_bytes());
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes());

    let now = chrono::Utc::now().to_rfc3339();
    let mut key_json = serde_json::json!({
        "private_key": private_b64,
        "public_key": public_b64,
        "created_at": now,
        "note": "Development signing key, for StaticTrust use only, not for production use"
    });
    if let Some(ref publisher) = args.publisher {
        if publisher.trim().is_empty() {
            anyhow::bail!("--publisher cannot be empty");
        }
        key_json["publisher"] = serde_json::Value::String(publisher.clone());
    }

    let json_str = serde_json::to_string_pretty(&key_json)?;
    std::fs::write(&key_path, &json_str)
        .with_context(|| format!("Failed to write key file: {}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&key_path, perms).ok();
    }

    println!("Key pair generated: {}", key_path.display());
    println!();
    println!("Public key (base64, Ed25519 verifying key — 32 bytes):");
    println!("  {}", public_b64);
    println!();
    println!("Use it as a StaticTrust anchor in your actr.toml:");
    println!("  [[trust]]");
    println!("  kind = \"static\"");
    println!("  pubkey_b64 = \"{}\"", public_b64);
    println!();
    println!("Or reference a public-key.json next to the .actr package:");
    println!("  [[trust]]");
    println!("  kind = \"static\"");
    println!("  pubkey_file = \"public-key.json\"");

    let global_path = crate::config::loader::global_config_path()?;
    let mut global_config =
        crate::config::loader::load_cli_config(&global_path)?.unwrap_or_default();
    global_config.publisher.keychain = Some(key_path.display().to_string());
    if let Some(parent) = global_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let content =
        toml::to_string_pretty(&global_config).with_context(|| "Failed to serialize config")?;
    std::fs::write(&global_path, content)
        .with_context(|| format!("Failed to write {}", global_path.display()))?;
    println!();
    println!(
        "✅ Global config updated: publisher.keychain = {}",
        key_path.display()
    );

    Ok(())
}

// ── sign ─────────────────────────────────────────────────────────────────────
//
// Parses manifest.toml, reads binary + proto files, builds a PackageManifest,
// serializes via to_toml(), and signs with Ed25519.
//
// Output:
//   1. Canonical manifest.toml — the exact bytes that were signed
//   2. manifest.sig — 64 bytes raw Ed25519 signature
//   3. A signed manifest.lock.toml hash when the source manifest has dependencies
//
// The output uses the same PackageManifest schema and lock validation as
// `actr build`; an external packager must include the referenced sibling lock.

async fn execute_sign(args: &PkgSignArgs, config_keychain: Option<&str>) -> Result<()> {
    use ed25519_dalek::Signer;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let key_path = resolve_key_path(args.key.as_deref(), config_keychain)?;
    let (signing_key, keychain_publisher) = load_signing_key_with_publisher(&key_path)?;
    let verifying_key = signing_key.verifying_key();
    let key_id = actr_pack::compute_key_id(&verifying_key.to_bytes());

    let config_path = &args.manifest_path;
    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "manifest.toml not found: {}",
            config_path.display()
        ));
    }
    let config_bytes = std::fs::read(config_path)?;
    let config_value: toml::Value =
        toml::from_slice(&config_bytes).with_context(|| "Invalid manifest.toml")?;
    let pkg = config_value
        .get("package")
        .ok_or_else(|| anyhow::anyhow!("manifest.toml missing [package] section"))?;

    let get_str = |key: &str| -> Result<String> {
        pkg.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("manifest.toml [package].{key} missing"))
    };

    let publisher = get_str("publisher")?;
    let name = get_str("name")?;
    let version = get_str("version")?;
    let parsed_config = actr_config::ConfigParser::from_manifest_file(config_path)
        .with_context(|| "Invalid manifest.toml package configuration")?;
    let lock_file = validated_packaged_lock(&parsed_config)?;

    // Reject cross-publisher signing before computing hashes or writing the
    // signature (issue #425).
    ensure_publisher_matches(keychain_publisher.as_deref(), &publisher)?;

    let (binary_hash, binary_size) = if let Some(binary_path) = &args.binary {
        let binary_data = std::fs::read(binary_path)
            .with_context(|| format!("Failed to read binary: {}", binary_path.display()))?;
        let hash = Sha256::digest(&binary_data);
        println!(
            "  binary:    {} ({} bytes)",
            binary_path.display(),
            binary_data.len()
        );
        (hex::encode(hash), Some(binary_data.len() as u64))
    } else {
        (String::new(), None)
    };

    let config_dir = args
        .manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut proto_entries = vec![];
    let exports = pkg
        .get("exports")
        .and_then(|e| e.as_array())
        .or_else(|| config_value.get("exports").and_then(|e| e.as_array()));
    if let Some(exports) = exports {
        for export_entry in exports {
            if let Some(proto_path_str) = export_entry.as_str() {
                let proto_path = config_dir.join(proto_path_str);
                match std::fs::read(&proto_path) {
                    Ok(content) => {
                        let filename = proto_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown.proto")
                            .to_string();
                        let hash = hex::encode(Sha256::digest(&content));
                        println!("  proto:     {} (hash: {}...)", filename, &hash[..16]);
                        proto_entries.push(actr_pack::ProtoFileEntry {
                            name: filename.clone(),
                            path: format!("proto/{}", filename),
                            hash,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to read proto file {:?}: {}", proto_path, e);
                    }
                }
            }
        }
    }

    let manifest = actr_pack::PackageManifest {
        publisher: publisher.clone(),
        name: name.clone(),
        version: version.clone(),
        binary: actr_pack::BinaryEntry {
            path: "bin/actor.wasm".to_string(),
            target: args.target.clone(),
            hash: binary_hash,
            size: binary_size,
            kind: binary_kind_for_target(&args.target),
        },
        signature_algorithm: "ed25519".to_string(),
        signing_key_id: Some(key_id.clone()),
        resources: vec![],
        proto_files: proto_entries,
        lock_file: lock_file.as_ref().map(|bytes| actr_pack::LockFileEntry {
            path: "manifest.lock.toml".to_string(),
            hash: hex::encode(Sha256::digest(bytes)),
        }),
        metadata: actr_pack::ManifestMetadata {
            description: pkg
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            license: pkg
                .get("license")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        },
    };

    let manifest_toml = manifest
        .to_toml()
        .map_err(|e| anyhow::anyhow!("Failed to serialize manifest: {e}"))?;
    let manifest_bytes = manifest_toml.as_bytes();

    let signature = signing_key.sign(manifest_bytes);
    let sig_bytes = signature.to_bytes();

    let manifest_path = {
        let mut p = args.manifest_path.clone();
        p.set_file_name("manifest.toml");
        p
    };
    std::fs::write(&manifest_path, manifest_bytes)
        .with_context(|| format!("Failed to write manifest: {}", manifest_path.display()))?;

    let sig_path = args.output.clone().unwrap_or_else(|| {
        let mut p = args.manifest_path.clone();
        p.set_file_name("manifest.sig");
        p
    });
    {
        let mut f = std::fs::File::create(&sig_path)?;
        f.write_all(&sig_bytes)?;
    }

    println!("✅ Manifest signed successfully");
    println!("  manifest:  {} (signed content)", manifest_path.display());
    println!("  sig file:  {} (64 bytes raw Ed25519)", sig_path.display());
    println!("  key_id:    {}", key_id);
    println!("  actr_type: {}/{}@{}", publisher, name, version);
    println!("  target:    {}", args.target);

    Ok(())
}

// ── verify ───────────────────────────────────────────────────────────────────

async fn execute_verify(args: &PkgVerifyArgs, config_keychain: Option<&str>) -> Result<()> {
    let package_bytes = std::fs::read(&args.package)
        .with_context(|| format!("Failed to read package: {}", args.package.display()))?;

    let pubkey = if let Some(pubkey_path) = &args.pubkey {
        load_verifying_key(pubkey_path)?
    } else {
        let key_path = resolve_key_path(None, config_keychain)?;
        load_verifying_key_from_dev_key(&key_path)?
    };

    if let Ok(manifest_source) = actr_pack::read_manifest_raw(&package_bytes) {
        if let Ok(manifest) = actr_pack::ActorPackageManifest::from_toml(&manifest_source) {
            let verified = actr_pack::verify_actor_package(&package_bytes, &pubkey)?;
            println!("Package verification passed");
            println!();
            println!("  publisher: {}", manifest.actor.publisher);
            println!("  type:         {}", verified.manifest.actor_type());
            println!("  workload:     {}", manifest.workload.entry);
            println!("  profile:      {}", manifest.workload.profile);
            println!("  signing_key:  {}", manifest.signature.key_id);
            println!("  files:        {}", manifest.files.len());
            return Ok(());
        }
    }

    let verified = actr_pack::verify(&package_bytes, &pubkey)?;

    if let Some(ref manifest_key_id) = verified.manifest.signing_key_id {
        let expected_key_id = actr_pack::compute_key_id(&pubkey.to_bytes());
        if manifest_key_id != &expected_key_id {
            anyhow::bail!(
                "signing_key_id mismatch: manifest says '{}' but the provided public key fingerprint is '{}'. \
                 This package will fail verification in Production mode. \
                 Rebuild with 'actr build' using the correct signing key.",
                manifest_key_id,
                expected_key_id,
            );
        }
    } else {
        anyhow::bail!(
            "Package manifest has no 'signing_key_id'. \
             This package will be rejected in Production mode. \
             Rebuild with the latest 'actr build' to embed a signing_key_id."
        );
    }

    println!("Package verification passed");
    println!();
    println!("  publisher: {}", verified.manifest.publisher);
    println!("  type:         {}", verified.manifest.actr_type_str());
    println!("  binary:       {}", verified.manifest.binary.path);
    println!(
        "  binary_hash:  {}...",
        &verified.manifest.binary.hash[..16]
    );
    println!("  target:       {}", verified.manifest.binary.target);
    if let Some(ref key_id) = verified.manifest.signing_key_id {
        println!("  signing_key:  {}", key_id);
    }
    if !verified.manifest.resources.is_empty() {
        println!("  resources:    {}", verified.manifest.resources.len());
    }

    Ok(())
}
